//! lightwalletd's block cache: `lengths` (a u32 LE per block) and `blocks`
//! ([8-byte checksum][CompactBlock protobuf] per block, from height 0).

use std::fs::File;
use std::io::{BufReader, Read};
use std::os::unix::fs::FileExt;
use std::path::Path;

use crate::proto::{Block, parse_block};

pub struct Cache {
    blocks: File,
    /// Offset of each block's protobuf, just after its checksum.
    offsets: Vec<u64>,
    lens: Vec<u32>,
}

impl Cache {
    pub fn open(dir: &Path) -> Cache {
        let raw = std::fs::read(dir.join("lengths")).expect("read lengths");
        let (chunks, rest) = raw.as_chunks::<4>();
        assert!(rest.is_empty(), "lengths file has a partial entry");
        let lens: Vec<u32> = chunks.iter().map(|c| u32::from_le_bytes(*c)).collect();
        let mut offsets = Vec::with_capacity(lens.len());
        let mut at = 0u64;
        for &len in &lens {
            offsets.push(at + 8);
            at += 8 + u64::from(len);
        }
        let blocks = File::open(dir.join("blocks")).expect("open blocks");
        Cache {
            blocks,
            offsets,
            lens,
        }
    }

    pub fn tip(&self) -> u64 {
        self.lens.len() as u64 - 1
    }

    /// One block, read in place.
    pub fn block(&self, h: u64) -> Block {
        let i = usize::try_from(h).expect("height");
        let mut buf = vec![0u8; self.lens[i] as usize];
        self.blocks
            .read_exact_at(&mut buf, self.offsets[i])
            .expect("read block");
        let b = parse_block(&buf);
        assert_eq!(b.height, h, "cache out of order");
        b
    }

    /// Every block from 0 through `tip`, in order, read sequentially.
    pub fn scan(&self, dir: &Path, tip: u64, mut each: impl FnMut(Block)) {
        let mut f = BufReader::with_capacity(
            16 << 20,
            File::open(dir.join("blocks")).expect("open blocks"),
        );
        let mut buf = Vec::new();
        for h in 0..=tip {
            let mut checksum = [0u8; 8];
            f.read_exact(&mut checksum).expect("read checksum");
            buf.resize(self.lens[usize::try_from(h).expect("height")] as usize, 0);
            f.read_exact(&mut buf).expect("read block");
            let b = parse_block(&buf);
            assert_eq!(b.height, h, "cache out of order");
            each(b);
        }
    }
}
