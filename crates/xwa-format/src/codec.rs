//! Content chunk compression (spec 7.1): raw deflate for the compact
//! profile, zstd with an optional trained dictionary for the full profile.

use zstd::zstd_safe::CParameter;

use crate::error::corrupt;
use crate::{Error, Result, TextCodec};

/// Largest uncompressed chunk a reader must accept.
pub const MAX_CHUNK: usize = 32 * 1024;

/// Compression settings, shared by all worker threads.
#[derive(Debug, Clone)]
pub struct ChunkCodec {
    pub codec: TextCodec,
    /// libdeflate level (1-12) or zstd level (1-22).
    pub level: i32,
    /// zstd dictionary (full profile only).
    pub dict: Option<Vec<u8>>,
}

/// Sets a zstd context up for chunks: chunks carry their own CRC-32 and
/// the archive has one dictionary, so no checksum or dictionary ID.
///
/// The dictionary is always attached (searched where it lies) rather than
/// copied into the context: a copied dictionary is a prefix to the chunk,
/// and zstd finds different matches when the chunk happens to start in
/// memory where the dictionary ends, so the same text could compress
/// differently from run to run. Attached, it never can.
fn configure(ctx: &mut zstd::zstd_safe::CCtx, level: i32) -> Result<()> {
    let zstd =
        |code: usize| Error::Invalid(format!("zstd: {}", zstd::zstd_safe::get_error_name(code)));
    for p in [
        CParameter::CompressionLevel(level),
        CParameter::ChecksumFlag(false),
        CParameter::DictIdFlag(false),
        CParameter::ContentSizeFlag(true),
        CParameter::ForceAttachDict(zstd::zstd_safe::DictAttachPref::ForceAttach),
    ] {
        ctx.set_parameter(p).map_err(zstd)?;
    }
    Ok(())
}

/// A compressor for one thread.
pub enum Encoder {
    Deflate(libdeflater::Compressor),
    Zstd(Box<zstd::bulk::Compressor<'static>>),
}

impl ChunkCodec {
    pub fn encoder(&self) -> Result<Encoder> {
        match self.codec {
            TextCodec::Deflate => {
                let level = libdeflater::CompressionLvl::new(self.level)
                    .map_err(|_| Error::Invalid(format!("deflate level {} (1-12)", self.level)))?;
                Ok(Encoder::Deflate(libdeflater::Compressor::new(level)))
            }
            TextCodec::Zstd => {
                let mut c = match &self.dict {
                    Some(d) => zstd::bulk::Compressor::with_dictionary(self.level, d)?,
                    None => zstd::bulk::Compressor::new(self.level)?,
                };
                configure(c.context_mut(), self.level)?;
                Ok(Encoder::Zstd(Box::new(c)))
            }
        }
    }

