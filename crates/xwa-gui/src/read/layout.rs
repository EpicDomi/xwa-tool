//! Laying an article out in pages, as the e-reader does: its blocks become
//! lines at the screen's width, and lines fill pages. Text is measured with
//! the reader's font (Noto Sans) and, for characters the e-reader's fonts
//! lack, with the archive's own fallback font, which draws them.

use std::sync::OnceLock;

use xwa_format::content::{
    Article, Block, Formula, Infobox, InfoboxRow, Inline, ListKind, Place, Style, Table, Tree,
    TreeNode,
};
use xwa_format::font::{FontStyle, needs_fallback};

use super::Wiki;

/// Body text size, as the e-reader's default (in the device's pixels).
pub const BODY: f32 = 19.0;
const SMALL: f32 = 16.0;
const LARGE: f32 = 25.0;
/// Line height, as a multiple of the font size.
const SPACING: f32 = 1.32;

struct Faces([ttf_parser::Face<'static>; 4]);

fn faces() -> &'static Faces {
    static FACES: OnceLock<Faces> = OnceLock::new();
    FACES.get_or_init(|| {
        let f = |b: &'static [u8]| ttf_parser::Face::parse(b, 0).expect("bundled font");
        Faces([
            f(crate::eink::NOTO_SANS),
            f(crate::eink::NOTO_SANS_BOLD),
            f(crate::eink::NOTO_SANS_ITALIC),
            f(crate::eink::NOTO_SANS_BOLD_ITALIC),
        ])
    })
}

/// How a piece of text is drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sty {
    pub bold: bool,
    pub italic: bool,
    pub size: f32,
    /// How far above the baseline (superscript) or below (negative).
    pub rise: f32,
}

impl Sty {
    fn new(size: f32) -> Self {
        Sty {
            bold: false,
            italic: false,
            size,
            rise: 0.0,
        }
    }

    pub fn font_style(self) -> FontStyle {
        FontStyle::new(self.bold, self.italic)
    }

    fn face(self) -> &'static ttf_parser::Face<'static> {
        &faces().0[match (self.bold, self.italic) {
            (false, false) => 0,
            (true, false) => 1,
            (false, true) => 2,
            (true, true) => 3,
        }]
    }

    pub fn ascent(self) -> f32 {
        let f = self.face();
        f32::from(f.ascender()) / f32::from(f.units_per_em()) * self.size
    }

    pub fn descent(self) -> f32 {
        let f = self.face();
        -f32::from(f.descender()) / f32::from(f.units_per_em()) * self.size
    }
}

/// Where a link leads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Link {
    pub article: u32,
    pub place: Place,
}

/// Something drawn on a page, in page coordinates.
#[derive(Debug, Clone)]
pub enum Item {
    /// Text in the reader's font; `base` is its baseline.
    Text {
        x: f32,
        base: f32,
        text: String,
        sty: Sty,
    },
    /// Characters drawn from the archive's fallback font.
    Glyphs {
        x: f32,
        base: f32,
        text: String,
        sty: Sty,
    },
    /// A filled rectangle (rules, borders, tree branches); `grey` for the
    /// light fill under table headings.
    Fill {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        grey: bool,
    },
    Picture {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        image: u32,
    },
    /// A formula from the layout's list, drawn `scale` pixels per unit.
    Formula {
        x: f32,
        base: f32,
        scale: f32,
        formula: usize,
    },
}

impl Item {
    fn shift(&mut self, dx: f32, dy: f32) {
        match self {
            Item::Text { x, base, .. }
            | Item::Glyphs { x, base, .. }
            | Item::Formula { x, base, .. } => {
                *x += dx;
                *base += dy;
            }
            Item::Fill { x, y, .. } | Item::Picture { x, y, .. } => {
                *x += dx;
                *y += dy;
            }
        }
    }
}

/// What clicking somewhere on a page opens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Target {
    Link(Link),
    /// A table too wide for the page, in a view of its own.
    Table(u32),
}

/// A link's place on a page.
#[derive(Debug, Clone, Copy)]
pub struct Area {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub target: Target,
}

#[derive(Debug, Clone, Default)]
pub struct Page {
    pub items: Vec<Item>,
    pub areas: Vec<Area>,
}

/// An article in pages.
#[derive(Debug, Clone, Default)]
pub struct Laid {
    pub pages: Vec<Page>,
    pub formulas: Vec<Formula>,
    /// The page each contents entry (heading) starts on, in order.
    pub section_pages: Vec<usize>,
}

/// One line of a block, before it is placed on a page; y from its top.
#[derive(Debug, Clone, Default)]
struct Line {
    h: f32,
    /// Space above, left out at the top of a page.
    gap: f32,
    /// Not to end a page (a heading).
    keep: bool,
    items: Vec<Item>,
    areas: Vec<Area>,
    /// The contents entry (heading) that starts here.
    section: Option<usize>,
}

