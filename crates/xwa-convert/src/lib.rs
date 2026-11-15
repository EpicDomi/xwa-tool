//! ZIM to XWA conversion.
//!
//! One pass over the ZIM: article ids follow the ZIM's own storage order,
//! so each cluster is decompressed exactly once, by one worker thread.
//! Links are resolved through a table of path hashes built from the
//! directory before any article is read.

pub mod html;
pub mod images;
pub mod links;
pub mod math;
pub mod mathml;

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use rayon::prelude::*;
use xwa_format::Profile;
use xwa_format::content::{Article, Formula, MathItem, Ref};
use xwa_format::writer::{ArchiveWriter, Summary, WriterOptions};
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
}

impl Options {
    pub fn new(device: Device) -> Self {
        Self {
            device,
            threads: 0,
            images: true,
            jpeg_quality: 75,
            math: true,
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
    /// Converting pictures; `done` and `total` count images.
    Images,
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
    pub zim_bytes: u64,
    pub elapsed: Duration,
}

struct ArticleEntry {
    entry: u32,
    path: String,
    title: String,
    cluster: u32,
    blob: u32,
}

struct Redirect {
    entry: u32,
    title: String,
}

/// Where an image's bytes are in the ZIM.
#[derive(Clone, Copy)]
struct ImageEntry {
    cluster: u32,
    blob: u32,
}

/// Link and image lookups for the extractor.
struct Tables {
    links: PathTable,
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

    fn image(&self, path: &str) -> Option<u32> {
        self.images.as_ref()?.get(path)
    }

    fn formula(&self, tex: &str, display: bool) -> Option<Formula> {
        self.math.as_ref()?.layout(tex, display)
    }
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
    let mut redirects: Vec<Redirect> = Vec::new();
    let mut paths: Vec<(u64, u32)> = Vec::new();
    let mut leads_to = vec![NONE; entry_count as usize];
    let mut image_paths: Vec<(u64, u32)> = Vec::new();
    let mut image_entries: Vec<ImageEntry> = Vec::new();
    let mut scanned = 0u64;
    let mut cancelled = false;
    let scan = zim.for_each_entry(|e| {
        scanned += 1;
        if scanned.is_multiple_of(65_536)
            && !progress(Progress {
                stage: Stage::Scanning,
                done: scanned,
                total: entry_count as u64,
            })
        {
            cancelled = true;
            return Err(xwa_zim::Error::Corrupt("cancelled".into()));
        }
        // Content lives in "C" (new ZIMs), "A" and "I" (old ones).
        if !matches!(e.namespace, b'C' | b'A' | b'I') {
            return Ok(());
        }
        if let Target::Blob { cluster, blob } = e.target
            && opts.images
            && zim.mime_type(&e).is_some_and(is_raster)
        {
            image_paths.push((links::path_hash(&e.path), image_entries.len() as u32));
            image_entries.push(ImageEntry { cluster, blob });
            return Ok(());
        }
        // mwoffliner's own files (styles, scripts, assets) start with "_".
        if e.namespace == b'I' || e.path.starts_with('_') {
            return Ok(());
        }
        paths.push((links::path_hash(&e.path), e.index));
        match e.target {
            Target::Redirect(to) => {
                leads_to[e.index as usize] = REDIRECT | to;
                redirects.push(Redirect {
                    entry: e.index,
                    title: e.display_title().to_string(),
                });
            }
            Target::Blob { cluster, blob }
                if zim
                    .mime_type(&e)
                    .is_some_and(|m| m.starts_with("text/html")) =>
            {
                let title = e.display_title().to_string();
                candidates.push(ArticleEntry {
                    entry: e.index,
                    path: e.path,
                    title,
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
    let mut check = |stage, done, total| {
        if progress(Progress { stage, done, total }) {
            Ok(())
        } else {
            Err(Error::Cancelled)
        }
    };
    candidates.sort_by_key(|a| (a.cluster, a.blob));
    let by_path = PathTable::from_hashed(paths);

    // mwoffliner writes redirects to a section as tiny HTML pages; find them
    // so they become redirects rather than articles.
    let refresh: Vec<Option<String>> = pool.install(|| {
        cluster_groups(&candidates)
            .par_iter()
            .flat_map_iter(|range| {
                let cluster = zim.cluster(candidates[range.start].cluster).ok();
                range
                    .clone()
                    .map(|i| {
                        let html = cluster.as_ref()?.blob(candidates[i].blob as usize)?;
                        let url = links::refresh_target(html)?;
                        links::resolve_href(&candidates[i].path, &url).map(|h| h.path)
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    });
    let mut articles = Vec::with_capacity(candidates.len());
    for (a, target) in candidates.into_iter().zip(refresh) {
        match target {
            Some(path) => {
                if let Some(to) = by_path.get(&path) {
                    leads_to[a.entry as usize] = REDIRECT | to;
                }
                redirects.push(Redirect {
                    entry: a.entry,
                    title: a.title,
                });
            }
            None => articles.push(a),
        }
    }
    if articles.is_empty() {
        return Err(Error::NoArticles);
    }
    for (id, a) in articles.iter().enumerate() {
        leads_to[a.entry as usize] = id as u32;
    }
    let follow = |mut entry: u32| -> Option<u32> {
        for _ in 0..16 {
            match *leads_to.get(entry as usize)? {
                NONE => return None,
                v if v & REDIRECT != 0 => entry = v & !REDIRECT,
                id => return Some(id),
            }
        }
        None
    };
    let mut dangling_redirects = 0u64;
    let resolved_redirects: Vec<(String, u32)> = redirects
        .into_iter()
        .filter_map(|r| match follow(r.entry) {
            Some(id) => Some((r.title, id)),
            None => {
                dangling_redirects += 1;
                None
            }
        })
        .collect();
    // Links resolve through every path that leads to an article.
    let table = Tables {
        links: PathTable::from_hashed(
            by_path
                .into_entries()
                .into_iter()
                .filter_map(|(hash, entry)| follow(entry).map(|id| (hash, id)))
                .collect(),
        ),
        images: opts.images.then(|| PathTable::from_hashed(image_paths)),
        math: opts.math.then(math::MathEngine::new),
    };
    let main_article = zim.header().main_page.and_then(follow);
    drop(leads_to);

    let profile = opts.device.profile();
    let mut writer = ArchiveWriter::create(
        out_path,
        WriterOptions {
            threads,
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

    // Articles, a batch of clusters at a time, written in id order.
    let total = articles.len() as u64;
    let mut stats = Stats::default();
    let mut failed_articles = 0u64;
    let mut incoming = vec![0u32; articles.len()];
    // Image ids in order of first use; each image is stored once.
    let mut image_ids: HashMap<u32, u32> = HashMap::new();
    let mut image_uses: Vec<html::ImageUse> = Vec::new();
    // Math glyphs, also numbered in order of first use.
    let mut glyph_ids: HashMap<u32, u32> = HashMap::new();
    let mut glyph_order: Vec<u32> = Vec::new();
    let groups = cluster_groups(&articles);
    check(Stage::Articles, 0, total)?;
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
                            let x = html.map(|h| {
                                html::extract(
                                    &String::from_utf8_lossy(h),
                                    &a.title,
                                    &a.path,
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
                            *image_ids.entry(u.key).or_insert_with(|| {
                                image_uses.push(*u);
                                image_uses.len() as u32 - 1
                            })
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
                        if let Ref::Link(target) = r
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
                    let title = articles[id as usize].title.clone();
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
        check(Stage::Articles, writer.article_count() as u64, total)?;
    }

    // Pictures, a batch of ZIM clusters at a time, in the ZIM's order.
    let total = image_uses.len() as u64;
    let settings = images::Settings {
        jpeg_quality: opts.jpeg_quality,
        ..images::Settings::new(profile)
    };
    let mut failed_images = 0u64;
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
    check(Stage::Images, 0, total)?;
    while !groups.is_empty() {
        let take = groups.len().min(threads * 2);
        let batch: Vec<&[(ImageEntry, u32)]> = groups.drain(..take).collect();
        let converted: Vec<Vec<(u32, Option<images::Encoded>)>> = pool.install(|| {
            batch
                .par_iter()
                .map(|group| {
                    let cluster = zim.cluster(group[0].0.cluster).ok();
                    group
                        .iter()
                        .map(|(entry, id)| {
                            let bytes = cluster.as_ref().and_then(|c| c.blob(entry.blob as usize));
                            let kind = image_uses[*id as usize].kind;
                            (
                                *id,
                                bytes.and_then(|b| images::convert(b, kind, &settings).ok()),
                            )
                        })
                        .collect()
                })
                .collect()
        });
        for (id, encoded) in converted.into_iter().flatten() {
            let e = encoded.unwrap_or_else(|| {
                failed_images += 1;
                images::placeholder(profile)
            });
            writer.add_image(id, e.info, e.data)?;
            done += 1;
        }
        check(Stage::Images, done, total)?;
    }

    check(Stage::Finishing, 0, 1)?;
    if let Some(engine) = &table.math
        && !glyph_order.is_empty()
    {
        let glyphs = xwa_format::math_glyphs::encode(&engine.glyph_table(&glyph_order))?;
        writer.set_math_glyphs(glyphs);
        writer.set_meta(
            "license.math_font",
            "Formula glyphs from the KaTeX fonts, SIL Open Font License 1.1",
        );
    }
    for (title, id) in &resolved_redirects {
        writer.add_redirect(title, *id);
    }
    for (id, n) in incoming.iter().enumerate() {
        // Log scale: 1 link -> 13, 1,000 -> 130, 500,000+ -> 255.
        let popularity = ((1.0 + *n as f64).log2() * 13.0).round().min(255.0) as u8;
        writer.set_popularity(id as u32, popularity)?;
    }
    let (_, summary) = writer.finish()?;
    check(Stage::Finishing, 1, 1)?;
    Ok(Report {
        summary,
        stats,
        failed_articles,
        failed_images,
        dangling_redirects,
        zim_bytes,
        elapsed: started.elapsed(),
    })
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
