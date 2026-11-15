use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use clap::{Parser, Subcommand};
use xwa_format::normalize::search_key;
use xwa_format::title_index::{self, TitleEntry};
use xwa_zim::{Archive, Compression, Target};

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
    /// Print facts about a ZIM file: header, content types, clusters.
    Inspect {
        zim: PathBuf,
        /// Also decompress every cluster to report decompressed sizes.
        #[arg(long)]
        decompress: bool,
    },
    /// Build the archive title index from a ZIM file and report its size.
    TitleIndex {
        zim: PathBuf,
        /// Run prefix searches against the built index.
        #[arg(long = "query")]
        queries: Vec<String>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Inspect { zim, decompress } => inspect(&zim, decompress),
        Command::TitleIndex { zim, queries } => title_index_cmd(&zim, &queries),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn mb(bytes: u64) -> f64 {
    bytes as f64 / 1e6
}

fn inspect(path: &PathBuf, decompress: bool) -> xwa_zim::Result<()> {
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

fn title_index_cmd(path: &PathBuf, queries: &[String]) -> xwa_zim::Result<()> {
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
