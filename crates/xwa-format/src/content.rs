//! Article content and its byte encoding (spec sections 7.2-7.4 and 8.1).
//!
//! In memory, styles and links wrap their contents; on disk they become
//! push/pop opcodes inside length-prefixed inline runs.

use std::borrow::Cow;

use unicode_normalization::{UnicodeNormalization, is_nfc};

use crate::error::corrupt;
use crate::font::FontStyle;
use crate::{Result, varint};

/// Deepest nesting of blocks or inlines the decoder accepts.
const MAX_DEPTH: usize = 64;

mod op {
    pub const PARA: u8 = 0x01;
    pub const HEADING: u8 = 0x02;
    pub const LIST_START: u8 = 0x03;
    pub const LIST_ITEM: u8 = 0x04;
    pub const LIST_END: u8 = 0x05;
    pub const QUOTE_START: u8 = 0x06;
    pub const QUOTE_END: u8 = 0x07;
    pub const PRE: u8 = 0x08;
    pub const RULE: u8 = 0x09;
    pub const FIGURE: u8 = 0x0A;
    pub const MATH_BLOCK: u8 = 0x0B;
    pub const TABLE_REF: u8 = 0x0C;
    pub const INFOBOX_REF: u8 = 0x0D;
    pub const GALLERY: u8 = 0x0E;
    pub const TREE: u8 = 0x0F;
    pub const BLOCK_EXT: u8 = 0x7F;

    pub const TEXT_LONG: u8 = 0x80;
    pub const STYLE_PUSH: u8 = 0x81;
    pub const STYLE_POP: u8 = 0x82;
    pub const LINK_START: u8 = 0x83;
    pub const LINK_END: u8 = 0x84;
    pub const MATH: u8 = 0x85;
    pub const ICON: u8 = 0x86;
    pub const BREAK: u8 = 0x87;
    pub const NOTE_REF: u8 = 0x88;
    pub const LINK_ANCHOR: u8 = 0x89;
    pub const INLINE_EXT: u8 = 0xFF;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Bold,
    Italic,
    Subscript,
    Superscript,
    Monospace,
    SmallCaps,
}

impl Style {
    const ALL: [Style; 6] = [
        Style::Bold,
        Style::Italic,
        Style::Subscript,
        Style::Superscript,
        Style::Monospace,
        Style::SmallCaps,
    ];

    fn code(self) -> u8 {
        Self::ALL.iter().position(|s| *s == self).unwrap() as u8
    }
}

/// Where in its target article a link leads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Place {
    /// A section by its number in the target's table of contents (1 is the
    /// first entry), or 0 for the top.
    Section(u32),
    /// A named place in the target, by [`anchor_hash`]; [`find_anchor`]
    /// gives its section.
    Anchor(u32),
}

impl Place {
    pub const TOP: Place = Place::Section(0);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inline {
    Text(String),
    Styled(Style, Vec<Inline>),
    /// A link to another article.
    Link {
        article: u32,
        place: Place,
        content: Vec<Inline>,
    },
    Math(Formula),
    /// An image in a line of text, such as a flag or symbol.
    Icon {
        image: u32,
        /// The height to draw it at, in sixteenths of the body text size
        /// (1-255); the width follows from the image's proportions.
        height: u8,
    },
    Break,
    NoteRef(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListKind {
    Bullet,
    Number,
    Definition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Para(Vec<Inline>),
    Heading {
        level: u8,
        content: Vec<Inline>,
    },
    List {
        kind: ListKind,
        items: Vec<Vec<Block>>,
    },
    Quote(Vec<Block>),
    Pre(String),
    Rule,
    Figure {
        image: u32,
        caption: Vec<Inline>,
    },
    MathBlock(Formula),
    /// Index into the article's table list.
    TableRef(u32),
    InfoboxRef,
    Gallery(Vec<(u32, Vec<Inline>)>),
    Tree(Tree),
}

/// A tree diagram drawn in the text: a cladogram (spec 7.3, TREE).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Tree {
    pub caption: Vec<Inline>,
    pub root: TreeNode,
}

/// A node of a [`Tree`]: a leaf, with its text in `leaf`, or a branching
/// point, with children.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TreeNode {
    /// Text on the branch that leads to the node, above it (a clade's
    /// name).
    pub label: Vec<Inline>,
    /// Text under that branch (a date, a support value).
    pub sublabel: Vec<Inline>,
    /// A leaf's text; always empty when the node has children.
    pub leaf: Vec<Inline>,
    pub children: Vec<TreeNode>,
}

impl Tree {
    /// Every inline run in the tree: the caption, then each node's label,
    /// sublabel and leaf, in the order they are stored.
    pub fn runs(&self) -> Vec<&Vec<Inline>> {
        fn walk<'a>(n: &'a TreeNode, out: &mut Vec<&'a Vec<Inline>>) {
            out.extend([&n.label, &n.sublabel, &n.leaf]);
            n.children.iter().for_each(|c| walk(c, out));
        }
        let mut out = vec![&self.caption];
        walk(&self.root, &mut out);
        out
    }

    /// [`Tree::runs`], to change.
    pub fn for_each_run_mut(&mut self, f: &mut impl FnMut(&mut Vec<Inline>)) {
        fn walk(n: &mut TreeNode, f: &mut impl FnMut(&mut Vec<Inline>)) {
            f(&mut n.label);
            f(&mut n.sublabel);
            f(&mut n.leaf);
            n.children.iter_mut().for_each(|c| walk(c, f));
        }
        f(&mut self.caption);
        walk(&mut self.root, f);
    }

    /// How many levels deep it goes: 1 for a root without children.
    pub fn depth(&self) -> usize {
        fn d(n: &TreeNode) -> usize {
            1 + n.children.iter().map(d).max().unwrap_or(0)
        }
        d(&self.root)
    }
}

/// A typeset formula: glyph placements and rules (spec 8.1).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Formula {
    pub display: bool,
    pub width: i32,
    pub ascent: i32,
    pub descent: i32,
    pub items: Vec<MathItem>,
    pub fallback_image: Option<u32>,
}

/// One item of a formula. Each item's position is the previous item's
/// position plus (`dx`, `dy`), starting from the formula's origin (the left
/// end of its baseline). Units are MATH_GLYPHS font units; y points up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MathItem {
    /// A glyph with its origin at the position, scaled by MATH_GLYPHS
    /// scale `scale`.
    Glyph {
        glyph: u32,
        scale: u8,
        dx: i32,
        dy: i32,
    },
    /// A glyph scaled by `sx`, `sy` (8.8 fixed point), for sizes outside the
    /// scale table.
    GlyphXY {
        glyph: u32,
        sx: i16,
        sy: i16,
        dx: i32,
        dy: i32,
    },
    /// A filled rectangle (fraction bar, overline) with its bottom-left
    /// corner at the position.
    Rule { dx: i32, dy: i32, w: u32, h: u32 },
    /// A filled outline (radical sign, stretchy brace or arrow), nonzero
    /// winding, with coordinates relative to the position.
    Path { dx: i32, dy: i32, ops: Vec<PathOp> },
}

/// A drawing command of a [`MathItem::Path`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathOp {
    Move(i32, i32),
    Line(i32, i32),
    /// Control point, end point.
    Quad(i32, i32, i32, i32),
    /// Two control points, end point.
    Cubic(i32, i32, i32, i32, i32, i32),
    Close,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Table {
    pub rows: u32,
    pub cols: u32,
    pub flags: u8,
    /// Cells in row-major order of their top-left corners.
    pub cells: Vec<Cell>,
    /// The table's caption (empty when it has none).
    pub caption: Vec<Inline>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    pub header: bool,
    pub rowspan: u32,
    pub colspan: u32,
    pub content: Vec<Inline>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InfoboxRow {
    Header(Vec<Inline>),
    Image {
        image: u32,
        caption: Vec<Inline>,
    },
    Field {
        label: Vec<Inline>,
        value: Vec<Inline>,
    },
    Full(Vec<Inline>),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Infobox {
    pub rows: Vec<InfoboxRow>,
}

/// Everything stored for one article.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Article {
    pub title: String,
    pub body: Vec<Block>,
    pub tables: Vec<Table>,
    pub infobox: Option<Infobox>,
    /// Named places other than what [`heading_anchors`] derives from the
    /// headings (spec 7.2): anchors inside sections, and headings whose
    /// anchor is not the derived one.
    pub anchors: Vec<Anchor>,
}

/// A named place in an article and the section it is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Anchor {
    /// [`anchor_hash`] of its name.
    pub hash: u32,
    /// Section number as in [`Place::Section`].
    pub section: u32,
}

