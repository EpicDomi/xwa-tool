use std::collections::{BTreeMap, HashMap};
use std::error::Error;
use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};
use xwa_convert::{Device, Stage};
use xwa_format::normalize::search_key;
use xwa_format::reader::{self, Archive as Xwa};
use xwa_format::records::image_encoding;
use xwa_format::title_index::{self, TitleEntry};
use xwa_format::{la2, optional, required};
use xwa_zim::{Archive, Compression, Target};

type CliResult = Result<(), Box<dyn Error>>;

#[derive(Parser)]
#[command(
    name = "xwa-tool",
    version,
    about = "Fetch and convert ZIM archives to XWA format for Xteink devices"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Convert a ZIM file into an XWA archive.
    Convert {
        /// The ZIM file.
        zim: PathBuf,
        /// Where to write the archive (default: next to the ZIM, ending in .xwa).
        #[arg(long, short)]
        out: Option<PathBuf>,
        /// The e-reader the archive is for.
        #[arg(long, value_enum, default_value_t = DeviceArg::X4Pro)]
        device: DeviceArg,
        /// Worker threads (default: one per CPU).
        #[arg(long, default_value_t = 0)]
        threads: usize,
        /// Leave pictures out (a much smaller archive).
        #[arg(long)]
        no_images: bool,
        /// JPEG quality for photos, 1-100.
        #[arg(long, default_value_t = 75, value_parser = clap::value_parser!(u8).range(1..=100))]
        jpeg_quality: u8,
        /// Write formulas as text instead of drawing them.
        #[arg(long)]
        math_as_text: bool,
    },
    /// List the ZIM files in the Kiwix library.
    Catalog {
        /// Language, as an ISO 639-3 code (eng, fra, deu...).
        #[arg(long, default_value = "eng")]
        lang: String,
        /// Category; "all" for every category.
        #[arg(long, default_value = "wikipedia")]
        category: String,
        /// Only books whose name or title contains this.
        #[arg(long)]
        search: Option<String>,
        /// List the languages instead.
        #[arg(long)]
        languages: bool,
    },
    /// Download a ZIM file from the Kiwix library (resumes, checks its checksum).
    Download {
        /// The book's name, as `catalog` lists it (e.g. wikipedia_en_ray-charles).
        name: String,
        /// maxi (complete, with pictures), nopic or mini.
        #[arg(long, default_value = "maxi")]
        flavour: String,
        /// Folder to download into.
        #[arg(long, default_value = ".")]
        dir: PathBuf,
    },
    /// Copy an archive to an SD card (or another folder).
    Copy {
        /// The XWA file.
        file: PathBuf,
        /// The SD card, or a folder standing for it (default: the one removable
        /// drive, if there is exactly one). The archive goes in its `wiki` folder,
        /// where the e-reader looks for archives.
        #[arg(long)]
        to: Option<PathBuf>,
    },
    /// Describe a ZIM or XWA file.
    Info {
        /// The ZIM or XWA file.
        file: PathBuf,
        /// ZIM only: decompress every cluster to report decompressed sizes.
        #[arg(long)]
        decompress: bool,
    },
    /// Check every checksum, chunk, article, link, image and title in an XWA file.
    Verify {
        /// The XWA file.
        file: PathBuf,
    },
    /// Print an article, search titles, or extract an image from an XWA file.
    Dump(DumpArgs),
    /// Build the title index from a ZIM file and report its size.
    TitleIndex {
        zim: PathBuf,
        /// Run prefix searches against the built index.
        #[arg(long = "query")]
        queries: Vec<String>,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum DeviceArg {
    /// X4 Pro: full profile.
    #[value(name = "x4-pro")]
    X4Pro,
    /// X4: compact profile.
    X4,
    /// X3: compact profile.
    X3,
}

#[derive(Args)]
#[group(skip)]
#[command(group = ArgGroup::new("what").required(true).args(["id", "title", "search", "image"]))]
struct DumpArgs {
    /// The XWA file.
    file: PathBuf,
    /// Print the article with this id.
    #[arg(long)]
    id: Option<u32>,
    /// Print the article with this title (case and accents ignored).
    #[arg(long)]
    title: Option<String>,
    /// List titles starting with this prefix.
    #[arg(long)]
    search: Option<String>,
    /// Extract this image (see --out).
    #[arg(long)]
    image: Option<u32>,
    /// Where --image writes. Line art is converted when the name ends in .pgm.
    #[arg(long, short, requires = "image")]
    out: Option<PathBuf>,
    /// With --id or --title: also write the article's formulas, drawn, to
    /// this HTML page.
    #[arg(long)]
    formulas: Option<PathBuf>,
    /// Maximum results for --search.
    #[arg(long, default_value_t = 20)]
    limit: usize,
}

fn main() -> ExitCode {
    // Stop quietly when the output is piped into something like `head`.
    #[cfg(unix)]
    // SAFETY: restoring the default signal disposition at startup, before any threads exist.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Convert {
            zim,
            out,
            device,
            threads,
            no_images,
            jpeg_quality,
            math_as_text,
        } => {
            let device = match device {
                DeviceArg::X4Pro => Device::X4Pro,
                DeviceArg::X4 => Device::X4,
                DeviceArg::X3 => Device::X3,
            };
            let opts = xwa_convert::Options {
                device,
                threads,
                images: !no_images,
                jpeg_quality,
                math: !math_as_text,
            };
            convert(&zim, out, &opts)
        }
        Command::Catalog {
            lang,
            category,
            search,
            languages,
        } => catalog(&lang, &category, search.as_deref(), languages),
        Command::Download { name, flavour, dir } => download(&name, &flavour, &dir),
        Command::Copy { file, to } => copy(&file, to.as_deref()),
        Command::Info { file, decompress } => info(&file, decompress),
        Command::Verify { file } => verify(&file),
        Command::Dump(args) => dump_cmd(&args),
        Command::TitleIndex { zim, queries } => title_index_cmd(&zim, &queries).map_err(Into::into),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn convert(zim: &Path, out: Option<PathBuf>, opts: &xwa_convert::Options) -> CliResult {
    let device = opts.device;
    let out = out.unwrap_or_else(|| zim.with_extension("xwa"));
    if out == zim {
        return Err("the output would overwrite the ZIM file".into());
    }
    let mut progress = Progress::new();
    let report = xwa_convert::convert(zim, &out, opts, &mut |p| {
        let stage = match p.stage {
            Stage::Scanning => "reading the ZIM directory",
            Stage::Articles => "converting articles",
            Stage::Images => "converting pictures",
            Stage::Finishing => "writing the title index and tables",
        };
        progress.update(stage, p.done, p.total);
        true
    });
    progress.clear();
    let r = report?;
    let s = &r.summary;
    println!(
        "wrote {} for the {}: {} articles, {} redirects, {:.1} MB (ZIM: {:.1} MB)",
        out.display(),
        device.name(),
        s.articles,
        s.redirects,
        mb(s.file_size),
        mb(r.zim_bytes)
    );
    println!(
        "  text: {:.1} MB of article content, {:.1} MB stored in {} chunks{}",
        mb(s.text_raw),
        mb(s.text_stored),
        s.chunks,
        if s.dictionary_bytes > 0 {
            " (zstd with dictionary)"
        } else {
            ""
        }
    );
    println!(
        "  links: {} ({} to pages not in this ZIM, kept as text); tables: {}",
        r.stats.links, r.stats.unresolved_links, r.stats.tables
    );
    println!(
        "  pictures: {} stored ({:.1} MB) for {} places in articles; {} left out",
        s.images,
        mb(s.image_bytes),
        r.stats.images,
        r.stats.images_skipped
    );
    println!(
        "  formulas: {} drawn as math, {} as text",
        r.stats.formulas_drawn,
        r.stats.formulas - r.stats.formulas_drawn
    );
    if r.pages_left_out > 0 {
        println!(
            "  left out {} pages that are not articles (categories and the like)",
            r.pages_left_out
        );
    }
    if r.failed_articles > 0 || r.failed_images > 0 || r.dangling_redirects > 0 {
        println!(
            "  unreadable: {} articles (kept as titles), {} pictures (left blank); redirects to missing pages: {}",
            r.failed_articles, r.failed_images, r.dangling_redirects
        );
    }
    println!("  took {:.1}s", r.elapsed.as_secs_f64());
    Ok(())
}

fn catalog(lang: &str, category: &str, search: Option<&str>, languages: bool) -> CliResult {
    let client = xwa_fetch::Client::new();
    if languages {
        for l in client.languages()? {
            println!("{:<5} {:<30} {:>5} books", l.code, l.name, l.books);
        }
        return Ok(());
    }
    let filter = xwa_fetch::Filter {
        lang: Some(lang).filter(|l| !l.is_empty() && *l != "all"),
        category: Some(category).filter(|c| !c.is_empty() && *c != "all"),
        name: None,
    };
    let needle = search.map(str::to_lowercase);
    let books = client.books(&filter)?;
    let mut shown = 0;
    for b in &books {
        if let Some(n) = &needle
            && !b.name.to_lowercase().contains(n)
            && !b.title.to_lowercase().contains(n)
        {
            continue;
        }
        shown += 1;
        println!(
            "{:<45} {:<6} {}  {:>9}  {:>10} articles  {}",
            b.name,
            b.flavour,
            b.date,
            xwa_fetch::size_text(b.size),
            b.article_count,
            b.title
        );
    }
    println!("{shown} books");
    Ok(())
}

/// Throttled progress for downloads and copies.
fn transfer_progress(progress: &mut Progress, p: xwa_fetch::Progress) {
    let stage = match p.stage {
        xwa_fetch::Stage::Downloading => "transferring",
        xwa_fetch::Stage::Checking => "checking",
    };
    progress.update(stage, p.done, p.total);
}

fn download(name: &str, flavour: &str, dir: &Path) -> CliResult {
    let client = xwa_fetch::Client::new();
    let books = client.books(&xwa_fetch::Filter {
        name: Some(name),
        ..Default::default()
    })?;
    let book = books.iter().find(|b| b.flavour == flavour).ok_or_else(|| {
        let flavours: Vec<&str> = books.iter().map(|b| b.flavour.as_str()).collect();
        if flavours.is_empty() {
            format!("the Kiwix library has no book named {name}; see `xwa-tool catalog`")
        } else {
            format!("{name} comes in these flavours: {}", flavours.join(", "))
        }
    })?;
    println!(
        "{} ({}, {}): {}",
        book.file_name(),
        book.flavour_description(),
        book.date,
        xwa_fetch::size_text(book.size)
    );
    std::fs::create_dir_all(dir)?;
    let mut progress = Progress::new();
    let result = client.download(&book.url, dir, &mut |p| {
        transfer_progress(&mut progress, p);
        true
    });
    progress.clear();
    println!("saved {}", result?.display());
    Ok(())
}

fn copy(file: &Path, to: Option<&Path>) -> CliResult {
    let drives = xwa_fetch::drives::removable();
    let (card, drive) = match to {
        Some(dir) => (
            dir.to_path_buf(),
            drives.iter().find(|d| dir.starts_with(&d.mount)),
        ),
        None => match drives.as_slice() {
            [one] => (one.mount.clone(), Some(one)),
            [] => return Err("no SD card found; insert one or pass --to with a folder".into()),
            many => {
                for d in many {
                    println!("{}", d.label());
                }
                return Err("several removable drives found; pass --to with the one to use".into());
            }
        },
    };
    let dir = xwa_fetch::drives::archive_dir(&card);
    let mut progress = Progress::new();
    let result = xwa_fetch::drives::copy(file, &dir, drive, &mut |p| {
        transfer_progress(&mut progress, p);
        true
    });
    progress.clear();
    println!("copied to {}", result?.display());
    Ok(())
}

/// Picks the reader by the file's signature.
fn info(path: &Path, decompress: bool) -> CliResult {
    let mut magic = [0u8; 8];
    let n = std::fs::File::open(path)?.read(&mut magic)?;
    if magic[..n].starts_with(&xwa_format::MAGIC) {
        xwa_info(path)
    } else if magic[..n].starts_with(&[0x5A, 0x49, 0x4D, 0x04]) {
        Ok(inspect(path, decompress)?)
    } else {
        Err(format!("{} is neither a ZIM nor an XWA file", path.display()).into())
    }
}

fn feature_names(bits: u32, names: &[(u32, &str)]) -> String {
    let mut out: Vec<String> = names
        .iter()
        .filter(|(b, _)| bits & b != 0)
        .map(|(_, n)| n.to_string())
        .collect();
    let known = names.iter().fold(0, |acc, (b, _)| acc | b);
    if bits & !known != 0 {
        out.push(format!("unknown {:#x}", bits & !known));
    }
    if out.is_empty() {
        "none".into()
    } else {
        out.join(", ")
    }
}

fn xwa_info(path: &Path) -> CliResult {
    let a = Xwa::open(path)?;
    let sb = a.superblock();
    println!(
        "XWA {}.{}  {:?} profile, {:?} text{}  {:.1} MB",
        sb.version_major,
        sb.version_minor,
        sb.profile,
        sb.text_codec,
        if a.has_dictionary() {
            " with dictionary"
        } else {
            ""
        },
        mb(sb.file_size)
    );
    let req = [
        (required::ZSTD_TEXT, "zstd text"),
        (required::MATH, "math"),
        (required::LINE_ART, "line art"),
        (required::JPEG, "JPEG"),
    ];
    let opt = [
        (optional::FONT, "font"),
        (optional::WORD_INDEX, "word index"),
        (optional::POPULARITY, "popularity"),
        (optional::HD_IMAGES, "HD images"),
    ];
    println!("  requires: {}", feature_names(sb.required_features, &req));
    println!("  optional: {}", feature_names(sb.optional_features, &opt));
    println!(
        "  {} articles, {} chunks, {} images{}",
        a.article_count(),
        a.chunk_count(),
        a.image_count(),
        a.glyph_count()
            .map_or(String::new(), |g| format!(", {g} math glyphs"))
    );
    println!("sections:");
    let mut sections = sb.sections.clone();
    sections.sort_by_key(|s| s.offset);
    for s in sections {
        println!(
            "  {:<13} at {:>12}  {:>12} bytes  crc {:08x}",
            reader::section_name(s.kind),
            s.offset,
            s.length,
            s.crc32c
        );
    }
    println!("metadata:");
    for (k, v) in a.meta() {
        println!("  {k}: {v}");
    }
    Ok(())
}

/// Throttled progress on stderr, only when it is a terminal.
struct Progress {
    enabled: bool,
    last: Instant,
    shown: bool,
}

impl Progress {
    fn new() -> Self {
        Self {
            enabled: std::io::stderr().is_terminal(),
            last: Instant::now(),
            shown: false,
        }
    }

    fn update(&mut self, stage: &str, done: u64, total: u64) {
        if !self.enabled || (self.last.elapsed() < Duration::from_millis(100) && done < total) {
            return;
        }
        self.last = Instant::now();
        let pct = if total == 0 {
            100.0
        } else {
            done as f64 * 100.0 / total as f64
        };
        eprint!("\r\x1b[2K{stage}: {pct:.0}%");
        let _ = std::io::stderr().flush();
        self.shown = true;
    }

    fn clear(&mut self) {
        if self.shown {
            eprint!("\r\x1b[2K");
            self.shown = false;
        }
    }
}

fn verify(path: &Path) -> CliResult {
    let t = Instant::now();
    let a = Xwa::open(path)?;
    let mut progress = Progress::new();
    let report = a.verify(&mut |stage, done, total| progress.update(stage, done, total));
    progress.clear();
    let r = report?;
    println!(
        "{} articles ({:.1} MB of text in {} chunks), {} links, {} formulas",
        r.articles,
        mb(r.text_bytes),
        r.chunks,
        r.links,
        r.formulas
    );
    println!(
        "{} images ({:.1} MB), {} titles ({} redirects); checked in {:.1}s",
        r.images,
        mb(r.image_bytes),
        r.title_entries,
        r.redirects,
        t.elapsed().as_secs_f64()
    );
    if r.ok() {
        println!("OK: no problems found");
        return Ok(());
    }
    for p in &r.problems {
        println!("problem: {p}");
    }
    if r.problem_count > r.problems.len() as u64 {
        println!("... and {} more", r.problem_count - r.problems.len() as u64);
    }
    Err(format!("{} problems found", r.problem_count).into())
}

fn dump_cmd(args: &DumpArgs) -> CliResult {
    let a = Xwa::open(&args.file)?;
    let title_of = |id: u32| a.title(id).ok();
    let id = if let Some(id) = args.id {
        Some(id)
    } else if let Some(title) = &args.title {
        let key = search_key(title);
        let hits = a.search(title, 1000)?;
        let mut exact: Vec<_> = hits
            .iter()
            .filter(|h| search_key(&h.display) == key)
            .collect();
        exact.sort_by_key(|h| h.flags & title_index::FLAG_REDIRECT);
        match exact.first() {
            Some(h) => Some(h.article),
            None => return Err(format!("no article titled {title:?}").into()),
        }
    } else {
        None
    };
    if let Some(id) = id {
        let article = a.article(id)?;
        let rec = a.article_record(id)?;
        println!(
            "article {id}: chunk {} offset {}, {} chunk(s), flags {:#06x}, popularity {}",
            rec.first_chunk, rec.offset_in_chunk, rec.chunk_count, rec.flags, rec.popularity
        );
        print!("{}", xwa_format::text::article_text(&article, &title_of));
        if let Some(page) = &args.formulas {
            let glyphs = a
                .math_glyphs()?
                .ok_or("this archive has no drawn formulas")?;
            let mut html = format!(
                "<!DOCTYPE html><meta charset=utf-8><title>{}</title><body style=\"font: 16px serif\">",
                article.title.replace('&', "&amp;").replace('<', "&lt;")
            );
            let mut n = 0;
            xwa_format::content::walk_refs(&article, &mut |r| {
                if let xwa_format::content::Ref::Formula(f) = r {
                    n += 1;
                    let svg = xwa_format::math_svg::formula_svg(f, &glyphs, 24.0);
                    html.push_str(&format!("<p>{n}. {svg}</p>"));
                }
            });
            std::fs::write(page, html)?;
            println!("wrote {n} formulas to {}", page.display());
        }
        return Ok(());
    }
    if let Some(prefix) = &args.search {
        for h in a.search(prefix, args.limit)? {
            if h.flags & title_index::FLAG_REDIRECT != 0 {
                let target = title_of(h.article).unwrap_or_default();
                println!("{:>9}  {} -> {target}", h.article, h.display);
            } else {
                println!("{:>9}  {}", h.article, h.display);
            }
        }
        return Ok(());
    }
    let image = args.image.expect("clap requires one of the options");
    let (rec, data) = a.image(image)?;
    println!(
        "image {image}: {}x{}, encoding {}, kind {}, flags {:#x}, {} bytes",
        rec.width, rec.height, rec.encoding, rec.kind, rec.flags, rec.size
    );
    if let Some(out) = &args.out {
        let pgm = out
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("pgm"));
        if pgm && rec.encoding == image_encoding::LINE_ART {
            std::fs::write(out, la2::decode(&data)?.to_pgm())?;
        } else {
            std::fs::write(out, &data)?;
        }
        println!("wrote {}", out.display());
    }
    Ok(())
}

