//! Formulas drawn as vector math (spec 8): KaTeX's layout of the LaTeX
//! source each ZIM carries (via katex-native, a native port), turned into
//! glyph placements, rules and paths over KaTeX's own fonts.
//!
//! Measured on Wikipedia: 99.83% of the mathematics ZIM's formulas lay out
//! (most failures are `\mbox`, which is rewritten as `\text` here) and
//! 90% of the chemistry ZIM's (mhchem's `\ce` is not supported; those
//! formulas keep the text fallback, which suits chemical formulas).

use std::collections::HashMap;

use katex_native::katex::KatexContext;
use katex_native::{FONTS, Font, Item, PathCommand, Rect};
use ttf_parser::{Face, GlyphId, OutlineBuilder};
use xwa_format::content::{Formula, MathItem, PathOp};
use xwa_format::math_glyphs::{GlyphOp, GlyphTable, Outline};

/// Font units per em in formula records and MATH_GLYPHS.
pub const UPEM: u16 = 1000;
/// The MATH_GLYPHS size table (8.8 fixed point): 1.0, 0.7 and 0.5 em,
/// KaTeX's text, script and scriptscript sizes (99.99% of glyphs).
pub const SCALES: [u16; 3] = [256, 179, 128];

/// Lays out formulas; shared by all worker threads.
pub struct MathEngine {
    ctx: KatexContext,
    faces: Vec<Face<'static>>,
    /// First dense glyph id of each face: dense id = base + glyph index.
    bases: Vec<u32>,
    by_font: HashMap<Font, usize>,
    main: usize,
}

impl Default for MathEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl MathEngine {
    pub fn new() -> Self {
        let faces: Vec<Face<'static>> = FONTS
            .iter()
            .map(|f| Face::parse(f.data, 0).expect("KaTeX fonts parse"))
            .collect();
        let mut bases = Vec::with_capacity(faces.len());
        let mut next = 0u32;
        for f in &faces {
            bases.push(next);
            next += f.number_of_glyphs() as u32;
        }
        let by_font: HashMap<Font, usize> =
            FONTS.iter().enumerate().map(|(i, f)| (f.font, i)).collect();
        let main = by_font[&Font::default()];
        Self {
            ctx: KatexContext::default(),
            faces,
            bases,
            by_font,
            main,
        }
    }

    /// The font file for a face, or the family's regular file for faces
    /// KaTeX does not ship.
    fn face_index(&self, font: Font) -> usize {
        self.by_font
            .get(&font)
            .or_else(|| {
                self.by_font.get(&Font {
                    family: font.family,
                    bold: false,
                    italic: false,
                })
            })
            .copied()
            .unwrap_or(self.main)
    }

    /// Lays out `tex`. `None` when KaTeX cannot parse it or a character has
    /// no glyph in KaTeX's fonts; the caller then falls back to text.
    pub fn layout(&self, tex: &str, display: bool) -> Option<Formula> {
        let tex = tex.replace("\\mbox", "\\text");
        let layout = katex_native::layout(&self.ctx, &tex, display).ok()?;
        let mut b = Builder::default();
        for item in &layout.items {
            match item {
                Item::Text {
                    x,
                    y,
                    text,
                    font,
                    size,
                    ..
                } => {
                    let mut fi = self.face_index(*font);
                    let mut pen = *x;
                    for ch in text.chars() {
                        let gid = match self.faces[fi].glyph_index(ch) {
                            Some(g) => g,
                            None => {
                                fi = self.main;
                                self.faces[fi].glyph_index(ch)?
                            }
                        };
                        let face = &self.faces[fi];
                        let em = face.units_per_em() as f64;
                        if face.glyph_bounding_box(gid).is_some() {
                            b.glyph(self.bases[fi] + gid.0 as u32, *size, pen, *y);
                        }
                        pen += face.glyph_hor_advance(gid).unwrap_or(0) as f64 / em * size;
                    }
                }
                Item::Rect {
                    x,
                    y,
                    width,
                    height,
                    ..
                } => b.rule(*x, *y + *height, *width, *height),
                Item::Path { commands, clip, .. } => b.path(&clip_path(commands, clip)),
                Item::Line {
                    from,
                    to,
                    thickness,
                    ..
                } => b.line(*from, *to, *thickness),
                _ => {}
            }
        }
        Some(Formula {
            display,
            width: units(layout.width),
            ascent: units(layout.height),
            descent: units(layout.depth),
            items: b.items,
            fallback_image: None,
        })
    }

