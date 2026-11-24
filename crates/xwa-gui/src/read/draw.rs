//! Drawing a laid-out page on the panel: text in the reader's font,
//! characters and formulas from the archive's outlines, pictures, and the
//! lines of rules, tables and trees. Clicking a link opens it.

use iced::alignment;
use iced::mouse;
use iced::widget::canvas::{self, Action, Event, Frame, Geometry, path};
use iced::widget::text::Shaping;
use iced::{Color, Font, Pixels, Point, Rectangle, Renderer, Size, Theme, font};
use xwa_format::content::{MathItem, PathOp};
use xwa_format::math_glyphs::{GlyphOp, Outline};

use super::Wiki;
use super::layout::{Item, Laid, Page, Sty, Target};
use crate::eink::{INK, LIGHT};

/// The reader's font in `sty`.
pub fn font(sty: Sty) -> Font {
    Font {
        family: font::Family::Name("Noto Sans"),
        weight: if sty.bold {
            font::Weight::Bold
        } else {
            font::Weight::Normal
        },
        style: if sty.italic {
            font::Style::Italic
        } else {
            font::Style::Normal
        },
        ..Font::DEFAULT
    }
}

/// One page of an article, drawn by a canvas.
pub struct PageView<'a, M> {
    pub wiki: &'a Wiki,
    pub laid: &'a Laid,
    pub page: usize,
    /// What clicking a link (or a table's line) sends.
    pub on_link: fn(Target) -> M,
}

impl<M> PageView<'_, M> {
    fn page(&self) -> Option<&Page> {
        self.laid.pages.get(self.page)
    }

    fn link_at(&self, p: Point) -> Option<Target> {
        self.page()?
            .areas
            .iter()
            .find(|a| p.x >= a.x && p.x < a.x + a.w && p.y >= a.y && p.y < a.y + a.h)
            .map(|a| a.target)
    }
}

/// Adds a glyph outline (font units, y up) to `b`, scaled by `k` with its
/// origin at (x, y) on the page.
fn outline(b: &mut path::Builder, o: &Outline, x: f32, y: f32, kx: f32, ky: f32) {
    let p = |px: i16, py: i16| Point::new(x + px as f32 * kx, y - py as f32 * ky);
    for op in &o.ops {
        match *op {
            GlyphOp::Move(a, c) => b.move_to(p(a, c)),
            GlyphOp::Line(a, c) => b.line_to(p(a, c)),
            GlyphOp::Quad(a, c, d, e) => b.quadratic_curve_to(p(a, c), p(d, e)),
            GlyphOp::Cubic(a, c, d, e, f, g) => b.bezier_curve_to(p(a, c), p(d, e), p(f, g)),
            GlyphOp::Close => b.close(),
        }
    }
}

impl<M: Clone> canvas::Program<M> for PageView<'_, M> {
    type State = ();

    fn update(
        &self,
        _state: &mut (),
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<Action<M>> {
        if let Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) = event
            && let Some(p) = cursor.position_in(bounds)
            && let Some(link) = self.link_at(p)
        {
            return Some(Action::publish((self.on_link)(link)).and_capture());
        }
        None
    }

    fn mouse_interaction(
        &self,
        _state: &(),
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        match cursor.position_in(bounds).and_then(|p| self.link_at(p)) {
            Some(_) => mouse::Interaction::Pointer,
            None => mouse::Interaction::default(),
        }
    }

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let Some(page) = self.page() else {
            return vec![frame.into_geometry()];
        };
        let mut ink = path::Builder::new();
        for item in &page.items {
            match item {
                Item::Text { x, base, text, sty } => {
                    let f = &faces_metrics(*sty);
                    frame.fill_text(canvas::Text {
                        content: text.clone(),
                        position: Point::new(*x, base - f.0),
                        max_width: f32::INFINITY,
                        color: INK,
                        size: Pixels(sty.size),
                        line_height: iced::widget::text::LineHeight::Absolute(Pixels(f.0 + f.1)),
                        font: font(*sty),
                        align_x: iced::widget::text::Alignment::Left,
                        align_y: alignment::Vertical::Top,
                        shaping: Shaping::Advanced,
                    });
                }
                Item::Glyphs { x, base, text, sty } => {
                    let mut cx = *x;
                    for c in text.chars() {
                        let Some((font, glyph)) = self.wiki.fallback_glyph(c, sty.font_style())
                        else {
                            continue;
                        };
                        let k = sty.size / f32::from(font.upem.max(1));
                        // A slant where the archive has no italic glyph.
                        outline(&mut ink, &glyph.outline, cx, *base, k, k);
                        cx += f32::from(glyph.advance) * k;
                    }
                }
                Item::Fill { x, y, w, h, grey } => {
                    let colour = if *grey { LIGHT } else { INK };
                    if *grey {
                        frame.fill_rectangle(
                            Point::new(*x, *y),
                            Size::new(*w, *h),
                            Color { a: 0.45, ..colour },
                        );
                    } else {
                        frame.fill_rectangle(Point::new(*x, *y), Size::new(*w, *h), colour);
                    }
                }
                Item::Picture { x, y, w, h, image } => {
                    if let Some(p) = self.wiki.picture(*image) {
                        frame.draw_image(
                            Rectangle::new(Point::new(*x, *y), Size::new(*w, *h)),
                            canvas::Image::new(p.handle)
                                .filter_method(iced::widget::image::FilterMethod::Linear),
                        );
                    }
                }
                Item::Formula {
                    x,
                    base,
                    scale,
                    formula,
                } => {
                    if let (Some(f), Some(glyphs)) =
                        (self.laid.formulas.get(*formula), self.wiki.glyphs.as_ref())
                    {
                        formula_path(&mut ink, f, glyphs, *x, *base, *scale);
                    }
                }
            }
        }
        frame.fill(
            &ink.build(),
            canvas::Fill {
                style: canvas::Style::Solid(INK),
                rule: canvas::fill::Rule::NonZero,
            },
        );
        vec![frame.into_geometry()]
    }
}

