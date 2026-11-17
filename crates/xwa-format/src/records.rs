//! Fixed-size table records: articles (spec 5.1), chunks (7.1), images (9.1).

use crate::Result;
use crate::error::corrupt;

fn put_u48(out: &mut [u8], v: u64) {
    out.copy_from_slice(&v.to_le_bytes()[..6]);
}

fn put_u24(out: &mut [u8], v: u32) {
    out.copy_from_slice(&v.to_le_bytes()[..3]);
}

fn get_u48(b: &[u8]) -> u64 {
    let mut x = [0u8; 8];
    x[..6].copy_from_slice(b);
    u64::from_le_bytes(x)
}

fn get_u24(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], 0])
}

pub const U48_MAX: u64 = (1 << 48) - 1;
pub const U24_MAX: u32 = (1 << 24) - 1;

fn record<const N: usize>(table: &[u8], index: usize, what: &str) -> Result<[u8; N]> {
    table
        .get(index * N..(index + 1) * N)
        .map(|b| b.try_into().unwrap())
        .ok_or_else(|| corrupt(format!("{what} {index} out of range")))
}

/// Article flag bits.
pub mod article_flags {
    pub const INFOBOX: u16 = 1 << 0;
    pub const TABLES: u16 = 1 << 1;
    pub const MATH: u16 = 1 << 2;
    pub const IMAGES: u16 = 1 << 3;
    pub const DISAMBIGUATION: u16 = 1 << 4;
    pub const LIST: u16 = 1 << 5;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ArticleRecord {
    pub first_chunk: u32,
    pub offset_in_chunk: u16,
    pub chunk_count: u16,
    pub title_ref: u32,
    pub flags: u16,
    pub popularity: u8,
}

impl ArticleRecord {
    pub const SIZE: usize = 16;
    /// Where the popularity byte is in an encoded record.
    pub const POPULARITY: usize = 14;

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        b[0..4].copy_from_slice(&self.first_chunk.to_le_bytes());
        b[4..6].copy_from_slice(&self.offset_in_chunk.to_le_bytes());
        b[6..8].copy_from_slice(&self.chunk_count.to_le_bytes());
        b[8..12].copy_from_slice(&self.title_ref.to_le_bytes());
        b[12..14].copy_from_slice(&self.flags.to_le_bytes());
        b[Self::POPULARITY] = self.popularity;
        b
    }

    pub fn decode(table: &[u8], index: usize) -> Result<Self> {
        let b: [u8; Self::SIZE] = record(table, index, "article")?;
        Ok(Self {
            first_chunk: u32::from_le_bytes(b[0..4].try_into().unwrap()),
            offset_in_chunk: u16::from_le_bytes([b[4], b[5]]),
            chunk_count: u16::from_le_bytes([b[6], b[7]]),
            title_ref: u32::from_le_bytes(b[8..12].try_into().unwrap()),
            flags: u16::from_le_bytes([b[12], b[13]]),
            popularity: b[14],
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ChunkEntry {
    /// Offset within BLOBS.
    pub offset: u64,
    /// Compressed bytes plus the 4-byte CRC-32.
    pub stored_size: u32,
    pub uncompressed_size: u32,
}

impl ChunkEntry {
    pub const SIZE: usize = 12;

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        put_u48(&mut b[0..6], self.offset);
        put_u24(&mut b[6..9], self.stored_size);
        put_u24(&mut b[9..12], self.uncompressed_size);
        b
    }

    pub fn decode(table: &[u8], index: usize) -> Result<Self> {
        let b: [u8; Self::SIZE] = record(table, index, "chunk")?;
        Ok(Self {
            offset: get_u48(&b[0..6]),
            stored_size: get_u24(&b[6..9]),
            uncompressed_size: get_u24(&b[9..12]),
        })
    }
}

/// Image encodings (spec 9.2).
pub mod image_encoding {
    pub const JPEG: u8 = 1;
    pub const LINE_ART: u8 = 2;
}

/// Image kinds (spec 9.1).
pub mod image_kind {
    pub const PHOTO: u8 = 0;
    pub const LINE_ART: u8 = 1;
    pub const ICON: u8 = 2;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ImageRecord {
    /// Offset within BLOBS.
    pub offset: u64,
    pub size: u32,
    pub encoding: u8,
    pub width: u16,
    pub height: u16,
    pub kind: u8,
    pub flags: u8,
}

impl ImageRecord {
    pub const SIZE: usize = 16;

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        put_u48(&mut b[0..6], self.offset);
        put_u24(&mut b[6..9], self.size);
        b[9] = self.encoding;
        b[10..12].copy_from_slice(&self.width.to_le_bytes());
        b[12..14].copy_from_slice(&self.height.to_le_bytes());
        b[14] = self.kind;
        b[15] = self.flags;
        b
    }

    pub fn decode(table: &[u8], index: usize) -> Result<Self> {
        let b: [u8; Self::SIZE] = record(table, index, "image")?;
        Ok(Self {
            offset: get_u48(&b[0..6]),
            size: get_u24(&b[6..9]),
            encoding: b[9],
            width: u16::from_le_bytes([b[10], b[11]]),
            height: u16::from_le_bytes([b[12], b[13]]),
            kind: b[14],
            flags: b[15],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_round_trip() {
        let a = ArticleRecord {
            first_chunk: 70_000,
            offset_in_chunk: 32_000,
            chunk_count: 3,
            title_ref: 9,
            flags: 5,
            popularity: 200,
        };
        assert_eq!(ArticleRecord::decode(&a.encode(), 0).unwrap(), a);
        let c = ChunkEntry {
            offset: U48_MAX,
            stored_size: U24_MAX,
            uncompressed_size: 32_768,
        };
        assert_eq!(ChunkEntry::decode(&c.encode(), 0).unwrap(), c);
        let i = ImageRecord {
            offset: 1 << 40,
            size: 123_456,
            encoding: 2,
            width: 800,
            height: 480,
            kind: 1,
            flags: 1,
        };
        assert_eq!(ImageRecord::decode(&i.encode(), 0).unwrap(), i);
        assert!(ArticleRecord::decode(&a.encode(), 1).is_err());
    }
}