/// A table-of-contents entry in the article header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TocEntry {
    pub level: u8,
    pub text: String,
    /// Byte position of the heading, measured from the start of the body.
    pub pos: u32,
}

/// The header of an article stream (spec 7.2), readable without the body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArticleHeader {
    pub title: String,
    pub body_size: u32,
    pub toc: Vec<TocEntry>,
    pub table_pos: Vec<u32>,
    pub infobox_pos: Option<u32>,
    pub anchors: Vec<Anchor>,
}

/// The hash anchors are stored and looked up by (spec 7.2): 32-bit FNV-1a
/// of the name's UTF-8 bytes, with each space taken as `_`, as in URLs.
pub fn anchor_hash(name: &str) -> u32 {
    name.bytes().fold(0x811C_9DC5u32, |h, b| {
        let b = if b == b' ' { b'_' } else { b };
        (h ^ b as u32).wrapping_mul(0x0100_0193)
    })
}

/// The anchor a table-of-contents entry gets by rule (spec 7.2): its text
/// with the spaces at either end removed and every run of spaces inside
/// made one `_`. Each entry on its own: two entries with one text get one
/// name, and the converter lists the anchor of the second.
pub fn heading_anchor(text: &str) -> String {
    text.split(' ')
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join("_")
}

/// [`anchor_hash`] of [`heading_anchor`], without building the name.
pub fn heading_anchor_hash(text: &str) -> u32 {
    let mut h = 0x811C_9DC5u32;
    let mut first = true;
    for word in text.split(' ').filter(|w| !w.is_empty()) {
        if !first {
            h = (h ^ b'_' as u32).wrapping_mul(0x0100_0193);
        }
        first = false;
        for b in word.bytes() {
            h = (h ^ b as u32).wrapping_mul(0x0100_0193);
        }
    }
    h
}

/// The section a named place is in (spec 7.2): the article's own anchor
/// list first, then the anchors its headings get by rule, the first that
/// matches. `None` when the article has no such place; links then open at
/// the top.
pub fn find_anchor(header: &ArticleHeader, hash: u32) -> Option<u32> {
    if let Some(a) = header.anchors.iter().find(|a| a.hash == hash) {
        return Some(a.section);
    }
    header
        .toc
        .iter()
        .position(|e| heading_anchor_hash(&e.text) == hash)
        .map(|i| i as u32 + 1)
}

/// Concatenated text of inlines, for headings in the table of contents.
/// A tidy line (spec 7.3), as [`tidy_line`] makes it.
pub fn plain_text(inlines: &[Inline]) -> String {
    let mut out = String::new();
    fn walk(inlines: &[Inline], out: &mut String) {
        for i in inlines {
            match i {
                Inline::Text(t) => out.push_str(t),
                Inline::Styled(_, c) | Inline::Link { content: c, .. } => walk(c, out),
                Inline::Break => out.push(' '),
                Inline::Math(_) | Inline::Icon { .. } | Inline::NoteRef(_) => {}
            }
        }
    }
    walk(inlines, &mut out);
    match tidy_line(&out) {
        Cow::Borrowed(_) => out,
        Cow::Owned(tidy) => tidy,
    }
}

// ------------------------------------------------------------- text rules

/// The no-break spaces, which text keeps as they are (spec 7.3).
pub fn is_no_break_space(c: char) -> bool {
    matches!(c, '\u{A0}' | '\u{2007}' | '\u{202F}')
}

/// White space text may not hold: all but the space and the no-break
/// spaces.
fn is_other_space(c: char) -> bool {
    c != ' ' && c.is_whitespace() && !is_no_break_space(c)
}

/// Whether `s` is a tidy line (spec 7.3): in NFC, without control
/// characters, white space only as single spaces (U+0020) and no-break
/// spaces, and no space at either end.
pub fn is_tidy_line(s: &str) -> bool {
    let mut after_space = true;
    for c in s.chars() {
        if c == ' ' {
            if after_space {
                return false;
            }
            after_space = true;
        } else if c.is_control() || is_other_space(c) {
            return false;
        } else {
            after_space = false;
        }
    }
    !s.ends_with(' ') && is_nfc(s)
}

/// `s` as a tidy line: control characters left out, runs of other white
/// space made one space, none at either end, then NFC.
pub fn tidy_line(s: &str) -> Cow<'_, str> {
    if is_tidy_line(s) {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut space = false;
    for c in s.chars() {
        if c == ' ' || is_other_space(c) {
            space = !out.is_empty();
        } else if !c.is_control() {
            if space {
                out.push(' ');
                space = false;
            }
            out.push(c);
        }
    }
    Cow::Owned(out.nfc().collect())
}

/// Preformatted text as stored (spec 7.3): in NFC, lines ending in a line
/// feed alone, tabs expanded with spaces to the next multiple of 8
/// characters, no other control characters, and no empty lines or
/// trailing spaces at the ends.
pub fn tidy_pre(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut column = 0usize;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\n');
                column = 0;
            }
            '\n' | '\u{85}' | '\u{2028}' | '\u{2029}' => {
                out.push('\n');
                column = 0;
            }
            '\t' => {
                let to = (column / 8 + 1) * 8;
                out.extend(std::iter::repeat_n(' ', to - column));
                column = to;
            }
            c if c.is_control() => {}
            c => {
                out.push(c);
                column += 1;
            }
        }
    }
    let out: String = out.nfc().collect();
    out.trim_start_matches('\n')
        .trim_end_matches(['\n', ' '])
        .to_string()
}

/// Whether `s` is as [`tidy_pre`] leaves preformatted text.
fn is_tidy_pre(s: &str) -> bool {
    !s.starts_with('\n')
        && !s.ends_with(['\n', ' '])
        && s.chars().all(|c| c == '\n' || !c.is_control())
        && !s.contains(['\u{2028}', '\u{2029}'])
        && is_nfc(s)
}