impl Line {
    fn shift(&mut self, dx: f32) {
        for i in &mut self.items {
            i.shift(dx, 0.0);
        }
        for a in &mut self.areas {
            a.x += dx;
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Align {
    Justify,
    Start,
    Centre,
}

/// One unit of text layout.
enum Atom {
    Word {
        text: String,
        sty: Sty,
        link: Option<Link>,
        w: f32,
    },
    Space {
        w: f32,
        link: Option<Link>,
    },
    Icon {
        image: u32,
        w: f32,
        h: f32,
        link: Option<Link>,
    },
    Math {
        formula: usize,
        scale: f32,
        w: f32,
        asc: f32,
        desc: f32,
        link: Option<Link>,
    },
    Break,
}

impl Atom {
    fn width(&self) -> f32 {
        match self {
            Atom::Word { w, .. }
            | Atom::Space { w, .. }
            | Atom::Icon { w, .. }
            | Atom::Math { w, .. } => *w,
            Atom::Break => 0.0,
        }
    }
}

struct Cx<'a> {
    wiki: &'a Wiki,
    page_h: f32,
    formulas: Vec<Formula>,
    section: usize,
}

/// Whether the archive's font draws `c` in `sty` (the e-reader's own fonts
/// lack it).
fn from_archive(wiki: &Wiki, c: char, sty: Sty) -> bool {
    needs_fallback(c) && wiki.fallback_glyph(c, sty.font_style()).is_some()
}

fn char_width(wiki: &Wiki, c: char, sty: Sty) -> f32 {
    if needs_fallback(c)
        && let Some((font, glyph)) = wiki.fallback_glyph(c, sty.font_style())
    {
        return f32::from(glyph.advance) / f32::from(font.upem.max(1)) * sty.size;
    }
    let f = sty.face();
    match f.glyph_index(c).and_then(|g| f.glyph_hor_advance(g)) {
        Some(a) => f32::from(a) / f32::from(f.units_per_em()) * sty.size,
        None if xwa_format::font::draws_nothing(c) => 0.0,
        None => sty.size * 0.6,
    }
}

fn text_width(wiki: &Wiki, s: &str, sty: Sty) -> f32 {
    s.chars().map(|c| char_width(wiki, c, sty)).sum()
}

/// Lays `article` out in pages `width` wide and `height` tall.
pub fn lay_out(wiki: &Wiki, article: &Article, width: f32, height: f32) -> Laid {
    let mut cx = Cx {
        wiki,
        page_h: height,
        formulas: Vec::new(),
        section: 0,
    };
    let mut lines = Vec::new();
    // The title, as the e-reader starts an article.
    let title = cx.flow(
        &[Inline::Text(article.title.clone())],
        Sty {
            bold: true,
            ..Sty::new(LARGE + 3.0)
        },
        0.0,
        width,
        Align::Start,
    );
    lines.extend(title);
    lines.push(rule_line(0.0, width, 2.0, 6.0));
    for b in &article.body {
        cx.block(article, b, 0.0, width, 0, &mut lines);
    }
    paginate(lines, height, cx.formulas, cx.section)
}

const TABLE_PAD: f32 = 4.0;

/// A table's cells placed in its grid (row, column, columns spanned), its
/// last row, and each column's least and most width.
struct Plan<'t> {
    placed: Vec<(usize, usize, usize, &'t xwa_format::content::Cell)>,
    last_row: usize,
    least: Vec<f32>,
    most: Vec<f32>,
}

/// A table on its own, as wide as it needs (at least `min_w`, at most
/// a few screens): its layout on one page, and its width and height.
pub fn lay_out_table(wiki: &Wiki, t: &Table, min_w: f32) -> (Laid, f32, f32) {
    let mut cx = Cx {
        wiki,
        page_h: f32::INFINITY,
        formulas: Vec::new(),
        section: 0,
    };
    // Wide enough that no column breaks inside a word, and no wider than
    // its text on one line, within a few screens.
    let plan = cx.plan(t);
    let (least, most): (f32, f32) = (plan.least.iter().sum(), plan.most.iter().sum());
    let w = least.max(most.min(min_w * 1.5)).clamp(min_w, min_w * 5.0);
    let mut lines = Vec::new();
    cx.table(t, None, 0.0, w, &mut lines);
    let h: f32 = lines.iter().map(|l| l.h + l.gap).sum();
    let laid = paginate(lines, f32::INFINITY, cx.formulas, 0);
    (laid, w, h)
}

fn rule_line(x: f32, w: f32, thickness: f32, gap: f32) -> Line {
    Line {
        h: thickness + 4.0,
        gap,
        items: vec![Item::Fill {
            x,
            y: 2.0,
            w,
            h: thickness,
            grey: false,
        }],
        ..Line::default()
    }
}

fn paginate(lines: Vec<Line>, page_h: f32, formulas: Vec<Formula>, sections: usize) -> Laid {
    let mut pages = vec![Page::default()];
    let mut section_pages = vec![0; sections];
    let mut y = 0.0f32;
    let n = lines.len();
    for i in 0..n {
        let line = &lines[i];
        let gap = if y > 0.0 { line.gap } else { 0.0 };
        // A heading goes with the line after it.
        let need = if line.keep && i + 1 < n {
            line.h + lines[i + 1].gap + lines[i + 1].h
        } else {
            line.h
        };
        if y > 0.0 && y + gap + need.min(page_h) > page_h {
            pages.push(Page::default());
            y = 0.0;
        }
        let top = if y > 0.0 { y + gap } else { 0.0 };
        if let Some(s) = line.section {
            section_pages[s] = pages.len() - 1;
        }
        let page = pages.last_mut().unwrap();
        for item in &line.items {
            let mut it = item.clone();
            it.shift(0.0, top);
            page.items.push(it);
        }
        for a in &line.areas {
            let mut a = *a;
            a.y += top;
            page.areas.push(a);
        }
        y = top + line.h;
    }
    Laid {
        pages,
        formulas,
        section_pages,
    }
}

impl Cx<'_> {
    fn gap(&self) -> f32 {
        BODY * 0.55
    }

    fn block(
        &mut self,
        article: &Article,
        b: &Block,
        x: f32,
        w: f32,
        depth: usize,
        out: &mut Vec<Line>,
    ) {
        let first = out.len();
        let gap = self.gap();
        match b {
            Block::Para(run) => {
                out.extend(self.flow(run, Sty::new(BODY), x, w, Align::Justify));
            }
            Block::Heading { level, content } => {
                let sty = Sty {
                    bold: true,
                    italic: *level >= 5,
                    ..Sty::new(if *level <= 2 { LARGE } else { BODY + 1.0 })
                };
                let mut lines = self.flow(content, sty, x, w, Align::Start);
                if *level <= 2 {
                    lines.push(rule_line(x, w, 1.0, 0.0));
                }
                for l in &mut lines {
                    l.keep = true;
                }
                if depth == 0 {
                    if let Some(l) = lines.first_mut() {
                        l.section = Some(self.section);
                    }
                    self.section += 1;
                }
                if let Some(l) = lines.first_mut() {
                    l.gap = BODY * if *level <= 2 { 1.1 } else { 0.8 };
                }
                out.extend(lines);
                return;
            }
            Block::List { kind, items } => {
                let indent = BODY * 1.4;
                for (i, item) in items.iter().enumerate() {
                    let start = out.len();
                    for (k, b) in item.iter().enumerate() {
                        let before = out.len();
                        self.block(article, b, x + indent, w - indent, depth + 1, out);
                        if k == 0 && out.len() > before {
                            out[before].gap = BODY * 0.2;
                        }
                    }
                    let marker = match kind {
                        ListKind::Bullet => "•".to_string(),
                        ListKind::Number => format!("{}.", i + 1),
                        ListKind::Definition => String::new(),
                    };
                    if let Some(l) = out.get_mut(start)
                        && !marker.is_empty()
                    {
                        let sty = Sty::new(BODY);
                        let base = l
                            .items
                            .iter()
                            .find_map(|it| match it {
                                Item::Text { base, .. } | Item::Glyphs { base, .. } => Some(*base),
                                _ => None,
                            })
                            .unwrap_or(sty.ascent());
                        let mw = text_width(self.wiki, &marker, sty);
                        l.items.push(Item::Text {
                            x: x + indent - mw - BODY * 0.4,
                            base,
                            text: marker,
                            sty,
                        });
                    }
                }
            }
            Block::Quote(inner) => {
                let indent = BODY * 1.2;
                let start = out.len();
                for b in inner {
                    self.block(article, b, x + indent, w - indent, depth + 1, out);
                }
                for l in &mut out[start..] {
                    l.items.push(Item::Fill {
                        x: x + indent * 0.35,
                        y: 0.0,
                        w: 2.0,
                        h: l.h,
                        grey: true,
                    });
                }
            }
            Block::Pre(text) => {
                let sty = Sty::new(SMALL);
                for line in text.lines() {
                    let mut l = Line {
                        h: SMALL * SPACING,
                        ..Line::default()
                    };
                    l.items.push(Item::Text {
                        x: x + 8.0,
                        base: sty.ascent(),
                        text: line.to_string(),
                        sty,
                    });
                    out.push(l);
                }
            }
            Block::Rule => out.push(rule_line(x + w / 6.0, w * 2.0 / 3.0, 1.0, gap)),
            Block::Figure { image, caption } => {
                self.picture(*image, x, w, self.page_h * 0.6, out);
                out.extend(self.flow(caption, Sty::new(SMALL), x + 8.0, w - 16.0, Align::Centre));
                if out.len() > first + 1 {
                    out[first].keep = true;
                }
            }
            Block::Gallery(items) => {
                let cols = if items.len() > 1 { 2 } else { 1 };
                let cell = (w - 10.0 * (cols as f32 - 1.0)) / cols as f32;
                for row in items.chunks(cols) {
                    let mut columns = Vec::new();
                    for (image, caption) in row {
                        let mut c = Vec::new();
                        self.picture(*image, 0.0, cell, self.page_h / 3.0, &mut c);
                        c.extend(self.flow(caption, Sty::new(SMALL), 0.0, cell, Align::Centre));
                        columns.push(c);
                    }
                    let xs: Vec<f32> = (0..columns.len())
                        .map(|i| x + i as f32 * (cell + 10.0))
                        .collect();
                    out.extend(zip(columns, &xs));
                }
            }
            Block::MathBlock(f) => {
                let upem = self.upem();
                let mut scale = BODY / upem;
                let fw = f.width as f32 * scale;
                if fw > w {
                    scale *= w / fw;
                }
                let (asc, desc) = (f.ascent as f32 * scale, f.descent as f32 * scale);
                let fw = f.width as f32 * scale;
                self.formulas.push(f.clone());
                out.push(Line {
                    h: asc + desc + 6.0,
                    items: vec![Item::Formula {
                        x: x + (w - fw) / 2.0,
                        base: asc + 3.0,
                        scale,
                        formula: self.formulas.len() - 1,
                    }],
                    ..Line::default()
                });
            }
            Block::TableRef(i) => {
                if let Some(t) = article.tables.get(*i as usize) {
                    self.table(t, Some(*i), x, w, out);
                }
            }
            Block::InfoboxRef => {
                if let Some(ib) = &article.infobox {
                    self.infobox(ib, x, w, out);
                }
            }
            Block::Tree(t) => self.tree(t, x, w, out),
        }
        if out.len() > first {
            out[first].gap = out[first].gap.max(gap);
        }
    }

    fn upem(&self) -> f32 {
        self.wiki
            .glyphs
            .as_ref()
            .map_or(1000.0, |g| g.upem.max(1) as f32)
    }

    /// A picture as large as the e-reader shows it: no wider than the
    /// column, no taller than `max_h`, and not enlarged.
    fn picture(&mut self, image: u32, x: f32, w: f32, max_h: f32, out: &mut Vec<Line>) {
        let Some((iw, ih)) = self.wiki.picture_size(image) else {
            return;
        };
        let k = (w / iw as f32).min(max_h / ih as f32).min(1.6);
        let (pw, ph) = (iw as f32 * k, ih as f32 * k);
        out.push(Line {
            h: ph + 4.0,
            items: vec![Item::Picture {
                x: x + (w - pw) / 2.0,
                y: 2.0,
                w: pw,
                h: ph,
                image,
            }],
            ..Line::default()
        });
    }

    // ------------------------------------------------------------- text

    fn atoms(&mut self, run: &[Inline], sty: Sty, link: Option<Link>, out: &mut Vec<Atom>) {
        for i in run {
            match i {
                Inline::Text(t) => {
                    let mut word = String::new();
                    for c in t.chars() {
                        if c == ' ' {
                            if !word.is_empty() {
                                let w = text_width(self.wiki, &word, sty);
                                out.push(Atom::Word {
                                    text: std::mem::take(&mut word),
                                    sty,
                                    link,
                                    w,
                                });
                            }
                            out.push(Atom::Space {
                                w: char_width(self.wiki, ' ', sty),
                                link,
                            });
                        } else {
                            word.push(c);
                        }
                    }
                    if !word.is_empty() {
                        let w = text_width(self.wiki, &word, sty);
                        out.push(Atom::Word {
                            text: word,
                            sty,
                            link,
                            w,
                        });
                    }
                }
                Inline::Styled(style, content) => {
                    let mut s = sty;
                    match style {
                        Style::Bold => s.bold = true,
                        Style::Italic => s.italic = true,
                        Style::Superscript => {
                            s.rise += sty.size * 0.33;
                            s.size *= 0.7;
                        }
                        Style::Subscript => {
                            s.rise -= sty.size * 0.15;
                            s.size *= 0.7;
                        }
                        Style::Monospace | Style::SmallCaps => {}
                    }
                    self.atoms(content, s, link, out);
                }
                Inline::Link {
                    article,
                    place,
                    content,
                } => {
                    let l = Some(Link {
                        article: *article,
                        place: *place,
                    });
                    self.atoms(content, sty, l, out);
                }
                Inline::Math(f) => {
                    let scale = sty.size / self.upem();
                    self.formulas.push(f.clone());
                    out.push(Atom::Math {
                        formula: self.formulas.len() - 1,
                        scale,
                        w: f.width as f32 * scale,
                        asc: f.ascent as f32 * scale,
                        desc: f.descent as f32 * scale,
                        link,
                    });
                }
                Inline::Icon { image, height } => {
                    let h = *height as f32 / 16.0 * BODY;
                    let (iw, ih) = self.wiki.picture_size(*image).unwrap_or((1, 1));
                    out.push(Atom::Icon {
                        image: *image,
                        w: h * iw as f32 / ih as f32,
                        h,
                        link,
                    });
                }
                Inline::Break => out.push(Atom::Break),
                Inline::NoteRef(_) => {}
            }
        }
    }

    /// An inline run as lines `w` wide, starting at `x`.
    fn flow(&mut self, run: &[Inline], sty: Sty, x: f32, w: f32, align: Align) -> Vec<Line> {
        let mut atoms = Vec::new();
        self.atoms(run, sty, None, &mut atoms);
        // Words wider than the line are split where they must be.
        let mut split = Vec::with_capacity(atoms.len());
        for a in atoms {
            match a {
                Atom::Word {
                    text,
                    sty,
                    link,
                    w: ww,
                } if ww > w => {
                    let mut part = String::new();
                    let mut pw = 0.0;
                    for c in text.chars() {
                        let cw = char_width(self.wiki, c, sty);
                        if pw + cw > w && !part.is_empty() {
                            split.push(Atom::Word {
                                text: std::mem::take(&mut part),
                                sty,
                                link,
                                w: pw,
                            });
                            pw = 0.0;
                        }
                        part.push(c);
                        pw += cw;
                    }
                    split.push(Atom::Word {
                        text: part,
                        sty,
                        link,
                        w: pw,
                    });
                }
                a => split.push(a),
            }
        }
        let mut lines: Vec<Vec<Atom>> = vec![Vec::new()];
        let mut lw = 0.0;
        let mut ends = Vec::new(); // whether each line ends a paragraph
        for a in split {
            match a {
                Atom::Break => {
                    lines.push(Vec::new());
                    ends.push(true);
                    lw = 0.0;
                }
                Atom::Space { .. } => {
                    if !lines.last().unwrap().is_empty() {
                        lw += a.width();
                        lines.last_mut().unwrap().push(a);
                    }
                }
                a => {
                    let aw = a.width();
                    if lw + aw > w + 0.01 && !lines.last().unwrap().is_empty() {
                        // Spaces at the end of a line take no room.
                        while matches!(lines.last().unwrap().last(), Some(Atom::Space { .. })) {
                            lines.last_mut().unwrap().pop();
                        }
                        lines.push(Vec::new());
                        ends.push(false);
                        lw = 0.0;
                    }
                    lw += aw;
                    lines.last_mut().unwrap().push(a);
                }
            }
        }
        ends.push(true);
        let mut out = Vec::new();
        for (atoms, last) in lines.into_iter().zip(ends) {
            let mut atoms = atoms;
            while matches!(atoms.last(), Some(Atom::Space { .. })) {
                atoms.pop();
            }
            if atoms.is_empty() && out.is_empty() && last {
                continue;
            }
            out.push(self.line(
                atoms,
                sty,
                x,
                w,
                if last && align == Align::Justify {
                    Align::Start
                } else {
                    align
                },
            ));
        }
        out
    }

    fn line(&mut self, atoms: Vec<Atom>, sty: Sty, x: f32, w: f32, align: Align) -> Line {
        let mut asc = sty.ascent();
        let mut desc = sty.descent();
        for a in &atoms {
            match a {
                Atom::Word { sty, .. } => {
                    asc = asc.max(sty.ascent() + sty.rise);
                    desc = desc.max(sty.descent() - sty.rise);
                }
                Atom::Icon { h, .. } => asc = asc.max(*h),
                Atom::Math {
                    asc: a, desc: d, ..
                } => {
                    asc = asc.max(*a);
                    desc = desc.max(*d);
                }
                _ => {}
            }
        }
        let natural = asc + desc;
        let h = natural.max(sty.size * SPACING);
        let base = (h - natural) / 2.0 + asc;
        let total: f32 = atoms.iter().map(Atom::width).sum();
        let spaces = atoms
            .iter()
            .filter(|a| matches!(a, Atom::Space { .. }))
            .count();
        let (mut cx, extra) = match align {
            Align::Start => (x, 0.0),
            Align::Centre => (x + ((w - total) / 2.0).max(0.0), 0.0),
            Align::Justify if spaces > 0 => (x, ((w - total) / spaces as f32).max(0.0)),
            Align::Justify => (x, 0.0),
        };
        let mut line = Line {
            h,
            ..Line::default()
        };
        for a in atoms {
            let aw = a.width();
            let space = matches!(a, Atom::Space { .. });
            let link = match &a {
                Atom::Word { link, .. }
                | Atom::Space { link, .. }
                | Atom::Icon { link, .. }
                | Atom::Math { link, .. } => *link,
                Atom::Break => None,
            };
            let adv = aw + if space { extra } else { 0.0 };
            match a {
                Atom::Word { text, sty, .. } => {
                    // Links are underlined, as the e-reader marks them.
                    if link.is_some() {
                        line.items.push(Item::Fill {
                            x: cx,
                            y: base + (sty.size * 0.12).max(1.5),
                            w: aw,
                            h: 1.0,
                            grey: false,
                        });
                    }
                    self.word(&mut line, text, sty, cx, base)
                }
                Atom::Icon {
                    image,
                    w: iw,
                    h: ih,
                    ..
                } => line.items.push(Item::Picture {
                    x: cx,
                    y: base - ih,
                    w: iw,
                    h: ih,
                    image,
                }),
                Atom::Math { formula, scale, .. } => line.items.push(Item::Formula {
                    x: cx,
                    base,
                    scale,
                    formula,
                }),
                Atom::Space { .. } | Atom::Break => {}
            }
            if let Some(link) = link {
                line.areas.push(Area {
                    x: cx,
                    y: 0.0,
                    w: adv,
                    h,
                    target: Target::Link(link),
                });
            }
            cx += adv;
        }
        line
    }

    /// A word's text, in runs of the reader's font and of the archive's.
    fn word(&self, line: &mut Line, text: String, sty: Sty, x: f32, base: f32) {
        let base = base - sty.rise;
        let mut run = String::new();
        let mut run_archive = false;
        let mut rx = x;
        let flush = |run: &mut String, archive: bool, rx: &mut f32, line: &mut Line| {
            if run.is_empty() {
                return;
            }
            let w = text_width(self.wiki, run, sty);
            let text = std::mem::take(run);
            line.items.push(if archive {
                Item::Glyphs {
                    x: *rx,
                    base,
                    text,
                    sty,
                }
            } else {
                Item::Text {
                    x: *rx,
                    base,
                    text,
                    sty,
                }
            });
            *rx += w;
        };
        // Right-to-left letters in their reading order within the word
        // (the e-reader orders whole lines; this preview, each word).
        let text = rtl_runs_reversed(&text);
        for c in text.chars() {
            let a = from_archive(self.wiki, c, sty);
            if a != run_archive && !run.is_empty() {
                flush(&mut run, run_archive, &mut rx, line);
            }
            run_archive = a;
            run.push(c);
        }
        flush(&mut run, run_archive, &mut rx, line);
    }

    // ------------------------------------------------------- tables

    /// Where a table's cells go, and how wide its columns can be.
    fn plan<'t>(&self, t: &'t Table) -> Plan<'t> {
        let cols = t.cols.max(1) as usize;
        // Place cells as a browser does: each in the first free slot of
        // its row, spans taking slots below and to the right.
        let mut grid: Vec<Vec<bool>> = vec![vec![false; cols]; t.rows as usize];
        let mut placed = Vec::new();
        let (mut r, mut c) = (0usize, 0usize);
        let mut last_row = 0;
        for cell in &t.cells {
            while r < grid.len() && (c >= cols || grid[r][c]) {
                c += 1;
                if c >= cols {
                    r += 1;
                    c = 0;
                }
            }
            if r >= grid.len() {
                break;
            }
            let span = (cell.colspan.max(1) as usize).min(cols - c);
            for row in grid.iter_mut().skip(r).take(cell.rowspan.max(1) as usize) {
                row[c..c + span].iter_mut().for_each(|t| *t = true);
            }
            placed.push((r, c, span, cell));
            last_row = last_row.max(r);
            c += span;
        }
        // As a browser lays a table out: each column at least as wide as
        // its longest word, and the rest shared by how much text it holds.
        let sty = Sty::new(SMALL);
        let mut least = vec![16.0f32; cols];
        let mut most = vec![16.0f32; cols];
        for &(_, c, span, cell) in &placed {
            if span != 1 {
                continue;
            }
            let text = xwa_format::content::plain_text(&cell.content);
            let s = Sty {
                bold: cell.header,
                ..sty
            };
            let longest = text
                .split(' ')
                .map(|word| text_width(self.wiki, word, s))
                .fold(0.0, f32::max);
            least[c] = least[c].max(longest + 2.0 * TABLE_PAD);
            most[c] = most[c].max(text_width(self.wiki, &text, s) + 2.0 * TABLE_PAD);
        }
        Plan {
            placed,
            last_row,
            least,
            most,
        }
    }

    /// A table at the column's width; one too wide for it (`index` given)
    /// is a line that opens it in a view of its own, as on the e-reader.
    fn table(&mut self, t: &Table, index: Option<u32>, x: f32, w: f32, out: &mut Vec<Line>) {
        let Plan {
            placed,
            last_row,
            least,
            most,
        } = self.plan(t);
        let pad = TABLE_PAD;
        let sty = Sty::new(SMALL);
        let (sum_least, sum_most): (f32, f32) = (least.iter().sum(), most.iter().sum());
        if let Some(index) = index
            && sum_least > w * 1.05
        {
            self.table_link(t, index, x, w, out);
            return;
        }
        let widths: Vec<f32> = if sum_least >= w {
            least.iter().map(|v| v / sum_least * w).collect()
        } else if sum_most <= w {
            most.iter().map(|v| v / sum_most * w).collect()
        } else {
            let room = w - sum_least;
            let give = sum_most - sum_least;
            least
                .iter()
                .zip(&most)
                .map(|(l, m)| l + (m - l) / give * room)
                .collect()
        };
        let mut xs = vec![x];
        for cw in &widths {
            xs.push(xs.last().unwrap() + cw);
        }
        let caption = &t.caption;
        if !caption.is_empty() {
            let s = Sty {
                bold: true,
                ..Sty::new(SMALL)
            };
            out.extend(self.flow(caption, s, x, w, Align::Centre));
        }
        for row in 0..=last_row {
            let mut lines: Vec<(f32, f32, bool, Vec<Line>)> = Vec::new();
            for &(r, c, span, cell) in &placed {
                if r != row {
                    continue;
                }
                let cx = xs[c] + pad;
                let cw = xs[c + span] - xs[c] - 2.0 * pad;
                let s = Sty {
                    bold: cell.header,
                    ..sty
                };
                let ls = self.flow(&cell.content, s, 0.0, cw.max(8.0), Align::Start);
                lines.push((cx, xs[c + span] - xs[c], cell.header, ls));
            }
            let h = lines
                .iter()
                .map(|(_, _, _, ls)| ls.iter().map(|l| l.h).sum::<f32>())
                .fold(sty.size * SPACING, f32::max)
                + 2.0 * pad;
            let mut line = Line {
                h,
                ..Line::default()
            };
            line.items.push(Item::Fill {
                x,
                y: 0.0,
                w,
                h: 1.0,
                grey: false,
            });
            for (cx, cw, header, ls) in lines {
                if header {
                    line.items.push(Item::Fill {
                        x: cx - pad,
                        y: 1.0,
                        w: cw,
                        h: h - 1.0,
                        grey: true,
                    });
                }
                let mut y = pad;
                for l in ls {
                    for it in l.items {
                        let mut it = it;
                        it.shift(cx, y);
                        line.items.push(it);
                    }
                    for a in l.areas {
                        let mut a = a;
                        a.x += cx;
                        a.y += y;
                        line.areas.push(a);
                    }
                    y += l.h;
                }
            }
            for &bx in &xs {
                line.items.push(Item::Fill {
                    x: bx.min(x + w - 1.0),
                    y: 0.0,
                    w: 1.0,
                    h,
                    grey: false,
                });
            }
            if row == last_row {
                line.items.push(Item::Fill {
                    x,
                    y: h - 1.0,
                    w,
                    h: 1.0,
                    grey: false,
                });
            }
            out.push(line);
        }
    }

    /// A line that stands for a table and opens it.
    fn table_link(&mut self, t: &Table, index: u32, x: f32, w: f32, out: &mut Vec<Line>) {
        let what = if t.caption.is_empty() {
            format!("Table: {} rows, {} columns", t.rows, t.cols)
        } else {
            format!("Table: {}", xwa_format::content::plain_text(&t.caption))
        };
        let sty = Sty {
            bold: true,
            ..Sty::new(SMALL)
        };
        let mut lines = self.flow(&[Inline::Text(what)], sty, x + 8.0, w - 40.0, Align::Start);
        if lines.is_empty() {
            return;
        }
        let pad = 6.0;
        let n = lines.len();
        let total: f32 = lines.iter().map(|l| l.h).sum::<f32>() + 2.0 * pad;
        let mut y = 0.0;
        for (i, l) in lines.iter_mut().enumerate() {
            let dy = if i == 0 { pad } else { 0.0 };
            for it in &mut l.items {
                it.shift(0.0, dy);
            }
            l.h += dy + if i + 1 == n { pad } else { 0.0 };
            for bx in [x, x + w - 1.0] {
                l.items.push(Item::Fill {
                    x: bx,
                    y: 0.0,
                    w: 1.0,
                    h: l.h,
                    grey: false,
                });
            }
            if i == 0 {
                l.items.push(Item::Fill {
                    x,
                    y: 0.0,
                    w,
                    h: 1.0,
                    grey: false,
                });
                // An arrow: it opens.
                l.items.push(Item::Text {
                    x: x + w - 22.0,
                    base: pad + sty.ascent(),
                    text: "›".into(),
                    sty: Sty::new(LARGE),
                });
            }
            if i + 1 == n {
                let h = l.h;
                l.items.push(Item::Fill {
                    x,
                    y: h - 1.0,
                    w,
                    h: 1.0,
                    grey: false,
                });
            }
            l.areas.push(Area {
                x,
                y: 0.0,
                w,
                h: l.h,
                target: Target::Table(index),
            });
            y += l.h;
        }
        let _ = (y, total);
        out.extend(lines);
    }

    // ------------------------------------------------------- infobox

    fn infobox(&mut self, ib: &Infobox, x: f32, w: f32, out: &mut Vec<Line>) {
        let pad = 6.0;
        let inner = w - 2.0 * pad;
        let label_w = (inner * 0.38).round();
        let mut body: Vec<Line> = Vec::new();
        for row in &ib.rows {
            match row {
                InfoboxRow::Header(run) => {
                    let s = Sty {
                        bold: true,
                        ..Sty::new(SMALL + 1.0)
                    };
                    let mut ls = self.flow(run, s, 0.0, inner, Align::Centre);
                    if let Some(l) = ls.first_mut() {
                        l.gap = 4.0;
                        l.items.insert(
                            0,
                            Item::Fill {
                                x: -pad + 1.0,
                                y: 0.0,
                                w: w - 2.0,
                                h: 1.0,
                                grey: false,
                            },
                        );
                    }
                    body.extend(ls);
                }
                InfoboxRow::Image { image, caption } => {
                    self.picture(*image, 0.0, inner, self.page_h * 0.45, &mut body);
                    body.extend(self.flow(caption, Sty::new(SMALL), 0.0, inner, Align::Centre));
                }
                InfoboxRow::Field { label, value } => {
                    let s = Sty::new(SMALL);
                    let a = self.flow(
                        label,
                        Sty { bold: true, ..s },
                        0.0,
                        label_w - 6.0,
                        Align::Start,
                    );
                    let b = self.flow(value, s, 0.0, inner - label_w, Align::Start);
                    let mut ls = zip(vec![a, b], &[0.0, label_w]);
                    if let Some(l) = ls.first_mut() {
                        l.gap = 2.0;
                    }
                    body.extend(ls);
                }
                InfoboxRow::Full(run) => {
                    body.extend(self.flow(run, Sty::new(SMALL), 0.0, inner, Align::Centre));
                }
            }
        }
        // A frame round it, whole on every page it reaches.
        let n = body.len();
        for (i, mut l) in body.into_iter().enumerate() {
            l.shift(x + pad);
            let h = l.h + if i == 0 { pad } else { 0.0 } + if i + 1 == n { pad } else { 0.0 };
            let dy = if i == 0 { pad } else { 0.0 };
            for it in &mut l.items {
                it.shift(0.0, dy);
            }
            for a in &mut l.areas {
                a.y += dy;
            }
            l.h = h;
            for bx in [x, x + w - 1.0] {
                l.items.push(Item::Fill {
                    x: bx,
                    y: 0.0,
                    w: 1.0,
                    h,
                    grey: false,
                });
            }
            if i == 0 {
                l.items.push(Item::Fill {
                    x,
                    y: 0.0,
                    w,
                    h: 1.0,
                    grey: false,
                });
            }
            if i + 1 == n {
                l.items.push(Item::Fill {
                    x,
                    y: h - 1.0,
                    w,
                    h: 1.0,
                    grey: false,
                });
            }
            out.push(l);
        }
    }

    // ------------------------------------------------------- trees

    /// A tree as rows (spec 7.3): each leaf, and each node with a label,
    /// on its own row at its depth, joined to its children by lines.
    fn tree(&mut self, t: &Tree, x: f32, w: f32, out: &mut Vec<Line>) {
        struct N<'a> {
            node: &'a TreeNode,
            depth: usize,
            parent: Option<usize>,
            first: bool,
            last: bool,
        }
        fn walk<'a>(
            n: &'a TreeNode,
            depth: usize,
            parent: Option<usize>,
            first: bool,
            last: bool,
            out: &mut Vec<N<'a>>,
        ) {
            let me = out.len();
            out.push(N {
                node: n,
                depth,
                parent,
                first,
                last,
            });
            let k = n.children.len();
            for (i, c) in n.children.iter().enumerate() {
                walk(c, depth + 1, Some(me), i == 0, i + 1 == k, out);
            }
        }
        let owns =
            |n: &TreeNode| n.children.is_empty() || !n.label.is_empty() || !n.sublabel.is_empty();
        let mut root = &t.root;
        while root.label.is_empty() && root.sublabel.is_empty() && root.children.len() == 1 {
            root = &root.children[0];
        }
        let mut nodes = Vec::new();
        walk(root, 0, None, true, true, &mut nodes);
        let max_depth = nodes.iter().map(|n| n.depth).max().unwrap_or(0).max(1) as f32;
        let step = (SMALL * 1.1).min(w * 0.45 / max_depth).max(6.0);
        let thick = 2.0;
        let pad = 7.0;
        let vx = |j: usize| x + j as f32 * step + 2.0;
        let sty = Sty::new(SMALL);
        for ri in 0..nodes.len() {
            let r = &nodes[ri];
            if !owns(r.node) {
                continue;
            }
            let k = r.depth;
            let mut path = vec![0usize; k + 1];
            let mut a = Some(ri);
            for d in (0..=k).rev() {
                path[d] = a.unwrap_or(0);
                a = a.and_then(|i| nodes[i].parent);
            }
            let mut starts = vec![false; k + 1];
            starts[k] = true;
            for j in (0..k).rev() {
                starts[j] = starts[j + 1] && nodes[path[j + 1]].first && !owns(nodes[path[j]].node);
            }
            let tx = vx(k) + if k > 0 { pad } else { 0.0 };
            let tw = (x + w - tx).max(40.0);
            let mut rows = if r.node.children.is_empty() {
                self.flow(&r.node.leaf, sty, 0.0, tw, Align::Start)
            } else {
                self.flow(
                    &r.node.label,
                    Sty { bold: true, ..sty },
                    0.0,
                    tw,
                    Align::Start,
                )
            };
            if rows.is_empty() {
                rows.push(Line {
                    h: sty.size * SPACING,
                    ..Line::default()
                });
            }
            if !r.node.sublabel.is_empty() {
                rows.extend(self.flow(
                    &r.node.sublabel,
                    Sty {
                        italic: true,
                        ..sty
                    },
                    0.0,
                    tw,
                    Align::Start,
                ));
            }
            for (li, mut l) in rows.into_iter().enumerate() {
                l.shift(tx);
                let yc = (l.h / 2.0).round();
                let fill = |l: &mut Line, fx: f32, fy: f32, fw: f32, fh: f32| {
                    if fw > 0.0 && fh > 0.0 {
                        l.items.push(Item::Fill {
                            x: fx,
                            y: fy,
                            w: fw,
                            h: fh,
                            grey: false,
                        });
                    }
                };
                for j in 0..k {
                    let c = &nodes[path[j + 1]];
                    if li == 0 && starts[j + 1] {
                        let to = if j + 1 == k { tx - 4.0 } else { vx(j + 1) };
                        fill(&mut l, vx(j), yc - thick / 2.0, to - vx(j) + thick, thick);
                        if !starts[j] {
                            fill(&mut l, vx(j), 0.0, thick, yc + thick / 2.0);
                        }
                        if !c.last {
                            let h = l.h;
                            fill(&mut l, vx(j), yc, thick, h - yc);
                        }
                    } else if !c.last {
                        let h = l.h;
                        fill(&mut l, vx(j), 0.0, thick, h);
                    }
                }
                if !r.node.children.is_empty() && li > 0 {
                    let h = l.h;
                    fill(&mut l, vx(k), 0.0, thick, h);
                }
                out.push(l);
            }
        }
        if !t.caption.is_empty() {
            let mut c = self.flow(
                &t.caption,
                Sty::new(SMALL),
                x + 8.0,
                w - 16.0,
                Align::Centre,
            );
            if let Some(l) = c.first_mut() {
                l.gap = SMALL * 0.4;
            }
            out.extend(c);
        }
    }
}

