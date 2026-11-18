//! The fallback FONT (spec 10): outline glyphs, from the Noto fonts, for
//! the characters an archive's text uses that the firmware's own fonts
//! cannot draw. `cargo xtask fonts` makes the catalogue of fonts.

use std::collections::BTreeMap;

use ttf_parser::{Face, OutlineBuilder};
use unicode_normalization::UnicodeNormalization;
use xwa_format::font::{Font, Glyph, needs_fallback};
use xwa_format::math_glyphs::{GlyphOp, Outline};
use xwa_format::normalize::CharSet;

/// One line per font: family, URL, SHA-256, size, coverage.
static CATALOGUE: &[u8] = include_bytes!("../data/fonts.tsv.zst");

/// What every glyph is scaled to.
const UPEM: u16 = 1000;
/// Line metrics, Noto Sans's (the fallback for Latin, Greek and Cyrillic).
const ASCENDER: i16 = 1069;
const DESCENDER: i16 = -293;

/// META `license.font`, when an archive has FONT.
pub const LICENSE: &str =
    "Fallback glyphs from the Noto fonts (Google Fonts), SIL Open Font License 1.1";

/// A font the fallback can draw glyphs from.
#[derive(Debug, Clone)]
pub struct CatalogFont {
    pub family: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
    /// The code points it has glyphs for, as sorted inclusive ranges.
    pub coverage: Vec<(u32, u32)>,
}

impl CatalogFont {
    pub fn covers(&self, c: char) -> bool {
        let c = c as u32;
        let i = self.coverage.partition_point(|&(_, hi)| hi < c);
        self.coverage.get(i).is_some_and(|&(lo, _)| lo <= c)
    }
}

/// The catalogue, in the order fonts are looked in.
pub fn catalogue() -> Vec<CatalogFont> {
    let table = zstd::decode_all(CATALOGUE).expect("the font catalogue is zstd");
    let table = String::from_utf8(table).expect("the font catalogue is UTF-8");
    table
        .lines()
        .filter_map(|line| {
            let mut f = line.split('\t');
            let (family, url, sha256, size, cover) =
                (f.next()?, f.next()?, f.next()?, f.next()?, f.next()?);
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
            Some(CatalogFont {
                family: family.to_string(),
                url: url.to_string(),
                sha256: sha256.to_string(),
                size: size.parse().ok()?,
                coverage,
            })
        })
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
    /// Fonts that could not be had, and why.
    pub failed: Vec<(String, String)>,
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

/// The fonts in `fonts` that [`build`] would load for `wanted`, when every
/// one can be had.
pub fn fonts_needed(wanted: &[char], fonts: &[CatalogFont]) -> usize {
    let (cjk, rest): (Vec<char>, Vec<char>) = wanted.iter().partition(|&&c| is_cjk(c));
    let mut used = std::collections::BTreeSet::new();
    for c in cjk {
        if let Some(f) = fonts.iter().find(|f| is_cjk_font(f) && f.covers(c)) {
            used.insert(f.family.as_str());
            continue;
        }
        if let Some(f) = fonts.iter().find(|f| f.covers(c)) {
            used.insert(f.family.as_str());
        }
    }
    for c in rest {
        if let Some(f) = fonts.iter().find(|f| f.covers(c)) {
            used.insert(f.family.as_str());
        }
    }
    used.len()
}

/// Builds FONT for `wanted` from `fonts`, looking in them in order. `load`
/// gives a font's file; when it fails, the characters are looked for in
/// the fonts after it. `None` when no glyph was found.
pub fn build(
    wanted: &[char],
    text: &CharSet,
    fonts: &[CatalogFont],
    load: &mut dyn FnMut(&CatalogFont) -> Result<Vec<u8>, String>,
) -> (Option<Vec<u8>>, Report) {
    let mut report = Report::default();
    let mut glyphs: BTreeMap<char, Glyph> = BTreeMap::new();
    let mut left: Vec<char> = wanted.to_vec();
    // CJK characters look in the CJK fonts first, whatever their place.
    let order = fonts
        .iter()
        .filter(|f| is_cjk_font(f))
        .map(|f| (f, true))
        .chain(fonts.iter().map(|f| (f, false)));
    let mut loaded: BTreeMap<String, Option<Vec<u8>>> = BTreeMap::new();
    for (font, cjk_pass) in order {
        let here: Vec<char> = left
            .iter()
            .copied()
            .filter(|&c| (!cjk_pass || is_cjk(c)) && font.covers(c))
            .collect();
        if here.is_empty() {
            continue;
        }
        let entry = loaded
            .entry(font.family.clone())
            .or_insert_with(|| match load(font) {
                Ok(d) => Some(d),
                Err(e) => {
                    report.failed.push((font.family.clone(), e));
                    None
                }
            });
        let Some(data) = entry.as_deref() else {
            continue;
        };
        let Ok(face) = Face::parse(data, 0) else {
            report
                .failed
                .push((font.family.clone(), "not a font".to_string()));
            *entry = None;
            continue;
        };
        let mut found = 0;
        for c in here {
            if let Some(g) = glyph(&face, c) {
                glyphs.insert(c, g);
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
    report.missing = left.iter().filter(|&&c| text.contains(c)).count();
    if glyphs.is_empty() {
        return (None, report);
    }
    let font = Font {
        upem: UPEM,
        ascender: ASCENDER,
        descender: DESCENDER,
        line_gap: 0,
        glyphs,
    };
    report.glyphs = font.glyphs.len();
    match font.encode() {
        Ok(bytes) => {
            report.bytes = bytes.len();
            (Some(bytes), report)
        }
        Err(e) => {
            report.failed.push(("FONT".to_string(), e.to_string()));
            (None, report)
        }
    }
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
        // KaTeX's fonts stand in for Noto: Greek from Main, the rest missing.
        let face_data = katex_native::FONTS
            .iter()
            .find(|f| f.postscript_name() == "KaTeX_Main-Regular")
            .expect("KaTeX Main")
            .data;
        let main = Face::parse(face_data, 0).unwrap();
        let stand_in = |family: &str, covers: &str| CatalogFont {
            family: family.to_string(),
            url: String::new(),
            sha256: String::new(),
            size: 0,
            coverage: covers.chars().map(|c| (c as u32, c as u32)).collect(),
        };
        let fonts = [stand_in("Broken", "Γ"), stand_in("Main", "ΓΔ∞")];
        let mut text = CharSet::default();
        text.insert_str("ΓΔ∞ж中");
        let w = wanted(&text);
        let (bytes, report) = build(&w, &text, &fonts, &mut |f| match f.family.as_str() {
            "Main" => Ok(face_data.to_vec()),
            _ => Err("offline".to_string()),
        });
        let font = Font::decode(&bytes.unwrap()).unwrap();
        assert!(font.glyphs.contains_key(&'Γ') && font.glyphs.contains_key(&'∞'));
        assert_eq!(report.fonts, ["Main"]);
        assert_eq!(report.failed.len(), 1);
        // 中 has no font; ж is the firmware's own.
        assert_eq!(report.missing, 1);
        let gamma = &font.glyphs[&'Γ'];
        let scale = 1000.0 / main.units_per_em() as f32;
        let id = main.glyph_index('Γ').unwrap();
        let advance = (main.glyph_hor_advance(id).unwrap() as f32 * scale).round() as u16;
        assert_eq!(gamma.advance, advance);
        assert!(!gamma.outline.ops.is_empty());
    }
}