    pub fn decoder(&self) -> Result<Decoder> {
        Ok(match self.codec {
            TextCodec::Deflate => Decoder::Deflate(libdeflater::Decompressor::new()),
            TextCodec::Zstd => Decoder::Zstd(Box::new(match &self.dict {
                Some(d) => zstd::bulk::Decompressor::with_dictionary(d)?,
                None => zstd::bulk::Decompressor::new()?,
            })),
        })
    }
}

impl Encoder {
    pub fn compress(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        match self {
            Encoder::Deflate(c) => {
                let mut out = vec![0; c.deflate_compress_bound(data.len())];
                let n = c
                    .deflate_compress(data, &mut out)
                    .map_err(|e| Error::Invalid(format!("deflate: {e}")))?;
                out.truncate(n);
                Ok(out)
            }
            Encoder::Zstd(c) => Ok(c.compress(data)?),
        }
    }
}

/// A decompressor for one thread.
pub enum Decoder {
    Deflate(libdeflater::Decompressor),
    Zstd(Box<zstd::bulk::Decompressor<'static>>),
}

impl Decoder {
    /// Decompresses `data`, which must expand to exactly `size` bytes.
    pub fn decompress(&mut self, data: &[u8], size: usize) -> Result<Vec<u8>> {
        let out = match self {
            Decoder::Deflate(d) => {
                let mut out = vec![0; size];
                let n = d
                    .deflate_decompress(data, &mut out)
                    .map_err(|e| corrupt(format!("deflate: {e}")))?;
                out.truncate(n);
                out
            }
            Decoder::Zstd(d) => d
                .decompress(data, size)
                .map_err(|e| corrupt(format!("zstd: {e}")))?,
        };
        if out.len() != size {
            return Err(corrupt(format!(
                "chunk expanded to {} bytes, expected {size}",
                out.len()
            )));
        }
        Ok(out)
    }
}

/// Trains a zstd dictionary on sample chunks. Returns `None` when there is
/// too little text to train on.
pub fn train_dictionary(samples: &[Vec<u8>], size: usize) -> Option<Vec<u8>> {
    let total: usize = samples.iter().map(Vec::len).sum();
    // zstd needs several times the dictionary size in samples to be useful.
    if samples.len() < 8 || total < size.saturating_mul(4) {
        return None;
    }
    zstd::dict::from_samples(samples, size).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(n: usize) -> Vec<u8> {
        let words = [
            "theorem ",
            "prime ",
            "number ",
            "the ",
            "of ",
            "Euler ",
            "proof ",
            "a ",
            "function ",
        ];
        let mut seed = 12345u32;
        let mut out = Vec::with_capacity(n);
        while out.len() < n {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            out.extend_from_slice(words[(seed >> 16) as usize % words.len()].as_bytes());
        }
        out.truncate(n);
        out
    }

    #[test]
    fn both_codecs_round_trip() {
        let samples: Vec<Vec<u8>> = (0..64).map(|i| text(4000 + i * 37)).collect();
        let dict = train_dictionary(&samples, 4096);
        assert!(dict.is_some());
        for codec in [
            ChunkCodec {
                codec: TextCodec::Deflate,
                level: 12,
                dict: None,
            },
            ChunkCodec {
                codec: TextCodec::Zstd,
                level: 19,
                dict: None,
            },
            ChunkCodec {
                codec: TextCodec::Zstd,
                level: 19,
                dict,
            },
        ] {
            let mut enc = codec.encoder().unwrap();
            let mut dec = codec.decoder().unwrap();
            for data in [Vec::new(), text(1), text(MAX_CHUNK)] {
                let packed = enc.compress(&data).unwrap();
                assert_eq!(dec.decompress(&packed, data.len()).unwrap(), data);
                if !data.is_empty() {
                    assert!(dec.decompress(&packed, data.len() + 1).is_err());
                }
            }
        }
    }

    #[test]
    fn a_chunk_compresses_the_same_wherever_it_lies_in_memory() {
        // zstd treats data that starts where the dictionary ends as one
        // with it, and finds different matches, so a chunk that happened
        // to be allocated right after a worker's copy of the dictionary
        // (as macOS's allocator can place it) came out differently. Put
        // one there on purpose.
        use zstd::zstd_safe::{CCtx, CDict};
        let samples: Vec<Vec<u8>> = (0..64).map(|i| text(4000 + i * 37)).collect();
        let dict = train_dictionary(&samples, 16 << 10).unwrap();
        let chunk = text(MAX_CHUNK);
        let mut joined = dict.clone();
        joined.extend_from_slice(&chunk);
        let (in_place, after) = joined.split_at(dict.len());
        for level in [3, 19] {
            let cdict = CDict::create_by_reference(in_place, level);
            let compress = |data: &[u8]| {
                let mut ctx = CCtx::create();
                configure(&mut ctx, level).unwrap();
                ctx.ref_cdict(&cdict).unwrap();
                let mut out = Vec::with_capacity(zstd::zstd_safe::compress_bound(data.len()));
                ctx.compress2(&mut out, data).unwrap();
                out
            };
            assert_eq!(compress(after), compress(&chunk), "level {level}");
        }
    }

    #[test]
    fn too_little_text_trains_no_dictionary() {
        assert!(train_dictionary(&[text(100)], 112_640).is_none());
    }
}
