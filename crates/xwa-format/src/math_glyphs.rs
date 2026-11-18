//! MATH_GLYPHS section (spec 8.2): the glyph outlines and size table that
//! formula items refer to.
//!
//! ```text
//! u16 upem, u16 glyph_count, u8 scale_count, u16 scales[scale_count] (8.8)
//! u32 glyph_offset[glyph_count + 1]   (from the start of the glyph data)
//! per glyph: i16 xmin, ymin, xmax, ymax, then commands
//!   0 MOVE x y | 1 LINE x y | 2 QUAD cx cy x y | 3 CUBIC c1x c1y c2x c2y x y
//!   4 CLOSE | 5 END                    (i16, font units, y up)
//! ```

use crate::error::corrupt;
use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlyphOp {
    Move(i16, i16),
    Line(i16, i16),
    Quad(i16, i16, i16, i16),
    Cubic(i16, i16, i16, i16, i16, i16),
    Close,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Outline {
    /// xmin, ymin, xmax, ymax.
    pub bbox: [i16; 4],
    pub ops: Vec<GlyphOp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlyphTable {
    pub upem: u16,
    /// Scale factors (8.8 fixed point) that `Glyph` items index.
    pub scales: Vec<u16>,
    pub glyphs: Vec<Outline>,
}

fn put_i16s(out: &mut Vec<u8>, values: &[i16]) {
    for v in values {
        out.extend_from_slice(&v.to_le_bytes());
    }
}

pub(crate) fn encode_outline(o: &Outline, out: &mut Vec<u8>) {
    put_i16s(out, &o.bbox);
    for op in &o.ops {
        match *op {
            GlyphOp::Move(x, y) => {
                out.push(0);
                put_i16s(out, &[x, y]);
            }
            GlyphOp::Line(x, y) => {
                out.push(1);
                put_i16s(out, &[x, y]);
            }
            GlyphOp::Quad(cx, cy, x, y) => {
                out.push(2);
                put_i16s(out, &[cx, cy, x, y]);
            }
            GlyphOp::Cubic(ax, ay, bx, by, x, y) => {
                out.push(3);
                put_i16s(out, &[ax, ay, bx, by, x, y]);
            }
            GlyphOp::Close => out.push(4),
        }
    }
    out.push(5);
}

pub fn encode(t: &GlyphTable) -> Result<Vec<u8>> {
    let count = u16::try_from(t.glyphs.len())
        .map_err(|_| Error::Invalid(format!("{} math glyphs", t.glyphs.len())))?;
    let scales = u8::try_from(t.scales.len())
        .map_err(|_| Error::Invalid(format!("{} math scales", t.scales.len())))?;
    let mut data = Vec::new();
    let mut offsets = Vec::with_capacity(t.glyphs.len() + 1);
    for g in &t.glyphs {
        offsets.push(data.len() as u32);
        encode_outline(g, &mut data);
    }
    offsets.push(data.len() as u32);
    let mut out = Vec::with_capacity(5 + t.scales.len() * 2 + offsets.len() * 4 + data.len());
    out.extend_from_slice(&t.upem.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.push(scales);
    for s in &t.scales {
        out.extend_from_slice(&s.to_le_bytes());
    }
    for o in offsets {
        out.extend_from_slice(&o.to_le_bytes());
    }
    out.extend_from_slice(&data);
    Ok(out)
}

pub fn decode(b: &[u8]) -> Result<GlyphTable> {
    let mut p = 0usize;
    let mut take = |n: usize| -> Result<&[u8]> {
        let s = b
            .get(p..p + n)
            .ok_or_else(|| corrupt("MATH_GLYPHS truncated"))?;
        p += n;
        Ok(s)
    };
    let u16_of = |s: &[u8]| u16::from_le_bytes([s[0], s[1]]);
    let upem = u16_of(take(2)?);
    let count = u16_of(take(2)?) as usize;
    let scale_count = take(1)?[0] as usize;
    let scales = take(scale_count * 2)?.chunks(2).map(u16_of).collect();
    let offsets: Vec<usize> = take((count + 1) * 4)?
        .chunks(4)
        .map(|c| u32::from_le_bytes(c.try_into().unwrap()) as usize)
        .collect();
    let data = &b[p..];
    let mut glyphs = Vec::with_capacity(count);
    for i in 0..count {
        let g = data
            .get(offsets[i]..offsets[i + 1])
            .filter(|_| offsets[i] <= offsets[i + 1])
            .ok_or_else(|| corrupt(format!("math glyph {i} out of range")))?;
        glyphs.push(decode_outline(g).map_err(|e| corrupt(format!("math glyph {i}: {e}")))?);
    }
    Ok(GlyphTable {
        upem,
        scales,
        glyphs,
    })
}

fn i16s(g: &[u8], p: &mut usize, n: usize) -> std::result::Result<Vec<i16>, String> {
    let s = g.get(*p..*p + n * 2).ok_or("truncated")?;
    *p += n * 2;
    Ok(s.chunks(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect())
}

pub(crate) fn decode_outline(g: &[u8]) -> std::result::Result<Outline, String> {
    let mut p = 0usize;
    let bb = i16s(g, &mut p, 4)?;
    let mut o = Outline {
        bbox: [bb[0], bb[1], bb[2], bb[3]],
        ops: Vec::new(),
    };
    loop {
        let op = *g.get(p).ok_or("missing END")?;
        p += 1;
        o.ops.push(match op {
            0 => {
                let a = i16s(g, &mut p, 2)?;
                GlyphOp::Move(a[0], a[1])
            }
            1 => {
                let a = i16s(g, &mut p, 2)?;
                GlyphOp::Line(a[0], a[1])
            }
            2 => {
                let a = i16s(g, &mut p, 4)?;
                GlyphOp::Quad(a[0], a[1], a[2], a[3])
            }
            3 => {
                let a = i16s(g, &mut p, 6)?;
                GlyphOp::Cubic(a[0], a[1], a[2], a[3], a[4], a[5])
            }
            4 => GlyphOp::Close,
            5 => break,
            k => return Err(format!("unknown command {k}")),
        });
    }
    if p != g.len() {
        return Err("bytes after END".into());
    }
    Ok(o)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_rejects_damage() {
        let t = GlyphTable {
            upem: 1000,
            scales: vec![256, 179, 128],
            glyphs: vec![
                Outline::default(),
                Outline {
                    bbox: [-10, -200, 500, 700],
                    ops: vec![
                        GlyphOp::Move(0, 0),
                        GlyphOp::Line(500, 0),
                        GlyphOp::Quad(250, 700, 0, 0),
                        GlyphOp::Cubic(1, 2, 3, 4, 5, 6),
                        GlyphOp::Close,
                    ],
                },
            ],
        };
        let b = encode(&t).unwrap();
        assert_eq!(decode(&b).unwrap(), t);
        assert_eq!(
            u16::from_le_bytes([b[2], b[3]]),
            2,
            "readers find the glyph count at bytes 2-3"
        );
        for cut in 0..b.len() {
            assert!(decode(&b[..cut]).is_err());
        }
    }
}
