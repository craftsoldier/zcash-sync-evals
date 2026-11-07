//! Note commitment trees in zcashd's legacy form, which z_gettreestate returns.

use incrementalmerkletree::Hashable;
use incrementalmerkletree::frontier::CommitmentTree;
use orchard::tree::MerkleHashOrchard;
use serde_json::{Value, json};

use crate::proto::{Block, arr32};

pub type Sap = sapling::Node;
pub type Orch = MerkleHashOrchard;
pub const DEPTH: u8 = 32;
pub type Tree<H> = CommitmentTree<H, DEPTH>;

pub trait Node: Hashable + Clone {
    /// Sapling roots are shown byte-reversed; Orchard roots are not.
    const REVERSE_ROOT: bool = false;
    fn bytes(&self) -> [u8; 32];
    fn parse(b: &[u8; 32]) -> Self;
}

impl Node for Sap {
    const REVERSE_ROOT: bool = true;
    fn bytes(&self) -> [u8; 32] {
        self.to_bytes()
    }
    fn parse(b: &[u8; 32]) -> Self {
        Option::from(Sap::from_bytes(*b)).expect("valid Sapling node")
    }
}

impl Node for Orch {
    fn bytes(&self) -> [u8; 32] {
        self.to_bytes()
    }
    fn parse(b: &[u8; 32]) -> Self {
        Option::from(Orch::from_bytes(b)).expect("valid Orchard node")
    }
}

/// All three pools' trees.
pub struct Trees {
    pub sapling: Tree<Sap>,
    pub orchard: Tree<Orch>,
    pub ironwood: Tree<Orch>,
}

impl Trees {
    /// Reads the three serialized trees stored in one checkpoint.
    pub fn read(mut b: &[u8]) -> Trees {
        let mut part = || {
            let n = u32::from_le_bytes(b[..4].try_into().expect("length")) as usize;
            let p = &b[4..4 + n];
            b = &b[4 + n..];
            p
        };
        Trees {
            sapling: read_tree(part()),
            orchard: read_tree(part()),
            ironwood: read_tree(part()),
        }
    }

    pub fn apply(&mut self, b: &Block) {
        for tx in &b.txs {
            for c in &tx.sapling {
                self.sapling
                    .append(Sap::parse(c))
                    .expect("Sapling tree full");
            }
            for c in &tx.orchard {
                self.orchard
                    .append(Orch::parse(c))
                    .expect("Orchard tree full");
            }
            for c in &tx.ironwood {
                self.ironwood
                    .append(Orch::parse(c))
                    .expect("Ironwood tree full");
            }
        }
    }
}

/// zcashd's legacy CommitmentTree encoding: Optional(left) Optional(right)
/// CompactSize-prefixed Vec<Optional(parent)>. This is z_gettreestate's finalState.
pub fn write_tree<H: Node>(t: &Tree<H>) -> Vec<u8> {
    fn opt<H: Node>(out: &mut Vec<u8>, n: Option<&H>) {
        match n {
            Some(n) => {
                out.push(1);
                out.extend_from_slice(&n.bytes());
            }
            None => out.push(0),
        }
    }
    // An empty tree is "000000", as zcashd writes it. Otherwise the parents list
    // is padded with empty slots to DEPTH - 1, as Zakura/Zebra write it.
    if t.size() == 0 {
        return vec![0, 0, 0];
    }
    let mut out = Vec::new();
    opt(&mut out, t.left().as_ref());
    opt(&mut out, t.right().as_ref());
    let n = DEPTH - 1;
    assert!(t.parents().len() <= usize::from(n));
    out.push(n);
    for i in 0..usize::from(n) {
        opt(&mut out, t.parents().get(i).and_then(Option::as_ref));
    }
    out
}

/// Reads what `write_tree` wrote.
fn read_tree<H: Node>(mut b: &[u8]) -> Tree<H> {
    fn opt<H: Node>(b: &mut &[u8]) -> Option<H> {
        let tag = b[0];
        *b = &b[1..];
        (tag == 1).then(|| {
            let n = H::parse(&arr32(&b[..32]));
            *b = &b[32..];
            n
        })
    }
    let left = opt(&mut b);
    let right = opt(&mut b);
    let n = b[0];
    b = &b[1..];
    let parents = (0..n).map(|_| opt(&mut b)).collect();
    CommitmentTree::from_parts(left, right, parents).expect("tree parts")
}

pub fn tree_json<H: Node>(t: &Tree<H>) -> Value {
    let mut root = t.root().bytes();
    if H::REVERSE_ROOT {
        root.reverse();
    }
    json!({"commitments": {"finalRoot": hex::encode(root), "finalState": hex::encode(write_tree(t))}})
}
