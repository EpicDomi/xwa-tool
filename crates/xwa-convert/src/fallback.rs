//! The fallback FONT (spec 10): outline glyphs, from the Noto fonts, for
//! the characters an archive's text uses that the firmware's own fonts
//! cannot draw, and FONT_BOLD (10.1): their bold glyphs, from the bold of
//! the same family. `cargo xtask fonts` makes the catalogue of fonts.
//!
//! Optionally, rare characters too: what no Noto font has, from Plangothic
//! (the rare CJK ideographs of Unicode's extensions B to I, and many of the
//! newest letters and symbols), GNU Unifont, and the other designs of Noto
//! scripts. `cargo xtask fonts --rare` makes their catalogue.

use std::collections::BTreeMap;

use ttf_parser::{Face, OutlineBuilder};
use unicode_normalization::UnicodeNormalization;
use xwa_format::font::{Font, FontChars, FontStyle, Glyph, needs_fallback};
use xwa_format::math_glyphs::{GlyphOp, Outline};
use xwa_format::normalize::CharSet;

/// One line per file: family, style, URL, SHA-256, size, coverage.
static CATALOGUE: &[u8] = include_bytes!("../data/fonts.tsv.zst");
/// The fonts for rare characters, looked in after the Noto fonts: the same,
/// and the mean size of the glyphs each adds.
static RARE: &[u8] = include_bytes!("../data/fonts-rare.tsv.zst");

/// What every glyph is scaled to.
const UPEM: u16 = 1000;
/// Line metrics, Noto Sans's (the fallback for Latin, Greek and Cyrillic).
const ASCENDER: i16 = 1069;
const DESCENDER: i16 = -293;

/// META `license.font`, when an archive has FONT: the sources of the
/// families its glyphs came from.
pub fn license(families: &[String]) -> String {
    let from = |prefix: &str| families.iter().any(|f| f.starts_with(prefix));
    let mut sources = vec!["the Noto fonts (Google Fonts)"];
    if from("Plangothic") {
        sources.push("Plangothic");
    }
    if from("Unifont") {
        sources.push("GNU Unifont");
    }
    let list = match sources.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
        _ => sources[0].to_string(),
    };
    format!("Fallback glyphs from {list}, SIL Open Font License 1.1")
}

/// META key an archive built with rare characters carries (value `yes`),
/// so completing its font later keeps them.
pub const RARE_META: &str = "font.rare_characters";

/// One style of a font family, as a file on Google Fonts.
#[derive(Debug, Clone)]
pub struct FontFile {
    pub url: String,
    pub sha256: String,
    pub size: u64,
    /// The code points it has glyphs for, as sorted inclusive ranges.
    pub coverage: Vec<(u32, u32)>,
    /// For a font for rare characters, the mean size in FONT of the glyphs
    /// it adds; 0 when not known.
    pub glyph_bytes: u32,
}

impl FontFile {
    pub fn covers(&self, c: char) -> bool {
        let c = c as u32;
        let i = self.coverage.partition_point(|&(_, hi)| hi < c);
        self.coverage.get(i).is_some_and(|&(lo, _)| lo <= c)
    }
}

/// A font family the fallback can draw glyphs from.
#[derive(Debug, Clone)]
pub struct CatalogFont {
    pub family: String,
    pub regular: FontFile,
    /// Its bold, italic and bold italic, where the family has them.
    pub styled: [Option<FontFile>; 3],
}

impl CatalogFont {
    /// Whether its regular style has a glyph for `c`.
    pub fn covers(&self, c: char) -> bool {
        self.regular.covers(c)
    }

    /// The file for `style`, if the family has it.
    pub fn file(&self, style: FontStyle) -> Option<&FontFile> {
        match style {
            FontStyle::Regular => Some(&self.regular),
            _ => self.styled[style as usize - 1].as_ref(),
        }
    }
}

/// The catalogue, in the order fonts are looked in.
pub fn catalogue() -> Vec<CatalogFont> {
    parse(CATALOGUE)
}

/// The fonts for rare characters, in the order they are looked in after
/// the Noto fonts.
pub fn rare_catalogue() -> Vec<CatalogFont> {
    parse(RARE)
}

/// The fonts to look in for an archive in `language`: the Noto fonts, then
/// with `rare` the fonts for rare characters.
pub fn fonts(language: Option<&str>, rare: bool) -> Vec<CatalogFont> {
    let mut fonts = search_order(catalogue(), language);
    if rare {
        fonts.extend(rare_catalogue());
    }
    fonts
}