fn mb(bytes: u64) -> f64 {
    bytes as f64 / 1e6
}

fn inspect(path: &Path, decompress: bool) -> xwa_zim::Result<()> {
    let zim = Archive::open(path)?;
    let h = zim.header();
    println!(
        "ZIM {}.{}  entries {}  clusters {}",
        h.major_version, h.minor_version, h.entry_count, h.cluster_count
    );
    for name in ["Title", "Language", "Date", "Flavour", "Scraper"] {
        if let Some(v) = zim.metadata(name)? {
            println!("  {name}: {}", String::from_utf8_lossy(&v));
        }
    }
    let mut by_mime: BTreeMap<String, u64> = BTreeMap::new();
    let mut redirects = 0u64;
    zim.for_each_entry(|e| {
        match zim.mime_type(&e) {
            Some(m) => {
                *by_mime
                    .entry(m.split(';').next().unwrap_or(m).to_string())
                    .or_default() += 1
            }
            None => redirects += 1,
        }
        Ok(())
    })?;
    println!("entries by type:");
    let mut sorted: Vec<_> = by_mime.into_iter().collect();
    sorted.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    for (mime, n) in sorted {
        println!("  {n:>9}  {mime}");
    }
    println!("  {redirects:>9}  (redirects)");
    let mut stats: HashMap<&str, (u64, u64)> = HashMap::new();
    let mut windows: BTreeMap<u64, u64> = BTreeMap::new();
    let (mut dec_total, mut dec_max, mut dec_n) = (0u64, 0u64, 0u64);
    let t = Instant::now();
    for c in 0..zim.cluster_count() {
        let info = zim.cluster_info(c)?;
        let name = match info.compression {
            Compression::None => "uncompressed",
            Compression::Zstd => "zstd",
            Compression::Xz => "xz",
        };
        let s = stats.entry(name).or_default();
        s.0 += 1;
        s.1 += info.stored_size;
        if let Some(w) = info.zstd_window {
            *windows.entry(w).or_default() += 1;
        }
        if decompress && info.compression != Compression::None {
            let len = zim.cluster(c)?.decompressed_len() as u64;
            dec_total += len;
            dec_max = dec_max.max(len);
            dec_n += 1;
        }
    }
    println!("clusters:");
    for (name, (n, bytes)) in &stats {
        println!(
            "  {name:>12}: {n:>6} clusters, {:>10.1} MB stored",
            mb(*bytes)
        );
    }
    for (w, n) in &windows {
        println!(
            "  zstd frames declaring a {:.0} KB window: {n}",
            *w as f64 / 1024.0
        );
    }
    if decompress && dec_n > 0 {
        println!(
            "  compressed clusters decompress to {:.2} MB on average (max {:.2} MB); {:.0} MB/s here including disk reads",
            mb(dec_total) / dec_n as f64,
            mb(dec_max),
            mb(dec_total) / t.elapsed().as_secs_f64()
        );
    }
    Ok(())
}