    /// The outline of a dense glyph id, in [`UPEM`] units.
    pub fn outline(&self, dense: u32) -> Outline {
        let fi = self.bases.partition_point(|&b| b <= dense) - 1;
        let face = &self.faces[fi];
        let scale = UPEM as f32 / face.units_per_em() as f32;
        let mut sink = OutlineSink {
            scale,
            ops: Vec::new(),
        };
        let gid = GlyphId((dense - self.bases[fi]) as u16);
        let bbox = face.outline_glyph(gid, &mut sink).map_or([0; 4], |r| {
            let s = |v: i16| (v as f32 * scale).round() as i16;
            [s(r.x_min), s(r.y_min), s(r.x_max), s(r.y_max)]
        });
        Outline {
            bbox,
            ops: sink.ops,
        }
    }

    /// The MATH_GLYPHS table for glyphs in the given (dense id) order.
    pub fn glyph_table(&self, dense_ids: &[u32]) -> GlyphTable {
        GlyphTable {
            upem: UPEM,
            scales: SCALES.to_vec(),
            glyphs: dense_ids.iter().map(|&d| self.outline(d)).collect(),
        }
    }
}

struct OutlineSink {
    scale: f32,
    ops: Vec<GlyphOp>,
}

impl OutlineSink {
    fn s(&self, v: f32) -> i16 {
        (v * self.scale).round() as i16
    }
}

impl OutlineBuilder for OutlineSink {
    fn move_to(&mut self, x: f32, y: f32) {
        self.ops.push(GlyphOp::Move(self.s(x), self.s(y)));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.ops.push(GlyphOp::Line(self.s(x), self.s(y)));
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.ops
            .push(GlyphOp::Quad(self.s(x1), self.s(y1), self.s(x), self.s(y)));
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let s = |v| self.s(v);
        self.ops
            .push(GlyphOp::Cubic(s(x1), s(y1), s(x2), s(y2), s(x), s(y)));
    }
    fn close(&mut self) {
        self.ops.push(GlyphOp::Close);
    }
}

/// Ems to font units.
fn units(em: f64) -> i32 {
    (em * UPEM as f64).round() as i32
}

/// Items with absolute positions (font units, y up), turned into the
/// relative positions formula records store.
#[derive(Default)]
struct Builder {
    items: Vec<MathItem>,
    last: (i32, i32),
}

impl Builder {
    fn delta(&mut self, x: i32, y: i32) -> (i32, i32) {
        let d = (x - self.last.0, y - self.last.1);
        self.last = (x, y);
        d
    }

    /// A glyph with its origin at (`x`, `y`) in KaTeX's ems (y down).
    fn glyph(&mut self, glyph: u32, size: f64, x: f64, y: f64) {
        let (dx, dy) = self.delta(units(x), -units(y));
        let scale = SCALES
            .iter()
            .position(|&s| (s as f64 / 256.0 - size).abs() < 0.005);
        self.items.push(match scale {
            Some(scale) => MathItem::Glyph {
                glyph,
                scale: scale as u8,
                dx,
                dy,
            },
            None => {
                let s = (size * 256.0).round() as i16;
                MathItem::GlyphXY {
                    glyph,
                    sx: s,
                    sy: s,
                    dx,
                    dy,
                }
            }
        });
    }

    /// A rectangle whose bottom-left corner is at (`x`, `bottom`) in ems (y down).
    fn rule(&mut self, x: f64, bottom: f64, width: f64, height: f64) {
        let (dx, dy) = self.delta(units(x), -units(bottom));
        self.items.push(MathItem::Rule {
            dx,
            dy,
            w: units(width).max(1) as u32,
            h: units(height).max(1) as u32,
        });
    }

