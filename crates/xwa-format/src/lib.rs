//! The offline wiki archive format, draft v0 (see `docs/format-spec-v0.md`).
//!
//! This crate is the reference implementation the converter writes with and
//! the tests read back with. The firmware has its own C++ reader.

pub mod normalize;
pub mod title_index;
pub mod varint;

/// Block size for index blocks and section alignment.
pub const BLOCK_SIZE: usize = 4096;
