//! FONT section (spec 10): outline glyphs for the characters an archive's
//! text uses that the firmware's own fonts cannot draw.
//!
//! ```text
//! u16 upem, i16 ascender, i16 descender, u16 line_gap
//! u32 range_count, then per range (sorted, not overlapping):
//!     u32 first_code_point, u32 last_code_point, u32 first_glyph
//!     (glyph = first_glyph + code point - first_code_point)
//! u32 glyph_count
//! u16 advance[glyph_count]            (font units)
//! u32 glyph_offset[glyph_count + 1]   (from the start of the glyph data)
//! glyph data, as MATH_GLYPHS (spec 8.2)
//! ```

use std::collections::BTreeMap;

use crate::error::corrupt;
use crate::math_glyphs::{Outline, decode_outline, encode_outline};
use crate::{Error, Result};

/// The code points the firmware's built-in reader fonts draw (Bookerly
/// and Noto Sans, both sizes; spec 10), as inclusive ranges.
pub const BUILT_IN: &[(u32, u32)] = &[
    (0x0000, 0x0000),
    (0x000D, 0x000D),
    (0x0020, 0x007E),
    (0x00A0, 0x017F),
    (0x01A0, 0x01A1),
    (0x01AF, 0x01B0),
    (0x01C4, 0x01F5),
    (0x01F8, 0x021F),
    (0x0300, 0x0304),
    (0x0306, 0x030C),
    (0x030F, 0x030F),
    (0x0311, 0x0315),
    (0x031B, 0x031B),
    (0x0323, 0x0329),
    (0x032D, 0x032E),
    (0x0330, 0x0331),
    (0x0342, 0x0342),
    (0x0344, 0x0345),
    (0x0400, 0x045F),
    (0x0462, 0x0463),
    (0x0472, 0x0475),
    (0x048A, 0x04FF),
    (0x1EA0, 0x1EF9),
    (0x2000, 0x2030),
    (0x2032, 0x203C),
    (0x203E, 0x203E),
    (0x2043, 0x2044),
    (0x2047, 0x2049),
    (0x2053, 0x2053),
    (0x2057, 0x2057),
    (0x205F, 0x2064),
    (0x2070, 0x2070),
    (0x2074, 0x2079),
    (0x207F, 0x2089),
    (0x20A3, 0x20A4),
    (0x20A7, 0x20A7),
    (0x20AC, 0x20AC),
    (0x2212, 0x2212),
    (0xFB00, 0xFB06),
    (0xFFFD, 0xFFFD),
];