    fn line(&mut self, from: (f64, f64), to: (f64, f64), thickness: f64) {
        let t = thickness / 2.0;
        if (from.1 - to.1).abs() < 1e-9 {
            self.rule(
                from.0.min(to.0),
                from.1 + t,
                (to.0 - from.0).abs(),
                thickness,
            );
        } else if (from.0 - to.0).abs() < 1e-9 {
            self.rule(
                from.0 - t,
                from.1.max(to.1),
                thickness,
                (to.1 - from.1).abs(),
            );
        } else {
            // A slanted stroke as a thin quadrilateral.
            let (vx, vy) = (to.0 - from.0, to.1 - from.1);
            let len = (vx * vx + vy * vy).sqrt();
            let (nx, ny) = (-vy / len * t, vx / len * t);
            let pts = [
                (from.0 + nx, from.1 + ny),
                (to.0 + nx, to.1 + ny),
                (to.0 - nx, to.1 - ny),
                (from.0 - nx, from.1 - ny),
            ];
            let mut ops = vec![PathCommand::MoveTo(pts[0].0, pts[0].1)];
            ops.extend(pts[1..].iter().map(|p| PathCommand::LineTo(p.0, p.1)));
            ops.push(PathCommand::Close);
            self.path(&ops);
        }
    }

    /// A filled path, already clipped, in ems (y down).
    fn path(&mut self, commands: &[PathCommand]) {
        let Some(origin) = commands.iter().find_map(|c| match c {
            PathCommand::MoveTo(x, y) => Some((units(*x), -units(*y))),
            _ => None,
        }) else {
            return;
        };
        let p = |x: &f64, y: &f64| (units(*x) - origin.0, -units(*y) - origin.1);
        let ops = commands
            .iter()
            .map(|c| match c {
                PathCommand::MoveTo(x, y) => {
                    let (x, y) = p(x, y);
                    PathOp::Move(x, y)
                }
                PathCommand::LineTo(x, y) => {
                    let (x, y) = p(x, y);
                    PathOp::Line(x, y)
                }
                PathCommand::CubicTo(ax, ay, bx, by, x, y) => {
                    let ((ax, ay), (bx, by), (x, y)) = (p(ax, ay), p(bx, by), p(x, y));
                    PathOp::Cubic(ax, ay, bx, by, x, y)
                }
                PathCommand::Close => PathOp::Close,
            })
            .collect();
        let (dx, dy) = self.delta(origin.0, origin.1);
        self.items.push(MathItem::Path { dx, dy, ops });
    }
}

/// One side of the clip box: points with `pick(axis) >= bound` (or `<=`)
/// are inside.
struct Side {
    axis: usize,
    bound: f64,
    keep_above: bool,
}

/// One edge of a polygon: its end point, and control points if it is a
/// cubic curve.
#[derive(Clone, Copy)]
struct Edge {
    to: (f64, f64),
    curve: Option<((f64, f64), (f64, f64))>,
}

