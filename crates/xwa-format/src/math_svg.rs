//! A reference renderer: a formula record as SVG. It shows exactly how
//! positions, scales, rules and paths combine (spec 8), for checking
//! archives and as a model for device renderers.

use std::fmt::Write;

use crate::content::{Formula, MathItem, PathOp};
use crate::math_glyphs::{GlyphOp, GlyphTable};

fn glyph_path(ops: &[GlyphOp]) -> String {
    let mut d = String::new();
    for op in ops {
        let _ = match *op {
            GlyphOp::Move(x, y) => write!(d, "M{x} {y}"),
            GlyphOp::Line(x, y) => write!(d, "L{x} {y}"),
            GlyphOp::Quad(a, b, x, y) => write!(d, "Q{a} {b} {x} {y}"),
            GlyphOp::Cubic(a, b, c, e, x, y) => write!(d, "C{a} {b} {c} {e} {x} {y}"),
            GlyphOp::Close => write!(d, "Z"),
        };
    }
    d
}

fn item_path(ops: &[PathOp]) -> String {
    let mut d = String::new();
    for op in ops {
        let _ = match *op {
            PathOp::Move(x, y) => write!(d, "M{x} {y}"),
            PathOp::Line(x, y) => write!(d, "L{x} {y}"),
            PathOp::Quad(a, b, x, y) => write!(d, "Q{a} {b} {x} {y}"),
            PathOp::Cubic(a, b, c, e, x, y) => write!(d, "C{a} {b} {c} {e} {x} {y}"),
            PathOp::Close => write!(d, "Z"),
        };
    }
    d
}

/// The formula as a standalone SVG, `px_per_em` pixels per em.
pub fn formula_svg(f: &Formula, glyphs: &GlyphTable, px_per_em: f64) -> String {
    let upem = glyphs.upem.max(1) as f64;
    let (w, h) = (f.width.max(1), (f.ascent + f.descent).max(1));
    let mut out = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{:.1}" height="{:.1}" viewBox="0 {} {w} {h}"><g transform="scale(1,-1)">"#,
        w as f64 / upem * px_per_em,
        h as f64 / upem * px_per_em,
        -f.ascent,
    );
    // Item positions accumulate from the formula's origin; y points up,
    // which the group's flip turns into SVG's y down.
    let (mut x, mut y) = (0i64, 0i64);
    for item in &f.items {
        let _ = match item {
            MathItem::Glyph {
                glyph,
                scale,
                dx,
                dy,
            } => {
                x += *dx as i64;
                y += *dy as i64;
                let k = glyphs.scales.get(*scale as usize).copied().unwrap_or(256) as f64 / 256.0;
                let d = glyphs
                    .glyphs
                    .get(*glyph as usize)
                    .map(|g| glyph_path(&g.ops))
                    .unwrap_or_default();
                write!(
                    out,
                    r#"<path transform="translate({x} {y}) scale({k})" d="{d}"/>"#
                )
            }
            MathItem::GlyphXY {
                glyph,
                sx,
                sy,
                dx,
                dy,
            } => {
                x += *dx as i64;
                y += *dy as i64;
                let d = glyphs
                    .glyphs
                    .get(*glyph as usize)
                    .map(|g| glyph_path(&g.ops))
                    .unwrap_or_default();
                let (kx, ky) = (*sx as f64 / 256.0, *sy as f64 / 256.0);
                write!(
                    out,
                    r#"<path transform="translate({x} {y}) scale({kx} {ky})" d="{d}"/>"#
                )
            }
            MathItem::Rule { dx, dy, w, h } => {
                x += *dx as i64;
                y += *dy as i64;
                write!(out, r#"<rect x="{x}" y="{y}" width="{w}" height="{h}"/>"#)
            }
            MathItem::Path { dx, dy, ops } => {
                x += *dx as i64;
                y += *dy as i64;
                write!(
                    out,
                    r#"<path transform="translate({x} {y})" d="{}"/>"#,
                    item_path(ops)
                )
            }
        };
    }
    out.push_str("</g></svg>");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math_glyphs::Outline;

    #[test]
    fn positions_accumulate_and_y_points_up() {
        let glyphs = GlyphTable {
            upem: 1000,
            scales: vec![256, 179],
            glyphs: vec![Outline {
                bbox: [0, 0, 10, 10],
                ops: vec![GlyphOp::Move(0, 0), GlyphOp::Line(10, 0), GlyphOp::Close],
            }],
        };
        let f = Formula {
            display: false,
            width: 2000,
            ascent: 800,
            descent: 200,
            items: vec![
                MathItem::Glyph {
                    glyph: 0,
                    scale: 0,
                    dx: 0,
                    dy: 0,
                },
                MathItem::Glyph {
                    glyph: 0,
                    scale: 1,
                    dx: 500,
                    dy: 400,
                },
                MathItem::Rule {
                    dx: 100,
                    dy: -400,
                    w: 300,
                    h: 40,
                },
            ],
            fallback_image: None,
        };
        let svg = formula_svg(&f, &glyphs, 20.0);
        assert!(svg.starts_with(r#"<svg xmlns="http://www.w3.org/2000/svg" width="40.0" height="20.0" viewBox="0 -800 2000 1000">"#), "{svg}");
        assert!(
            svg.contains(r#"translate(500 400) scale(0.69921875)"#),
            "{svg}"
        );
        assert!(
            svg.contains(r#"<rect x="600" y="0" width="300" height="40"/>"#),
            "{svg}"
        );
    }
}
