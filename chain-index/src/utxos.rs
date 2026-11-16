//! Transparent coins of watched addresses.
//!
//! The full transparent UTXO set is tens of millions of coins (early pool
//! payouts), too big to hold here, so only the addresses in a watch file are
//! tracked. A question about any other address is refused, not answered empty.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use crate::proto::Block;

/// txid (protocol byte order) followed by the output index, little-endian.
pub type OutPoint = [u8; 36];

pub struct Coin {
    pub height: u32,
    pub value: u64,
    pub script: Vec<u8>,
}

pub type Coins = HashMap<OutPoint, Coin>;

fn outpoint(txid: &[u8], index: u32) -> OutPoint {
    let mut k = [0u8; 36];
    k[..32].copy_from_slice(txid);
    k[32..].copy_from_slice(&index.to_le_bytes());
    k
}

/// t1 (P2PKH) or t3 (P2SH) mainnet address to its scriptPubKey.
pub fn address_script(addr: &str) -> Option<Vec<u8>> {
    let raw = bs58::decode(addr).with_check(None).into_vec().ok()?;
    if raw.len() != 22 {
        return None;
    }
    let hash = &raw[2..];
    match [raw[0], raw[1]] {
        [0x1c, 0xb8] => Some([&[0x76, 0xa9, 0x14][..], hash, &[0x88, 0xac]].concat()),
        [0x1c, 0xbd] => Some([&[0xa9, 0x14][..], hash, &[0x87]].concat()),
        _ => None,
    }
}

/// Scripts of the addresses in a watch file: one address per line, `#` comments.
pub fn read_watched(path: &Path) -> HashSet<Vec<u8>> {
    std::fs::read_to_string(path)
        .expect("read watch file")
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|a| address_script(a).unwrap_or_else(|| panic!("not a mainnet t-address: {a}")))
        .collect()
}

/// A transaction that spends from or pays to a watched address.
pub struct Touch {
    pub script: Vec<u8>,
    pub height: u32,
    /// Position in its block, for answering in chain order.
    pub position: u32,
    /// Display (big-endian) order, as nodes give it.
    pub txid: String,
}

/// Spends and creates the watched coins in one block, and notes each
/// transaction that touches a watched address (repeats are removed when answering).
pub fn apply(coins: &mut Coins, touches: &mut Vec<Touch>, b: &Block, watched: &HashSet<Vec<u8>>) {
    let height = u32::try_from(b.height).expect("height");
    for (i, tx) in b.txs.iter().enumerate() {
        let position = u32::try_from(i).expect("position");
        let txid = {
            let mut t = tx.txid.clone();
            t.reverse();
            hex::encode(t)
        };
        // The coinbase (first) transaction spends nothing.
        if i > 0 {
            for (prev, idx) in &tx.vin {
                if let Some(c) = coins.remove(&outpoint(prev, *idx)) {
                    touches.push(Touch {
                        script: c.script,
                        height,
                        position,
                        txid: txid.clone(),
                    });
                }
            }
        }
        for (idx, (value, script)) in tx.vout.iter().enumerate() {
            if watched.contains(script) {
                touches.push(Touch {
                    script: script.clone(),
                    height,
                    position,
                    txid: txid.clone(),
                });
                let k = outpoint(&tx.txid, u32::try_from(idx).expect("output index"));
                coins.insert(
                    k,
                    Coin {
                        height,
                        value: *value,
                        script: script.clone(),
                    },
                );
            }
        }
    }
}

/// `utxos.bin`: per coin, [u16 script length][script][outpoint][u64 value][u32 height].
pub fn write(path: &Path, coins: &Coins) {
    let mut f = BufWriter::new(File::create(path).expect("create utxos.bin"));
    for (k, c) in coins {
        let len = u16::try_from(c.script.len()).expect("script length");
        for part in [
            &len.to_le_bytes()[..],
            &c.script,
            k,
            &c.value.to_le_bytes(),
            &c.height.to_le_bytes(),
        ] {
            f.write_all(part).expect("write utxos.bin");
        }
    }
    f.flush().expect("write utxos.bin");
}

pub fn read(path: &Path) -> Coins {
    let raw = std::fs::read(path).expect("read utxos.bin");
    let mut coins = Coins::new();
    let mut b = &raw[..];
    while !b.is_empty() {
        let len = usize::from(u16::from_le_bytes(b[..2].try_into().expect("length")));
        let script = b[2..2 + len].to_vec();
        let r = &b[2 + len..];
        let k: OutPoint = r[..36].try_into().expect("outpoint");
        let value = u64::from_le_bytes(r[36..44].try_into().expect("value"));
        let height = u32::from_le_bytes(r[44..48].try_into().expect("height"));
        coins.insert(
            k,
            Coin {
                height,
                value,
                script,
            },
        );
        b = &r[48..];
    }
    coins
}

/// `txids.txt`: per touch, one line of `height position txid script`, in chain order.
pub fn write_touches(path: &Path, touches: &[Touch]) {
    let text: String = touches
        .iter()
        .map(|t| {
            format!(
                "{} {} {} {}\n",
                t.height,
                t.position,
                t.txid,
                hex::encode(&t.script)
            )
        })
        .collect();
    std::fs::write(path, text).expect("write txids.txt");
}

pub fn read_touches(path: &Path) -> Vec<Touch> {
    std::fs::read_to_string(path)
        .expect(
            "read txids.txt (an index from before it existed needs `chain-index rebuild-watched`)",
        )
        .lines()
        .map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            let [height, position, txid, script] = f[..] else {
                panic!("txids.txt line: {l}")
            };
            Touch {
                script: hex::decode(script).expect("script"),
                height: height.parse().expect("height"),
                position: position.parse().expect("position"),
                txid: txid.to_string(),
            }
        })
        .collect()
}
