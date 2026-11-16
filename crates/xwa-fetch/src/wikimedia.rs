//! Sharper copies of Wikipedia's diagrams.
//!
//! A ZIM holds each SVG diagram only as a small picture, at the size the
//! article shows it. Wikimedia's thumbnail service renders the original SVG
//! at any of a set of standard widths; [`Diagrams`] fetches those
//! renderings a few at a time, waits whenever Wikimedia asks it to, and
//! keeps what it fetched in a cache folder, so converting again (for
//! another e-reader, say) downloads nothing.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use md5::{Digest, Md5};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};

use crate::{Error, Result};

/// Where Wikimedia keeps uploaded files.
pub const UPLOADS: &str = "https://upload.wikimedia.org";

/// The widths Wikimedia renders thumbnails at. It throttles requests for
/// any other width.
pub const STEPS: [u32; 11] = [20, 40, 60, 120, 250, 330, 500, 960, 1280, 1920, 3840];

/// Wikimedia asks tools to say who they are and where to find out more.
const USER_AGENT: &str = concat!(
    "xwa-tool/",
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("CARGO_PKG_REPOSITORY"),
    ")"
);

/// How MediaWiki writes file names in URLs: everything but letters, digits
/// and `-_.;@$!*(),/~:` is percent-encoded.
const FILE_NAME: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b';')
    .remove(b'@')
    .remove(b'$')
    .remove(b'!')
    .remove(b'*')
    .remove(b'(')
    .remove(b')')
    .remove(b',')
    .remove(b'/')
    .remove(b'~')
    .remove(b':');

/// A diagram wanted: its file name on the wiki (`Pythagorean.svg`) and its
/// size there, which gives its shape.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Wanted {
    pub file: String,
    pub width: u32,
    pub height: u32,
}

/// What [`Diagrams::fetch`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FetchReport {
    /// Diagrams asked for (each file once).
    pub wanted: u64,
    /// In the cache now, fetched this time or before.
    pub ready: u64,
    /// Downloaded this time.
    pub downloaded: u64,
    pub bytes: u64,
    /// Not on Wikimedia under that name.
    pub missing: u64,
    /// Not fetched because of network trouble or a busy server.
    pub failed: u64,
    /// Why fetching stopped early, if it did.
    pub stopped: Option<String>,
}

enum Outcome {
    Cached,
    Downloaded(u64),
    Missing,
    Failed,
    /// Wikimedia refuses to serve this client at all.
    Refused(String),
}

/// After this many diagrams in a row fail, the rest are not tried.
const GIVE_UP_AFTER: u64 = 25;

pub struct Diagrams {
    agent: ureq::Agent,
    base: String,
    cache: PathBuf,
    /// The long side, in pixels, renderings should have at least.
    long_side: u32,
    workers: usize,
    /// No request goes out before this time: set when Wikimedia says to wait.
    pause_until: Mutex<Instant>,
}

impl std::fmt::Debug for Diagrams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Diagrams")
            .field("base", &self.base)
            .field("cache", &self.cache)
            .finish_non_exhaustive()
    }
}

impl Diagrams {
    /// Fetches from Wikimedia into the `cache` folder, which is created
    /// when needed.
    pub fn new(cache: PathBuf) -> Self {
        Self::with_base(UPLOADS, cache)
    }