fn is_rtl(c: char) -> bool {
    matches!(c as u32, 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF)
}

/// `s` with each run of right-to-left letters (and the marks on them)
/// reversed, so that drawn from left to right they read correctly.
fn rtl_runs_reversed(s: &str) -> String {
    if !s.chars().any(is_rtl) {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        if !is_rtl(chars[i]) {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && is_rtl(chars[i]) {
            i += 1;
        }
        // Clusters (a letter and its marks) keep their order inside.
        let mut clusters: Vec<String> = Vec::new();
        for &c in &chars[start..i] {
            let mark =
                matches!(c as u32, 0x0591..=0x05C7 | 0x0610..=0x061A | 0x064B..=0x065F | 0x0670);
            match clusters.last_mut() {
                Some(last) if mark => last.push(c),
                _ => clusters.push(c.to_string()),
            }
        }
        clusters.reverse();
        out.extend(clusters);
    }
    out
}

/// Columns side by side: line k of each on one line, as tall as the
/// tallest.
fn zip(columns: Vec<Vec<Line>>, xs: &[f32]) -> Vec<Line> {
    let rows = columns.iter().map(Vec::len).max().unwrap_or(0);
    let mut out = Vec::with_capacity(rows);
    for k in 0..rows {
        let mut line = Line::default();
        for (c, col) in columns.iter().enumerate() {
            if let Some(l) = col.get(k) {
                line.h = line.h.max(l.h);
                line.gap = line.gap.max(l.gap);
                for it in &l.items {
                    let mut it = it.clone();
                    it.shift(xs[c], 0.0);
                    line.items.push(it);
                }
                for a in &l.areas {
                    let mut a = *a;
                    a.x += xs[c];
                    line.areas.push(a);
                }
            }
        }
        out.push(line);
    }
    out
}