fn parse(packed: &[u8]) -> Vec<CatalogFont> {
    let table = zstd::decode_all(packed).expect("the font catalogue is zstd");
    let table = String::from_utf8(table).expect("the font catalogue is UTF-8");
    let mut out: Vec<CatalogFont> = Vec::new();
    for line in table.lines() {
        let mut f = line.split('\t');
        let (Some(family), Some(style), Some(url), Some(sha256), Some(size), Some(cover)) =
            (f.next(), f.next(), f.next(), f.next(), f.next(), f.next())
        else {
            continue;
        };
        let glyph_bytes = f.next().and_then(|b| b.parse().ok()).unwrap_or(0);
        let coverage = cover
            .split(',')
            .filter_map(|r| {
                let (a, b) = r.split_once('-').unwrap_or((r, r));
                Some((
                    u32::from_str_radix(a, 16).ok()?,
                    u32::from_str_radix(b, 16).ok()?,
                ))
            })
            .collect();
        let Ok(size) = size.parse() else { continue };
        let file = FontFile {
            url: url.to_string(),
            sha256: sha256.to_string(),
            size,
            coverage,
            glyph_bytes,
        };
        // A family's regular file comes first, its other styles after it.
        let styled = match style {
            "bold" => FontStyle::Bold,
            "italic" => FontStyle::Italic,
            "bold-italic" => FontStyle::BoldItalic,
            _ => {
                out.push(CatalogFont {
                    family: family.to_string(),
                    regular: file,
                    styled: Default::default(),
                });
                continue;
            }
        };
        if let Some(font) = out.last_mut().filter(|f| f.family == family) {
            font.styled[styled as usize - 1] = Some(file);
        }
    }
    out
}

/// Every font file the fallback font can be drawn from: each family's
/// regular, bold, italic and bold italic, where it has them.
pub fn all_files() -> Vec<FontFile> {
    catalogue()
        .into_iter()
        .flat_map(|f| std::iter::once(f.regular).chain(f.styled.into_iter().flatten()))
        .collect()
}

/// The CJK fonts in the order a text in `language` (ISO 639-3) wants them:
/// Han characters look different in Japan, Korea and the Chinese regions.
fn cjk_order(language: Option<&str>) -> [&'static str; 5] {
    match language {
        Some("jpn") => ["JP", "SC", "TC", "HK", "KR"],
        Some("kor") => ["KR", "SC", "TC", "HK", "JP"],
        Some("yue") => ["HK", "TC", "SC", "JP", "KR"],
        _ => ["SC", "TC", "HK", "JP", "KR"],
    }
}

/// Whether `c` is in a CJK block: CJK punctuation, kana, Hangul, Han and
/// full-width forms, which other scripts' fonts have some of too but the
/// CJK fonts should draw.
fn is_cjk(c: char) -> bool {
    matches!(
        c as u32,
        0x2E80..=0x2FDF
            | 0x3000..=0x33FF
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xA960..=0xA97F
            | 0xAC00..=0xD7FF
            | 0xF900..=0xFAFF
            | 0xFE30..=0xFE4F
            | 0xFF00..=0xFFEF
            | 0x1F200..=0x1F2FF
            | 0x20000..=0x3FFFF
    )
}

fn is_cjk_font(f: &CatalogFont) -> bool {
    f.family
        .strip_prefix("Noto Sans ")
        .is_some_and(|r| ["SC", "TC", "HK", "JP", "KR"].contains(&r))
}

/// `fonts` in the order to look in them for an archive in `language`.
pub fn search_order(mut fonts: Vec<CatalogFont>, language: Option<&str>) -> Vec<CatalogFont> {
    let order = cjk_order(language);
    let rank = |f: &CatalogFont| {
        f.family
            .strip_prefix("Noto Sans ")
            .and_then(|r| order.iter().position(|o| *o == r))
    };
    let first_cjk = fonts.iter().position(|f| rank(f).is_some());
    if let Some(at) = first_cjk {
        let mut cjk: Vec<CatalogFont> = Vec::new();
        let mut i = at;
        while i < fonts.len() {
            if rank(&fonts[i]).is_some() {
                cjk.push(fonts.remove(i));
            } else {
                i += 1;
            }
        }
        cjk.sort_by_key(|f| rank(f));
        fonts.splice(at..at, cjk);
    }
    fonts
}

/// Whether `c` is an Arabic letter or mark, which the firmware shapes into
/// presentation forms.
fn is_arabic(c: char) -> bool {
    matches!(c as u32, 0x0600..=0x06FF | 0x0750..=0x077F | 0x08A0..=0x08FF)
}

