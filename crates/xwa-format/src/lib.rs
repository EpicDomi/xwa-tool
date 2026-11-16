//! The XWA offline wiki archive format, draft v0 (see `docs/format-spec-v0.md`).
//!
//! This crate is the reference implementation: the converter writes archives
//! with [`writer::ArchiveWriter`] and checks them with [`reader::Archive`].
//! The firmware has its own C++ reader.

pub mod codec;
pub mod content;
mod error;
pub mod la2;
pub mod math_glyphs;
pub mod math_svg;
pub mod meta;
pub mod normalize;
pub mod reader;
pub mod records;
pub mod superblock;
pub mod text;
pub mod title_index;
pub mod varint;
pub mod writer;

pub use error::{Error, Result};

#[cfg(test)]
mod tests;

/// Block size for index blocks and section alignment.
pub const BLOCK_SIZE: usize = 4096;

/// File signature, in the style of PNG's: catches text-mode and truncation damage.
pub const MAGIC: [u8; 8] = *b"XWA\0\r\n\x1a\n";

/// Draft format version written by this crate. While the major version is
/// 0, each minor version may change the layout, so readers take only their
/// own (spec 3.1).
pub const VERSION_MAJOR: u16 = 0;
pub const VERSION_MINOR: u16 = 2;

/// Section type codes (spec 3.2).
pub mod section {
    pub const META: u32 = 1;
    pub const ARTICLES: u32 = 2;
    pub const TITLES: u32 = 3;
    pub const TINDEX_LEAF: u32 = 4;
    pub const TINDEX_INNER: u32 = 5;
    pub const CHUNK_TABLE: u32 = 6;
    pub const BLOBS: u32 = 7;
    pub const IMAGE_TABLE: u32 = 8;
    pub const MATH_GLYPHS: u32 = 9;
    pub const ZSTD_DICT: u32 = 10;
    pub const FONT: u32 = 11;
    pub const WORD_INDEX: u32 = 12;
}

/// Required feature bits (spec 3.3): a reader lacking one must refuse the archive.
pub mod required {
    pub const ZSTD_TEXT: u32 = 1 << 0;
    pub const MATH: u32 = 1 << 1;
    pub const LINE_ART: u32 = 1 << 2;
    pub const JPEG: u32 = 1 << 3;
    pub const ALL_KNOWN: u32 = ZSTD_TEXT | MATH | LINE_ART | JPEG;
}

/// Optional feature bits (spec 3.3): safe to ignore.
pub mod optional {
    pub const FONT: u32 = 1 << 0;
    pub const WORD_INDEX: u32 = 1 << 1;
    pub const POPULARITY: u32 = 1 << 2;
    pub const HD_IMAGES: u32 = 1 << 3;
}

/// Device profile (spec 1.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    /// X4/X3: deflate everywhere, readable with uzlib.
    Compact,
    /// X4 Pro: zstd with a trained dictionary.
    Full,
}

impl Profile {
    pub fn code(self) -> u8 {
        match self {
            Profile::Compact => 0,
            Profile::Full => 1,
        }
    }

    pub fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Profile::Compact),
            1 => Some(Profile::Full),
            _ => None,
        }
    }
}

/// Codec used for content chunks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextCodec {
    Deflate,
    Zstd,
}

impl TextCodec {
    pub fn code(self) -> u8 {
        match self {
            TextCodec::Deflate => 0,
            TextCodec::Zstd => 1,
        }
    }

    pub fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(TextCodec::Deflate),
            1 => Some(TextCodec::Zstd),
            _ => None,
        }
    }
}