/// Where an article's text breaks the rules readers may rely on (spec
/// 7.3), one line per problem.
pub fn text_problems(a: &Article) -> Vec<String> {
    let mut problems = Vec::new();
    if !is_tidy_line(&a.title) {
        problems.push(format!("title {:?} is not a tidy line", a.title));
    }
    fn blocks(list: &[Block], p: &mut Vec<String>) {
        for b in list {
            match b {
                Block::Para(c) | Block::Heading { content: c, .. } => run(c, p),
                Block::Figure { caption, .. } => run(caption, p),
                Block::Gallery(items) => items.iter().for_each(|(_, c)| run(c, p)),
                Block::Tree(t) => t.runs().into_iter().for_each(|c| run(c, p)),
                Block::List { items, .. } => items.iter().for_each(|i| blocks(i, p)),
                Block::Quote(inner) => blocks(inner, p),
                Block::Pre(t) if !is_tidy_pre(t) => {
                    p.push(format!("preformatted text {t:?} is not tidy"))
                }
                Block::Pre(_)
                | Block::Rule
                | Block::MathBlock(_)
                | Block::TableRef(_)
                | Block::InfoboxRef => {}
            }
        }
    }
    /// What the rules see of a run: its characters, with anything else as
    /// a stand-in (`\0` for an icon, formula or note, `\n` for a break).
    fn run(inlines: &[Inline], p: &mut Vec<String>) {
        let mut flat = String::new();
        fn walk(inlines: &[Inline], flat: &mut String, p: &mut Vec<String>) {
            for i in inlines {
                match i {
                    Inline::Text(t) => {
                        if t.is_empty()
                            || !is_nfc(t)
                            || t.chars().any(|c| c.is_control() || is_other_space(c))
                        {
                            p.push(format!("text {t:?} is empty, not NFC, or holds control characters or other white space"));
                        }
                        flat.push_str(t);
                    }
                    Inline::Styled(_, c) | Inline::Link { content: c, .. } => walk(c, flat, p),
                    Inline::Break => flat.push('\n'),
                    Inline::Math(_) | Inline::NoteRef(_) => flat.push('\0'),
                    Inline::Icon { height, .. } => {
                        if *height == 0 {
                            p.push("an icon has height 0".to_string());
                        }
                        flat.push('\0');
                    }
                }
            }
        }
        walk(inlines, &mut flat, p);
        for bad in ["  ", " \n", "\n ", "\n\n"] {
            if flat.contains(bad) {
                p.push(format!("text {flat:?} has {bad:?}"));
            }
        }
        if flat.starts_with([' ', '\n']) || flat.ends_with([' ', '\n']) {
            p.push(format!(
                "text {flat:?} starts or ends with a space or break"
            ));
        }
    }
    blocks(&a.body, &mut problems);
    for t in &a.tables {
        for c in &t.cells {
            run(&c.content, &mut problems);
        }
        run(&t.caption, &mut problems);
    }
    if let Some(ib) = &a.infobox {
        for row in &ib.rows {
            match row {
                InfoboxRow::Header(c) | InfoboxRow::Full(c) => run(c, &mut problems),
                InfoboxRow::Image { caption, .. } => run(caption, &mut problems),
                InfoboxRow::Field { label, value } => {
                    run(label, &mut problems);
                    run(value, &mut problems);
                }
            }
        }
    }
    problems
}

// ---------------------------------------------------------------- encoding

fn put_str(out: &mut Vec<u8>, s: &str) {
    varint::put(out, s.len() as u64);
    out.extend_from_slice(s.as_bytes());
}

fn encode_inline_ops(out: &mut Vec<u8>, inlines: &[Inline]) {
    for i in inlines {
        match i {
            Inline::Text(t) => {
                let b = t.as_bytes();
                if b.is_empty() {
                } else if b.len() < 0x80 {
                    out.push(b.len() as u8);
                    out.extend_from_slice(b);
                } else {
                    out.push(op::TEXT_LONG);
                    varint::put(out, b.len() as u64);
                    out.extend_from_slice(b);
                }
            }
            Inline::Styled(style, content) => {
                out.push(op::STYLE_PUSH);
                out.push(style.code());
                encode_inline_ops(out, content);
                out.push(op::STYLE_POP);
            }
            Inline::Link {
                article,
                place,
                content,
            } => {
                match place {
                    Place::Section(section) => {
                        out.push(op::LINK_START);
                        varint::put(out, *article as u64);
                        varint::put(out, *section as u64);
                    }
                    Place::Anchor(hash) => {
                        out.push(op::LINK_ANCHOR);
                        varint::put(out, *article as u64);
                        out.extend_from_slice(&hash.to_le_bytes());
                    }
                }
                encode_inline_ops(out, content);
                out.push(op::LINK_END);
            }
            Inline::Math(f) => {
                out.push(op::MATH);
                encode_formula(out, f);
            }
            Inline::Icon { image, height } => {
                out.push(op::ICON);
                varint::put(out, *image as u64);
                out.push(*height);
            }
            Inline::Break => out.push(op::BREAK),
            Inline::NoteRef(n) => {
                out.push(op::NOTE_REF);
                varint::put(out, *n as u64);
            }
        }
    }
}

/// An inline run: `varint length` then inline opcodes.
fn encode_run(out: &mut Vec<u8>, inlines: &[Inline]) {
    let mut ops = Vec::new();
    encode_inline_ops(&mut ops, inlines);
    varint::put(out, ops.len() as u64);
    out.extend_from_slice(&ops);
}

/// A formula record (spec 8.1): `varint byte_length`, then the record.
pub fn encode_formula(out: &mut Vec<u8>, f: &Formula) {
    let mut record = Vec::new();
    encode_formula_record(&mut record, f);
    varint::put(out, record.len() as u64);
    out.extend_from_slice(&record);
}

/// The record after its length: the box, the fallback image, then the
/// items up to the end.
fn encode_formula_record(out: &mut Vec<u8>, f: &Formula) {
    out.push(u8::from(f.display) | (u8::from(f.fallback_image.is_some()) << 1));
    for v in [f.width, f.ascent, f.descent] {
        varint::put_signed(out, v as i64);
    }
    if let Some(img) = f.fallback_image {
        varint::put(out, img as u64);
    }
    for item in &f.items {
        match *item {
            MathItem::Path { dx, dy, ref ops } => {
                out.push(3);
                varint::put_signed(out, dx as i64);
                varint::put_signed(out, dy as i64);
                varint::put(out, ops.len() as u64);
                // Every point is a delta from the point written before it.
                let mut pen = (0i64, 0i64);
                let mut point = |out: &mut Vec<u8>, x: i32, y: i32| {
                    varint::put_signed(out, x as i64 - pen.0);
                    varint::put_signed(out, y as i64 - pen.1);
                    pen = (x as i64, y as i64);
                };
                for op in ops {
                    match *op {
                        PathOp::Move(x, y) => {
                            out.push(0);
                            point(out, x, y);
                        }
                        PathOp::Line(x, y) => {
                            out.push(1);
                            point(out, x, y);
                        }
                        PathOp::Quad(cx, cy, x, y) => {
                            out.push(2);
                            point(out, cx, cy);
                            point(out, x, y);
                        }
                        PathOp::Cubic(ax, ay, bx, by, x, y) => {
                            out.push(3);
                            point(out, ax, ay);
                            point(out, bx, by);
                            point(out, x, y);
                        }
                        PathOp::Close => out.push(4),
                    }
                }
            }
            MathItem::Glyph {
                glyph,
                scale,
                dx,
                dy,
            } => {
                out.push(0);
                varint::put(out, glyph as u64);
                out.push(scale);
                varint::put_signed(out, dx as i64);
                varint::put_signed(out, dy as i64);
            }
            MathItem::GlyphXY {
                glyph,
                sx,
                sy,
                dx,
                dy,
            } => {
                out.push(1);
                varint::put(out, glyph as u64);
                out.extend_from_slice(&sx.to_le_bytes());
                out.extend_from_slice(&sy.to_le_bytes());
                varint::put_signed(out, dx as i64);
                varint::put_signed(out, dy as i64);
            }
            MathItem::Rule { dx, dy, w, h } => {
                out.push(2);
                varint::put_signed(out, dx as i64);
                varint::put_signed(out, dy as i64);
                varint::put(out, w as u64);
                varint::put(out, h as u64);
            }
        }
    }
}