fn in_ranges(ranges: &[(u32, u32)], c: char) -> bool {
    let c = c as u32;
    ranges
        .binary_search_by(|&(lo, hi)| {
            if hi < c {
                std::cmp::Ordering::Less
            } else if lo > c {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// Whether the firmware's built-in fonts draw `c`.
pub fn is_built_in(c: char) -> bool {
    in_ranges(BUILT_IN, c)
}

/// Code points that draw nothing: control characters and Unicode's
/// default-ignorable code points (joiners, direction marks, variation
/// selectors, tags).
pub fn draws_nothing(c: char) -> bool {
    const IGNORABLE: &[(u32, u32)] = &[
        (0x00AD, 0x00AD),
        (0x034F, 0x034F),
        (0x061C, 0x061C),
        (0x115F, 0x1160),
        (0x17B4, 0x17B5),
        (0x180B, 0x180F),
        (0x200B, 0x200F),
        (0x202A, 0x202E),
        (0x2060, 0x206F),
        (0x3164, 0x3164),
        (0xFE00, 0xFE0F),
        (0xFEFF, 0xFEFF),
        (0xFFA0, 0xFFA0),
        (0xFFF0, 0xFFF8),
        (0x1BCA0, 0x1BCA3),
        (0x1D173, 0x1D17A),
        (0xE0000, 0xE0FFF),
    ];
    c.is_control() || in_ranges(IGNORABLE, c)
}

/// Whether FONT should have a glyph for `c` when the text uses it.
pub fn needs_fallback(c: char) -> bool {
    !is_built_in(c) && !draws_nothing(c)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Glyph {
    /// Advance width, in font units.
    pub advance: u16,
    pub outline: Outline,
}

/// A FONT section: line metrics and a glyph per code point.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Font {
    pub upem: u16,
    pub ascender: i16,
    pub descender: i16,
    pub line_gap: u16,
    pub glyphs: BTreeMap<char, Glyph>,
}

impl Font {
    /// The section's bytes: glyphs in code point order, one range per run
    /// of consecutive code points.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut ranges: Vec<(u32, u32, u32)> = Vec::new();
        for (i, &c) in self.glyphs.keys().enumerate() {
            match ranges.last_mut() {
                Some(r) if r.1 + 1 == c as u32 => r.1 = c as u32,
                _ => ranges.push((c as u32, c as u32, i as u32)),
            }
        }
        let mut data = Vec::new();
        let mut offsets = Vec::with_capacity(self.glyphs.len() + 1);
        for g in self.glyphs.values() {
            offsets.push(data.len() as u32);
            encode_outline(&g.outline, &mut data);
        }
        offsets.push(data.len() as u32);
        let n = self.glyphs.len();
        let mut out = Vec::with_capacity(16 + ranges.len() * 12 + n * 6 + data.len());
        out.extend_from_slice(&self.upem.to_le_bytes());
        out.extend_from_slice(&self.ascender.to_le_bytes());
        out.extend_from_slice(&self.descender.to_le_bytes());
        out.extend_from_slice(&self.line_gap.to_le_bytes());
        out.extend_from_slice(&(ranges.len() as u32).to_le_bytes());
        for (first, last, glyph) in &ranges {
            for v in [first, last, glyph] {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        out.extend_from_slice(&(n as u32).to_le_bytes());
        for g in self.glyphs.values() {
            out.extend_from_slice(&g.advance.to_le_bytes());
        }
        for o in offsets {
            out.extend_from_slice(&o.to_le_bytes());
        }
        out.extend_from_slice(&data);
        if out.len() > u32::MAX as usize {
            return Err(Error::Invalid("FONT is over 4 GB".into()));
        }
        Ok(out)
    }

    pub fn decode(b: &[u8]) -> Result<Font> {
        let bad = |what: &str| corrupt(format!("FONT: {what}"));
        let mut p = 0usize;
        let mut take = |n: usize| -> Result<&[u8]> {
            let s = p
                .checked_add(n)
                .and_then(|end| b.get(p..end))
                .ok_or_else(|| bad("truncated"))?;
            p += n;
            Ok(s)
        };
        let u16_at = |s: &[u8]| u16::from_le_bytes([s[0], s[1]]);
        let u32_at = |s: &[u8]| u32::from_le_bytes(s[..4].try_into().unwrap());
        let head = take(8)?;
        let (upem, ascender, descender, line_gap) = (
            u16_at(&head[0..]),
            u16_at(&head[2..]) as i16,
            u16_at(&head[4..]) as i16,
            u16_at(&head[6..]),
        );
        if upem == 0 {
            return Err(bad("upem is 0"));
        }
        let range_count = u32_at(take(4)?) as usize;
        let ranges: Vec<(u32, u32, u32)> = take(
            range_count
                .checked_mul(12)
                .ok_or_else(|| bad("too many ranges"))?,
        )?
        .chunks(12)
        .map(|r| (u32_at(&r[0..]), u32_at(&r[4..]), u32_at(&r[8..])))
        .collect();
        let count = u32_at(take(4)?) as usize;
        let advances: Vec<u16> = take(count.checked_mul(2).ok_or_else(|| bad("too many glyphs"))?)?
            .chunks(2)
            .map(u16_at)
            .collect();
        let offsets: Vec<usize> = take((count + 1) * 4)?
            .chunks(4)
            .map(|o| u32_at(o) as usize)
            .collect();
        let data = &b[p..];
        let mut glyphs = BTreeMap::new();
        let mut after = 0u32;
        for &(first, last, glyph) in &ranges {
            if first < after || last < first {
                return Err(bad("ranges out of order"));
            }
            after = last
                .checked_add(1)
                .ok_or_else(|| bad("range past U+10FFFF"))?;
            for cp in first..=last {
                let c = char::from_u32(cp).ok_or_else(|| bad("not a code point"))?;
                let i = (glyph as usize)
                    .checked_add((cp - first) as usize)
                    .filter(|&i| i < count)
                    .ok_or_else(|| bad("glyph out of range"))?;
                let g = data
                    .get(offsets[i]..offsets[i + 1])
                    .filter(|_| offsets[i] <= offsets[i + 1])
                    .ok_or_else(|| bad(&format!("glyph {i} out of range")))?;
                let outline = decode_outline(g).map_err(|e| bad(&format!("glyph {i}: {e}")))?;
                glyphs.insert(
                    c,
                    Glyph {
                        advance: advances[i],
                        outline,
                    },
                );
            }
        }
        Ok(Font {
            upem,
            ascender,
            descender,
            line_gap,
            glyphs,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math_glyphs::GlyphOp;

    fn square(size: i16) -> Outline {
        Outline {
            bbox: [0, 0, size, size],
            ops: vec![
                GlyphOp::Move(0, 0),
                GlyphOp::Line(size, 0),
                GlyphOp::Quad(size, size, 0, size),
                GlyphOp::Close,
            ],
        }
    }

    #[test]
    fn round_trips_with_a_range_per_run() {
        let mut f = Font {
            upem: 1000,
            ascender: 1069,
            descender: -293,
            line_gap: 0,
            ..Default::default()
        };
        for (c, size) in [
            ('α', 500),
            ('β', 510),
            ('γ', 520),
            ('中', 900),
            ('\u{3000}', 0),
        ] {
            let outline = if size == 0 {
                Outline::default()
            } else {
                square(size)
            };
            f.glyphs.insert(
                c,
                Glyph {
                    advance: size as u16 + 50,
                    outline,
                },
            );
        }
        let bytes = f.encode().unwrap();
        // Three runs: α-γ, U+3000, 中.
        assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()), 3);
        assert_eq!(Font::decode(&bytes).unwrap(), f);
        for cut in 0..bytes.len() {
            assert!(Font::decode(&bytes[..cut]).is_err() || cut == bytes.len());
        }
    }

    #[test]
    fn knows_what_the_firmware_draws() {
        for c in ['a', 'é', 'Ж', 'ữ', '€', '−', '\u{2019}'] {
            assert!(is_built_in(c), "{c}");
        }
        for c in ['α', 'ə', '→', '∞', 'ש', 'ك', 'क', '中', '한'] {
            assert!(needs_fallback(c), "{c}");
        }
        for c in ['\u{200D}', '\u{FE0F}', '\u{7}', '\u{2066}'] {
            assert!(!needs_fallback(c), "{c:?}");
        }
    }
}
