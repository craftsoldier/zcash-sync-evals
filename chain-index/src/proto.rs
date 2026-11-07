//! Just enough protobuf to read lightwalletd's CompactBlocks: only varint and
//! length-delimited fields carry anything we use.

/// Iterates a message's fields as (field number, value).
pub struct Fields<'a>(pub &'a [u8]);

pub enum Val<'a> {
    Int(u64),
    Bytes(&'a [u8]),
}

fn varint(b: &mut &[u8]) -> u64 {
    let (mut v, mut shift) = (0u64, 0);
    loop {
        let byte = b[0];
        *b = &b[1..];
        v |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return v;
        }
        shift += 7;
    }
}

impl<'a> Iterator for Fields<'a> {
    type Item = (u64, Val<'a>);

    fn next(&mut self) -> Option<Self::Item> {
        if self.0.is_empty() {
            return None;
        }
        let key = varint(&mut self.0);
        let (field, wire) = (key >> 3, key & 7);
        let val = match wire {
            0 => Val::Int(varint(&mut self.0)),
            2 => {
                let n = usize::try_from(varint(&mut self.0)).expect("field length");
                let (v, rest) = self.0.split_at(n);
                self.0 = rest;
                Val::Bytes(v)
            }
            // Fixed-width fields: skipped, nothing here uses them.
            1 => {
                self.0 = &self.0[8..];
                Val::Int(0)
            }
            5 => {
                self.0 = &self.0[4..];
                Val::Int(0)
            }
            w => panic!("unexpected protobuf wire type {w}"),
        };
        Some((field, val))
    }
}

#[derive(Default)]
pub struct Block {
    pub height: u64,
    /// Block hash in protocol (little-endian) byte order.
    pub hash: Vec<u8>,
    pub time: u64,
    pub txs: Vec<Tx>,
    /// Sapling, Orchard and Ironwood tree sizes after this block, when the
    /// block's chainMetadata states them.
    pub sizes: [Option<u64>; 3],
}

#[derive(Default)]
pub struct Tx {
    pub txid: Vec<u8>,
    pub sapling: Vec<[u8; 32]>,
    pub orchard: Vec<[u8; 32]>,
    pub ironwood: Vec<[u8; 32]>,
    /// Spent outpoints: (txid, output index).
    pub vin: Vec<([u8; 32], u32)>,
    /// Outputs: (value in zatoshis, scriptPubKey).
    pub vout: Vec<(u64, Vec<u8>)>,
}

pub fn arr32(b: &[u8]) -> [u8; 32] {
    b.try_into().expect("32-byte field")
}

fn bytes_field(msg: &[u8], field: u64) -> Option<[u8; 32]> {
    Fields(msg).find_map(|(f, v)| match v {
        Val::Bytes(b) if f == field => Some(arr32(b)),
        _ => None,
    })
}

pub fn parse_block(buf: &[u8]) -> Block {
    let mut blk = Block::default();
    for (f, v) in Fields(buf) {
        match (f, v) {
            (2, Val::Int(h)) => blk.height = h,
            (3, Val::Bytes(b)) => blk.hash = b.to_vec(),
            (5, Val::Int(t)) => blk.time = t,
            (7, Val::Bytes(b)) => blk.txs.push(parse_tx(b)),
            (8, Val::Bytes(b)) => {
                for (f, v) in Fields(b) {
                    if let (1..=3, Val::Int(n)) = (f, v) {
                        blk.sizes[usize::try_from(f).expect("field") - 1] = Some(n);
                    }
                }
            }
            _ => {}
        }
    }
    blk
}

fn parse_tx(buf: &[u8]) -> Tx {
    let mut tx = Tx::default();
    for (f, v) in Fields(buf) {
        let Val::Bytes(b) = v else { continue };
        match f {
            2 => tx.txid = b.to_vec(),
            5 => tx.sapling.extend(bytes_field(b, 1)),
            6 => tx.orchard.extend(bytes_field(b, 2)),
            9 => tx.ironwood.extend(bytes_field(b, 2)),
            7 => {
                let (mut id, mut idx) = (None, 0u32);
                for (f, v) in Fields(b) {
                    match (f, v) {
                        (1, Val::Bytes(t)) => id = Some(arr32(t)),
                        (2, Val::Int(i)) => idx = u32::try_from(i).expect("output index"),
                        _ => {}
                    }
                }
                tx.vin.extend(id.map(|t| (t, idx)));
            }
            8 => {
                let (mut value, mut script) = (0u64, Vec::new());
                for (f, v) in Fields(b) {
                    match (f, v) {
                        (1, Val::Int(n)) => value = n,
                        (2, Val::Bytes(s)) => script = s.to_vec(),
                        _ => {}
                    }
                }
                tx.vout.push((value, script));
            }
            _ => {}
        }
    }
    tx
}