/// The characters FONT should draw for this text: what it uses that the
/// firmware's fonts lack, and for the Arabic letters among them the
/// presentation forms the firmware's shaper can turn them into, the
/// Lam-Alef ligatures included.
pub fn wanted(text: &CharSet) -> Vec<char> {
    let mut out: Vec<char> = text.iter().filter(|&c| needs_fallback(c)).collect();
    if out.iter().any(|&c| is_arabic(c)) {
        let alef = ['\u{622}', '\u{623}', '\u{625}', '\u{627}'];
        let lam_alef = text.contains('\u{644}') && alef.iter().any(|&a| text.contains(a));
        for cp in (0xFB50..=0xFDFF).chain(0xFE70..=0xFEFF) {
            let Some(form) = char::from_u32(cp) else {
                continue;
            };
            let base: Vec<char> = std::iter::once(form).nfkd().collect();
            let wanted = match base[..] {
                // A letter's initial, medial, final or isolated form.
                [b] => is_arabic(b) && text.contains(b),
                _ => (0xFEF5..=0xFEFC).contains(&cp) && lam_alef,
            };
            if wanted {
                out.push(form);
            }
        }
        out.sort_unstable();
        out.dedup();
    }
    out
}

/// What to build for an archive's text: FONT's code points, and each other
/// section's. FONT_BOLD is for text drawn bold or bold italic (a reader
/// draws bold italic from the bold glyph when it has no bold italic);
/// FONT_ITALIC and FONT_BOLD_ITALIC only when real italic is wanted.
pub fn requests(chars: &FontChars) -> (Vec<char>, Vec<(FontStyle, Vec<char>)>) {
    let mut bold = chars.set(FontStyle::Bold).clone();
    bold.union(chars.set(FontStyle::BoldItalic));
    let mut styled = vec![(FontStyle::Bold, wanted(&bold))];
    if chars.real_italic {
        for style in [FontStyle::Italic, FontStyle::BoldItalic] {
            styled.push((style, wanted(chars.set(style))));
        }
    }
    (wanted(chars.set(FontStyle::Regular)), styled)
}

/// Glyphs an archive's font sections lack that `fonts` could supply: how
/// many for FONT, and how many for the other styles. `has(style, c)` says
/// whether the archive has a glyph.
pub fn missing(
    chars: &FontChars,
    fonts: &[CatalogFont],
    has: &dyn Fn(FontStyle, char) -> bool,
) -> (usize, usize) {
    let (wanted, styled) = requests(chars);
    let regular = wanted
        .iter()
        .filter(|&&c| !has(FontStyle::Regular, c) && source(c, fonts).is_some())
        .count();
    let other = styled
        .iter()
        .flat_map(|(style, chars)| chars.iter().map(move |&c| (*style, c)))
        .filter(|&(style, c)| {
            !has(style, c)
                && source(c, fonts)
                    .and_then(|i| fonts[i].file(style))
                    .is_some_and(|f| f.covers(c))
        })
        .count();
    (regular, other)
}

/// What the fonts for rare characters offer, for saying what the option
/// gives and costs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RareSummary {
    /// Characters (assigned in Unicode) they have and no Noto font has.
    pub characters: usize,
    /// How many of those are CJK ideographs of Unicode's extensions B to I.
    pub ideographs: usize,
    /// About how many bytes all of them would add to an archive.
    pub bytes: u64,
    /// The size of their font files, downloaded when first needed.
    pub download: u64,
}

/// What the fonts for rare characters offer beyond the Noto fonts.
pub fn rare_summary() -> RareSummary {
    static SUMMARY: std::sync::OnceLock<RareSummary> = std::sync::OnceLock::new();
    *SUMMARY.get_or_init(|| {
        let mut taken = vec![false; 0x11_0000];
        for (lo, hi) in no_character() {
            taken[*lo as usize..=*hi as usize].fill(true);
        }
        for f in catalogue() {
            for &(lo, hi) in &f.regular.coverage {
                taken[lo as usize..=hi as usize].fill(true);
            }
        }
        let mut out = RareSummary::default();
        for f in rare_catalogue() {
            out.download += f.regular.size;
            for &(lo, hi) in &f.regular.coverage {
                for cp in lo..=hi {
                    if !std::mem::replace(&mut taken[cp as usize], true) {
                        out.characters += 1;
                        out.ideographs += usize::from((0x20000..0x40000).contains(&cp));
                        out.bytes += u64::from(f.regular.glyph_bytes);
                    }
                }
            }
        }
        out
    })
}

/// Characters an archive's FONT has no glyph for, sorted by why.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lacking {
    /// A Noto font has them: completing the font would add them.
    pub in_noto: Vec<char>,
    /// No Noto font has them, but a font for rare characters does.
    pub in_rare: Vec<char>,
    /// About how many bytes their glyphs would add to FONT.
    pub rare_bytes: u64,
    /// How many of them are CJK ideographs of planes 2 and 3 (Unicode's
    /// extensions B to I), of which Noto has only the commonest.
    pub rare_ideographs: usize,
    /// Characters no font xwa-tool knows has.
    pub not_in_noto: Vec<char>,
    /// How many of those are CJK ideographs of planes 2 and 3.
    pub unknown_ideographs: usize,
    /// Code points no font has: not assigned in Unicode, private use, or
    /// controls.
    pub no_character: Vec<char>,
}

