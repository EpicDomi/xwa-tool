//! Getting ZIM files from the Kiwix library, and putting archives on SD
//! cards.
//!
//! The catalog comes from the library's OPDS feeds. Downloads go through
//! the Metalink file the library links to, which lists mirrors in priority
//! order and a SHA-256: an interrupted download resumes from its `.part`
//! file, a failing mirror hands over to the next, and the file is checked
//! before it gets its real name.

pub mod catalog;
pub mod drives;
pub mod fonts;
pub mod metalink;
pub mod wikimedia;
mod xml;

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub use catalog::{Book, Language};

/// The Kiwix library.
pub const LIBRARY: &str = "https://library.kiwix.org";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("network error: {0}")]
    Network(String),
    #[error("unexpected reply from the server: {0}")]
    Format(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("the download was damaged (its checksum does not match); please try again")]
    Checksum,
    #[error("cancelled")]
    Cancelled,
    #[error("not enough free space: {need_mb} MB needed, {free_mb} MB free")]
    NoSpace { need_mb: u64, free_mb: u64 },
    #[error(
        "this card is formatted as FAT32, which cannot hold files over 4 GB; reformat it as exFAT"
    )]
    Fat32,
}

pub type Result<T> = std::result::Result<T, Error>;

fn network(e: impl std::fmt::Display) -> Error {
    Error::Network(e.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Downloading,
    /// Checking the finished download against its SHA-256.
    Checking,
}

#[derive(Debug, Clone, Copy)]
pub struct Progress {
    pub stage: Stage,
    pub done: u64,
    pub total: u64,
}

pub struct Client {
    agent: ureq::Agent,
    base: String,
}

/// What [`Client::books`] asks the library for; `None` means any.
#[derive(Debug, Clone, Copy, Default)]
pub struct Filter<'a> {
    /// ISO 639-3 code, e.g. `eng`.
    pub lang: Option<&'a str>,
    /// e.g. `wikipedia`.
    pub category: Option<&'a str>,
    /// A book name such as `wikipedia_en_all`.
    pub name: Option<&'a str>,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    pub fn new() -> Self {
        Self::with_base(LIBRARY)
    }

    /// A client for another OPDS server.
    pub fn with_base(base: &str) -> Self {
        // The operating system's certificate store, so company proxies and
        // the like work as they do for the browser. The proxy itself comes
        // from HTTPS_PROXY and friends.
        let tls = ureq::tls::TlsConfig::builder()
            .root_certs(ureq::tls::RootCerts::PlatformVerifier)
            .build();
        let config = ureq::Agent::config_builder()
            .tls_config(tls)
            .user_agent(format!("xwa-tool/{}", env!("CARGO_PKG_VERSION")))
            .timeout_connect(Some(Duration::from_secs(30)))
            .build();
        Self {
            agent: config.into(),
            base: base.trim_end_matches('/').to_string(),
        }
    }

    fn get_text(&self, url: &str) -> Result<String> {
        let mut resp = self.agent.get(url).call().map_err(network)?;
        resp.body_mut()
            .with_config()
            .limit(64 << 20)
            .read_to_string()
            .map_err(network)
    }

    /// Languages the library has books in, by name.
    pub fn languages(&self) -> Result<Vec<Language>> {
        let mut langs = catalog::parse_languages(
            &self.get_text(&format!("{}/catalog/v2/languages", self.base))?,
        )?;
        langs.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(langs)
    }

    /// Every book matching the filter, newest first within each name and
    /// flavour.
    pub fn books(&self, filter: &Filter) -> Result<Vec<Book>> {
        const PAGE: usize = 500;
        let mut books = Vec::new();
        loop {
            let mut url = format!(
                "{}/catalog/v2/entries?count={PAGE}&start={}",
                self.base,
                books.len()
            );
            for (key, value) in [
                ("lang", filter.lang),
                ("category", filter.category),
                ("name", filter.name),
            ] {
                if let Some(v) = value.filter(|v| !v.is_empty()) {
                    let v: String = v
                        .chars()
                        .filter(|c| c.is_ascii_alphanumeric() || "-_".contains(*c))
                        .collect();
                    url.push_str(&format!("&{key}={v}"));
                }
            }
            let page = catalog::parse_books(&self.get_text(&url)?)?;
            let got = page.books.len();
            books.extend(page.books);
            if got < PAGE || books.len() as u64 >= page.total {
                break;
            }
        }
        books.sort_by(|a, b| (&a.name, &a.flavour, &b.date).cmp(&(&b.name, &b.flavour, &a.date)));
        Ok(books)
    }

    /// Downloads a book (its Metalink URL) into `dir` and returns the file's
    /// path. Returning `false` from `progress` cancels; a later call resumes.
    pub fn download(
        &self,
        metalink_url: &str,
        dir: &Path,
        progress: &mut dyn FnMut(Progress) -> bool,
    ) -> Result<PathBuf> {
        let meta = metalink::parse(&self.get_text(metalink_url)?)?;
        let dest = dir.join(&meta.name);
        let part = dir.join(format!("{}.part", meta.name));
        if dest.metadata().is_ok_and(|m| m.len() == meta.size) {
            // Already here: check it rather than trust the size.
            if check(&dest, &meta, progress)? {
                return Ok(dest);
            }
            std::fs::rename(&dest, &part)?;
            File::options().write(true).open(&part)?.set_len(0)?;
        }
        // A resumed download that fails its check is fetched once more from
        // the start: the damage may be in the part kept from before.
        let resumed = part.metadata().is_ok_and(|m| m.len() > 0);
        for attempt in 0..if resumed { 2 } else { 1 } {
            let mut last_error = None;
            for url in &meta.urls {
                match self.fetch(url, &part, meta.size, progress) {
                    Ok(()) => {
                        last_error = None;
                        break;
                    }
                    Err(Error::Cancelled) => return Err(Error::Cancelled),
                    Err(e) => last_error = Some(e),
                }
            }
            if let Some(e) = last_error {
                return Err(e);
            }
            if check(&part, &meta, progress)? {
                std::fs::rename(&part, &dest)?;
                return Ok(dest);
            }
            if attempt == 0 && resumed {
                File::options().write(true).open(&part)?.set_len(0)?;
            }
        }
        std::fs::remove_file(&part)?;
        Err(Error::Checksum)
    }

    /// Downloads `url` into `part`, resuming from what it already holds.
    fn fetch(
        &self,
        url: &str,
        part: &Path,
        size: u64,
        progress: &mut dyn FnMut(Progress) -> bool,
    ) -> Result<()> {
        let mut have = part.metadata().map(|m| m.len()).unwrap_or(0);
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(part)?;
        if have > size {
            file.set_len(0)?;
            have = 0;
        }
        if have == size {
            return Ok(());
        }
        let mut req = self.agent.get(url);
        if have > 0 {
            req = req.header("Range", format!("bytes={have}-"));
        }
        let resp = req.call().map_err(network)?;
        match resp.status().as_u16() {
            206 => {
                // The mirror must send exactly the part asked for.
                let range = resp
                    .headers()
                    .get("content-range")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("");
                if !range.starts_with(&format!("bytes {have}-")) {
                    file.set_len(0)?;
                    return Err(Error::Network(format!(
                        "{url} sent the wrong part ({range})"
                    )));
                }
                file.seek(SeekFrom::Start(have))?;
            }
            _ => {
                // The whole file again (the server ignored the range).
                file.set_len(0)?;
                file.seek(SeekFrom::Start(0))?;
                have = 0;
            }
        }
        let mut body = resp.into_body().into_reader();
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = body.read(&mut buf).map_err(network)?;
            if n == 0 {
                break;
            }
            let n = n.min((size - have) as usize);
            file.write_all(&buf[..n])?;
            have += n as u64;
            if !progress(Progress {
                stage: Stage::Downloading,
                done: have,
                total: size,
            }) {
                file.flush()?;
                return Err(Error::Cancelled);
            }
            if have == size {
                break;
            }
        }
        file.flush()?;
        if have != size {
            return Err(Error::Network(format!(
                "{url} ended after {have} of {size} bytes"
            )));
        }
        Ok(())
    }
}