/// The reader's font's ascent and descent at `sty`'s size.
fn faces_metrics(sty: Sty) -> (f32, f32) {
    (sty.ascent(), sty.descent())
}

/// A formula record's items as outlines, its origin at (x, base).
fn formula_path(
    b: &mut path::Builder,
    f: &xwa_format::content::Formula,
    glyphs: &xwa_format::math_glyphs::GlyphTable,
    x: f32,
    base: f32,
    scale: f32,
) {
    let (mut px, mut py) = (0i64, 0i64);
    let at = |px: i64, py: i64| (x + px as f32 * scale, base - py as f32 * scale);
    for item in &f.items {
        match item {
            MathItem::Glyph {
                glyph,
                scale: s,
                dx,
                dy,
            } => {
                px += *dx as i64;
                py += *dy as i64;
                let k =
                    glyphs.scales.get(*s as usize).copied().unwrap_or(256) as f32 / 256.0 * scale;
                if let Some(o) = glyphs.glyphs.get(*glyph as usize) {
                    let (gx, gy) = at(px, py);
                    outline(b, o, gx, gy, k, k);
                }
            }
            MathItem::GlyphXY {
                glyph,
                sx,
                sy,
                dx,
                dy,
            } => {
                px += *dx as i64;
                py += *dy as i64;
                if let Some(o) = glyphs.glyphs.get(*glyph as usize) {
                    let (gx, gy) = at(px, py);
                    outline(
                        b,
                        o,
                        gx,
                        gy,
                        *sx as f32 / 256.0 * scale,
                        *sy as f32 / 256.0 * scale,
                    );
                }
            }
            MathItem::Rule { dx, dy, w, h } => {
                px += *dx as i64;
                py += *dy as i64;
                let (rx, ry) = at(px, py);
                let (rw, rh) = (*w as f32 * scale, (*h as f32 * scale).max(1.0));
                b.rectangle(Point::new(rx, ry - rh), Size::new(rw, rh));
            }
            MathItem::Path { dx, dy, ops } => {
                px += *dx as i64;
                py += *dy as i64;
                let (ox, oy) = (px, py);
                let pt = |a: i32, c: i32| {
                    let (x, y) = at(ox + a as i64, oy + c as i64);
                    Point::new(x, y)
                };
                for op in ops {
                    match *op {
                        PathOp::Move(a, c) => b.move_to(pt(a, c)),
                        PathOp::Line(a, c) => b.line_to(pt(a, c)),
                        PathOp::Quad(a, c, d, e) => b.quadratic_curve_to(pt(a, c), pt(d, e)),
                        PathOp::Cubic(a, c, d, e, f, g) => {
                            b.bezier_curve_to(pt(a, c), pt(d, e), pt(f, g))
                        }
                        PathOp::Close => b.close(),
                    }
                }
            }
        }
    }
}