/// Code points no font draws (Unicode's general categories Cn, Co, Cs
/// and Cc), as sorted inclusive ranges; `cargo xtask unicode` makes the
/// table.
fn no_character() -> &'static [(u32, u32)] {
    static TABLE: std::sync::OnceLock<Vec<(u32, u32)>> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        include_str!("../data/no-character.txt")
            .lines()
            .filter(|l| !l.starts_with('#'))
            .filter_map(|l| {
                let (a, b) = l.split_once('-').unwrap_or((l, l));
                Some((
                    u32::from_str_radix(a, 16).ok()?,
                    u32::from_str_radix(b, 16).ok()?,
                ))
            })
            .collect()
    })
}

/// Sorts `chars`, the characters an archive's FONT lacks, by why: `fonts`
/// are the Noto fonts, `rare` the fonts for rare characters.
pub fn lacking(chars: &[char], fonts: &[CatalogFont], rare: &[CatalogFont]) -> Lacking {
    let none = no_character();
    let mut out = Lacking::default();
    for &c in chars {
        let ideograph = usize::from((0x20000..0x40000).contains(&(c as u32)));
        // A font's private-use glyphs are its own, not the text's.
        let i = none.partition_point(|&(_, hi)| hi < c as u32);
        if none.get(i).is_some_and(|&(lo, _)| lo <= c as u32) {
            out.no_character.push(c);
        } else if source(c, fonts).is_some() {
            out.in_noto.push(c);
        } else if let Some(f) = rare.iter().find(|f| f.covers(c)) {
            out.in_rare.push(c);
            out.rare_bytes += u64::from(f.regular.glyph_bytes);
            out.rare_ideographs += ideograph;
        } else {
            out.unknown_ideographs += ideograph;
            out.not_in_noto.push(c);
        }
    }
    out
}

/// What building the fallback found.
#[derive(Debug, Clone, Default)]
pub struct Report {
    /// Glyphs in FONT, and its size.
    pub glyphs: usize,
    pub bytes: usize,
    /// The fonts glyphs came from.
    pub fonts: Vec<String>,
    /// Characters of the text no font had a glyph for.
    pub missing: usize,
    /// Those characters.
    pub lacking: Vec<char>,
    /// Of those, how many the fonts for rare characters have, and about
    /// how many bytes they would add, when they were not looked in.
    pub rare: (usize, u64),
    /// FONT_BOLD and the like: glyphs, size, and the code points asked
    /// for that their family has no face for, which the reader
    /// synthesises.
    pub styled: Vec<StyledReport>,
    /// Fonts that could not be had, and why.
    pub failed: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
pub struct StyledReport {
    pub style: FontStyle,
    pub glyphs: usize,
    pub bytes: usize,
    pub synthesised: usize,
}

/// Collects a glyph's outline in the FONT encoding, scaled to [`UPEM`].
struct Pen {
    scale: f32,
    ops: Vec<GlyphOp>,
}

impl Pen {
    fn at(&self, v: f32) -> i16 {
        (v * self.scale)
            .round()
            .clamp(i16::MIN as f32, i16::MAX as f32) as i16
    }
}

impl OutlineBuilder for Pen {
    fn move_to(&mut self, x: f32, y: f32) {
        let op = GlyphOp::Move(self.at(x), self.at(y));
        self.ops.push(op);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let op = GlyphOp::Line(self.at(x), self.at(y));
        self.ops.push(op);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let op = GlyphOp::Quad(self.at(x1), self.at(y1), self.at(x), self.at(y));
        self.ops.push(op);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let op = GlyphOp::Cubic(
            self.at(x1),
            self.at(y1),
            self.at(x2),
            self.at(y2),
            self.at(x),
            self.at(y),
        );
        self.ops.push(op);
    }
    fn close(&mut self) {
        self.ops.push(GlyphOp::Close);
    }
}

/// A character's glyph from `face`, scaled to [`UPEM`].
fn glyph(face: &Face, c: char) -> Option<Glyph> {
    let id = face.glyph_index(c).filter(|g| g.0 != 0)?;
    let scale = UPEM as f32 / face.units_per_em() as f32;
    let mut pen = Pen {
        scale,
        ops: Vec::new(),
    };
    let bbox = face.outline_glyph(id, &mut pen);
    let outline = match bbox {
        Some(b) => Outline {
            bbox: [
                pen.at(b.x_min as f32),
                pen.at(b.y_min as f32),
                pen.at(b.x_max as f32),
                pen.at(b.y_max as f32),
            ],
            ops: pen.ops,
        },
        // Nothing to draw, as for a space.
        None => Outline::default(),
    };
    let advance = face.glyph_hor_advance(id).unwrap_or(0) as f32 * scale;
    Some(Glyph {
        advance: advance.round().clamp(0.0, u16::MAX as f32) as u16,
        outline,
    })
}

/// The font in `fonts` a character's regular glyph is first looked for
/// in: a CJK font for a character in a CJK block, then the first that has
/// it.
fn source(c: char, fonts: &[CatalogFont]) -> Option<usize> {
    is_cjk(c)
        .then(|| fonts.iter().position(|f| is_cjk_font(f) && f.covers(c)))
        .flatten()
        .or_else(|| fonts.iter().position(|f| f.covers(c)))
}

/// The font files [`build`] would load for `wanted` and `styled`, when
/// every one can be had.
pub fn fonts_needed(
    wanted: &[char],
    styled: &[(FontStyle, Vec<char>)],
    fonts: &[CatalogFont],
) -> usize {
    let mut used = std::collections::BTreeSet::new();
    for &c in wanted {
        if let Some(i) = source(c, fonts) {
            used.insert((i, FontStyle::Regular));
        }
    }
    for (style, chars) in styled {
        for &c in chars {
            if let Some(i) = source(c, fonts)
                && fonts[i].file(*style).is_some_and(|f| f.covers(c))
            {
                used.insert((i, *style));
            }
        }
    }
    used.len()
}

/// Gives a font file's bytes.
pub type Load<'a> = dyn FnMut(&CatalogFont, &FontFile) -> Result<Vec<u8>, String> + 'a;

/// Font files, each loaded once.
struct Files<'a> {
    load: &'a mut Load<'a>,
    /// By SHA-256; `None` for one that could not be had.
    loaded: BTreeMap<String, Option<Vec<u8>>>,
}