fn title_index_cmd(path: &Path, queries: &[String]) -> xwa_zim::Result<()> {
    let zim = Archive::open(path)?;
    let t = Instant::now();
    // Articles get ids in path order; redirects point at them.
    let mut article_ids: HashMap<u32, u32> = HashMap::new();
    let mut articles: Vec<(u32, String)> = Vec::new();
    let mut redirects: Vec<(String, u32)> = Vec::new();
    zim.for_each_entry(|e| {
        let content_ns = e.namespace == b'C' || e.namespace == b'A';
        if !content_ns || e.path.starts_with('_') {
            return Ok(());
        }
        match e.target {
            Target::Redirect(to) => redirects.push((e.display_title().to_string(), to)),
            Target::Blob { .. }
                if zim
                    .mime_type(&e)
                    .is_some_and(|m| m.starts_with("text/html")) =>
            {
                article_ids.insert(e.index, articles.len() as u32);
                articles.push((e.index, e.display_title().to_string()));
            }
            Target::Blob { .. } => {}
        }
        Ok(())
    })?;
    let mut entries: Vec<TitleEntry> = articles
        .iter()
        .enumerate()
        .map(|(id, (_, t))| TitleEntry::new(t.clone(), id as u32, false))
        .collect();
    let (mut dropped, mut dangling) = (0usize, 0usize);
    for (title, to) in &redirects {
        // Follow redirect chains to an article.
        let mut target = *to;
        let mut id = None;
        for _ in 0..8 {
            if let Some(&a) = article_ids.get(&target) {
                id = Some(a);
                break;
            }
            match zim.entry(target)?.target {
                Target::Redirect(next) => target = next,
                Target::Blob { .. } => break,
            }
        }
        let Some(id) = id else {
            dangling += 1;
            continue;
        };
        if search_key(title) == search_key(&articles[id as usize].1) {
            dropped += 1;
            continue;
        }
        entries.push(TitleEntry::new(title.clone(), id, true));
    }
    let idx = title_index::build(entries);
    let n = idx.entry_count;
    let bytes = idx.leaves.len() + idx.inner.len();
    println!(
        "articles {}, redirects {} ({} dropped as case/accent duplicates, {} dangling)",
        articles.len(),
        redirects.len(),
        dropped,
        dangling
    );
    println!(
        "index: {n} entries in {} leaf + {} inner blocks ({} levels) = {:.2} MB, {:.1} bytes/entry, built in {:.1}s",
        idx.leaf_blocks(),
        idx.inner_blocks(),
        idx.levels,
        mb(bytes as u64),
        idx.leaves.len() as f64 / n.max(1) as f64,
        t.elapsed().as_secs_f64()
    );
    let reader = idx.reader();
    for q in queries {
        match reader.prefix(q, 8) {
            Ok(hits) => {
                println!("query {q:?}:");
                for h in hits {
                    let kind = if h.flags & title_index::FLAG_REDIRECT != 0 {
                        " (redirect)"
                    } else {
                        ""
                    };
                    println!(
                        "  {} -> {}{kind}",
                        h.display, articles[h.article as usize].1
                    );
                }
            }
            Err(e) => println!("query {q:?} failed: {e:?}"),
        }
    }
    Ok(())
}
