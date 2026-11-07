//! `build`: one pass over the cache, writing the index.
//!
//!   checkpoints.bin   all three trees every CHECKPOINT_BLOCKS blocks (and every
//!                     CHECKPOINT_LEAVES new notes), so any tree state is at most
//!                     a few hundred blocks of replay away
//!   subtrees.json     every completed 2^16-leaf subtree root, per pool
//!   utxos.bin         the watched addresses' coins at the tip
//!   index.json        a summary, including the tip it was built to
//!
//! Each pool's tree is hashed on its own thread while the main thread reads.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::mpsc::{Receiver, sync_channel};
use std::time::Instant;

use serde_json::{Value, json};

use crate::cache::Cache;
use crate::trees::{DEPTH, Node, Orch, Sap, Tree, write_tree};
use crate::utxos::{self, Coins};

const CHECKPOINT_BLOCKS: u64 = 1000;
const CHECKPOINT_LEAVES: u64 = 20_000;
const BATCH_ITEMS: usize = 500;
const SUBTREE_LEAVES: u64 = 1 << 16;
const SUBTREE_LEVEL: usize = 16;

pub fn build(cache_dir: &Path, out: &Path, watch_file: &Path, stop: Option<u64>) {
    std::fs::create_dir_all(out).expect("create index dir");
    let watched = utxos::read_watched(watch_file);
    std::fs::copy(watch_file, out.join("watched-addresses.txt")).expect("copy watch file");
    let cache = Cache::open(cache_dir);
    let tip = stop.map_or(cache.tip(), |s| s.min(cache.tip()));

    let (tx_s, rx_s) = sync_channel::<Vec<Item>>(64);
    let (tx_o, rx_o) = sync_channel::<Vec<Item>>(64);
    let (tx_i, rx_i) = sync_channel::<Vec<Item>>(64);
    let w_s = std::thread::spawn(move || pool_worker::<Sap>(&rx_s));
    let w_o = std::thread::spawn(move || pool_worker::<Orch>(&rx_o));
    let w_i = std::thread::spawn(move || pool_worker::<Orch>(&rx_i));

    let mut coins = Coins::new();
    let (mut sizes, mut last_ckpt_leaves, mut mismatches) = ([0u64; 3], 0u64, 0u64);
    let mut ckpt_heights = Vec::new();
    let mut batches: [Vec<Item>; 3] = Default::default();
    let started = Instant::now();

    cache.scan(cache_dir, tip, |b| {
        let h = b.height;
        let mut leaves: [Vec<[u8; 32]>; 3] = Default::default();
        for tx in &b.txs {
            leaves[0].extend_from_slice(&tx.sapling);
            leaves[1].extend_from_slice(&tx.orchard);
            leaves[2].extend_from_slice(&tx.ironwood);
        }
        utxos::apply(&mut coins, &b, &watched);

        for ((size, stated), (batch, new)) in sizes
            .iter_mut()
            .zip(b.sizes)
            .zip(batches.iter_mut().zip(leaves))
        {
            *size += new.len() as u64;
            // The cache states each tree's size after every block: check it.
            if let Some(s) = stated.filter(|&s| s != 0 && s != *size) {
                mismatches += 1;
                if mismatches < 10 {
                    eprintln!("size mismatch at {h}: cache says {s}, built {size}");
                }
            }
            if !new.is_empty() {
                batch.push(Item::Block(h, new));
            }
        }

        let total: u64 = sizes.iter().sum();
        if h.is_multiple_of(CHECKPOINT_BLOCKS)
            || total - last_ckpt_leaves >= CHECKPOINT_LEAVES
            || h == tip
        {
            ckpt_heights.push(h);
            for batch in &mut batches {
                batch.push(Item::Checkpoint);
            }
            last_ckpt_leaves = total;
        }
        if batches.iter().map(Vec::len).sum::<usize>() >= BATCH_ITEMS || h == tip {
            for (tx, batch) in [&tx_s, &tx_o, &tx_i].into_iter().zip(&mut batches) {
                tx.send(std::mem::take(batch)).expect("tree thread");
            }
        }
        if h.is_multiple_of(100_000) {
            let secs = started.elapsed().as_secs_f64();
            eprintln!(
                "{h:>8} / {tip}  {secs:>6.0}s  sapling {} orchard {} ironwood {}",
                sizes[0], sizes[1], sizes[2]
            );
        }
    });
    drop((tx_s, tx_o, tx_i));
    eprintln!(
        "all blocks read in {:.0}s; waiting for tree threads",
        started.elapsed().as_secs_f64()
    );
    let pools = [
        w_s.join().expect("sapling"),
        w_o.join().expect("orchard"),
        w_i.join().expect("ironwood"),
    ];
    assert_eq!(pools.each_ref().map(|p| p.size), sizes);

    let mut ckpt =
        BufWriter::new(File::create(out.join("checkpoints.bin")).expect("create checkpoints.bin"));
    for (n, h) in ckpt_heights.iter().enumerate() {
        let mut rec = Vec::new();
        for p in &pools {
            let part = &p.checkpoints[n];
            rec.extend_from_slice(&u32::try_from(part.len()).expect("length").to_le_bytes());
            rec.extend_from_slice(part);
        }
        let h = u32::try_from(*h).expect("height");
        let len = u32::try_from(rec.len()).expect("length");
        for part in [&h.to_le_bytes()[..], &len.to_le_bytes(), &rec] {
            ckpt.write_all(part).expect("write checkpoints.bin");
        }
    }
    ckpt.flush().expect("write checkpoints.bin");

    let names = ["sapling", "orchard", "ironwood"];
    let subtrees: serde_json::Map<String, Value> = names
        .iter()
        .zip(&pools)
        .map(|(n, p)| ((*n).to_string(), subtrees_json(n, &p.subtrees)))
        .collect();
    std::fs::write(
        out.join("subtrees.json"),
        serde_json::to_vec(&subtrees).expect("json"),
    )
    .expect("write subtrees.json");

    utxos::write(&out.join("utxos.bin"), &coins);

    let summary = json!({
        "tip": tip,
        "sapling_size": sizes[0], "orchard_size": sizes[1], "ironwood_size": sizes[2],
        "sapling_subtrees": pools[0].subtrees.len(),
        "orchard_subtrees": pools[1].subtrees.len(),
        "ironwood_subtrees": pools[2].subtrees.len(),
        "checkpoints": ckpt_heights.len(),
        "watched_addresses": watched.len(),
        "utxos": coins.len(),
        "size_mismatches": mismatches,
        "seconds": started.elapsed().as_secs_f64(),
    });
    std::fs::write(
        out.join("index.json"),
        serde_json::to_vec_pretty(&summary).expect("json"),
    )
    .expect("write index.json");
    eprintln!("{summary}");
}

