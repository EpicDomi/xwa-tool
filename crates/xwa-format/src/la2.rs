//! LA2 line-art images (spec 9.2): `u16 width, u16 height, u8 bpp,
//! u8 codec`, then rows packed MSB-first and compressed as one stream.
//! Pixel values are ink levels: 0 is paper (white), the maximum is black.

use crate::error::corrupt;
use crate::{Error, Result};

pub const HEADER: usize = 6;

/// Pixel compression inside an LA2 image.
pub mod codec {
    pub const DEFLATE: u8 = 0;
    pub const ZSTD: u8 = 1;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub width: u16,
    pub height: u16,
    pub bpp: u8,
    pub codec: u8,
}

impl Header {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let b = data
            .get(..HEADER)
            .ok_or_else(|| corrupt("LA2 header truncated"))?;
        let h = Header {
            width: u16::from_le_bytes([b[0], b[1]]),
            height: u16::from_le_bytes([b[2], b[3]]),
            bpp: b[4],
            codec: b[5],
        };
        if h.bpp != 1 && h.bpp != 2 {
            return Err(corrupt(format!("LA2 with {} bits per pixel", h.bpp)));
        }
        if h.codec > codec::ZSTD {
            return Err(Error::Unsupported(format!("LA2 codec {}", h.codec)));
        }
        Ok(h)
    }

    /// Bytes per packed row.
    pub fn stride(&self) -> usize {
        (self.width as usize * self.bpp as usize).div_ceil(8)
    }

    pub fn max_level(&self) -> u8 {
        (1 << self.bpp) - 1
    }
}

/// A decoded image: one byte per pixel, ink level `0..=max_level`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub header: Header,
    pub levels: Vec<u8>,
}

/// Decompresses and unpacks an LA2 image.
pub fn decode(data: &[u8]) -> Result<Image> {
    let header = Header::parse(data)?;
    let size = header.stride() * header.height as usize;
    let packed = match header.codec {
        codec::DEFLATE => {
            let mut out = vec![0u8; size];
            let n = libdeflater::Decompressor::new()
                .deflate_decompress(&data[HEADER..], &mut out)
                .map_err(|e| corrupt(format!("LA2 deflate: {e}")))?;
            out.truncate(n);
            out
        }
        _ => zstd::bulk::decompress(&data[HEADER..], size)
            .map_err(|e| corrupt(format!("LA2 zstd: {e}")))?,
    };
    if packed.len() != size {
        return Err(corrupt(format!(
            "LA2 pixels are {} bytes, expected {size}",
            packed.len()
        )));
    }
    let (w, bpp) = (header.width as usize, header.bpp as usize);
    let mut levels = Vec::with_capacity(w * header.height as usize);
    if header.stride() > 0 {
        for row in packed.chunks(header.stride()) {
            for x in 0..w {
                let bit = x * bpp;
                levels.push((row[bit / 8] >> (8 - bpp - bit % 8)) & header.max_level());
            }
        }
    }
    Ok(Image { header, levels })
}

impl Image {
    /// The image as a binary PGM (white paper, black ink).
    pub fn to_pgm(&self) -> Vec<u8> {
        let max = self.header.max_level() as u16;
        let mut out =
            format!("P5\n{} {}\n255\n", self.header.width, self.header.height).into_bytes();
        out.extend(
            self.levels
                .iter()
                .map(|&v| (255 - v as u16 * 255 / max) as u8),
        );
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(width: u16, height: u16, bpp: u8, levels: &[u8]) -> Vec<u8> {
        let h = Header {
            width,
            height,
            bpp,
            codec: codec::DEFLATE,
        };
        let mut packed = vec![0u8; h.stride() * height as usize];
        for (i, &v) in levels.iter().enumerate() {
            let (y, x) = (i / width as usize, i % width as usize);
            let bit = x * bpp as usize;
            packed[y * h.stride() + bit / 8] |= v << (8 - bpp as usize - bit % 8);
        }
        let mut c = libdeflater::Compressor::new(libdeflater::CompressionLvl::default());
        let mut z = vec![0; c.deflate_compress_bound(packed.len())];
        let n = c.deflate_compress(&packed, &mut z).unwrap();
        let mut out = vec![];
        out.extend_from_slice(&width.to_le_bytes());
        out.extend_from_slice(&height.to_le_bytes());
        out.extend_from_slice(&[bpp, codec::DEFLATE]);
        out.extend_from_slice(&z[..n]);
        out
    }

    #[test]
    fn decodes_both_depths() {
        let levels: Vec<u8> = (0..5 * 3).map(|i| (i % 4) as u8).collect();
        let img = decode(&encode(5, 3, 2, &levels)).unwrap();
        assert_eq!(img.levels, levels);
        assert_eq!(&img.to_pgm()[..11], b"P5\n5 3\n255\n");
        assert_eq!(img.to_pgm()[11..14], [255, 170, 85]);
        let bits: Vec<u8> = (0..9 * 2).map(|i| (i % 3 == 0) as u8).collect();
        assert_eq!(decode(&encode(9, 2, 1, &bits)).unwrap().levels, bits);
    }

    #[test]
    fn rejects_bad_headers() {
        assert!(decode(&[1, 0, 1, 0, 3, 0]).is_err());
        assert!(decode(&[1, 0, 1, 0, 2, 7]).is_err());
        assert!(decode(&[1, 0]).is_err());
        let mut short = encode(5, 3, 2, &[0; 15]);
        short[0] = 9; // claims a wider image than the pixels hold
        assert!(decode(&short).is_err());
    }
}
