//! `verify`: ask the index every question a real node answered in an answers
//! file, and compare. Offline, all cores. Exits non-zero on any difference.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::Value;

use crate::index::{Index, RpcResult};
use crate::node::{Node, height};

const CHECKED: [&str; 3] = ["z_gettreestate", "z_getsubtreesbyindex", "getaddressutxos"];

pub fn verify(
    cache_dir: &Path,
    dir: &Path,
    answers: &Path,
    heights: std::ops::RangeInclusive<u64>,
) {
    let node = Node::new(Index::load(cache_dir, dir, None), Value::Null);
    let rows: Vec<Value> = std::fs::read_to_string(answers)
        .expect("read answers")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("answers line"))
        .filter(|r: &Value| {
            let m = r["method"].as_str().unwrap_or_default();
            r.get("result").is_some()
                && CHECKED.contains(&m)
                && (m != "z_gettreestate"
                    || height(&r["result"]["height"]).is_some_and(|h| heights.contains(&h)))
        })
        .collect();

    let next = AtomicUsize::new(0);
    let threads = std::thread::available_parallelism().map_or(4, std::num::NonZero::get);
    let mut results: Vec<(usize, Vec<String>)> = std::thread::scope(|s| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| {
                    let mut out = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(row) = rows.get(i) else { return out };
                        let m = row["method"].as_str().unwrap_or_default();
                        let params = row["params"].as_array().map_or(&[][..], Vec::as_slice);
                        out.push((i, compare(m, &row["result"], node.answer(m, params))));
                    }
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|w| w.join().expect("verify thread"))
            .collect()
    });
    results.sort_by_key(|(i, _)| *i);

    let mut counts: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    let mut failed = Vec::new();
    for (i, diffs) in &results {
        let m = rows[*i]["method"].as_str().unwrap_or_default();
        let c = counts.entry(m).or_default();
        c.0 += 1;
        if !diffs.is_empty() {
            c.1 += 1;
            failed.push(format!("DIFF {m} {} {diffs:?}", rows[*i]["params"]));
        }
    }
    for (m, (n, bad)) in &counts {
        println!(
            "{m:22} {n:5} checked  {:5} identical  {bad} different",
            n - bad
        );
    }
    for f in failed.iter().take(20) {
        println!("{f}");
    }
    std::process::exit(i32::from(!failed.is_empty() || counts.is_empty()));
}

/// The fields that differ between a recorded answer and the index's.
fn compare(method: &str, want: &Value, got: RpcResult) -> Vec<String> {
    let got = match got {
        Ok(v) => v,
        Err(e) => return vec![format!("error {} {}", e.code, e.message)],
    };
    match method {
        "z_gettreestate" => {
            let mut d: Vec<String> = ["hash", "height", "time"]
                .into_iter()
                .filter(|k| want[k] != got[k])
                .map(String::from)
                .collect();
            for pool in ["sapling", "orchard", "ironwood"] {
                let (w, g) = (&want[pool]["commitments"], &got[pool]["commitments"]);
                // A node may give an empty tree with no finalState; zcashd's form is "000000".
                let ws = w["finalState"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .unwrap_or("000000");
                if Some(ws) != g["finalState"].as_str() {
                    d.push(format!("{pool}.finalState"));
                }
                if w.get("finalRoot").is_some() && w["finalRoot"] != g["finalRoot"] {
                    d.push(format!("{pool}.finalRoot"));
                }
            }
            d
        }
        "getaddressutxos" => {
            let sorted = |v: &Value| {
                let mut a = v.as_array().cloned().unwrap_or_default();
                a.sort_by_key(|u| {
                    (
                        u["txid"].as_str().map(String::from),
                        u["outputIndex"].as_u64(),
                    )
                });
                a
            };
            if sorted(want) == sorted(&got) {
                vec![]
            } else {
                vec!["utxos".into()]
            }
        }
        _ => {
            if *want == got {
                vec![]
            } else {
                vec!["subtrees".into()]
            }
        }
    }
}