/// `rebuild-utxos`: redo only utxos.bin, for a new watch file, keeping the trees.
pub fn rebuild_utxos(cache_dir: &Path, out: &Path, watch_file: &Path) {
    let watched = utxos::read_watched(watch_file);
    let cache = Cache::open(cache_dir);
    let tip = crate::index::built_tip(out);
    let mut coins = Coins::new();
    cache.scan(cache_dir, tip, |b| utxos::apply(&mut coins, &b, &watched));
    utxos::write(&out.join("utxos.bin"), &coins);
    std::fs::copy(watch_file, out.join("watched-addresses.txt")).expect("copy watch file");
    eprintln!("rebuilt {} watched UTXOs at tip {tip}", coins.len());
}

enum Item {
    Block(u64, Vec<[u8; 32]>),
    Checkpoint,
}

struct PoolResult {
    /// The serialized tree at each checkpoint, in order.
    checkpoints: Vec<Vec<u8>>,
    /// (root hex, end height) of each completed subtree.
    subtrees: Vec<(String, u64)>,
    size: u64,
}

/// One pool's tree, on its own thread.
fn pool_worker<H: Node>(rx: &Receiver<Vec<Item>>) -> PoolResult {
    let mut tree = Legacy::<H>::new();
    let (mut roots, mut ends, mut checkpoints) = (Vec::new(), Vec::new(), Vec::new());
    for item in rx.iter().flatten() {
        match item {
            Item::Block(h, leaves) => {
                for c in &leaves {
                    tree.append(H::parse(c), &mut roots);
                    if tree.size.is_multiple_of(SUBTREE_LEAVES) {
                        ends.push(h);
                    }
                }
            }
            Item::Checkpoint => checkpoints.push(write_tree(&tree.to_tree())),
        }
    }
    if roots.len() < ends.len() {
        roots.push(tree.finished_subtree_now());
    }
    assert_eq!(roots.len(), ends.len());
    let subtrees = roots
        .iter()
        .zip(ends)
        .map(|(r, h)| (hex::encode(r.bytes()), h))
        .collect();
    PoolResult {
        checkpoints,
        subtrees,
        size: tree.size,
    }
}

