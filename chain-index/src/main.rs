//! chain-index: answer lightwalletd's node questions from its own compact-block cache.
//!
//! lightwalletd keeps compact blocks but asks a full node for tree states, subtree
//! roots and transparent UTXOs. Every one of those can be rebuilt from the compact
//! blocks themselves: they carry every note commitment (Sapling cmu, Orchard and
//! Ironwood cmx) and every transparent input and output. So no node is needed.
//!
//!   chain-index build <lwd-cache-dir> <index-dir> <watched-addresses.txt> [--tip <height>]
//!   chain-index rebuild-utxos <lwd-cache-dir> <index-dir> <watched-addresses.txt>
//!   chain-index serve <lwd-cache-dir> <index-dir> <node.json> <bind> [--tip <height>]
//!   chain-index verify <lwd-cache-dir> <index-dir> <answers.jsonl> [--min-height <h>] [--max-height <h>]
//!
//! `serve` is the whole node lightwalletd talks to, frozen at the tip (the cache's
//! last block, or --tip). Tree states, subtree roots and UTXOs come from the index;
//! lightwalletd's startup questions come from node.json, replies a real node gave
//! once, rewritten to describe the tip. Answers are in the shape Zakura/zcashd give.
//! A question it cannot answer is an error, logged as MISS, never a guess.
//!
//! `verify` checks the index against answers recorded from a real node, offline.

mod build;
mod cache;
mod index;
mod node;
mod proto;
mod trees;
mod utxos;
mod verify;

use std::path::PathBuf;

const USAGE: &str = "usage:
  chain-index build <lwd-cache-dir> <index-dir> <watched-addresses.txt> [--tip <height>]
  chain-index rebuild-utxos <lwd-cache-dir> <index-dir> <watched-addresses.txt>
  chain-index serve <lwd-cache-dir> <index-dir> <node.json> <bind> [--tip <height>]
  chain-index verify <lwd-cache-dir> <index-dir> <answers.jsonl> [--min-height <h>] [--max-height <h>]";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (positional, flags) = split_flags(&args);
    let path = |i: usize| PathBuf::from(&positional[i]);
    match (positional.first().map(String::as_str), positional.len()) {
        (Some("build"), 4) => build::build(&path(1), &path(2), &path(3), flag(&flags, "--tip")),
        (Some("rebuild-utxos"), 4) => build::rebuild_utxos(&path(1), &path(2), &path(3)),
        (Some("serve"), 5) => node::serve(
            &path(1),
            &path(2),
            &path(3),
            &positional[4],
            flag(&flags, "--tip"),
        ),
        (Some("verify"), 4) => verify::verify(
            &path(1),
            &path(2),
            &path(3),
            flag(&flags, "--min-height").unwrap_or(0)
                ..=flag(&flags, "--max-height").unwrap_or(u64::MAX),
        ),
        _ => {
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    }
}

/// Splits `--name value` pairs from positional arguments.
fn split_flags(args: &[String]) -> (Vec<String>, Vec<(String, String)>) {
    let (mut positional, mut flags) = (Vec::new(), Vec::new());
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a.starts_with("--") {
            let v = it
                .next()
                .unwrap_or_else(|| panic!("{a} needs a value\n{USAGE}"));
            flags.push((a.clone(), v.clone()));
        } else {
            positional.push(a.clone());
        }
    }
    (positional, flags)
}

fn flag(flags: &[(String, String)], name: &str) -> Option<u64> {
    flags.iter().find(|(k, _)| k == name).map(|(_, v)| {
        v.parse()
            .unwrap_or_else(|_| panic!("{name}: not a number: {v}"))
    })
}
