//! Reading an archive in the app, as the e-reader shows it: its articles
//! laid out in pages on the device's screen, with links to follow,
//! pictures, formulas, trees and the characters the archive's own font
//! draws.

pub mod draw;
pub mod layout;

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};

use iced::widget::image;
use xwa_format::content::{Article, Place};
use xwa_format::font::{Font, FontStyle};
use xwa_format::math_glyphs::GlyphTable;
use xwa_format::reader::Archive;
use xwa_format::title_index::Hit;

/// A picture ready to draw.
#[derive(Clone)]
pub struct Picture {
    pub handle: image::Handle,
}

/// An archive opened for reading.
pub struct Wiki {
    pub path: PathBuf,
    archive: Archive<File>,
    pub title: String,
    pub articles: u32,
    pub glyphs: Option<GlyphTable>,
    /// FONT, FONT_BOLD, FONT_ITALIC and FONT_BOLD_ITALIC, where present.
    fonts: [Option<Font>; 4],
    pictures: RefCell<HashMap<u32, Option<Picture>>>,
}

impl std::fmt::Debug for Wiki {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Wiki({})", self.path.display())
    }
}

fn style_index(style: FontStyle) -> usize {
    match style {
        FontStyle::Regular => 0,
        FontStyle::Bold => 1,
        FontStyle::Italic => 2,
        FontStyle::BoldItalic => 3,
    }
}

impl Wiki {
    pub fn open(path: &Path) -> Result<Wiki, String> {
        let archive = Archive::open(path).map_err(|e| e.to_string())?;
        let title = archive
            .meta_value("title")
            .map(str::to_string)
            .unwrap_or_else(|| crate::screens::file_name(path));
        let glyphs = archive.math_glyphs().map_err(|e| e.to_string())?;
        let mut fonts: [Option<Font>; 4] = Default::default();
        for style in [
            FontStyle::Regular,
            FontStyle::Bold,
            FontStyle::Italic,
            FontStyle::BoldItalic,
        ] {
            fonts[style_index(style)] = archive.styled_font(style).map_err(|e| e.to_string())?;
        }
        Ok(Wiki {
            path: path.to_path_buf(),
            articles: archive.article_count(),
            archive,
            title,
            glyphs,
            fonts,
            pictures: RefCell::new(HashMap::new()),
        })
    }

    pub fn article(&self, id: u32) -> Result<Article, String> {
        self.archive.article(id).map_err(|e| e.to_string())
    }

    pub fn article_title(&self, id: u32) -> String {
        self.archive.title(id).unwrap_or_default()
    }

    /// Titles starting with `text`, the most popular first, as the
    /// e-reader's search lists them.
    pub fn search(&self, text: &str, limit: usize) -> Vec<Hit> {
        if text.trim().is_empty() {
            return Vec::new();
        }
        self.archive.search_popular(text, limit).unwrap_or_default()
    }

    /// The article a link or search result leads to, and the section in it
    /// (0 for the top).
    pub fn resolve(&self, article: u32, place: Place) -> u32 {
        match place {
            Place::Section(s) => s,
            Place::Anchor(hash) => self
                .archive
                .find_place(article, hash)
                .ok()
                .flatten()
                .unwrap_or(0),
        }
    }

    pub fn main_article(&self) -> Option<u32> {
        self.archive
            .meta_value("main_article")
            .and_then(|v| v.parse().ok())
            .filter(|&id| id < self.articles)
    }

    /// Today's "On this day" article, where the archive has them.
    pub fn today(&self, month: u32, day: u32) -> Option<u32> {
        let days = self.archive.days().ok().flatten()?;
        // January 1 to December 31, with February 29.
        const BEFORE: [u32; 12] = [0, 31, 60, 91, 121, 152, 182, 213, 244, 274, 305, 335];
        let i = BEFORE.get(month.checked_sub(1)? as usize)? + day.checked_sub(1)?;
        days.get(i as usize).copied().flatten()
    }

    /// An article picked at random (not a disambiguation page or a stub
    /// without text, where it can tell).
    pub fn random(&self, seed: u64) -> Option<u32> {
        if self.articles == 0 {
            return None;
        }
        let mut x = seed | 1;
        for _ in 0..16 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let id = (x % self.articles as u64) as u32;
            let flags = self
                .archive
                .article_record(id)
                .map(|r| r.flags)
                .unwrap_or(0);
            if flags & xwa_format::records::article_flags::DISAMBIGUATION == 0 {
                return Some(id);
            }
        }
        Some((x % self.articles as u64) as u32)
    }

    /// The archive's font for `style`, for characters the e-reader's own
    /// fonts lack; a bold, italic or bold italic glyph it lacks comes from
    /// the regular one, as the e-reader slants or thickens it.
    pub fn fallback_glyph(
        &self,
        c: char,
        style: FontStyle,
    ) -> Option<(&Font, &xwa_format::font::Glyph)> {
        let styled = self.fonts[style_index(style)].as_ref();
        if let Some(f) = styled
            && let Some(g) = f.glyphs.get(&c)
        {
            return Some((f, g));
        }
        let f = self.fonts[0].as_ref()?;
        f.glyphs.get(&c).map(|g| (f, g))
    }

    /// A picture, decoded once: photos as stored, line art in the panel's
    /// greys.
    pub fn picture(&self, id: u32) -> Option<Picture> {
        if let Some(p) = self.pictures.borrow().get(&id) {
            return p.clone();
        }
        let p = self.decode_picture(id);
        self.pictures.borrow_mut().insert(id, p.clone());
        p
    }

    /// A picture's size without decoding it.
    pub fn picture_size(&self, id: u32) -> Option<(u32, u32)> {
        let r = self.archive.image_record(id).ok()?;
        Some((r.width.max(1) as u32, r.height.max(1) as u32))
    }

    fn decode_picture(&self, id: u32) -> Option<Picture> {
        let (rec, data) = self.archive.image(id).ok()?;
        let (w, h, luma) = match rec.encoding {
            xwa_format::records::image_encoding::LINE_ART => {
                let img = xwa_format::la2::decode(&data).ok()?;
                let max = img.header.max_level().max(1) as u32;
                let luma: Vec<u8> = img
                    .levels
                    .iter()
                    .map(|&l| (255 - l as u32 * 255 / max) as u8)
                    .collect();
                (img.header.width as u32, img.header.height as u32, luma)
            }
            _ => {
                let img = ::image::load_from_memory(&data).ok()?.to_luma8();
                (img.width(), img.height(), img.into_raw())
            }
        };
        // Paper where the picture is white, so it sits on the page.
        let paper = crate::eink::PAPER;
        let ink = crate::eink::INK;
        let mix = |a: f32, b: f32, t: f32| ((a + (b - a) * t) * 255.0).round() as u8;
        let mut rgba = Vec::with_capacity(luma.len() * 4);
        for &l in &luma {
            let t = l as f32 / 255.0;
            rgba.extend_from_slice(&[
                mix(ink.r, paper.r, t),
                mix(ink.g, paper.g, t),
                mix(ink.b, paper.b, t),
                255,
            ]);
        }
        Some(Picture {
            handle: image::Handle::from_rgba(w, h, rgba),
        })
    }
}
