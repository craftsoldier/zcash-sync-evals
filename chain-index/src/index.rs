//! A built index, loaded for answering, frozen at a tip.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde_json::{Value, json};

use crate::cache::Cache;
use crate::trees::{Trees, tree_json};
use crate::utxos::{self, Coin, OutPoint};

/// A JSON-RPC error: code and message, as zcashd gives them.
#[derive(Debug)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

/// The code for "this node cannot answer that": the only errors logged as MISS.
pub const CANNOT_ANSWER: i64 = -32603;

impl RpcError {
    /// A question this node cannot answer exactly. Refused rather than guessed.
    pub fn cannot_answer(message: impl Into<String>) -> RpcError {
        RpcError::new(CANNOT_ANSWER, message)
    }

    pub fn new(code: i64, message: impl Into<String>) -> RpcError {
        RpcError {
            code,
            message: message.into(),
        }
    }
}

pub type RpcResult = Result<Value, RpcError>;

type CoinsByScript = HashMap<Vec<u8>, Vec<(OutPoint, Coin)>>;

pub struct Index {
    pub cache: Cache,
    /// The tip everything is answered at.
    pub tip: u64,
    /// The tip's block hash, in display (big-endian) order.
    pub tip_hash: String,
    /// The tip the index was built to; its coins are as of this height.
    built_tip: u64,
    ckpt_heights: Vec<u64>,
    ckpt_data: Vec<Vec<u8>>,
    subtrees: Value,
    watched: HashSet<Vec<u8>>,
    coins: CoinsByScript,
}

/// The tip an index directory was built to, from its index.json.
pub fn built_tip(dir: &Path) -> u64 {
    let summary: Value =
        serde_json::from_slice(&std::fs::read(dir.join("index.json")).expect("read index.json"))
            .expect("index.json");
    summary["tip"].as_u64().expect("index.json has no tip")
}

impl Index {
    /// Loads the index in `dir` over the cache in `cache_dir`, frozen at `tip`
    /// (default: the cache's last block).
    pub fn load(cache_dir: &Path, dir: &Path, tip: Option<u64>) -> Index {
        let raw = std::fs::read(dir.join("checkpoints.bin")).expect("read checkpoints.bin");
        let (mut ckpt_heights, mut ckpt_data) = (Vec::new(), Vec::new());
        let mut b = &raw[..];
        while !b.is_empty() {
            let h = u32::from_le_bytes(b[..4].try_into().expect("height"));
            let n = u32::from_le_bytes(b[4..8].try_into().expect("length")) as usize;
            ckpt_heights.push(u64::from(h));
            ckpt_data.push(b[8..8 + n].to_vec());
            b = &b[8 + n..];
        }
        let mut coins = CoinsByScript::new();
        for (k, c) in utxos::read(&dir.join("utxos.bin")) {
            coins.entry(c.script.clone()).or_default().push((k, c));
        }
        let cache = Cache::open(cache_dir);
        let built_tip = built_tip(dir);
        let tip = tip.unwrap_or(cache.tip());
        assert!(
            tip <= cache.tip(),
            "tip {tip} is past the cache's last block {}",
            cache.tip()
        );
        assert!(
            tip <= built_tip,
            "tip {tip} is past the index's last block {built_tip}"
        );
        let mut hash = cache.block(tip).hash;
        hash.reverse();
        Index {
            cache,
            tip,
            tip_hash: hex::encode(hash),
            built_tip,
            ckpt_heights,
            ckpt_data,
            subtrees: serde_json::from_slice(
                &std::fs::read(dir.join("subtrees.json")).expect("read subtrees.json"),
            )
            .expect("subtrees.json"),
            watched: utxos::read_watched(&dir.join("watched-addresses.txt")),
            coins,
        }
    }

    pub fn checkpoints(&self) -> usize {
        self.ckpt_heights.len()
    }

    /// z_gettreestate: the nearest checkpoint at or below `h`, then replay.
    pub fn tree_state(&self, h: u64) -> RpcResult {
        if h > self.tip {
            return Err(RpcError::new(-8, "block height beyond snapshot tip"));
        }
        let i = self.ckpt_heights.partition_point(|&c| c <= h) - 1;
        let mut trees = Trees::read(&self.ckpt_data[i]);
        for x in self.ckpt_heights[i] + 1..=h {
            trees.apply(&self.cache.block(x));
        }
        let blk = self.cache.block(h);
        let mut hash = blk.hash;
        hash.reverse();
        Ok(json!({
            "hash": hex::encode(hash),
            "height": h,
            "time": blk.time,
            "sapling": tree_json(&trees.sapling),
            "orchard": tree_json(&trees.orchard),
            "ironwood": tree_json(&trees.ironwood),
        }))
    }

    /// z_getsubtreesbyindex: only subtrees completed at or below the tip.
    pub fn subtrees(&self, pool: &str, start: usize, limit: Option<usize>) -> RpcResult {
        let all = self.subtrees[pool]["subtrees"]
            .as_array()
            .ok_or_else(|| RpcError::new(-8, format!("unknown pool {pool}")))?;
        let done: Vec<&Value> = all
            .iter()
            .take_while(|s| s["end_height"].as_u64().is_some_and(|e| e <= self.tip))
            .collect();
        let end = limit.map_or(done.len(), |l| (start + l).min(done.len()));
        let page = done.get(start.min(done.len())..end).unwrap_or(&[]);
        Ok(json!({"pool": pool, "start_index": start, "subtrees": page}))
    }

    /// getaddressutxos, for watched addresses at the tip the index was built to.
    /// Anything else would be a guess, so it is refused.
    pub fn utxos(&self, q: &Value) -> RpcResult {
        if self.tip != self.built_tip {
            return Err(RpcError::cannot_answer(format!(
                "coins are indexed at {}, not at tip {}",
                self.built_tip, self.tip
            )));
        }
        let start = q["startHeight"].as_u64().unwrap_or(0);
        let max = q["maxEntries"].as_u64().filter(|&m| m > 0);
        let addrs = q["addresses"]
            .as_array()
            .ok_or_else(|| RpcError::new(-8, "addresses required"))?;
        let mut out = Vec::new();
        for a in addrs {
            let addr = a.as_str().unwrap_or_default();
            let script = utxos::address_script(addr)
                .ok_or_else(|| RpcError::new(-5, format!("Invalid address: {addr}")))?;
            if !self.watched.contains(&script) {
                return Err(RpcError::cannot_answer(format!(
                    "address not watched by this index: {addr}"
                )));
            }
            for (k, c) in self.coins.get(&script).into_iter().flatten() {
                if u64::from(c.height) < start {
                    continue;
                }
                let mut txid = k[..32].to_vec();
                txid.reverse();
                out.push(json!({
                    "address": addr,
                    "txid": hex::encode(txid),
                    "outputIndex": u32::from_le_bytes(k[32..].try_into().expect("index")),
                    "script": hex::encode(&c.script),
                    "satoshis": c.value,
                    "height": c.height,
                }));
            }
        }
        out.sort_by_key(|u| {
            (
                u["height"].as_u64(),
                u["txid"].as_str().map(String::from),
                u["outputIndex"].as_u64(),
            )
        });
        if let Some(m) = max {
            out.truncate(usize::try_from(m).unwrap_or(usize::MAX));
        }
        Ok(Value::Array(out))
    }
}
