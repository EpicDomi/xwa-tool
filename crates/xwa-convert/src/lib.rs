//! ZIM to XWA conversion.
//!
//! One pass over the ZIM: article ids follow the ZIM's own storage order,
//! so each cluster is decompressed exactly once, by one worker thread.
//! Links are resolved through a table of path hashes built from the
//! directory before any article is read.

mod days;
pub mod fallback;
pub mod html;
pub mod images;
pub mod links;
pub mod math;
pub mod mathml;

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rayon::prelude::*;
use xwa_fetch::wikimedia::{Diagrams, FetchReport, Wanted};
use xwa_format::Profile;
use xwa_format::content::{Article, Formula, MathItem, Ref, anchor_hash};
use xwa_format::font::{FontChars, FontStyle};
use xwa_format::writer::{ArchiveWriter, Summary, WriterOptions, image_flags};
use xwa_zim::Target;

use crate::html::{Extracted, Resolver, Stats};
use crate::links::PathTable;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Zim(#[from] xwa_zim::Error),
    #[error("{0}")]
    Xwa(#[from] xwa_format::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Fetch(#[from] xwa_fetch::Error),
    #[error("the ZIM file has no articles")]
    NoArticles,
    #[error("cancelled")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, Error>;

/// The e-reader an archive is made for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Device {
    X4Pro,
    X4,
    X3,
}

impl Device {
    pub fn profile(self) -> Profile {
        match self {
            Device::X4Pro => Profile::Full,
            Device::X4 | Device::X3 => Profile::Compact,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Device::X4Pro => "Xteink X4 Pro",
            Device::X4 => "Xteink X4",
            Device::X3 => "Xteink X3",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Options {
    pub device: Device,
    /// Worker threads; 0 means one per CPU.
    pub threads: usize,
    /// Include pictures.
    pub images: bool,
    /// JPEG quality for photos, 1-100.
    pub jpeg_quality: u8,
    /// Draw formulas as math; otherwise write them as text.
    pub math: bool,
    /// Where to get sharper copies of diagrams than the ZIM's pictures
    /// (Wikimedia, over the internet), if anywhere.
    pub diagrams: Option<Arc<Diagrams>>,
    /// Where to get the Noto fonts the fallback FONT is drawn from (spec
    /// 10); without it the archive has no FONT.
    pub fonts: Option<Arc<xwa_fetch::fonts::Fonts>>,
    /// With the fallback font, real italic glyphs too (FONT_ITALIC and
    /// FONT_BOLD_ITALIC, spec 10.1); without, the e-reader slants the
    /// regular ones.
    pub real_italic: bool,
}

impl Options {
    pub fn new(device: Device) -> Self {
        Self {
            device,
            threads: 0,
            images: true,
            jpeg_quality: 75,
            math: true,
            diagrams: None,
            fonts: None,
            real_italic: true,
        }
    }
}

/// Where the conversion is, for progress displays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Reading the ZIM's directory.
    Scanning,
    /// Converting articles; `done` and `total` count articles.
    Articles,
    /// Downloading sharper diagrams; `done` and `total` count diagrams.
    Diagrams,
    /// Converting pictures; `done` and `total` count images.
    Images,
    /// Getting the fonts the fallback FONT is drawn from; `done` and
    /// `total` count fonts.
    Fonts,
    /// Building the title index and writing the tables.
    Finishing,
}

#[derive(Debug, Clone, Copy)]
pub struct Progress {
    pub stage: Stage,
    pub done: u64,
    pub total: u64,
}

/// What a conversion produced.
#[derive(Debug, Clone)]
pub struct Report {
    pub summary: Summary,
    pub stats: Stats,
    /// Articles whose content could not be read; kept with their title only.
    pub failed_articles: u64,
    /// Images that could not be decoded; stored as blank placeholders.
    pub failed_images: u64,
    /// Redirects that lead nowhere in the archive.
    pub dangling_redirects: u64,
    /// Pages outside Wikipedia's article namespace, such as category pages.
    pub pages_left_out: u64,
    /// What fetching sharper diagrams did, when asked to.
    pub diagrams: Option<FetchReport>,
    /// Pictures stored from a sharper copy.
    pub sharper_images: u64,
    /// What building the fallback FONT found, when it was asked for.
    pub fallback_font: Option<fallback::Report>,
    /// False for a ZIM made without pictures, converted without them.
    pub zim_has_pictures: bool,
    pub zim_bytes: u64,
    pub elapsed: Duration,
}

/// An HTML page in the ZIM. Its path and title stay in the ZIM's directory
/// until they are needed, which keeps memory to 12 bytes a page.
struct ArticleEntry {
    entry: u32,
    cluster: u32,
    blob: u32,
}

/// What an HTML page in the ZIM is.
#[derive(Clone, Copy)]
enum PageKind {
    Article,
    /// An HTML redirect, to this ZIM entry (`NONE` if it leads nowhere).
    Redirect(u32),
    /// A MediaWiki page outside the article namespace (a category, say).
    OtherNamespace,
}

/// Where an image's bytes are in the ZIM, and its directory entry.
#[derive(Clone, Copy)]
struct ImageEntry {
    cluster: u32,
    blob: u32,
    entry: u32,
}

/// Link and image lookups for the extractor.
struct Tables {
    links: PathTable,
    /// The anchor hash of the place each redirect to a section leads to, by
    /// path.
    link_anchors: PathTable,
    /// Image path to an index into the image entries; `None` when pictures
    /// are off.
    images: Option<PathTable>,
    /// `None` when formulas are written as text.
    math: Option<math::MathEngine>,
}

impl Resolver for Tables {
    fn article(&self, path: &str) -> Option<u32> {
        self.links.get(path)
    }

    fn anchor(&self, path: &str) -> Option<u32> {
        self.link_anchors.get(path)
    }

    fn image(&self, path: &str) -> Option<u32> {
        self.images.as_ref()?.get(path)
    }

    fn formula(&self, tex: &str, display: bool) -> Option<Formula> {
        self.math.as_ref()?.layout(tex, display)
    }
}

/// What a ZIM holds, by the kind of cluster: compressed clusters hold its
/// pages, uncompressed ones its pictures and other media. Its search
/// indexes (Xapian), which are not converted, are left out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZimShares {
    pub text_bytes: u64,
    pub media_bytes: u64,
}

/// Measures [`ZimShares`] from the clusters found at 2,048 places spread
/// evenly through the file (so each cluster counts by its size): a few
/// seconds even from a slow card.
pub fn zim_shares(zim_path: &Path) -> Result<ZimShares> {
    const SAMPLES: u64 = 2048;
    let zim = xwa_zim::Archive::open(zim_path)?;
    let starts = zim.cluster_positions();
    let (Some(&first), Some(_)) = (starts.first(), starts.last()) else {
        return Ok(ZimShares {
            text_bytes: 0,
            media_bytes: 0,
        });
    };
    let last = zim.cluster_count() - 1;
    let end = first + zim.cluster_info(last).map_or(0, |i| i.stored_size + 1);
    let end = end.max(starts[last as usize] + 1);
    let span = end - first;
    let mut compressed = 0u64;
    let mut known = 0u64;
    let mut cache: HashMap<usize, bool> = HashMap::new();
    for k in 0..SAMPLES {
        let at = first + span * (2 * k + 1) / (2 * SAMPLES);
        let c = starts.partition_point(|&s| s <= at).saturating_sub(1);
        let is_text = match cache.get(&c) {
            Some(&t) => t,
            None => match zim.cluster_info(c as u32) {
                Ok(info) => {
                    let t = info.compression != xwa_zim::Compression::None;
                    cache.insert(c, t);
                    t
                }
                Err(_) => continue,
            },
        };
        known += 1;
        compressed += is_text as u64;
    }
    let text = span * compressed / known.max(1);
    // The full-text and title search indexes are large, uncompressed and
    // not converted.
    let mut index = 0u64;
    for path in ["fulltext/xapian", "title/xapian"] {
        if let Ok(Some(e)) = zim.find(b'X', path)
            && let xwa_zim::Target::Blob { cluster, .. } = e.target
            && let Ok(info) = zim.cluster_info(cluster)
            && info.compression == xwa_zim::Compression::None
        {
            index += info.stored_size;
        }
    }
    Ok(ZimShares {
        text_bytes: text,
        media_bytes: (span - text).saturating_sub(index),
    })
}

/// The largest the archive of a ZIM for `device` is likely to be, from
/// [`ZimShares`]. On the test ZIMs, an archive's text (with its titles and
/// tables) came to 26-49% of the ZIM's compressed pages for the X4 Pro,
/// and 32-59% with the X4's and X3's weaker compression, and its pictures
/// to 12-96% of the ZIM's media (English Wikipedia's 72 GB of pictures came
/// from about 75 GB). So this counts 55% or 65% of the pages, all the
/// media, and 64 MB for fonts and tables.
pub fn archive_size_estimate(shares: &ZimShares, images: bool, device: Device) -> u64 {
    const MARGIN: u64 = 64 << 20;
    let text_share = match device.profile() {
        Profile::Full => 0.55,
        Profile::Compact => 0.65,
    };
    let pictures = if images { shares.media_bytes } else { 0 };
    (shares.text_bytes as f64 * text_share) as u64 + pictures + MARGIN
}

/// Raster formats the image pipeline decodes.
fn is_raster(mime: &str) -> bool {
    matches!(
        mime,
        "image/webp" | "image/png" | "image/jpeg" | "image/gif"
    )
}

const NONE: u32 = u32::MAX;
const REDIRECT: u32 = 1 << 31;
/// A redirect whose target is missing; `follow` finds no such entry.
const NOWHERE: u32 = REDIRECT | (NONE >> 1);

/// Converts the ZIM at `zim_path` into an archive at `out_path`.
///
/// `progress` is called regularly; returning `false` cancels, and the
/// partial output file is removed.
pub fn convert(
    zim_path: &Path,
    out_path: &Path,
    opts: &Options,
    progress: &mut dyn FnMut(Progress) -> bool,
) -> Result<Report> {
    let started = Instant::now();
    let result = convert_inner(zim_path, out_path, opts, progress, started);
    if result.is_err() {
        let _ = std::fs::remove_file(out_path);
    }
    result
}

fn convert_inner(
    zim_path: &Path,
    out_path: &Path,
    opts: &Options,
    progress: &mut dyn FnMut(Progress) -> bool,
    started: Instant,
) -> Result<Report> {
    let zim = xwa_zim::Archive::open(zim_path)?;
    let zim_bytes = std::fs::metadata(zim_path)?.len();
    let entry_count = zim.entry_count();
    // A ZIM without pictures has only a few logos to find.
    let zim_has_pictures = zim.has_pictures();
    let images = opts.images && zim_has_pictures;
    let threads = match opts.threads {
        0 => std::thread::available_parallelism().map_or(1, |n| n.get()),
        n => n,
    };
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .map_err(|e| Error::Io(std::io::Error::other(e)))?;

    // Directory scan: candidate articles, redirects, and every content path.
    let mut candidates: Vec<ArticleEntry> = Vec::new();
    let mut paths: Vec<(u64, u32)> = Vec::new();
    let mut leads_to = vec![NONE; entry_count as usize];
    let mut image_paths: Vec<(u64, u32)> = Vec::new();
    let mut image_entries: Vec<ImageEntry> = Vec::new();
    let mut scanned = 0u64;
    let mut cancelled = false;
    let scan = zim.for_each_entry(|e| {
        scanned += 1;
        if scanned.is_multiple_of(65_536) && !progress(scanning(0, scanned, entry_count as u64)) {
            cancelled = true;
            return Err(xwa_zim::Error::Corrupt("cancelled".into()));
        }
        // Content lives in "C" (new ZIMs), "A" and "I" (old ones).
        if !matches!(e.namespace, b'C' | b'A' | b'I') {
            return Ok(());
        }
        if let Target::Blob { cluster, blob } = e.target
            && images
            && zim.mime_type(&e).is_some_and(is_raster)
        {
            image_paths.push((links::path_hash(&e.path), image_entries.len() as u32));
            image_entries.push(ImageEntry {
                cluster,
                blob,
                entry: e.index,
            });
            return Ok(());
        }
        // mwoffliner's own files (styles, scripts, assets) start with "_".
        if e.namespace == b'I' || e.path.starts_with('_') {
            return Ok(());
        }
        paths.push((links::path_hash(&e.path), e.index));
        match e.target {
            Target::Redirect(to) => leads_to[e.index as usize] = REDIRECT | to,
            Target::Blob { cluster, blob }
                if zim
                    .mime_type(&e)
                    .is_some_and(|m| m.starts_with("text/html")) =>
            {
                candidates.push(ArticleEntry {
                    entry: e.index,
                    cluster,
                    blob,
                });
            }
            Target::Blob { .. } => {}
        }
        Ok(())
    });
    if cancelled {
        return Err(Error::Cancelled);
    }
    scan?;
    let mut check = |p: Progress| {
        if progress(p) {
            Ok(())
        } else {
            Err(Error::Cancelled)
        }
    };
    candidates.sort_by_key(|a| (a.cluster, a.blob));
    let by_path = PathTable::from_hashed(paths);

    // mwoffliner writes redirects to a section as tiny HTML pages, and newer
    // ZIMs also hold category pages: find both, so the first become
    // redirects and the second are left out.
    let page_groups = cluster_groups(&candidates);
    let mut kinds: Vec<PageKind> = Vec::with_capacity(candidates.len());
    // HTML redirects to a section: (entry, anchor hash of the section).
    let mut section_redirects: Vec<(u32, u32)> = Vec::new();
    for (n, batch) in page_groups.chunks(threads * 8).enumerate() {
        let part: Vec<Vec<(PageKind, Option<u32>)>> = pool.install(|| {
            batch
                .par_iter()
                .map(|range| {
                    let cluster = zim.cluster(candidates[range.start].cluster).ok();
                    range
                        .clone()
                        .map(|i| {
                            let Some(html) = cluster
                                .as_ref()
                                .and_then(|c| c.blob(candidates[i].blob as usize))
                            else {
                                return (PageKind::Article, None);
                            };
                            if let Some(url) = links::refresh_target(html) {
                                // The target is relative to the page's own path.
                                let href = zim
                                    .entry(candidates[i].entry)
                                    .ok()
                                    .and_then(|e| links::resolve_href(&e.path, &url));
                                let to = href.as_ref().and_then(|h| by_path.get(&h.path));
                                let anchor = href.and_then(|h| h.fragment).map(|f| anchor_hash(&f));
                                return (PageKind::Redirect(to.unwrap_or(NONE)), anchor);
                            }
                            let kind = match links::page_namespace(html) {
                                Some(ns) if ns != 0 => PageKind::OtherNamespace,
                                _ => PageKind::Article,
                            };
                            (kind, None)
                        })
                        .collect::<Vec<_>>()
                })
                .collect()
        });
        for (kind, anchor) in part.into_iter().flatten() {
            if let Some(hash) = anchor {
                section_redirects.push((candidates[kinds.len()].entry, hash));
            }
            kinds.push(kind);
        }
        let done = ((n + 1) * threads * 8).min(page_groups.len());
        check(scanning(1, done as u64, page_groups.len() as u64))?;
    }
    drop(page_groups);
    // The candidates that are articles stay, in place.
    let mut articles = candidates;
    let mut pages_left_out = 0u64;
    let mut kind = kinds.into_iter();
    articles.retain(|a| match kind.next() {
        Some(PageKind::Redirect(NONE)) => {
            leads_to[a.entry as usize] = NOWHERE;
            false
        }
        Some(PageKind::Redirect(to)) => {
            leads_to[a.entry as usize] = REDIRECT | to;
            false
        }
        Some(PageKind::OtherNamespace) => {
            pages_left_out += 1;
            false
        }
        Some(PageKind::Article) | None => true,
    });
    drop(kind);
    if articles.is_empty() {
        return Err(Error::NoArticles);
    }
    articles.shrink_to_fit();
    for (id, a) in articles.iter().enumerate() {
        leads_to[a.entry as usize] = id as u32;
    }
    section_redirects.sort_unstable();
    // The article an entry leads to, and the place in it when a redirect on
    // the way leads to a section (the one nearest the article counts).
    let follow = |mut entry: u32| -> Option<(u32, Option<u32>)> {
        let mut anchor = None;
        for _ in 0..16 {
            if let Ok(i) = section_redirects.binary_search_by_key(&entry, |r| r.0) {
                anchor = Some(section_redirects[i].1);
            }
            match *leads_to.get(entry as usize)? {
                NONE => return None,
                v if v & REDIRECT != 0 => entry = v & !REDIRECT,
                id => return Some((id, anchor)),
            }
        }
        None
    };
    // Links resolve through every path that leads to an article.
    let mut link_anchors = Vec::new();
    let links = PathTable::from_hashed(
        by_path
            .into_entries()
            .into_iter()
            .filter_map(|(hash, entry)| {
                let (id, anchor) = follow(entry)?;
                if let Some(a) = anchor {
                    link_anchors.push((hash, a));
                }
                Some((hash, id))
            })
            .collect(),
    );
    let table = Tables {
        links,
        link_anchors: PathTable::from_hashed(link_anchors),
        images: images.then(|| PathTable::from_hashed(image_paths)),
        math: opts.math.then(math::MathEngine::new),
    };
    let main_article = zim.header().main_page.and_then(follow).map(|(id, _)| id);
    // "On this day": the article for each day, where a Wikipedia has them.
    let days = wiki_code(&zim)
        .and_then(|code| days::titles(&code))
        .map(|titles| {
            let mut found = [None; xwa_format::DAYS_IN_YEAR];
            for (day, title) in titles.iter().enumerate() {
                if !title.is_empty() {
                    found[day] = table.links.get(&days::path(title));
                }
            }
            found
        })
        .filter(|found| found.iter().any(Option::is_some));

    let profile = opts.device.profile();
    // Temporary files go next to the archive: the system's temporary folder
    // may be small, or held in memory.
    let temp_dir = match out_path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => std::path::PathBuf::from("."),
    };
    let mut writer = ArchiveWriter::create(
        out_path,
        WriterOptions {
            threads,
            temp_dir: Some(temp_dir),
            ..WriterOptions::new(opts.device.profile())
        },
    )?;
    let meta_map = [
        ("Title", "title"),
        ("Description", "description"),
        ("Language", "language"),
        ("Name", "source.name"),
        ("Flavour", "source.flavour"),
        ("Date", "source.date"),
        ("Scraper", "source.scraper"),
        ("License", "license"),
        ("Creator", "attribution"),
    ];
    for (zim_key, key) in meta_map {
        if let Some(v) = zim.metadata(zim_key)? {
            let v = String::from_utf8_lossy(&v).trim().to_string();
            if !v.is_empty() {
                writer.set_meta(key, v);
            }
        }
    }
    let uuid: String = zim
        .header()
        .uuid
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    writer.set_meta("source.uuid", uuid);
    writer.set_meta(
        "converter",
        format!("xwa-tool {}", env!("CARGO_PKG_VERSION")),
    );
    writer.set_meta("device", opts.device.name());
    if let Some(id) = main_article {
        writer.set_meta("main_article", id.to_string());
    }
    if let Some(days) = &days {
        writer.set_days(days);
    }

    // Every article has its id now, so redirects can be followed; their
    // titles come from a second pass over the directory, and the entry
    // table is no longer needed after it.
    let mut dangling_redirects = 0u64;
    let mut scanned = 0u64;
    let mut failure: Option<Error> = None;
    let rescan = zim.for_each_entry(|e| {
        scanned += 1;
        if scanned.is_multiple_of(65_536)
            && let Err(err) = check(scanning(2, scanned, entry_count as u64))
        {
            failure = Some(err);
            return Err(xwa_zim::Error::Corrupt("cancelled".into()));
        }
        let lead = leads_to[e.index as usize];
        if lead == NONE || lead & REDIRECT == 0 {
            return Ok(());
        }
        match follow(e.index) {
            Some((id, anchor)) => {
                if let Err(err) = writer.add_redirect(e.display_title(), id, anchor) {
                    failure = Some(err.into());
                    return Err(xwa_zim::Error::Corrupt("stopped".into()));
                }
            }
            None => dangling_redirects += 1,
        }
        Ok(())
    });
    if let Some(e) = failure {
        return Err(e);
    }
    rescan?;
    drop(leads_to);

    // Articles, a batch of clusters at a time, written in id order.
    let total = articles.len() as u64;
    let mut stats = Stats::default();
    let mut failed_articles = 0u64;
    let mut incoming = vec![0u32; articles.len()];
    // Image ids in order of first use, by image entry; each image is
    // stored once.
    let mut image_ids: Vec<u32> = vec![NONE; image_entries.len()];
    let mut image_uses: Vec<html::ImageUse> = Vec::new();
    // Math glyphs, also numbered in order of first use.
    let mut glyph_ids: HashMap<u32, u32> = HashMap::new();
    let mut glyph_order: Vec<u32> = Vec::new();
    let groups = cluster_groups(&articles);
    check(Progress {
        stage: Stage::Articles,
        done: 0,
        total,
    })?;
    for batch in groups.chunks(threads * 4) {
        let converted: Vec<Vec<(u32, Option<Extracted>)>> = pool.install(|| {
            batch
                .par_iter()
                .map(|range| {
                    let cluster = zim.cluster(articles[range.start].cluster);
                    range
                        .clone()
                        .map(|id| {
                            let a = &articles[id];
                            let html = cluster.as_ref().ok().and_then(|c| c.blob(a.blob as usize));
                            let x = html.zip(zim.entry(a.entry).ok()).map(|(h, e)| {
                                html::extract(
                                    &String::from_utf8_lossy(h),
                                    e.display_title(),
                                    &e.path,
                                    id as u32,
                                    &table,
                                )
                            });
                            (id as u32, x)
                        })
                        .collect()
                })
                .collect()
        });
        for (id, x) in converted.into_iter().flatten() {
            match x {
                Some(mut x) => {
                    let global: Vec<u32> = x
                        .images
                        .iter()
                        .map(|u| {
                            let id = &mut image_ids[u.key as usize];
                            if *id == NONE {
                                image_uses.push(*u);
                                *id = image_uses.len() as u32 - 1;
                            }
                            *id
                        })
                        .collect();
                    html::for_each_image_mut(&mut x.article, &mut |i| *i = global[*i as usize]);
                    html::for_each_formula_mut(&mut x.article, &mut |f| {
                        for item in &mut f.items {
                            if let MathItem::Glyph { glyph, .. } | MathItem::GlyphXY { glyph, .. } =
                                item
                            {
                                *glyph = *glyph_ids.entry(*glyph).or_insert_with(|| {
                                    glyph_order.push(*glyph);
                                    glyph_order.len() as u32 - 1
                                });
                            }
                        }
                    });
                    xwa_format::content::walk_refs(&x.article, &mut |r| {
                        if let Ref::Link(target, _) = r
                            && target != id
                        {
                            incoming[target as usize] = incoming[target as usize].saturating_add(1);
                        }
                    });
                    stats += x.stats;
                    writer.add_article(id, &x.article, x.flags)?;
                }
                None => {
                    failed_articles += 1;
                    let title = zim
                        .entry(articles[id as usize].entry)
                        .map(|e| e.display_title().to_string())
                        .unwrap_or_default();
                    writer.add_article(
                        id,
                        &Article {
                            title,
                            ..Default::default()
                        },
                        0,
                    )?;
                }
            }
        }
        check(Progress {
            stage: Stage::Articles,
            done: writer.article_count() as u64,
            total,
        })?;
    }

    // Only the formula engine is needed from here on.
    let math_engine = table.math;
    drop((
        table.links,
        table.link_anchors,
        table.images,
        articles,
        image_ids,
    ));

    // Sharper diagrams, fetched before the pictures are converted: the
    // image ids of SVG diagrams, and their wiki file names and sizes.
    let mut sharper_ids: Vec<u32> = Vec::new();
    let mut wanted: Vec<Wanted> = Vec::new();
    let mut diagram_report = None;
    if let Some(source) = &opts.diagrams {
        for (id, u) in image_uses.iter().enumerate() {
            if u.kind != images::Kind::Drawing {
                continue;
            }
            let entry = zim.entry(image_entries[u.key as usize].entry);
            if let Some(file) = entry
                .ok()
                .and_then(|e| xwa_fetch::wikimedia::svg_file_name(&e.path))
            {
                let (width, height) = u.source_size;
                sharper_ids.push(id as u32);
                wanted.push(Wanted {
                    file,
                    width,
                    height,
                });
            }
        }
        let wiki = wiki_code(&zim);
        let fetched = source.fetch(&wanted, wiki.as_deref(), &mut |done, total| {
            check(Progress {
                stage: Stage::Diagrams,
                done,
                total,
            })
            .is_ok()
        });
        diagram_report = Some(match fetched {
            Ok(r) => r,
            Err(xwa_fetch::Error::Cancelled) => return Err(Error::Cancelled),
            // Sharper diagrams are a bonus: the ZIM's pictures still do.
            Err(e) => FetchReport {
                wanted: wanted.len() as u64,
                failed: wanted.len() as u64,
                stopped: Some(e.to_string()),
                ..Default::default()
            },
        });
    }

    // Pictures, a batch of ZIM clusters at a time, in the ZIM's order.
    let total = image_uses.len() as u64;
    let settings = images::Settings {
        jpeg_quality: opts.jpeg_quality,
        ..images::Settings::new(profile)
    };
    let mut failed_images = 0u64;
    let mut sharper_images = 0u64;
    writer.reserve_images(total as u32);
    let mut order: Vec<(ImageEntry, u32)> = image_uses
        .iter()
        .enumerate()
        .map(|(id, u)| (image_entries[u.key as usize], id as u32))
        .collect();
    order.sort_by_key(|(e, _)| (e.cluster, e.blob));
    let mut groups: Vec<&[(ImageEntry, u32)]> =
        order.chunk_by(|a, b| a.0.cluster == b.0.cluster).collect();
    // Keep batches small enough to hold in memory: large clusters alone.
    let mut done = 0u64;
    check(Progress {
        stage: Stage::Images,
        done: 0,
        total,
    })?;
    while !groups.is_empty() {
        let take = groups.len().min(threads * 2);
        let batch: Vec<&[(ImageEntry, u32)]> = groups.drain(..take).collect();
        let converted: Vec<Vec<(u32, Option<images::Encoded>, bool)>> = pool.install(|| {
            batch
                .par_iter()
                .map(|group| {
                    let cluster = zim.cluster(group[0].0.cluster).ok();
                    group
                        .iter()
                        .map(|(entry, id)| {
                            let kind = image_uses[*id as usize].kind;
                            // The sharper copy, if one was fetched and reads well.
                            let hd = opts.diagrams.as_ref().and_then(|source| {
                                let i = sharper_ids.binary_search(id).ok()?;
                                let png = source.get(&wanted[i])?;
                                let mut e = images::convert(&png, kind, &settings).ok()?;
                                e.info.flags |= image_flags::HD_SOURCE;
                                Some(e)
                            });
                            if hd.is_some() {
                                return (*id, hd, true);
                            }
                            let bytes = cluster.as_ref().and_then(|c| c.blob(entry.blob as usize));
                            let e = bytes.and_then(|b| images::convert(b, kind, &settings).ok());
                            (*id, e, false)
                        })
                        .collect()
                })
                .collect()
        });
        for (id, encoded, hd) in converted.into_iter().flatten() {
            let e = encoded.unwrap_or_else(|| {
                failed_images += 1;
                images::placeholder(profile)
            });
            sharper_images += hd as u64;
            writer.add_image(id, e.info, e.data)?;
            done += 1;
        }
        check(Progress {
            stage: Stage::Images,
            done,
            total,
        })?;
    }

    // Glyphs for what the firmware's own fonts cannot draw.
    let fallback_font = match &opts.fonts {
        Some(fonts) => {
            let styled = |style| writer.styled_chars(style);
            let [a, b, c, d] = FontStyle::ALL.map(styled);
            let chars = FontChars::new(opts.real_italic, [&a, &b, &c, &d]);
            let language = zim
                .metadata("Language")?
                .map(|v| String::from_utf8_lossy(&v).trim().to_string());
            let (sections, report) = build_fonts(&chars, language.as_deref(), fonts, &mut check)?;
            if !sections.is_empty() {
                writer.set_meta("license.font", fallback::LICENSE);
            }
            for (style, data) in sections {
                writer.set_font(style, data);
            }
            writer.set_font_chars(&chars);
            Some(report)
        }
        None => None,
    };
    check(Progress {
        stage: Stage::Finishing,
        done: 0,
        total: 1,
    })?;
    if let Some(engine) = &math_engine
        && !glyph_order.is_empty()
    {
        let glyphs = xwa_format::math_glyphs::encode(&engine.glyph_table(&glyph_order))?;
        writer.set_math_glyphs(glyphs);
        writer.set_meta(
            "license.math_font",
            "Formula glyphs from the KaTeX fonts, SIL Open Font License 1.1",
        );
    }
    for (id, n) in incoming.iter().enumerate() {
        // Log scale: 1 link -> 13, 1,000 -> 130, 500,000+ -> 255.
        let popularity = ((1.0 + *n as f64).log2() * 13.0).round().min(255.0) as u8;
        writer.set_popularity(id as u32, popularity)?;
    }
    let (_, summary) = writer.finish()?;
    check(Progress {
        stage: Stage::Finishing,
        done: 1,
        total: 1,
    })?;
    Ok(Report {
        summary,
        stats,
        failed_articles,
        failed_images,
        dangling_redirects,
        pages_left_out,
        diagrams: diagram_report,
        sharper_images,
        fallback_font,
        zim_has_pictures,
        zim_bytes,
        elapsed: started.elapsed(),
    })
}

/// The code of the wiki a ZIM was made from ("en" for
/// `wikipedia_en_all`), where diagrams not on Commons are looked for.
fn wiki_code(zim: &xwa_zim::Archive) -> Option<String> {
    let name = zim.metadata("Name").ok()??;
    let name = String::from_utf8_lossy(&name);
    let code = name.strip_prefix("wikipedia_")?.split('_').next()?;
    (!code.is_empty()).then(|| code.to_string())
}

/// Downloads every Noto font file the fallback font could need that
/// `fonts` does not keep yet, for converting offline later. `progress`
/// gets bytes done and in all, and stops it by returning false. Returns
/// how many files were downloaded.
pub fn download_all_fonts(
    fonts: &xwa_fetch::fonts::Fonts,
    progress: &mut dyn FnMut(u64, u64) -> bool,
) -> Result<usize> {
    let missing: Vec<_> = fallback::all_files()
        .into_iter()
        .filter(|f| !fonts.is_cached(&f.sha256))
        .collect();
    let total: u64 = missing.iter().map(|f| f.size).sum();
    let mut done = 0u64;
    for f in &missing {
        if !progress(done, total) {
            return Err(Error::Cancelled);
        }
        fonts.get(&f.url, &f.sha256, f.size)?;
        done += f.size;
    }
    progress(total, total);
    Ok(missing.len())
}

/// Built font sections: each style's bytes.
type FontSections = Vec<(FontStyle, Vec<u8>)>;

/// FONT and the other font sections for `chars` (spec 10), from the Noto
/// fonts in `fonts`, downloading what is not kept. Once the network fails
/// only fonts already kept are used, so offline work does not wait for
/// every font in turn.
fn build_fonts(
    chars: &FontChars,
    language: Option<&str>,
    fonts: &xwa_fetch::fonts::Fonts,
    check: &mut dyn FnMut(Progress) -> Result<()>,
) -> Result<(FontSections, fallback::Report)> {
    let (wanted, styled) = fallback::requests(chars);
    let order = fallback::search_order(fallback::catalogue(), language);
    let total = fallback::fonts_needed(&wanted, &styled, &order) as u64;
    check(Progress {
        stage: Stage::Fonts,
        done: 0,
        total,
    })?;
    let mut done = 0u64;
    let mut stopped = None;
    let mut offline = false;
    let text = chars.set(FontStyle::Regular);
    let built = fallback::build(&wanted, text, &styled, &order, &mut |_, f| {
        if stopped.is_some() {
            return Err("cancelled".to_string());
        }
        if offline && !fonts.is_cached(&f.sha256) {
            return Err("not downloaded: no connection".to_string());
        }
        let data = fonts.get(&f.url, &f.sha256, f.size);
        offline |= matches!(data, Err(xwa_fetch::Error::Network(_)));
        let data = data.map_err(|e| e.to_string());
        done += 1;
        if let Err(e) = check(Progress {
            stage: Stage::Fonts,
            done,
            total: total.max(done),
        }) {
            stopped = Some(e);
        }
        data
    });
    match stopped {
        Some(e) => Err(e),
        None => Ok(built),
    }
}

/// How complete an archive's font sections are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FontStatus {
    /// Code points of the text FONT has no glyph for that a Noto font has.
    pub missing: usize,
    /// Bold, italic and bold italic glyphs missing that the Noto fonts have.
    pub missing_styled: usize,
}