fn encode_block(out: &mut Vec<u8>, block: &Block) {
    match block {
        Block::Para(c) => {
            out.push(op::PARA);
            encode_run(out, c);
        }
        Block::Heading { level, content } => {
            out.push(op::HEADING);
            out.push(*level);
            encode_run(out, content);
        }
        Block::List { kind, items } => {
            out.push(op::LIST_START);
            out.push(match kind {
                ListKind::Bullet => 0,
                ListKind::Number => 1,
                ListKind::Definition => 2,
            });
            for item in items {
                out.push(op::LIST_ITEM);
                for b in item {
                    encode_block(out, b);
                }
            }
            out.push(op::LIST_END);
        }
        Block::Quote(blocks) => {
            out.push(op::QUOTE_START);
            for b in blocks {
                encode_block(out, b);
            }
            out.push(op::QUOTE_END);
        }
        Block::Pre(t) => {
            out.push(op::PRE);
            put_str(out, t);
        }
        Block::Rule => out.push(op::RULE),
        Block::Figure { image, caption } => {
            out.push(op::FIGURE);
            varint::put(out, *image as u64);
            encode_run(out, caption);
        }
        Block::MathBlock(f) => {
            out.push(op::MATH_BLOCK);
            encode_formula(out, f);
        }
        Block::TableRef(i) => {
            out.push(op::TABLE_REF);
            varint::put(out, *i as u64);
        }
        Block::InfoboxRef => out.push(op::INFOBOX_REF),
        Block::Gallery(items) => {
            out.push(op::GALLERY);
            varint::put(out, items.len() as u64);
            for (image, caption) in items {
                varint::put(out, *image as u64);
                encode_run(out, caption);
            }
        }
        Block::Tree(t) => {
            fn node(out: &mut Vec<u8>, n: &TreeNode) {
                varint::put(out, n.children.len() as u64);
                encode_run(out, &n.label);
                encode_run(out, &n.sublabel);
                if n.children.is_empty() {
                    encode_run(out, &n.leaf);
                }
                n.children.iter().for_each(|c| node(out, c));
            }
            out.push(op::TREE);
            encode_run(out, &t.caption);
            node(out, &t.root);
        }
    }
}

fn encode_table(out: &mut Vec<u8>, t: &Table) {
    varint::put(out, t.rows as u64);
    varint::put(out, t.cols as u64);
    out.push(t.flags);
    varint::put(out, t.cells.len() as u64);
    for c in &t.cells {
        out.push(u8::from(c.header));
        varint::put(out, c.rowspan as u64);
        varint::put(out, c.colspan as u64);
        encode_run(out, &c.content);
    }
    encode_run(out, &t.caption);
}

fn encode_infobox(out: &mut Vec<u8>, ib: &Infobox) {
    varint::put(out, ib.rows.len() as u64);
    for row in &ib.rows {
        match row {
            InfoboxRow::Header(c) => {
                out.push(0);
                encode_run(out, c);
            }
            InfoboxRow::Image { image, caption } => {
                out.push(1);
                varint::put(out, *image as u64);
                encode_run(out, caption);
            }
            InfoboxRow::Field { label, value } => {
                out.push(2);
                encode_run(out, label);
                encode_run(out, value);
            }
            InfoboxRow::Full(c) => {
                out.push(3);
                encode_run(out, c);
            }
        }
    }
}

/// Encodes a whole article stream, including the leading `stream_size` varint.
pub fn encode_article(a: &Article) -> Vec<u8> {
    let mut body = Vec::new();
    let mut toc = Vec::new();
    for block in &a.body {
        if let Block::Heading { level, content } = block {
            toc.push(TocEntry {
                level: *level,
                text: plain_text(content),
                pos: body.len() as u32,
            });
        }
        encode_block(&mut body, block);
    }
    let mut tail = Vec::new();
    let mut table_pos = Vec::with_capacity(a.tables.len());
    for t in &a.tables {
        table_pos.push((body.len() + tail.len()) as u32);
        encode_table(&mut tail, t);
    }
    let infobox_pos = a.infobox.as_ref().map(|ib| {
        let pos = (body.len() + tail.len()) as u32;
        encode_infobox(&mut tail, ib);
        pos
    });

    let mut header = Vec::new();
    put_str(&mut header, &a.title);
    varint::put(&mut header, body.len() as u64);
    varint::put(&mut header, toc.len() as u64);
    for e in &toc {
        header.push(e.level);
        put_str(&mut header, &e.text);
        varint::put(&mut header, e.pos as u64);
    }
    varint::put(&mut header, table_pos.len() as u64);
    for p in &table_pos {
        varint::put(&mut header, *p as u64);
    }
    varint::put(&mut header, infobox_pos.map_or(0, |p| p as u64 + 1));
    varint::put(&mut header, a.anchors.len() as u64);
    for anchor in &a.anchors {
        header.extend_from_slice(&anchor.hash.to_le_bytes());
        varint::put(&mut header, anchor.section as u64);
    }

    let mut out = Vec::with_capacity(8 + header.len() + body.len() + tail.len());
    varint::put(&mut out, (header.len() + body.len() + tail.len()) as u64);
    out.extend_from_slice(&header);
    out.extend_from_slice(&body);
    out.extend_from_slice(&tail);
    out
}

// ---------------------------------------------------------------- decoding

struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn at_end(&self) -> bool {
        self.pos >= self.buf.len()
    }

    fn u8(&mut self) -> Result<u8> {
        let b = *self
            .buf
            .get(self.pos)
            .ok_or_else(|| corrupt("content truncated"))?;
        self.pos += 1;
        Ok(b)
    }

    fn i16(&mut self) -> Result<i16> {
        let b = self.bytes(2)?;
        Ok(i16::from_le_bytes([b[0], b[1]]))
    }

    fn u32le(&mut self) -> Result<u32> {
        let b = self.bytes(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn varint(&mut self) -> Result<u64> {
        varint::get(self.buf, &mut self.pos).ok_or_else(|| corrupt("bad varint in content"))
    }

    fn u32v(&mut self) -> Result<u32> {
        u32::try_from(self.varint()?).map_err(|_| corrupt("value out of range in content"))
    }

    fn i32v(&mut self) -> Result<i32> {
        let v = varint::get_signed(self.buf, &mut self.pos)
            .ok_or_else(|| corrupt("bad varint in content"))?;
        i32::try_from(v).map_err(|_| corrupt("value out of range in content"))
    }

    fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.buf.len())
            .ok_or_else(|| corrupt("content truncated"))?;
        let b = &self.buf[self.pos..end];
        self.pos = end;
        Ok(b)
    }

    fn string(&mut self) -> Result<String> {
        let n = self.varint()? as usize;
        let b = self.bytes(n)?;
        String::from_utf8(b.to_vec()).map_err(|_| corrupt("text is not UTF-8"))
    }

    /// Counts that drive allocation must not exceed what the input could hold.
    fn count(&mut self) -> Result<usize> {
        let n = self.varint()?;
        if n > (self.buf.len() - self.pos) as u64 {
            return Err(corrupt("count larger than remaining content"));
        }
        Ok(n as usize)
    }
}

fn decode_run(c: &mut Cursor) -> Result<Vec<Inline>> {
    let len = c.varint()? as usize;
    let mut run = Cursor::new(c.bytes(len)?);
    let mut stack: Vec<(Option<Inline>, Vec<Inline>)> = vec![(None, Vec::new())];
    while !run.at_end() {
        let code = run.u8()?;
        let leaf = match code {
            0x01..=0x7F => Some(Inline::Text(utf8(run.bytes(code as usize)?)?)),
            op::TEXT_LONG => {
                let n = run.varint()? as usize;
                Some(Inline::Text(utf8(run.bytes(n)?)?))
            }
            op::STYLE_PUSH | op::LINK_START | op::LINK_ANCHOR => {
                if stack.len() > MAX_DEPTH {
                    return Err(corrupt("inline nesting too deep"));
                }
                let open = if code == op::STYLE_PUSH {
                    let s = run.u8()?;
                    let style = *Style::ALL
                        .get(s as usize)
                        .ok_or_else(|| corrupt(format!("unknown style {s}")))?;
                    Inline::Styled(style, Vec::new())
                } else {
                    let article = run.u32v()?;
                    let place = if code == op::LINK_START {
                        Place::Section(run.u32v()?)
                    } else {
                        Place::Anchor(run.u32le()?)
                    };
                    Inline::Link {
                        article,
                        place,
                        content: Vec::new(),
                    }
                };
                stack.push((Some(open), Vec::new()));
                None
            }
            op::STYLE_POP | op::LINK_END => {
                let (open, children) = stack
                    .pop()
                    .filter(|_| !stack.is_empty())
                    .ok_or_else(|| corrupt("unbalanced inline end"))?;
                let closed = match (open, code) {
                    (Some(Inline::Styled(s, _)), op::STYLE_POP) => Inline::Styled(s, children),
                    (Some(Inline::Link { article, place, .. }), op::LINK_END) => Inline::Link {
                        article,
                        place,
                        content: children,
                    },
                    _ => return Err(corrupt("mismatched inline end")),
                };
                Some(closed)
            }
            op::MATH => Some(Inline::Math(decode_formula(&mut run)?)),
            op::ICON => Some(Inline::Icon {
                image: run.u32v()?,
                height: run.u8()?,
            }),
            op::BREAK => Some(Inline::Break),
            op::NOTE_REF => Some(Inline::NoteRef(run.u32v()?)),
            op::INLINE_EXT => {
                let n = run.varint()? as usize;
                run.bytes(n)?;
                None
            }
            other => return Err(corrupt(format!("unknown inline opcode {other:#04x}"))),
        };
        if let Some(i) = leaf {
            stack.last_mut().unwrap().1.push(i);
        }
    }
    if stack.len() != 1 {
        return Err(corrupt("unclosed style or link"));
    }
    Ok(stack.pop().unwrap().1)
}