/// Clips a filled path to `clip`, so devices need not clip. KaTeX draws
/// stretchy shapes (radical bars, arrow shafts) hundreds of ems long and
/// clips them to their box. Curves wholly inside the box are kept; the
/// rest are flattened and clipped (Sutherland-Hodgman per subpath).
fn clip_path(commands: &[PathCommand], clip: &Rect) -> Vec<PathCommand> {
    let (x0, y0, x1, y1) = (clip.x, clip.y, clip.x + clip.width, clip.y + clip.height);
    let eps = 1e-9;
    let inside =
        |p: (f64, f64)| p.0 >= x0 - eps && p.0 <= x1 + eps && p.1 >= y0 - eps && p.1 <= y1 + eps;
    let mut subpaths: Vec<((f64, f64), Vec<Edge>)> = Vec::new();
    let mut pen = (0.0, 0.0);
    for c in commands {
        match *c {
            PathCommand::MoveTo(x, y) => {
                pen = (x, y);
                subpaths.push((pen, Vec::new()));
            }
            PathCommand::LineTo(x, y) => {
                if subpaths.is_empty() {
                    subpaths.push((pen, Vec::new()));
                }
                subpaths.last_mut().unwrap().1.push(Edge {
                    to: (x, y),
                    curve: None,
                });
                pen = (x, y);
            }
            PathCommand::CubicTo(ax, ay, bx, by, x, y) => {
                if subpaths.is_empty() {
                    subpaths.push((pen, Vec::new()));
                }
                let edges = &mut subpaths.last_mut().unwrap().1;
                let (p0, a, b, p3) = (pen, (ax, ay), (bx, by), (x, y));
                if [p0, a, b, p3].into_iter().all(inside) {
                    edges.push(Edge {
                        to: p3,
                        curve: Some((a, b)),
                    });
                } else {
                    const STEPS: usize = 16;
                    for i in 1..=STEPS {
                        let t = i as f64 / STEPS as f64;
                        let u = 1.0 - t;
                        let f = |p: f64, q: f64, r: f64, s: f64| {
                            u * u * u * p
                                + 3.0 * u * u * t * q
                                + 3.0 * u * t * t * r
                                + t * t * t * s
                        };
                        edges.push(Edge {
                            to: (f(p0.0, a.0, b.0, p3.0), f(p0.1, a.1, b.1, p3.1)),
                            curve: None,
                        });
                    }
                }
                pen = p3;
            }
            PathCommand::Close => {
                if let Some((start, _)) = subpaths.last() {
                    pen = *start;
                }
            }
        }
    }
    let mut out = Vec::new();
    for (start, mut edges) in subpaths {
        // Close the polygon explicitly.
        if edges.last().is_none_or(|e| e.to != start) {
            edges.push(Edge {
                to: start,
                curve: None,
            });
        }
        // Clip against each side in turn: x >= x0, x <= x1, y >= y0, y <= y1.
        let sides = [
            Side {
                axis: 0,
                bound: x0,
                keep_above: true,
            },
            Side {
                axis: 0,
                bound: x1,
                keep_above: false,
            },
            Side {
                axis: 1,
                bound: y0,
                keep_above: true,
            },
            Side {
                axis: 1,
                bound: y1,
                keep_above: false,
            },
        ];
        for Side {
            axis,
            bound,
            keep_above,
        } in sides
        {
            if edges.is_empty() {
                break;
            }
            let pick = |p: (f64, f64)| if axis == 0 { p.0 } else { p.1 };
            // A point on the side, within rounding, counts as inside.
            let ok = |p: (f64, f64)| {
                if keep_above {
                    pick(p) >= bound - eps
                } else {
                    pick(p) <= bound + eps
                }
            };
            let cross = |s: (f64, f64), e: (f64, f64)| {
                let t = (bound - pick(s)) / (pick(e) - pick(s));
                (s.0 + t * (e.0 - s.0), s.1 + t * (e.1 - s.1))
            };
            let mut clipped = Vec::with_capacity(edges.len() + 4);
            let mut s = edges.last().unwrap().to;
            for e in &edges {
                match (ok(s), ok(e.to)) {
                    (true, true) => clipped.push(*e),
                    (false, true) => {
                        clipped.push(Edge {
                            to: cross(s, e.to),
                            curve: None,
                        });
                        clipped.push(Edge {
                            to: e.to,
                            curve: None,
                        });
                    }
                    (true, false) => clipped.push(Edge {
                        to: cross(s, e.to),
                        curve: None,
                    }),
                    (false, false) => {}
                }
                s = e.to;
            }
            edges = clipped;
        }
        if edges.len() < 3 {
            continue;
        }
        let first = edges.last().unwrap().to;
        out.push(PathCommand::MoveTo(first.0, first.1));
        for e in &edges {
            out.push(match e.curve {
                Some((a, b)) => PathCommand::CubicTo(a.0, a.1, b.0, b.1, e.to.0, e.to.1),
                None => PathCommand::LineTo(e.to.0, e.to.1),
            });
        }
        out.push(PathCommand::Close);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> &'static MathEngine {
        static ENGINE: std::sync::OnceLock<MathEngine> = std::sync::OnceLock::new();
        ENGINE.get_or_init(MathEngine::new)
    }

    #[test]
    fn pythagoras_is_glyphs_at_three_sizes() {
        let f = engine()
            .layout(r"{\displaystyle a^{2}+b^{2}=c^{2}.}", false)
            .unwrap();
        assert!(!f.display);
        assert_eq!(f.items.len(), 9, "{:?}", f.items);
        let scales: Vec<u8> = f
            .items
            .iter()
            .filter_map(|i| {
                if let MathItem::Glyph { scale, .. } = i {
                    Some(*scale)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(scales, [0, 1, 0, 0, 1, 0, 0, 1, 0]);
        // The superscript sits above the baseline.
        let MathItem::Glyph { dy, .. } = f.items[1] else {
            panic!()
        };
        assert!(dy > 300, "{dy}");
        assert!(f.width > 4000 && f.ascent > 600 && f.descent >= 0, "{f:?}");
    }

    #[test]
    fn fractions_have_rules_and_roots_have_clipped_paths() {
        let f = engine()
            .layout(r"x={\frac {-b\pm {\sqrt {b^{2}-4ac}}}{2a}}", true)
            .unwrap();
        assert!(f.items.iter().any(|i| matches!(i, MathItem::Rule { .. })));
        let path = f
            .items
            .iter()
            .find_map(|i| {
                if let MathItem::Path { ops, .. } = i {
                    Some(ops)
                } else {
                    None
                }
            })
            .unwrap();
        // The radical's bar is drawn 400 em long and clipped to the formula.
        let xs = path.iter().flat_map(|op| match *op {
            PathOp::Move(x, _) | PathOp::Line(x, _) => vec![x],
            PathOp::Cubic(a, _, b, _, x, _) => vec![a, b, x],
            _ => vec![],
        });
        assert!(
            xs.into_iter().all(|x| x.abs() < f.width + 10),
            "path not clipped"
        );
    }

    #[test]
    fn outlines_and_table() {
        let f = engine().layout("x", false).unwrap();
        let MathItem::Glyph { glyph, .. } = f.items[0] else {
            panic!()
        };
        let table = engine().glyph_table(&[glyph]);
        let o = &table.glyphs[0];
        assert!(o.ops.len() > 4 && matches!(o.ops[0], GlyphOp::Move(..)));
        assert!(o.bbox[2] > o.bbox[0]);
        let bytes = xwa_format::math_glyphs::encode(&table).unwrap();
        assert_eq!(xwa_format::math_glyphs::decode(&bytes).unwrap(), table);
    }

    #[test]
    fn unsupported_input_returns_none() {
        assert!(engine().layout(r"\ce{H2O}", false).is_none());
        assert!(engine().layout(r"\frac{", false).is_none());
        assert!(engine().layout(r"\mbox{ok}", false).is_some());
    }

    #[test]
    fn clipping_keeps_inside_curves_and_cuts_long_bars() {
        let clip = Rect {
            x: 0.0,
            y: -1.0,
            width: 2.0,
            height: 1.0,
        };
        let cmds = [
            PathCommand::MoveTo(0.1, -0.1),
            PathCommand::CubicTo(0.2, -0.9, 0.3, -0.9, 0.4, -0.1),
            PathCommand::LineTo(400.0, -0.1),
            PathCommand::LineTo(400.0, -0.2),
            PathCommand::LineTo(0.1, -0.2),
            PathCommand::Close,
        ];
        let out = clip_path(&cmds, &clip);
        assert!(
            out.iter().any(|c| matches!(c, PathCommand::CubicTo(..))),
            "inside curve kept"
        );
        for c in &out {
            if let PathCommand::LineTo(x, _) | PathCommand::MoveTo(x, _) = c {
                assert!(*x <= 2.0 + 1e-6, "{x}");
            }
        }
    }
}
