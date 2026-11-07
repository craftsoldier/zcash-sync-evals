//! `serve`: the whole node lightwalletd talks to, frozen at the index's tip.

use std::path::Path;
use std::time::Instant;

use serde_json::{Value, json};

use crate::index::{CANNOT_ANSWER, Index, RpcError, RpcResult};

const THREADS: usize = 8;

/// Recorded node replies (node.json) plus the index, answering at one tip.
pub struct Node {
    pub index: Index,
    recorded: Value,
}

impl Node {
    pub fn new(index: Index, recorded: Value) -> Node {
        Node { index, recorded }
    }

    /// Answers one JSON-RPC method. A question this node cannot answer is an
    /// error, logged as MISS: never a guess.
    pub fn answer(&self, method: &str, p: &[Value]) -> RpcResult {
        let ix = &self.index;
        match method {
            "getinfo" | "getblockchaininfo" => self.frozen(method),
            "getbestblockhash" => Ok(json!(ix.tip_hash)),
            "getrawmempool" => Ok(json!([])),
            "sendrawtransaction" => Err(RpcError::new(
                -26,
                "frozen snapshot: transactions are not accepted",
            )),
            // lightwalletd asks for the block after its cache; past the tip there is none.
            "getblock" if p.first().and_then(height).is_some_and(|h| h > ix.tip) => {
                Err(RpcError::new(-8, "block height beyond snapshot tip"))
            }
            "z_gettreestate" => ix.tree_state(
                p.first()
                    .and_then(height)
                    .ok_or_else(|| RpcError::new(-8, "height required"))?,
            ),
            "z_getsubtreesbyindex" => ix.subtrees(
                p.first().and_then(Value::as_str).unwrap_or_default(),
                p.get(1).and_then(height).map_or(0, to_usize),
                p.get(2).and_then(height).map(to_usize),
            ),
            "getaddressutxos" => {
                let q = p.first().unwrap_or(&Value::Null);
                eprintln!("getaddressutxos {q}");
                ix.utxos(q)
            }
            _ => Err(RpcError::cannot_answer(format!("not answered: {method}"))),
        }
        .inspect_err(|e| {
            if e.code == CANNOT_ANSWER {
                eprintln!(
                    "MISS {method} {} -> {}",
                    Value::Array(p.to_vec()),
                    e.message
                );
            }
        })
    }

    /// A recorded startup reply, rewritten so it describes the chain at the tip.
    fn frozen(&self, method: &str) -> RpcResult {
        let mut r = self
            .recorded
            .get(method)
            .cloned()
            .ok_or_else(|| RpcError::cannot_answer(format!("{method} not in node.json")))?;
        let h = self.index.tip;
        let o = r.as_object_mut().ok_or_else(|| {
            RpcError::cannot_answer(format!("{method} in node.json is not an object"))
        })?;
        if method == "getinfo" {
            o.insert("blocks".into(), json!(h));
            o.insert("errors".into(), json!(""));
            o.remove("errorstimestamp");
            return Ok(r);
        }
        for k in ["blocks", "headers", "estimatedheight"] {
            o.insert(k.into(), json!(h));
        }
        o.insert("bestblockhash".into(), json!(self.index.tip_hash));
        o.insert("verificationprogress".into(), json!(1.0));
        for k in ["chainSupply", "valuePools", "size_on_disk"] {
            o.remove(k);
        }
        // The latest upgrade active at the tip, and the one the next block is under.
        let mut activations: Vec<(u64, String)> = Vec::new();
        if let Some(upgrades) = o.get_mut("upgrades").and_then(Value::as_object_mut) {
            for (branch, u) in upgrades.iter_mut() {
                let at = u["activationheight"].as_u64().unwrap_or(u64::MAX);
                u["status"] = json!(if at <= h { "active" } else { "pending" });
                activations.push((at, branch.clone()));
            }
        }
        let latest_by = |height: u64| activations.iter().filter(|(at, _)| *at <= height).max();
        if let (Some(tip), Some(next)) = (latest_by(h), latest_by(h + 1)) {
            o.insert(
                "consensus".into(),
                json!({"chaintip": tip.1, "nextblock": next.1}),
            );
        }
        Ok(r)
    }

    fn reply(&self, req: &Value) -> Value {
        let params = req["params"].as_array().map_or(&[][..], Vec::as_slice);
        match self.answer(req["method"].as_str().unwrap_or_default(), params) {
            Ok(v) => json!({"result": v, "error": null, "id": req["id"]}),
            Err(e) => {
                json!({"result": null, "error": {"code": e.code, "message": e.message}, "id": req["id"]})
            }
        }
    }
}

/// A height parameter, given as a number or a decimal string.
pub fn height(v: &Value) -> Option<u64> {
    v.as_u64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

fn to_usize(n: u64) -> usize {
    usize::try_from(n).unwrap_or(usize::MAX)
}

pub fn serve(cache_dir: &Path, dir: &Path, node_file: &Path, bind: &str, tip: Option<u64>) {
    let t = Instant::now();
    let recorded = serde_json::from_slice(&std::fs::read(node_file).expect("read node.json"))
        .expect("node.json");
    let node = Node::new(Index::load(cache_dir, dir, tip), recorded);
    eprintln!(
        "chain-index: tip {} {} | {} checkpoints | loaded in {:.1}s | {bind}",
        node.index.tip,
        &node.index.tip_hash[..16],
        node.index.checkpoints(),
        t.elapsed().as_secs_f64()
    );
    let server = tiny_http::Server::http(bind).expect("bind");
    std::thread::scope(|s| {
        for _ in 0..THREADS {
            s.spawn(|| {
                while let Ok(mut req) = server.recv() {
                    let mut body = String::new();
                    let reply = match req.as_reader().read_to_string(&mut body).map(|_| serde_json::from_str(&body)) {
                        Ok(Ok(Value::Array(batch))) => Value::Array(batch.iter().map(|r| node.reply(r)).collect()),
                        Ok(Ok(r)) => node.reply(&r),
                        _ => json!({"result": null, "error": {"code": -32700, "message": "parse error"}, "id": null}),
                    };
                    let _ = req.respond(tiny_http::Response::from_string(reply.to_string()));
                }
            });
        }
    });
}