fn utf8(b: &[u8]) -> Result<String> {
    String::from_utf8(b.to_vec()).map_err(|_| corrupt("text is not UTF-8"))
}

/// Reads what [`encode_formula`] writes.
pub fn decode_formula_bytes(buf: &[u8]) -> Result<Formula> {
    let mut c = Cursor::new(buf);
    let f = decode_formula(&mut c)?;
    if !c.at_end() {
        return Err(corrupt("bytes after the formula"));
    }
    Ok(f)
}

fn decode_formula(c: &mut Cursor) -> Result<Formula> {
    let len = c.varint()? as usize;
    decode_formula_record(&mut Cursor::new(c.bytes(len)?))
}

/// The record after its length; its items run to its end.
fn decode_formula_record(c: &mut Cursor) -> Result<Formula> {
    let flags = c.u8()?;
    let mut f = Formula {
        display: flags & 1 != 0,
        width: c.i32v()?,
        ascent: c.i32v()?,
        descent: c.i32v()?,
        ..Default::default()
    };
    if flags & 2 != 0 {
        f.fallback_image = Some(c.u32v()?);
    }
    while !c.at_end() {
        let item = match c.u8()? {
            0 => MathItem::Glyph {
                glyph: c.u32v()?,
                scale: c.u8()?,
                dx: c.i32v()?,
                dy: c.i32v()?,
            },
            1 => MathItem::GlyphXY {
                glyph: c.u32v()?,
                sx: c.i16()?,
                sy: c.i16()?,
                dx: c.i32v()?,
                dy: c.i32v()?,
            },
            2 => MathItem::Rule {
                dx: c.i32v()?,
                dy: c.i32v()?,
                w: c.u32v()?,
                h: c.u32v()?,
            },
            3 => {
                let (dx, dy) = (c.i32v()?, c.i32v()?);
                let n = c.count()?;
                let mut ops = Vec::with_capacity(n);
                let mut pen = (0i64, 0i64);
                let mut point = |c: &mut Cursor| -> Result<(i32, i32)> {
                    pen.0 += c.i32v()? as i64;
                    pen.1 += c.i32v()? as i64;
                    let fit = |v: i64| {
                        i32::try_from(v).map_err(|_| corrupt("path coordinate out of range"))
                    };
                    Ok((fit(pen.0)?, fit(pen.1)?))
                };
                for _ in 0..n {
                    ops.push(match c.u8()? {
                        0 => {
                            let (x, y) = point(c)?;
                            PathOp::Move(x, y)
                        }
                        1 => {
                            let (x, y) = point(c)?;
                            PathOp::Line(x, y)
                        }
                        2 => {
                            let (a, b) = (point(c)?, point(c)?);
                            PathOp::Quad(a.0, a.1, b.0, b.1)
                        }
                        3 => {
                            let (a, b, e) = (point(c)?, point(c)?, point(c)?);
                            PathOp::Cubic(a.0, a.1, b.0, b.1, e.0, e.1)
                        }
                        4 => PathOp::Close,
                        k => return Err(corrupt(format!("unknown path command {k}"))),
                    });
                }
                MathItem::Path { dx, dy, ops }
            }
            t => return Err(corrupt(format!("unknown math item {t}"))),
        };
        f.items.push(item);
    }
    Ok(f)
}

/// Decodes blocks until the cursor ends or `end` is the next opcode.
fn decode_blocks(c: &mut Cursor, end: Option<u8>, depth: usize) -> Result<Vec<Block>> {
    if depth > MAX_DEPTH {
        return Err(corrupt("block nesting too deep"));
    }
    let mut blocks = Vec::new();
    loop {
        if c.at_end() {
            return match end {
                None => Ok(blocks),
                Some(_) => Err(corrupt("unterminated list or quote")),
            };
        }
        let code = c.buf[c.pos];
        if Some(code) == end || (end == Some(op::LIST_END) && code == op::LIST_ITEM) {
            return Ok(blocks);
        }
        c.pos += 1;
        let block = match code {
            op::PARA => Block::Para(decode_run(c)?),
            op::HEADING => Block::Heading {
                level: c.u8()?,
                content: decode_run(c)?,
            },
            op::LIST_START => {
                let kind = match c.u8()? {
                    0 => ListKind::Bullet,
                    1 => ListKind::Number,
                    2 => ListKind::Definition,
                    k => return Err(corrupt(format!("unknown list kind {k}"))),
                };
                let mut items = Vec::new();
                loop {
                    match c.u8()? {
                        op::LIST_ITEM => {
                            items.push(decode_blocks(c, Some(op::LIST_END), depth + 1)?)
                        }
                        op::LIST_END => break,
                        other => {
                            return Err(corrupt(format!("expected list item, found {other:#04x}")));
                        }
                    }
                }
                Block::List { kind, items }
            }
            op::QUOTE_START => {
                let inner = decode_blocks(c, Some(op::QUOTE_END), depth + 1)?;
                c.u8()?;
                Block::Quote(inner)
            }
            op::PRE => Block::Pre(c.string()?),
            op::RULE => Block::Rule,
            op::FIGURE => Block::Figure {
                image: c.u32v()?,
                caption: decode_run(c)?,
            },
            op::MATH_BLOCK => Block::MathBlock(decode_formula(c)?),
            op::TABLE_REF => Block::TableRef(c.u32v()?),
            op::INFOBOX_REF => Block::InfoboxRef,
            op::GALLERY => {
                let n = c.count()?;
                let mut items = Vec::with_capacity(n);
                for _ in 0..n {
                    items.push((c.u32v()?, decode_run(c)?));
                }
                Block::Gallery(items)
            }
            op::TREE => {
                fn tree_node(c: &mut Cursor, depth: usize) -> Result<TreeNode> {
                    if depth > MAX_DEPTH {
                        return Err(corrupt("tree too deep"));
                    }
                    let n = c.count()?;
                    let mut node = TreeNode {
                        label: decode_run(c)?,
                        sublabel: decode_run(c)?,
                        ..TreeNode::default()
                    };
                    if n == 0 {
                        node.leaf = decode_run(c)?;
                    }
                    for _ in 0..n {
                        node.children.push(tree_node(c, depth + 1)?);
                    }
                    Ok(node)
                }
                let caption = decode_run(c)?;
                Block::Tree(Tree {
                    caption,
                    root: tree_node(c, 1)?,
                })
            }
            op::BLOCK_EXT => {
                let n = c.varint()? as usize;
                c.bytes(n)?;
                continue;
            }
            other => return Err(corrupt(format!("unknown block opcode {other:#04x}"))),
        };
        blocks.push(block);
    }
}