/// Whether `path` matches the Metalink's size and SHA-256 (size only when
/// it lists no SHA-256).
fn check(
    path: &Path,
    meta: &metalink::Metalink,
    progress: &mut dyn FnMut(Progress) -> bool,
) -> Result<bool> {
    use sha2::{Digest, Sha256};
    let mut file = File::open(path)?;
    if file.metadata()?.len() != meta.size {
        return Ok(false);
    }
    let Some(expected) = &meta.sha256 else {
        return Ok(true);
    };
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; 4 << 20];
    let mut done = 0u64;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
        done += n as u64;
        if !progress(Progress {
            stage: Stage::Checking,
            done,
            total: meta.size,
        }) {
            return Err(Error::Cancelled);
        }
    }
    let actual: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
    Ok(&actual == expected)
}

/// Human-readable size: "820 MB", "12.3 GB".
pub fn size_text(bytes: u64) -> String {
    let mb = bytes as f64 / 1e6;
    if mb < 1000.0 {
        format!("{mb:.0} MB")
    } else {
        format!("{:.1} GB", mb / 1000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_naturally() {
        assert_eq!(size_text(29_887_083), "30 MB");
        assert_eq!(size_text(127_418_088_448), "127.4 GB");
    }

    #[test]
    fn checks_size_and_sha256() {
        let dir = std::env::temp_dir().join(format!("xwa-fetch-check-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("f.zim");
        std::fs::write(&path, b"abc").unwrap();
        let mut meta = metalink::Metalink {
            name: "f.zim".into(),
            size: 3,
            sha256: Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into()),
            urls: vec![],
        };
        let mut ok = |_| true;
        assert!(check(&path, &meta, &mut ok).unwrap());
        meta.sha256 = Some("0".repeat(64));
        assert!(!check(&path, &meta, &mut ok).unwrap());
        meta.size = 4;
        assert!(!check(&path, &meta, &mut ok).unwrap());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