    /// The same against another server laid out like Wikimedia's.
    pub fn with_base(base: &str, cache: PathBuf) -> Self {
        let tls = ureq::tls::TlsConfig::builder()
            .root_certs(ureq::tls::RootCerts::PlatformVerifier)
            .build();
        let config = ureq::Agent::config_builder()
            .tls_config(tls)
            .user_agent(USER_AGENT)
            // 404s and 429s are answers here, not errors.
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(30)))
            .timeout_global(Some(Duration::from_secs(120)))
            .build();
        Self {
            agent: config.into(),
            base: base.trim_end_matches('/').to_string(),
            cache,
            long_side: 800,
            workers: 4,
            pause_until: Mutex::new(Instant::now()),
        }
    }

    /// The cache folder used when none is given: `xwa-tool/diagrams` in the
    /// system's cache folder.
    pub fn default_cache() -> Option<PathBuf> {
        dirs::cache_dir().map(|d| d.join("xwa-tool").join("diagrams"))
    }

    /// The thumbnail width to ask for: the smallest standard width at which
    /// a diagram this shape is at least the wanted size on its long side.
    pub fn width_for(&self, width: u32, height: u32) -> u32 {
        let (w, h) = (width.max(1) as u64, height.max(1) as u64);
        let long = self.long_side as u64;
        let need = if w >= h { long } else { (long * w).div_ceil(h) };
        STEPS
            .iter()
            .copied()
            .find(|&s| s as u64 >= need)
            .unwrap_or(STEPS[STEPS.len() - 1])
    }

    /// The rendering's URL on `wiki` ("commons", "en").
    fn thumb_url(&self, wiki: &str, file: &str, width: u32) -> String {
        let hash = md5_hex(file);
        let name = utf8_percent_encode(file, FILE_NAME).to_string();
        // Long names give their thumbnails a short stand-in name.
        let thumb_name = if file.len() > 160 {
            "thumbnail.svg".to_string()
        } else {
            name.clone()
        };
        format!(
            "{}/wikipedia/{wiki}/thumb/{}/{}/{name}/{width}px-{thumb_name}.png",
            self.base,
            &hash[..1],
            &hash[..2]
        )
    }

    fn cache_file(&self, file: &str, width: u32, ext: &str) -> PathBuf {
        self.cache.join(format!("{}-{width}.{ext}", md5_hex(file)))
    }

    /// A fetched rendering (PNG), if the cache has one.
    pub fn get(&self, wanted: &Wanted) -> Option<Vec<u8>> {
        let width = self.width_for(wanted.width, wanted.height);
        std::fs::read(self.cache_file(&wanted.file, width, "png")).ok()
    }

    /// Fetches every wanted diagram the cache does not have, looking on
    /// Wikimedia Commons and then on the wiki with code `wiki` ("en"), and
    /// calls `progress(done, total)` as it goes; returning `false` stops it
    /// with [`Error::Cancelled`].
    pub fn fetch(
        &self,
        wanted: &[Wanted],
        wiki: Option<&str>,
        progress: &mut dyn FnMut(u64, u64) -> bool,
    ) -> Result<FetchReport> {
        let mut wikis = vec!["commons".to_string()];
        if let Some(w) = wiki
            .filter(|w| !w.is_empty() && w.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
            && w != "commons"
        {
            wikis.push(w.to_ascii_lowercase());
        }
        let mut todo: Vec<(&str, u32)> = wanted
            .iter()
            .map(|w| (w.file.as_str(), self.width_for(w.width, w.height)))
            .collect();
        todo.sort_unstable();
        todo.dedup();
        let report = FetchReport {
            wanted: todo.len() as u64,
            ..Default::default()
        };
        if todo.is_empty() {
            return Ok(report);
        }
        std::fs::create_dir_all(&self.cache)?;

        let next = AtomicUsize::new(0);
        let done = AtomicU64::new(0);
        let running = AtomicUsize::new(0);
        let stop = AtomicBool::new(false);
        let failed_in_a_row = AtomicU64::new(0);
        let shared = Mutex::new(report);
        let mut cancelled = false;
        std::thread::scope(|scope| {
            let workers = self.workers.min(todo.len());
            running.store(workers, Ordering::SeqCst);
            for _ in 0..workers {
                scope.spawn(|| {
                    while !stop.load(Ordering::Relaxed) {
                        let Some(&(file, width)) = todo.get(next.fetch_add(1, Ordering::Relaxed))
                        else {
                            break;
                        };
                        let outcome = self.fetch_one(file, width, &wikis, &stop);
                        let failed = matches!(outcome, Outcome::Failed);
                        let mut r = shared.lock().unwrap();
                        match outcome {
                            Outcome::Cached => r.ready += 1,
                            Outcome::Downloaded(bytes) => {
                                r.ready += 1;
                                r.downloaded += 1;
                                r.bytes += bytes;
                            }
                            Outcome::Missing => r.missing += 1,
                            Outcome::Failed => r.failed += 1,
                            Outcome::Refused(why) => {
                                r.failed += 1;
                                r.stopped.get_or_insert(why);
                                stop.store(true, Ordering::Relaxed);
                            }
                        }
                        if failed {
                            if failed_in_a_row.fetch_add(1, Ordering::Relaxed) + 1 >= GIVE_UP_AFTER
                            {
                                r.stopped.get_or_insert_with(|| {
                                    "Wikimedia is not answering; the rest keep the ZIM's pictures"
                                        .into()
                                });
                                stop.store(true, Ordering::Relaxed);
                            }
                        } else {
                            failed_in_a_row.store(0, Ordering::Relaxed);
                        }
                        drop(r);
                        done.fetch_add(1, Ordering::Relaxed);
                    }
                    running.fetch_sub(1, Ordering::SeqCst);
                });
            }
            // Progress is reported from here: the callback is not shareable.
            loop {
                let finished = running.load(Ordering::SeqCst) == 0;
                if !cancelled && !progress(done.load(Ordering::Relaxed), todo.len() as u64) {
                    cancelled = true;
                    stop.store(true, Ordering::Relaxed);
                }
                if finished {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        });
        if cancelled {
            return Err(Error::Cancelled);
        }
        let mut report = shared.into_inner().unwrap();
        // Those never tried, when fetching stopped early.
        report.failed = report.wanted - (report.ready + report.missing);
        Ok(report)
    }

    fn fetch_one(&self, file: &str, width: u32, wikis: &[String], stop: &AtomicBool) -> Outcome {
        let png = self.cache_file(file, width, "png");
        let none = self.cache_file(file, width, "none");
        if png.exists() {
            return Outcome::Cached;
        }
        if none.exists() {
            return Outcome::Missing;
        }
        for wiki in wikis {
            let url = self.thumb_url(wiki, file, width);
            let mut attempt = 0u32;
            loop {
                if !self.wait_for_pause(stop) {
                    return Outcome::Failed;
                }
                let wait = match self.agent.get(&url).call() {
                    Ok(mut resp) => match resp.status().as_u16() {
                        200 => {
                            let body = resp.body_mut().with_config().limit(32 << 20).read_to_vec();
                            return match body {
                                Ok(b) if b.starts_with(b"\x89PNG\r\n\x1a\n") => {
                                    match store(&png, &b) {
                                        Ok(()) => Outcome::Downloaded(b.len() as u64),
                                        Err(_) => Outcome::Failed,
                                    }
                                }
                                _ => Outcome::Failed,
                            };
                        }
                        404 | 410 => break,
                        403 => {
                            return Outcome::Refused(
                                "Wikimedia refused the requests (HTTP 403)".into(),
                            );
                        }
                        429 | 503 => retry_after(&resp).unwrap_or_else(|| backoff(attempt)),
                        _ => backoff(attempt),
                    },
                    Err(_) => backoff(attempt),
                };
                attempt += 1;
                if attempt >= 6 {
                    return Outcome::Failed;
                }
                self.pause(wait);
            }
        }
        // Not found under this name anywhere: remember that, too.
        let _ = std::fs::write(&none, b"");
        Outcome::Missing
    }

    /// Makes every request wait `d` from now.
    fn pause(&self, d: Duration) {
        let mut until = self.pause_until.lock().unwrap();
        *until = (*until).max(Instant::now() + d);
    }

    /// Waits until requests may go out again; `false` if stopped meanwhile.
    fn wait_for_pause(&self, stop: &AtomicBool) -> bool {
        loop {
            if stop.load(Ordering::Relaxed) {
                return false;
            }
            let until = *self.pause_until.lock().unwrap();
            let now = Instant::now();
            if until <= now {
                return true;
            }
            std::thread::sleep((until - now).min(Duration::from_millis(250)));
        }
    }
}

/// Writes a cache file under a temporary name first, so a half-written
/// file never looks like a finished one.
fn store(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension(format!("part{}", std::process::id()));
    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(data)?;
    drop(f);
    std::fs::rename(&tmp, path)
}

/// The wait a 429 or 503 asks for, in seconds (capped at five minutes).
fn retry_after(resp: &ureq::http::Response<ureq::Body>) -> Option<Duration> {
    let secs: u64 = resp
        .headers()
        .get("retry-after")?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()?;
    Some(Duration::from_secs(secs.clamp(1, 300)))
}

/// 2, 4, 8 … 64 seconds.
fn backoff(attempt: u32) -> Duration {
    Duration::from_secs(2u64 << attempt.min(5))
}

fn md5_hex(s: &str) -> String {
    Md5::digest(s.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The wiki file name of an SVG diagram from its picture's name in a ZIM
/// (`Pythagorean.svg.png` or `Pythagorean.svg.webp` gives `Pythagorean.svg`),
/// or `None` for pictures that were not drawn from an SVG.
pub fn svg_file_name(zim_path: &str) -> Option<String> {
    let name = zim_path.rsplit('/').next()?;
    let (stem, _) = name.rsplit_once('.')?;
    let is_svg = stem.len() > 4 && stem[stem.len() - 4..].eq_ignore_ascii_case(".svg");
    is_svg.then(|| stem.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;

    #[test]
    fn urls_follow_wikimedia_layout() {
        let d = Diagrams::with_base("https://up.example", PathBuf::from("/tmp/x"));
        // As on Commons: upload.wikimedia.org/wikipedia/commons/d/d2/Pythagorean.svg
        assert_eq!(&md5_hex("Pythagorean.svg")[..2], "d2");
        assert_eq!(
            d.thumb_url("commons", "Pythagorean.svg", 960),
            "https://up.example/wikipedia/commons/thumb/d/d2/Pythagorean.svg/960px-Pythagorean.svg.png"
        );
        assert_eq!(
            d.thumb_url("en", "Caf\u{e9}_(map)_&_more.svg", 500),
            format!(
                "https://up.example/wikipedia/en/thumb/{0}/{1}/Caf%C3%A9_(map)_%26_more.svg/500px-Caf%C3%A9_(map)_%26_more.svg.png",
                &md5_hex("Caf\u{e9}_(map)_&_more.svg")[..1],
                &md5_hex("Caf\u{e9}_(map)_&_more.svg")[..2]
            )
        );
        let long = format!("{}.svg", "x".repeat(170));
        assert!(
            d.thumb_url("commons", &long, 960)
                .ends_with("/960px-thumbnail.svg.png")
        );
    }

    #[test]
    fn widths_are_standard_and_big_enough() {
        let d = Diagrams::with_base("https://up.example", PathBuf::from("/tmp/x"));
        assert_eq!(d.width_for(512, 466), 960); // landscape: 960 wide
        assert_eq!(d.width_for(502, 1292), 330); // portrait: 330 wide is 849 tall
        assert_eq!(d.width_for(100, 1000), 120);
        assert_eq!(d.width_for(0, 0), 960);
    }

    #[test]
    fn names_come_from_svg_pictures_only() {
        assert_eq!(
            svg_file_name("_assets_/d1f3/Pythagorean.svg.png").as_deref(),
            Some("Pythagorean.svg")
        );
        assert_eq!(svg_file_name("I/Map.SVG.webp").as_deref(), Some("Map.SVG"));
        assert_eq!(svg_file_name("_assets_/d1f3/Photo.jpg"), None);
        assert_eq!(svg_file_name("_assets_/d1f3/.svg.png"), None);
    }

    /// A server that answers each request with the next canned response
    /// (status, extra headers, body) and records the paths asked for.
    fn serve(
        responses: Vec<(u16, &'static str, Vec<u8>)>,
    ) -> (String, std::sync::mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for (status, headers, body) in responses {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let path = line.split_whitespace().nth(1).unwrap_or("").to_string();
                let _ = tx.send(path);
                loop {
                    let mut h = String::new();
                    if reader.read_line(&mut h).unwrap() <= 2 {
                        break;
                    }
                }
                let head = format!(
                    "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n",
                    body.len()
                );
                stream.write_all(head.as_bytes()).unwrap();
                stream.write_all(&body).unwrap();
            }
        });
        (format!("http://{addr}"), rx)
    }

    fn png() -> Vec<u8> {
        let mut b = b"\x89PNG\r\n\x1a\n".to_vec();
        b.extend_from_slice(b"not really, but enough for the cache");
        b
    }

    #[test]
    fn fetches_waits_when_asked_falls_back_and_caches() {
        let cache = std::env::temp_dir().join(format!("xwa-diagrams-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&cache);
        // One diagram on Commons after a 429, one only on the local wiki,
        // one nowhere.
        let (base, paths) = serve(vec![
            (429, "Retry-After: 1\r\n", Vec::new()),
            (200, "", png()),
            (404, "", Vec::new()),
            (200, "", png()),
            (404, "", Vec::new()),
            (404, "", Vec::new()),
        ]);
        let mut d = Diagrams::with_base(&base, cache.clone());
        d.workers = 1; // in order, for the canned answers
        let wanted = vec![
            Wanted {
                file: "A.svg".into(),
                width: 400,
                height: 300,
            },
            Wanted {
                file: "B.svg".into(),
                width: 300,
                height: 400,
            },
            Wanted {
                file: "C.svg".into(),
                width: 0,
                height: 0,
            },
            Wanted {
                file: "A.svg".into(),
                width: 400,
                height: 300,
            },
        ];
        let started = Instant::now();
        let mut calls = 0;
        let r = d
            .fetch(&wanted, Some("sv"), &mut |_, _| {
                calls += 1;
                true
            })
            .unwrap();
        assert!(
            started.elapsed() >= Duration::from_secs(1),
            "waited for Retry-After"
        );
        assert_eq!(
            (r.wanted, r.ready, r.downloaded, r.missing, r.failed),
            (3, 2, 2, 1, 0)
        );
        assert!(calls > 0);
        let asked: Vec<String> = paths.try_iter().collect();
        assert_eq!(asked.len(), 6);
        assert!(
            asked[0].starts_with("/wikipedia/commons/thumb/")
                && asked[0].ends_with("/960px-A.svg.png")
        );
        assert_eq!(asked[1], asked[0]);
        // 3:4 needs the 960 step to be 800 tall.
        assert!(
            asked[2].starts_with("/wikipedia/commons/") && asked[2].ends_with("/960px-B.svg.png")
        );
        assert!(
            asked[3].starts_with("/wikipedia/sv/thumb/") && asked[3].ends_with("/960px-B.svg.png")
        );
        assert_eq!(d.get(&wanted[0]), Some(png()));
        assert_eq!(d.get(&wanted[1]), Some(png()));
        assert_eq!(d.get(&wanted[2]), None);
        // Again: everything comes from the cache, nothing is asked.
        let r = d.fetch(&wanted, Some("sv"), &mut |_, _| true).unwrap();
        assert_eq!((r.ready, r.downloaded, r.missing), (2, 0, 1));
        std::fs::remove_dir_all(&cache).unwrap();
    }

    #[test]
    fn a_refusal_stops_everything() {
        let cache = std::env::temp_dir().join(format!("xwa-diagrams-403-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&cache);
        let (base, _) = serve(vec![(403, "", Vec::new())]);
        let mut d = Diagrams::with_base(&base, cache.clone());
        d.workers = 1;
        let wanted: Vec<Wanted> = (0..5)
            .map(|i| Wanted {
                file: format!("D{i}.svg"),
                width: 10,
                height: 10,
            })
            .collect();
        let r = d.fetch(&wanted, None, &mut |_, _| true).unwrap();
        assert_eq!((r.ready, r.failed), (0, 5));
        assert!(r.stopped.unwrap().contains("403"));
        std::fs::remove_dir_all(&cache).unwrap();
    }
}