fn decode_table(c: &mut Cursor) -> Result<Table> {
    let mut t = Table {
        rows: c.u32v()?,
        cols: c.u32v()?,
        flags: c.u8()?,
        cells: Vec::new(),
        caption: Vec::new(),
    };
    let n = c.count()?;
    t.cells.reserve(n);
    for _ in 0..n {
        let flags = c.u8()?;
        t.cells.push(Cell {
            header: flags & 1 != 0,
            rowspan: c.u32v()?,
            colspan: c.u32v()?,
            content: decode_run(c)?,
        });
    }
    t.caption = decode_run(c)?;
    Ok(t)
}

fn decode_infobox(c: &mut Cursor) -> Result<Infobox> {
    let n = c.count()?;
    let mut rows = Vec::with_capacity(n);
    for _ in 0..n {
        rows.push(match c.u8()? {
            0 => InfoboxRow::Header(decode_run(c)?),
            1 => InfoboxRow::Image {
                image: c.u32v()?,
                caption: decode_run(c)?,
            },
            2 => InfoboxRow::Field {
                label: decode_run(c)?,
                value: decode_run(c)?,
            },
            3 => InfoboxRow::Full(decode_run(c)?),
            k => return Err(corrupt(format!("unknown infobox row kind {k}"))),
        });
    }
    Ok(Infobox { rows })
}

/// Reads the `stream_size` prefix: returns (payload length, prefix length).
pub fn stream_prefix(bytes: &[u8]) -> Result<(usize, usize)> {
    let mut p = 0;
    let size = varint::get(bytes, &mut p).ok_or_else(|| corrupt("bad article stream size"))?;
    Ok((size as usize, p))
}

/// Decodes the header at the start of a stream payload (without the size
/// prefix). Returns the header and the payload offset where the body starts.
pub fn decode_header(payload: &[u8]) -> Result<(ArticleHeader, usize)> {
    let mut c = Cursor::new(payload);
    let title = c.string()?;
    let body_size = c.u32v()?;
    let n = c.count()?;
    let mut toc = Vec::with_capacity(n);
    for _ in 0..n {
        toc.push(TocEntry {
            level: c.u8()?,
            text: c.string()?,
            pos: c.u32v()?,
        });
    }
    let n = c.count()?;
    let table_pos = (0..n).map(|_| c.u32v()).collect::<Result<Vec<_>>>()?;
    let infobox_pos = match c.u32v()? {
        0 => None,
        p => Some(p - 1),
    };
    let n = c.count()?;
    let mut anchors = Vec::with_capacity(n);
    for _ in 0..n {
        anchors.push(Anchor {
            hash: c.u32le()?,
            section: c.u32v()?,
        });
    }
    Ok((
        ArticleHeader {
            title,
            body_size,
            toc,
            table_pos,
            infobox_pos,
            anchors,
        },
        c.pos,
    ))
}

/// Decodes a full article from its stream payload (without the size prefix).
pub fn decode_article(payload: &[u8]) -> Result<Article> {
    let (h, body_start) = decode_header(payload)?;
    let rest = &payload[body_start..];
    let body_bytes = rest
        .get(..h.body_size as usize)
        .ok_or_else(|| corrupt("article body truncated"))?;
    let body = decode_blocks(&mut Cursor::new(body_bytes), None, 0)?;
    let at = |pos: u32| -> Result<Cursor> {
        let b = rest
            .get(pos as usize..)
            .ok_or_else(|| corrupt("table or infobox position out of range"))?;
        Ok(Cursor::new(b))
    };
    let tables = h
        .table_pos
        .iter()
        .map(|&p| decode_table(&mut at(p)?))
        .collect::<Result<Vec<_>>>()?;
    let infobox = h
        .infobox_pos
        .map(|p| decode_infobox(&mut at(p)?))
        .transpose()?;
    Ok(Article {
        title: h.title,
        body,
        tables,
        infobox,
        anchors: h.anchors,
    })
}

/// Calls `f(target, place)` for every link in the article, in document
/// order, letting it change where the link leads.
pub fn for_each_link_mut(a: &mut Article, f: &mut impl FnMut(u32, &mut Place)) {
    fn inlines(list: &mut [Inline], f: &mut impl FnMut(u32, &mut Place)) {
        for i in list {
            match i {
                Inline::Link {
                    article,
                    place,
                    content,
                } => {
                    f(*article, place);
                    inlines(content, f);
                }
                Inline::Styled(_, c) => inlines(c, f),
                Inline::Text(_)
                | Inline::Math(_)
                | Inline::Icon { .. }
                | Inline::Break
                | Inline::NoteRef(_) => {}
            }
        }
    }
    fn blocks(list: &mut [Block], f: &mut impl FnMut(u32, &mut Place)) {
        for b in list {
            match b {
                Block::Para(c)
                | Block::Heading { content: c, .. }
                | Block::Figure { caption: c, .. } => inlines(c, f),
                Block::List { items, .. } => items.iter_mut().for_each(|i| blocks(i, f)),
                Block::Quote(inner) => blocks(inner, f),
                Block::Gallery(items) => items.iter_mut().for_each(|(_, c)| inlines(c, f)),
                Block::Tree(t) => t.for_each_run_mut(&mut |c| inlines(c, f)),
                Block::Pre(_)
                | Block::Rule
                | Block::MathBlock(_)
                | Block::TableRef(_)
                | Block::InfoboxRef => {}
            }
        }
    }
    blocks(&mut a.body, f);
    for t in &mut a.tables {
        t.cells.iter_mut().for_each(|c| inlines(&mut c.content, f));
        inlines(&mut t.caption, f);
    }
    if let Some(ib) = &mut a.infobox {
        for row in &mut ib.rows {
            match row {
                InfoboxRow::Header(c)
                | InfoboxRow::Full(c)
                | InfoboxRow::Image { caption: c, .. } => inlines(c, f),
                InfoboxRow::Field { label, value } => {
                    inlines(label, f);
                    inlines(value, f);
                }
            }
        }
    }
}

/// Something an article refers to outside its own text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ref<'a> {
    /// Link target article id, and where in it the link leads.
    Link(u32, Place),
    /// Image id (figures, galleries, icons, infobox images, formula fallbacks).
    Image(u32),
    Formula(&'a Formula),
    /// Index into the article's own table list.
    Table(u32),
    Infobox,
}

/// Calls `f` with every piece of text an article shows: its title, the
/// text of its body, tables and infobox, captions and preformatted text.
pub fn for_each_text<'a>(a: &'a Article, f: &mut impl FnMut(&'a str)) {
    for_each_styled_text(a, &mut |t, _| f(t));
}

