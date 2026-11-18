//! Font files for the fallback FONT section (spec 10), downloaded when an
//! archive first needs them and kept for next time, each checked against
//! the SHA-256 the converter's font catalogue gives.

use std::path::PathBuf;
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::{Error, Result, network};

/// Google asks tools to say who they are, as Wikimedia does.
const USER_AGENT: &str = concat!(
    "xwa-tool/",
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("CARGO_PKG_REPOSITORY"),
    ")"
);

pub struct Fonts {
    agent: ureq::Agent,
    cache: PathBuf,
}

impl std::fmt::Debug for Fonts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Fonts")
            .field("cache", &self.cache)
            .finish_non_exhaustive()
    }
}

fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

impl Fonts {
    /// Downloads into, and reads from, the `cache` folder, which is created
    /// when needed.
    pub fn new(cache: PathBuf) -> Self {
        let tls = ureq::tls::TlsConfig::builder()
            .root_certs(ureq::tls::RootCerts::PlatformVerifier)
            .build();
        let config = ureq::Agent::config_builder()
            .tls_config(tls)
            .user_agent(USER_AGENT)
            .timeout_connect(Some(Duration::from_secs(30)))
            .timeout_global(Some(Duration::from_secs(600)))
            .build();
        Self {
            agent: config.into(),
            cache,
        }
    }

    /// The cache folder used when none is given: `xwa-tool/fonts` in the
    /// system's cache folder.
    pub fn default_cache() -> Option<PathBuf> {
        dirs::cache_dir().map(|d| d.join("xwa-tool").join("fonts"))
    }

    /// Whether the font with this SHA-256 is already kept.
    pub fn is_cached(&self, sha256: &str) -> bool {
        self.cache.join(format!("{sha256}.ttf")).is_file()
    }

    /// The font file at `url`, whose SHA-256 is `sha256`: from the cache,
    /// or else downloaded and kept there.
    pub fn get(&self, url: &str, sha256: &str, size: u64) -> Result<Vec<u8>> {
        let path = self.cache.join(format!("{sha256}.ttf"));
        if let Ok(data) = std::fs::read(&path) {
            if sha256_hex(&data) == sha256 {
                return Ok(data);
            }
            // Damaged: fetch it again.
            let _ = std::fs::remove_file(&path);
        }
        let mut resp = self.agent.get(url).call().map_err(network)?;
        let data = resp
            .body_mut()
            .with_config()
            .limit(size.max(1) + 1)
            .read_to_vec()
            .map_err(network)?;
        if data.len() as u64 != size || sha256_hex(&data) != sha256 {
            return Err(Error::Checksum);
        }
        std::fs::create_dir_all(&self.cache)?;
        // Written whole, then renamed, so a cancelled run leaves nothing half done.
        let part = path.with_extension("part");
        std::fs::write(&part, &data)?;
        std::fs::rename(&part, &path)?;
        Ok(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cached_font_is_read_without_the_network_and_checked() {
        let dir = std::env::temp_dir().join(format!("xwa-fonts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let data = b"not really a font".to_vec();
        let sha = sha256_hex(&data);
        std::fs::write(dir.join(format!("{sha}.ttf")), &data).unwrap();
        let fonts = Fonts::new(dir.clone());
        assert!(fonts.is_cached(&sha));
        // The URL is never fetched: the cache has it.
        assert_eq!(
            fonts
                .get("https://invalid.invalid/x.ttf", &sha, data.len() as u64)
                .unwrap(),
            data
        );
        // A damaged file is not used.
        std::fs::write(dir.join(format!("{sha}.ttf")), b"damaged").unwrap();
        assert!(
            fonts
                .get("http://127.0.0.1:9/x.ttf", &sha, data.len() as u64)
                .is_err()
        );
        assert!(!fonts.is_cached(&sha));
        let _ = std::fs::remove_dir_all(dir);
    }
}