impl Files<'_> {
    fn open(
        &mut self,
        font: &CatalogFont,
        file: &FontFile,
        report: &mut Report,
    ) -> Option<Face<'_>> {
        let load = &mut self.load;
        let entry =
            self.loaded
                .entry(file.sha256.clone())
                .or_insert_with(|| match load(font, file) {
                    Ok(d) if Face::parse(&d, 0).is_ok() => Some(d),
                    Ok(_) => {
                        report
                            .failed
                            .push((font.family.clone(), "not a font".to_string()));
                        None
                    }
                    Err(e) => {
                        report.failed.push((font.family.clone(), e));
                        None
                    }
                });
        entry.as_deref().and_then(|d| Face::parse(d, 0).ok())
    }
}

/// Builds FONT for `wanted` from `fonts`, looking in them in order, and
/// for each of `styled` its section: the glyph of each code point FONT
/// has, from that style of the family its regular glyph came from. `load`
/// gives a font file; when a regular one fails, the characters are looked
/// for in the fonts after it. Gives each section that has a glyph.
pub fn build(
    wanted: &[char],
    text: &CharSet,
    styled: &[(FontStyle, Vec<char>)],
    fonts: &[CatalogFont],
    load: &mut Load,
) -> (Vec<(FontStyle, Vec<u8>)>, Report) {
    let mut report = Report::default();
    let mut files = Files {
        load,
        loaded: BTreeMap::new(),
    };

    // Regular: each character from the first font that has it.
    let mut glyphs: BTreeMap<char, Glyph> = BTreeMap::new();
    // The font each glyph came from.
    let mut came_from: BTreeMap<char, usize> = BTreeMap::new();
    let mut left: Vec<char> = wanted.to_vec();
    // CJK characters look in the CJK fonts first, whatever their place.
    let order = fonts
        .iter()
        .enumerate()
        .filter(|(_, f)| is_cjk_font(f))
        .map(|(i, f)| (i, f, true))
        .chain(fonts.iter().enumerate().map(|(i, f)| (i, f, false)));
    for (i, font, cjk_pass) in order {
        let here: Vec<char> = left
            .iter()
            .copied()
            .filter(|&c| (!cjk_pass || is_cjk(c)) && font.covers(c))
            .collect();
        if here.is_empty() {
            continue;
        }
        let Some(face) = files.open(font, &font.regular, &mut report) else {
            continue;
        };
        let mut found = 0;
        for c in here {
            if let Some(g) = glyph(&face, c) {
                glyphs.insert(c, g);
                came_from.insert(c, i);
                found += 1;
            }
        }
        if found > 0 && !report.fonts.contains(&font.family) {
            report.fonts.push(font.family.clone());
        }
        left.retain(|c| !glyphs.contains_key(c));
        if left.is_empty() {
            break;
        }
    }
    left.retain(|&c| text.contains(c));
    report.missing = left.len();
    report.lacking = left;
    if glyphs.is_empty() {
        return (Vec::new(), report);
    }

    // Other styles: from the same family, or not at all.
    let mut sections = Vec::new();
    let mut styled_fonts = Vec::new();
    for (style, chars) in styled {
        let mut by_font: BTreeMap<usize, Vec<char>> = BTreeMap::new();
        for c in chars {
            if let Some(&i) = came_from.get(c) {
                by_font.entry(i).or_default().push(*c);
            }
        }
        let asked: usize = by_font.values().map(Vec::len).sum();
        let mut styled_glyphs = BTreeMap::new();
        for (i, chars) in by_font {
            let font = &fonts[i];
            let Some(file) = font.file(*style) else {
                continue;
            };
            if !chars.iter().any(|&c| file.covers(c)) {
                continue;
            }
            let Some(face) = files.open(font, file, &mut report) else {
                continue;
            };
            for c in chars {
                if let Some(g) = glyph(&face, c) {
                    styled_glyphs.insert(c, g);
                }
            }
        }
        let synthesised = asked - styled_glyphs.len();
        styled_fonts.push((*style, styled_glyphs, synthesised));
    }

    let font = |glyphs| Font {
        upem: UPEM,
        ascender: ASCENDER,
        descender: DESCENDER,
        line_gap: 0,
        glyphs,
    };
    report.glyphs = glyphs.len();
    match font(glyphs).encode() {
        Ok(bytes) => {
            report.bytes = bytes.len();
            sections.push((FontStyle::Regular, bytes));
        }
        Err(e) => {
            report.failed.push(("FONT".to_string(), e.to_string()));
            return (Vec::new(), report);
        }
    }
    for (style, glyphs, synthesised) in styled_fonts {
        let mut r = StyledReport {
            style,
            glyphs: glyphs.len(),
            bytes: 0,
            synthesised,
        };
        if !glyphs.is_empty() {
            match font(glyphs).encode() {
                Ok(bytes) => {
                    r.bytes = bytes.len();
                    sections.push((style, bytes));
                }
                Err(e) => {
                    let name = style.section_name().to_string();
                    report.failed.push((name, e.to_string()));
                }
            }
        }
        report.styled.push(r);
    }
    (sections, report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalogue_covers_the_scripts_wikipedia_uses() {
        let fonts = search_order(catalogue(), None);
        assert!(fonts.len() > 40);
        assert_eq!(fonts[0].family, "Noto Sans");
        // The font `build` takes a character from.
        let first = |c: char| {
            fonts
                .iter()
                .filter(|f| is_cjk(c) && is_cjk_font(f))
                .chain(&fonts)
                .find(|f| f.covers(c))
                .map(|f| f.family.as_str())
        };
        assert_eq!(first('α'), Some("Noto Sans"));
        assert_eq!(first('ə'), Some("Noto Sans"));
        assert_eq!(first('∑'), Some("Noto Sans Math"));
        assert_eq!(first('→'), Some("Noto Sans Math"));
        assert_eq!(first('、'), Some("Noto Sans SC"));
        assert_eq!(first('ש'), Some("Noto Sans Hebrew"));
        assert_eq!(first('ك'), Some("Noto Sans Arabic"));
        assert_eq!(first('ﻻ'), Some("Noto Sans Arabic"));
        // Google's Noto Sans has Devanagari too, in the same design.
        assert_eq!(first('क'), Some("Noto Sans"));
        assert_eq!(first('ক'), Some("Noto Sans Bengali"));
        assert_eq!(first('中'), Some("Noto Sans SC"));
        assert_eq!(first('한'), Some("Noto Sans KR"));
        let ja = search_order(catalogue(), Some("jpn"));
        assert_eq!(
            ja.iter()
                .find(|f| is_cjk_font(f) && f.covers('中'))
                .unwrap()
                .family,
            "Noto Sans JP"
        );
    }

    #[test]
    fn arabic_letters_bring_their_presentation_forms() {
        let mut text = CharSet::default();
        text.insert_str("Plain text, π and سلام");
        let w = wanted(&text);
        assert!(w.contains(&'π') && w.contains(&'س'));
        assert!(!w.contains(&'P') && !w.contains(&' '));
        // Seen, initial, medial and final; Lam-Alef.
        for form in ['\u{FEB1}', '\u{FEB3}', '\u{FEB4}', '\u{FEB2}', '\u{FEFB}'] {
            assert!(w.contains(&form), "U+{:04X}", form as u32);
        }
        // Not the forms of letters the text does not use.
        assert!(!w.contains(&'\u{FE8F}'));
    }

    #[test]
    fn builds_from_the_fonts_it_can_have() {
        // KaTeX's fonts stand in for Noto: Greek from Main, ∑ from Size1,
        // which has no bold, the rest missing.
        let katex = |name: &str| {
            katex_native::FONTS
                .iter()
                .find(|f| f.postscript_name() == name)
                .expect(name)
                .data
        };
        let (main, main_bold, size1) = (
            katex("KaTeX_Main-Regular"),
            katex("KaTeX_Main-Bold"),
            katex("KaTeX_Size1-Regular"),
        );
        let file = |url: &str, covers: &str| FontFile {
            url: url.to_string(),
            sha256: url.to_string(),
            size: 0,
            coverage: covers.chars().map(|c| (c as u32, c as u32)).collect(),
            glyph_bytes: 0,
        };
        let stand_in = |family: &str, covers: &str, bold: bool| CatalogFont {
            family: family.to_string(),
            regular: file(family, covers),
            styled: [
                bold.then(|| file(&format!("{family} bold"), covers)),
                None,
                None,
            ],
        };
        let fonts = [
            stand_in("Broken", "Γ", true),
            stand_in("Size1", "∑", false),
            stand_in("Main", "ΓΔ∞", true),
        ];
        let mut text = CharSet::default();
        text.insert_str("ΓΔ∞∑ж中");
        let w = wanted(&text);
        let mut bold = CharSet::default();
        bold.insert_str("Γ∑ж中");
        let styled = [(FontStyle::Bold, wanted(&bold))];
        assert_eq!(fonts_needed(&w, &styled, &fonts), 4);
        let (sections, report) = build(
            &w,
            &text,
            &styled,
            &fonts,
            &mut |_, f| match f.url.as_str() {
                "Main" => Ok(main.to_vec()),
                "Main bold" => Ok(main_bold.to_vec()),
                "Size1" => Ok(size1.to_vec()),
                _ => Err("offline".to_string()),
            },
        );
        assert_eq!(sections.len(), 2);
        let font = Font::decode(&sections[0].1).unwrap();
        assert_eq!(sections[0].0, FontStyle::Regular);
        assert!(font.glyphs.contains_key(&'Γ') && font.glyphs.contains_key(&'∞'));
        assert_eq!(report.fonts, ["Size1", "Main"]);
        assert_eq!(report.failed.len(), 1);
        // 中 has no font; ж is the firmware's own.
        assert_eq!(report.missing, 1);
        let scaled = |face: &[u8], c: char| {
            let face = Face::parse(face, 0).unwrap();
            let id = face.glyph_index(c).unwrap();
            let scale = 1000.0 / face.units_per_em() as f32;
            (face.glyph_hor_advance(id).unwrap() as f32 * scale).round() as u16
        };
        let gamma = &font.glyphs[&'Γ'];
        assert_eq!(gamma.advance, scaled(main, 'Γ'));
        assert!(!gamma.outline.ops.is_empty());

        // Bold Γ from Main's bold; Size1 has no bold, so ∑ is synthesised.
        assert_eq!(sections[1].0, FontStyle::Bold);
        let bold = Font::decode(&sections[1].1).unwrap();
        assert_eq!(bold.glyphs.keys().collect::<String>(), "Γ");
        assert_eq!(bold.glyphs[&'Γ'].advance, scaled(main_bold, 'Γ'));
        assert_ne!(bold.glyphs[&'Γ'], *gamma);
        assert_eq!(report.styled.len(), 1);
        assert_eq!(
            (report.styled[0].glyphs, report.styled[0].synthesised),
            (1, 1)
        );
    }

    #[test]
    fn lacking_characters_are_sorted_by_why() {
        let fonts = search_order(catalogue(), None);
        // α is in Noto Sans; U+11FC0 is a Tamil fraction only Noto Sans
        // Tamil Supplement has, and U+10D40 a Garay letter (Unicode 16)
        // only Unifont Upper has; U+13460 is an Egyptian hieroglyph of
        // Extended-A, which no font xwa-tool knows has; U+0378 is
        // unassigned, and U+E000 private use (whatever a font may put
        // there).
        let chars = [
            'α',
            '\u{11FC0}',
            '\u{10D40}',
            '\u{13460}',
            '\u{378}',
            '\u{E000}',
        ];
        let l = lacking(&chars, &fonts, &rare_catalogue());
        assert_eq!(l.in_noto, ['α']);
        assert_eq!(l.in_rare, ['\u{11FC0}', '\u{10D40}']);
        assert!(
            l.rare_bytes > 100 && l.rare_bytes < 3000,
            "{}",
            l.rare_bytes
        );
        assert_eq!(l.not_in_noto, ['\u{13460}']);
        assert_eq!(l.no_character, ['\u{378}', '\u{E000}']);
        assert_eq!(no_character().len(), 735, "the whole table is read");
        // Without the fonts for rare characters, no font has them.
        let l = lacking(&chars, &fonts, &[]);
        assert_eq!(l.not_in_noto, ['\u{11FC0}', '\u{10D40}', '\u{13460}']);
        assert!(l.in_rare.is_empty() && l.rare_bytes == 0);
    }

    #[test]
    fn rare_characters_come_after_every_noto_font() {
        let rare = rare_catalogue();
        let families: Vec<&str> = rare.iter().map(|f| f.family.as_str()).collect();
        assert!(
            families.contains(&"Noto Sans Tamil Supplement"),
            "{families:?}"
        );
        assert!(
            families.ends_with(&["Unifont", "Unifont Upper"]),
            "{families:?}"
        );
        assert!(rare.iter().all(|f| f.regular.glyph_bytes > 0));
        let all = fonts(None, true);
        let noto = fonts(None, false);
        assert_eq!(all.len(), noto.len() + rare.len());
        assert_eq!(
            all[..noto.len()]
                .iter()
                .map(|f| &f.family)
                .collect::<Vec<_>>(),
            noto.iter().map(|f| &f.family).collect::<Vec<_>>()
        );
        // What Noto has still comes from Noto.
        assert_eq!(all[source('α', &all).unwrap()].family, "Noto Sans");
        assert_eq!(all[source('中', &all).unwrap()].family, "Noto Sans SC");
        assert_eq!(
            all[source('\u{11FC0}', &all).unwrap()].family,
            "Noto Sans Tamil Supplement"
        );
    }

    #[test]
    fn the_rare_summary_counts_what_noto_lacks() {
        let r = rare_summary();
        println!("{r:?}");
        // Thousands of rare CJK ideographs, and other characters too.
        assert!(
            r.ideographs > 5_000 && r.characters > r.ideographs + 1_000,
            "{r:?}"
        );
        assert!(r.ideographs < r.characters);
        assert!(r.bytes > r.characters as u64 * 100, "{r:?}");
        let files: u64 = rare_catalogue().iter().map(|f| f.regular.size).sum();
        assert_eq!(r.download, files);
    }

    #[test]
    fn the_licence_names_the_sources_used() {
        let names = |f: &[&str]| f.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            license(&names(&["Noto Sans", "Noto Sans SC"])),
            "Fallback glyphs from the Noto fonts (Google Fonts), SIL Open Font License 1.1"
        );
        assert_eq!(
            license(&names(&["Noto Sans", "Plangothic P1", "Unifont Upper"])),
            "Fallback glyphs from the Noto fonts (Google Fonts), Plangothic and GNU Unifont, \
             SIL Open Font License 1.1"
        );
        assert_eq!(
            license(&names(&["Unifont"])),
            "Fallback glyphs from the Noto fonts (Google Fonts) and GNU Unifont, \
             SIL Open Font License 1.1"
        );
    }

    #[test]
    fn missing_counts_only_what_a_font_could_give() {
        let fonts = search_order(catalogue(), None);
        let mut all = CharSet::default();
        // α is in Noto Sans (with a bold and an italic); U+0378 is
        // unassigned, so no font has it.
        all.insert_str("α\u{378}");
        let mut bold = CharSet::default();
        bold.insert('α');
        let none = CharSet::default();
        let chars = FontChars::new(true, [&all, &bold, &none, &none]);
        let (wanted, styled) = requests(&chars);
        assert_eq!(wanted, ['\u{378}', 'α']);
        assert_eq!(
            styled.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
            FontStyle::ALL[1..]
        );
        let nothing = |_: FontStyle, _: char| false;
        assert_eq!(missing(&chars, &fonts, &nothing), (1, 1));
        let regular_only = |s: FontStyle, _: char| s == FontStyle::Regular;
        assert_eq!(missing(&chars, &fonts, &regular_only), (0, 1));
        let everything = |_: FontStyle, _: char| true;
        assert_eq!(missing(&chars, &fonts, &everything), (0, 0));
        // Without real italic, italic text asks for nothing.
        let mut italic = CharSet::default();
        italic.insert('α');
        let upright = FontChars::new(false, [&all, &none, &italic, &none]);
        assert_eq!(missing(&upright, &fonts, &regular_only), (0, 0));
    }

    #[test]
    fn the_catalogue_has_the_styles_google_fonts_has() {
        let fonts = catalogue();
        let sans = &fonts[0];
        assert_eq!(sans.family, "Noto Sans");
        assert!(FontStyle::ALL.iter().all(|&s| sans.file(s).is_some()));
        let has = |family: &str, style| {
            fonts
                .iter()
                .find(|f| f.family == family)
                .unwrap()
                .file(style)
                .is_some()
        };
        assert!(has("Noto Sans SC", FontStyle::Bold));
        assert!(has("Noto Sans Arabic", FontStyle::Bold));
        assert!(!has("Noto Sans Arabic", FontStyle::Italic));
        assert!(!has("Noto Sans Math", FontStyle::Bold));
        assert_eq!(all_files().len(), fonts.len() + 63 + 2);
        // Only Noto Sans has an italic.
        let italic = fonts
            .iter()
            .filter(|f| f.file(FontStyle::Italic).is_some())
            .count();
        assert_eq!(italic, 1);
    }
}