/// Calls `f` for every piece of text in the article with the style the
/// reader draws it in (spec 10.1): the title, headings of levels 1-4,
/// infobox headers and labels, table header cells and table captions
/// bold; headings of levels 5 and 6 bold italic; everything else regular.
/// Bold and italic marks add to the style around them; preformatted text
/// is regular.
pub fn for_each_styled_text<'a>(a: &'a Article, f: &mut impl FnMut(&'a str, FontStyle)) {
    fn inlines<'a>(list: &'a [Inline], s: FontStyle, f: &mut impl FnMut(&'a str, FontStyle)) {
        for i in list {
            match i {
                Inline::Text(t) => f(t, s),
                Inline::Styled(Style::Bold, c) => {
                    inlines(c, FontStyle::new(true, s.is_italic()), f)
                }
                Inline::Styled(Style::Italic, c) => {
                    inlines(c, FontStyle::new(s.is_bold(), true), f)
                }
                Inline::Styled(_, c) | Inline::Link { content: c, .. } => inlines(c, s, f),
                Inline::Math(_) | Inline::Icon { .. } | Inline::Break | Inline::NoteRef(_) => {}
            }
        }
    }
    fn blocks<'a>(list: &'a [Block], f: &mut impl FnMut(&'a str, FontStyle)) {
        for b in list {
            match b {
                Block::Para(c) => inlines(c, FontStyle::Regular, f),
                Block::Heading { level, content } => {
                    let style = FontStyle::new(true, *level >= 5);
                    inlines(content, style, f)
                }
                Block::Figure { caption, .. } => inlines(caption, FontStyle::Regular, f),
                Block::Gallery(items) => items
                    .iter()
                    .for_each(|(_, c)| inlines(c, FontStyle::Regular, f)),
                Block::Tree(t) => t
                    .runs()
                    .into_iter()
                    .for_each(|c| inlines(c, FontStyle::Regular, f)),
                Block::List { items, .. } => items.iter().for_each(|i| blocks(i, f)),
                Block::Quote(inner) => blocks(inner, f),
                Block::Pre(t) => f(t, FontStyle::Regular),
                Block::MathBlock(_) | Block::TableRef(_) | Block::InfoboxRef | Block::Rule => {}
            }
        }
    }
    f(&a.title, FontStyle::Bold);
    blocks(&a.body, f);
    for t in &a.tables {
        for c in &t.cells {
            inlines(&c.content, FontStyle::new(c.header, false), f);
        }
        inlines(&t.caption, FontStyle::Bold, f);
    }
    for row in a.infobox.iter().flat_map(|ib| &ib.rows) {
        match row {
            InfoboxRow::Header(c) => inlines(c, FontStyle::Bold, f),
            InfoboxRow::Full(c) => inlines(c, FontStyle::Regular, f),
            InfoboxRow::Image { caption, .. } => inlines(caption, FontStyle::Regular, f),
            InfoboxRow::Field { label, value } => {
                inlines(label, FontStyle::Bold, f);
                inlines(value, FontStyle::Regular, f);
            }
        }
    }
}