impl FontStatus {
    pub fn complete(&self) -> bool {
        self.missing == 0 && self.missing_styled == 0
    }
}

/// What an archive's font sections lack for `chars` that the Noto fonts
/// could supply.
pub fn font_status(archive: &xwa_format::reader::Archive, chars: &FontChars) -> Result<FontStatus> {
    let fonts = FontStyle::ALL.map(|s| archive.styled_font(s));
    let mut sections = Vec::with_capacity(4);
    for f in fonts {
        sections.push(f?);
    }
    let order = fallback::search_order(fallback::catalogue(), archive.meta_value("language"));
    let has = |style: FontStyle, c: char| {
        sections[style as usize]
            .as_ref()
            .is_some_and(|f| f.glyphs.contains_key(&c))
    };
    let (missing, missing_styled) = fallback::missing(chars, &order, &has);
    Ok(FontStatus {
        missing,
        missing_styled,
    })
}

/// Builds the font sections of the archive at `path` again for `chars` (its
/// CHARS section, or what [`xwa_format::reader::Archive::scan_font_chars`]
/// found), downloading the Noto fonts it needs, and puts them in the
/// archive in place of the old ones, with a CHARS section. Nothing is
/// written unless the new sections hold more glyphs than the old.
pub fn rebuild_font(
    path: &Path,
    chars: &FontChars,
    fonts: &xwa_fetch::fonts::Fonts,
    progress: &mut dyn FnMut(Progress) -> bool,
) -> Result<fallback::Report> {
    let archive = xwa_format::reader::Archive::open(path)?;
    let glyphs = |archive: &xwa_format::reader::Archive| -> Result<usize> {
        let mut n = 0;
        for style in FontStyle::ALL {
            n += archive.styled_font(style)?.map_or(0, |f| f.glyphs.len());
        }
        Ok(n)
    };
    let before = glyphs(&archive)?;
    let language = archive.meta_value("language").map(str::to_string);
    let mut meta = archive.meta().to_vec();
    drop(archive);
    let mut check = |p: Progress| {
        if progress(p) {
            Ok(())
        } else {
            Err(Error::Cancelled)
        }
    };
    let (sections, report) = build_fonts(chars, language.as_deref(), fonts, &mut check)?;
    let after: usize = report.glyphs + report.styled.iter().map(|s| s.glyphs).sum::<usize>();
    if after <= before {
        return Ok(report);
    }
    let mut changes: Vec<(u32, Option<Vec<u8>>)> = FontStyle::ALL
        .iter()
        .map(|&style| {
            let data = sections
                .iter()
                .find(|(s, _)| *s == style)
                .map(|(_, d)| d.clone());
            (style.section(), data)
        })
        .collect();
    changes.push((xwa_format::section::CHARS, Some(chars.encode())));
    if !meta.iter().any(|(k, _)| k == "license.font") {
        meta.push(("license.font".into(), fallback::LICENSE.into()));
        changes.push((
            xwa_format::section::META,
            Some(xwa_format::meta::encode(&meta)),
        ));
    }
    xwa_format::writer::patch_sections(path, &changes)?;
    Ok(report)
}

/// Progress through the scanning stage's three passes (the directory, the
/// pages, the directory again for redirects) as one figure.
fn scanning(pass: u64, done: u64, total: u64) -> Progress {
    const PART: u64 = 1 << 20;
    Progress {
        stage: Stage::Scanning,
        done: pass * PART + done.min(total) * PART / total.max(1),
        total: 3 * PART,
    }
}

/// Index ranges of entries that share a cluster (input sorted by cluster).
fn cluster_groups(entries: &[ArticleEntry]) -> Vec<std::ops::Range<usize>> {
    let mut groups = Vec::new();
    let mut start = 0;
    for i in 1..=entries.len() {
        if i == entries.len() || entries[i].cluster != entries[start].cluster {
            groups.push(start..i);
            start = i;
        }
    }
    groups
}