fn subtrees_json(pool: &str, s: &[(String, u64)]) -> Value {
    json!({"pool": pool, "subtrees": s.iter().map(|(r, h)| json!({"root": r, "end_height": h})).collect::<Vec<_>>()})
}

/// zcashd's legacy tree, appended by hand so that each completed 2^16-leaf
/// subtree root is captured as it is hashed (the library hides it), instead of
/// hashing every note a second time just for subtree roots.
struct Legacy<H: Node> {
    left: Option<H>,
    right: Option<H>,
    parents: Vec<Option<H>>,
    size: u64,
}

impl<H: Node> Legacy<H> {
    fn new() -> Self {
        Legacy {
            left: None,
            right: None,
            parents: Vec::new(),
            size: 0,
        }
    }

    fn leaf_pair(&self) -> H {
        H::combine(
            0.into(),
            self.left.as_ref().expect("left"),
            self.right.as_ref().expect("right"),
        )
    }

    /// Appends a leaf. `level16` receives every finished level-16 node, in order.
    fn append(&mut self, node: H, level16: &mut Vec<H>) {
        self.size += 1;
        if self.left.is_none() {
            self.left = Some(node);
            return;
        }
        if self.right.is_none() {
            self.right = Some(node);
            return;
        }
        let mut combined = self.leaf_pair();
        self.left = Some(node);
        self.right = None;
        for (i, slot) in self.parents.iter_mut().enumerate() {
            if i + 1 == SUBTREE_LEVEL {
                level16.push(combined.clone());
            }
            match slot.take() {
                None => {
                    *slot = Some(combined);
                    return;
                }
                Some(p) => combined = H::combine(level(i + 1), &p, &combined),
            }
        }
        if self.parents.len() + 1 == SUBTREE_LEVEL {
            level16.push(combined.clone());
        }
        assert!(self.parents.len() < usize::from(DEPTH) - 1, "tree full");
        self.parents.push(Some(combined));
    }

    /// The level-16 node of a subtree that has just been completed, for the one
    /// case `append` cannot see: the tree ends exactly on a subtree boundary.
    fn finished_subtree_now(&self) -> H {
        let mut c = self.leaf_pair();
        for (i, p) in self.parents.iter().take(SUBTREE_LEVEL - 1).enumerate() {
            c = H::combine(level(i + 1), p.as_ref().expect("parent"), &c);
        }
        c
    }

    fn to_tree(&self) -> Tree<H> {
        Tree::from_parts(self.left.clone(), self.right.clone(), self.parents.clone())
            .expect("tree parts")
    }
}

fn level(i: usize) -> incrementalmerkletree::Level {
    u8::try_from(i).expect("level").into()
}