/// Calls `f` for every reference in the article, in document order.
pub fn walk_refs<'a>(a: &'a Article, f: &mut impl FnMut(Ref<'a>)) {
    fn formula<'a>(x: &'a Formula, f: &mut impl FnMut(Ref<'a>)) {
        f(Ref::Formula(x));
        if let Some(img) = x.fallback_image {
            f(Ref::Image(img));
        }
    }
    fn inlines<'a>(list: &'a [Inline], f: &mut impl FnMut(Ref<'a>)) {
        for i in list {
            match i {
                Inline::Styled(_, c) => inlines(c, f),
                Inline::Link {
                    article,
                    place,
                    content,
                } => {
                    f(Ref::Link(*article, *place));
                    inlines(content, f);
                }
                Inline::Math(x) => formula(x, f),
                Inline::Icon { image, .. } => f(Ref::Image(*image)),
                Inline::Text(_) | Inline::Break | Inline::NoteRef(_) => {}
            }
        }
    }
    fn blocks<'a>(list: &'a [Block], f: &mut impl FnMut(Ref<'a>)) {
        for b in list {
            match b {
                Block::Para(c) | Block::Heading { content: c, .. } => inlines(c, f),
                Block::Figure { image, caption } => {
                    f(Ref::Image(*image));
                    inlines(caption, f);
                }
                Block::Gallery(items) => {
                    for (image, caption) in items {
                        f(Ref::Image(*image));
                        inlines(caption, f);
                    }
                }
                Block::List { items, .. } => items.iter().for_each(|i| blocks(i, f)),
                Block::Quote(inner) => blocks(inner, f),
                Block::Tree(t) => t.runs().into_iter().for_each(|c| inlines(c, f)),
                Block::MathBlock(x) => formula(x, f),
                Block::TableRef(t) => f(Ref::Table(*t)),
                Block::InfoboxRef => f(Ref::Infobox),
                Block::Pre(_) | Block::Rule => {}
            }
        }
    }
    blocks(&a.body, f);
    for t in &a.tables {
        t.cells.iter().for_each(|c| inlines(&c.content, f));
        inlines(&t.caption, f);
    }
    if let Some(ib) = &a.infobox {
        for row in &ib.rows {
            match row {
                InfoboxRow::Header(c) | InfoboxRow::Full(c) => inlines(c, f),
                InfoboxRow::Image { image, caption } => {
                    f(Ref::Image(*image));
                    inlines(caption, f);
                }
                InfoboxRow::Field { label, value } => {
                    inlines(label, f);
                    inlines(value, f);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> Inline {
        Inline::Text(s.to_string())
    }

    pub(crate) fn sample_article() -> Article {
        let formula = Formula {
            display: false,
            width: 2181,
            ascent: 791,
            descent: 217,
            items: vec![
                MathItem::Glyph {
                    glyph: 3,
                    scale: 0,
                    dx: 0,
                    dy: 0,
                },
                MathItem::GlyphXY {
                    glyph: 9,
                    sx: 256,
                    sy: 720,
                    dx: 751,
                    dy: -12,
                },
                MathItem::Path {
                    dx: -5,
                    dy: 40_000,
                    ops: vec![
                        PathOp::Move(0, 0),
                        PathOp::Line(400_000, -3),
                        PathOp::Quad(1, 2, -3, 4),
                        PathOp::Cubic(5, -6, 7, 8, -9, 10),
                        PathOp::Close,
                    ],
                },
                MathItem::Rule {
                    dx: 1001,
                    dy: 230,
                    w: 1180,
                    h: 40,
                },
            ],
            fallback_image: Some(4),
        };
        Article {
            title: "Pythagorean theorem".into(),
            body: vec![
                Block::Para(vec![
                    text("In "),
                    Inline::Link {
                        article: 12,
                        place: Place::TOP,
                        content: vec![text("mathematics")],
                    },
                    text(", "),
                    Inline::Link {
                        article: 70_000,
                        place: Place::Anchor(anchor_hash("Early life")),
                        content: vec![text("early")],
                    },
                    Inline::Link {
                        article: 3,
                        place: Place::Section(2),
                        content: vec![text("proofs")],
                    },
                    Inline::Math(formula.clone()),
                    Inline::Styled(
                        Style::Bold,
                        vec![
                            text("bold "),
                            Inline::Styled(Style::Italic, vec![text("and italic")]),
                        ],
                    ),
                    Inline::Break,
                    Inline::Icon {
                        image: 2,
                        height: 15,
                    },
                    Inline::NoteRef(1),
                    text("long text ".repeat(40).trim_end()),
                ]),
                Block::Heading {
                    level: 2,
                    content: vec![text("History")],
                },
                Block::List {
                    kind: ListKind::Number,
                    items: vec![
                        vec![Block::Para(vec![text("one")])],
                        vec![],
                        vec![Block::Quote(vec![Block::Para(vec![text("nested")])])],
                    ],
                },
                Block::Pre("a  b\n c".into()),
                Block::Rule,
                Block::Figure {
                    image: 1,
                    caption: vec![text("A figure")],
                },
                Block::MathBlock(Formula {
                    display: true,
                    ..formula
                }),
                Block::TableRef(0),
                Block::InfoboxRef,
                Block::Gallery(vec![(5, vec![text("g1")]), (6, vec![])]),
                Block::Tree(Tree {
                    caption: vec![text("Phylogeny")],
                    root: TreeNode {
                        children: vec![
                            TreeNode {
                                leaf: vec![
                                    text("Snow leopard "),
                                    Inline::Icon {
                                        image: 8,
                                        height: 27,
                                    },
                                ],
                                ..TreeNode::default()
                            },
                            TreeNode {
                                label: vec![Inline::Styled(Style::Italic, vec![text("Panthera")])],
                                sublabel: vec![text("2 Ma")],
                                children: vec![
                                    TreeNode {
                                        leaf: vec![Inline::Link {
                                            article: 44,
                                            place: Place::TOP,
                                            content: vec![text("Tiger")],
                                        }],
                                        ..TreeNode::default()
                                    },
                                    TreeNode {
                                        leaf: vec![text("Lion")],
                                        ..TreeNode::default()
                                    },
                                ],
                                ..TreeNode::default()
                            },
                        ],
                        ..TreeNode::default()
                    },
                }),
                Block::Heading {
                    level: 3,
                    content: vec![
                        text("Proofs "),
                        Inline::Styled(Style::Italic, vec![text("by")]),
                        text(" area"),
                    ],
                },
            ],
            tables: vec![Table {
                caption: vec![
                    text("Areas of the "),
                    Inline::Styled(Style::Italic, vec![text("squares")]),
                ],
                rows: 2,
                cols: 2,
                flags: 1,
                cells: vec![
                    Cell {
                        header: true,
                        rowspan: 1,
                        colspan: 2,
                        content: vec![text("Header")],
                    },
                    Cell {
                        header: false,
                        rowspan: 1,
                        colspan: 1,
                        content: vec![text("a")],
                    },
                    Cell {
                        header: false,
                        rowspan: 1,
                        colspan: 1,
                        content: vec![],
                    },
                ],
            }],
            infobox: Some(Infobox {
                rows: vec![
                    InfoboxRow::Header(vec![text("Pythagoras")]),
                    InfoboxRow::Image {
                        image: 7,
                        caption: vec![text("bust")],
                    },
                    InfoboxRow::Field {
                        label: vec![text("Born")],
                        value: vec![text("c. 570 BC")],
                    },
                    InfoboxRow::Full(vec![text("Philosopher")]),
                ],
            }),
            anchors: vec![
                Anchor {
                    hash: anchor_hash("Euclid.27s_proof"),
                    section: 2,
                },
                Anchor {
                    hash: anchor_hash("Decomposition"),
                    section: 1,
                },
            ],
        }
    }

    #[test]
    fn anchors_hash_and_derive_by_rule() {
        // FNV-1a test vectors.
        assert_eq!(anchor_hash(""), 0x811C_9DC5);
        assert_eq!(anchor_hash("a"), 0xE40C_292C);
        assert_eq!(anchor_hash("foobar"), 0xBF9C_F968);
        // A space counts as the underscore links write.
        assert_eq!(anchor_hash("Early life"), anchor_hash("Early_life"));
        // Each heading on its own, spaces made one `_`.
        for (text, name) in [
            ("History", "History"),
            (" Early  life ", "Early_life"),
            ("History_2", "History_2"),
            ("", ""),
            ("  ", ""),
        ] {
            assert_eq!(heading_anchor(text), name);
            assert_eq!(heading_anchor_hash(text), anchor_hash(name), "{text:?}");
        }
    }

    #[test]
    fn anchors_are_found_in_the_list_then_the_headings() {
        let a = sample_article();
        let bytes = encode_article(&a);
        let (h, _) = decode_header(&bytes[stream_prefix(&bytes).unwrap().1..]).unwrap();
        assert_eq!(h.anchors, a.anchors);
        assert_eq!(find_anchor(&h, anchor_hash("History")), Some(1));
        assert_eq!(find_anchor(&h, anchor_hash("Proofs_by_area")), Some(2));
        assert_eq!(find_anchor(&h, anchor_hash("Euclid.27s_proof")), Some(2));
        assert_eq!(find_anchor(&h, anchor_hash("Decomposition")), Some(1));
        assert_eq!(find_anchor(&h, anchor_hash("Nowhere")), None);
    }

    #[test]
    fn article_round_trips() {
        let a = sample_article();
        let bytes = encode_article(&a);
        let (size, prefix) = stream_prefix(&bytes).unwrap();
        assert_eq!(size, bytes.len() - prefix);
        let payload = &bytes[prefix..];
        assert_eq!(decode_article(payload).unwrap(), a);
        let (h, _) = decode_header(payload).unwrap();
        assert_eq!(
            h.toc
                .iter()
                .map(|e| (e.level, e.text.as_str()))
                .collect::<Vec<_>>(),
            vec![(2, "History"), (3, "Proofs by area")]
        );
        let mut images = Vec::new();
        let (mut links, mut formulas, mut tables, mut infoboxes) = (Vec::new(), 0, Vec::new(), 0);
        walk_refs(&a, &mut |r| match r {
            Ref::Image(i) => images.push(i),
            Ref::Link(l, _) => links.push(l),
            Ref::Formula(_) => formulas += 1,
            Ref::Table(t) => tables.push(t),
            Ref::Infobox => infoboxes += 1,
        });
        assert_eq!(images, vec![4, 2, 1, 4, 5, 6, 8, 7]);
        assert_eq!(
            (links, formulas, tables, infoboxes),
            (vec![12, 70_000, 3, 44], 2, vec![0], 1)
        );
    }

    #[test]
    fn empty_article_round_trips() {
        let a = Article {
            title: String::new(),
            ..Default::default()
        };
        let bytes = encode_article(&a);
        assert_eq!(decode_article(&bytes[1..]).unwrap(), a);
    }

    #[test]
    fn every_truncation_is_an_error_not_a_panic() {
        let bytes = encode_article(&sample_article());
        let (_, prefix) = stream_prefix(&bytes).unwrap();
        let payload = &bytes[prefix..];
        for cut in 0..payload.len() {
            let _ = decode_article(&payload[..cut]);
        }
        let mut damaged = payload.to_vec();
        for i in 0..damaged.len() {
            damaged[i] ^= 0xA5;
            let _ = decode_article(&damaged);
            damaged[i] ^= 0xA5;
        }
    }

    #[test]
    fn lines_are_made_tidy() {
        for (raw, tidy) in [
            ("Plain title", "Plain title"),
            ("  Two\t\tspaces \n", "Two spaces"),
            ("Bell\u{7}less", "Bellless"),
            ("Caf\u{65}\u{301}", "Café"),
            ("10\u{202F}km\u{A0}", "10\u{202F}km\u{A0}"),
            ("", ""),
        ] {
            assert_eq!(tidy_line(raw), tidy, "{raw:?}");
            assert!(is_tidy_line(&tidy_line(raw)));
        }
        assert!(matches!(tidy_line("Already tidy"), Cow::Borrowed(_)));
    }

    #[test]
    fn preformatted_text_is_made_tidy() {
        assert_eq!(
            tidy_pre("\r\nint\tx;\r\n\tab\tc\rend  \n\n"),
            "int     x;\n        ab      c\nend"
        );
        assert_eq!(tidy_pre("a\u{0}b\u{2028}c"), "ab\nc");
    }

    #[test]
    fn text_problems_are_found() {
        assert_eq!(text_problems(&sample_article()), Vec::<String>::new());
        let bad = |run: Vec<Inline>| Article {
            title: "T".into(),
            body: vec![Block::Para(run)],
            ..Default::default()
        };
        let text = |t: &str| Inline::Text(t.into());
        for run in [
            vec![text(" lead")],
            vec![text("trail ")],
            vec![text("a "), Inline::Styled(Style::Bold, vec![text(" b")])],
            vec![text("a "), Inline::Break, text("b")],
            vec![text("a\tb")],
            vec![text("e\u{301}")],
            vec![Inline::Break, text("b")],
        ] {
            assert!(!text_problems(&bad(run.clone())).is_empty(), "{run:?}");
        }
        let fine = vec![
            text("a "),
            Inline::Styled(Style::Bold, vec![text("b")]),
            Inline::Break,
            text("c\u{A0}"),
        ];
        assert!(text_problems(&bad(fine)).is_empty());
    }

    #[test]
    fn rejects_unbalanced_inlines() {
        // run of 2 ops: STYLE_PUSH bold, no POP
        let mut c = Cursor::new(&[3, op::STYLE_PUSH, 0, 0x01]);
        assert!(decode_run(&mut c).is_err());
        let mut c = Cursor::new(&[1, op::STYLE_POP]);
        assert!(decode_run(&mut c).is_err());
    }
}
