//! Firmware from GitHub releases: the releases of a repository, the asset
//! in each for a reader, and the download, checked against the SHA-256
//! GitHub gives for it.

use std::io::Read;
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::{Error, Model, Result};

/// The firmware the app installs unless other firmware is allowed.
pub const FIRMWARE_REPO: &str = "EpicDomi/cpr-vcodex-wiki-x4pro";

const USER_AGENT: &str = concat!(
    "xwa-tool/",
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("CARGO_PKG_REPOSITORY"),
    ")"
);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    pub name: String,
    /// When it was published (an RFC 3339 time).
    pub published: String,
    pub prerelease: bool,
    pub assets: Vec<Asset>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
    /// Lowercase hex, when GitHub gives one.
    pub sha256: Option<String>,
}

impl Release {
    /// The firmware file in it for `model`: `<tag>-x4pro.bin` for the X4
    /// Pro, and the other `.bin` for the X4 and X3, which share one.
    pub fn firmware_for(&self, model: Model) -> Option<&Asset> {
        let bins = self.assets.iter().filter(|a| a.name.ends_with(".bin"));
        let pro = |a: &&Asset| a.name.to_lowercase().contains("x4pro");
        match model {
            Model::X4Pro => bins.filter(pro).min_by_key(|a| a.name.len()),
            Model::X4 | Model::X3 => bins.filter(|a| !pro(a)).min_by_key(|a| a.name.len()),
        }
    }

    /// The date it was published, `2026-11-24`.
    pub fn date(&self) -> &str {
        self.published.get(..10).unwrap_or(&self.published)
    }
}

fn agent() -> ureq::Agent {
    let tls = ureq::tls::TlsConfig::builder()
        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
        .build();
    ureq::Agent::config_builder()
        .tls_config(tls)
        .user_agent(USER_AGENT)
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_global(Some(Duration::from_secs(600)))
        .build()
        .into()
}

fn network(e: ureq::Error) -> Error {
    Error::Network(e.to_string())
}

/// Reads GitHub's list of releases (newest first), keeping those with
/// firmware in them: drafts, and releases of other files (the SD card font
/// releases), are left out.
pub fn parse_releases(json: &str) -> Result<Vec<Release>> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| Error::Format(e.to_string()))?;
    let list = v
        .as_array()
        .ok_or_else(|| Error::Format("no list of releases".into()))?;
    let text = |v: &serde_json::Value, k: &str| v[k].as_str().unwrap_or_default().to_string();
    Ok(list
        .iter()
        .filter(|r| !r["draft"].as_bool().unwrap_or(false))
        .map(|r| Release {
            tag: text(r, "tag_name"),
            name: text(r, "name"),
            published: text(r, "published_at"),
            prerelease: r["prerelease"].as_bool().unwrap_or(false),
            assets: r["assets"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|a| Asset {
                            name: text(a, "name"),
                            url: text(a, "browser_download_url"),
                            size: a["size"].as_u64().unwrap_or(0),
                            sha256: a["digest"]
                                .as_str()
                                .and_then(|d| d.strip_prefix("sha256:"))
                                .map(str::to_lowercase),
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
        .filter(|r| r.assets.iter().any(|a| a.name.ends_with(".bin")))
        .collect())
}

/// The latest releases of `repo` ("owner/name"), newest first.
pub fn releases(repo: &str) -> Result<Vec<Release>> {
    let url = format!("https://api.github.com/repos/{repo}/releases?per_page=20");
    let mut reply = agent()
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| match e {
            ureq::Error::StatusCode(404) => Error::Format(format!(
                "{repo} cannot be found; it may be private, and its releases can be \
                     downloaded only once it is public"
            )),
            e => network(e),
        })?;
    let body = reply
        .body_mut()
        .with_config()
        .limit(16 << 20)
        .read_to_string()
        .map_err(network)?;
    parse_releases(&body)
}

/// Downloads `asset`, checking its SHA-256 when GitHub gave one. `progress`
/// gets bytes done and in all, and stops it by returning false.
pub fn download(asset: &Asset, progress: &mut dyn FnMut(u64, u64) -> bool) -> Result<Vec<u8>> {
    let mut reply = agent().get(&asset.url).call().map_err(network)?;
    let mut reader = reply.body_mut().with_config().limit(64 << 20).reader();
    let mut data = Vec::with_capacity(asset.size as usize);
    let mut buf = vec![0u8; 64 << 10];
    loop {
        if !progress(data.len() as u64, asset.size) {
            return Err(Error::Cancelled);
        }
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
    }
    if let Some(want) = &asset.sha256 {
        let got: String = Sha256::digest(&data)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if &got != want {
            return Err(Error::Checksum);
        }
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPLY: &str = r#"[
      {"tag_name": "1.6.0.2-cpr-vcodex", "name": "1.6.0.2", "draft": false,
       "prerelease": true, "published_at": "2026-12-02T10:00:00Z",
       "assets": [
         {"name": "1.6.0.2-cpr-vcodex.bin", "size": 6000000,
          "browser_download_url": "https://github.com/x/y/releases/download/t/a.bin",
          "digest": "sha256:ABCDEF"},
         {"name": "1.6.0.2-cpr-vcodex-x4pro.bin", "size": 7000000,
          "browser_download_url": "https://github.com/x/y/releases/download/t/b.bin"},
         {"name": "1.6.0.2-cpr-vcodex.json", "size": 300,
          "browser_download_url": "https://github.com/x/y/releases/download/t/c.json"}
       ]},
      {"tag_name": "draft", "draft": true, "assets": []},
      {"tag_name": "sd-fonts-m1-b4", "name": "SD fonts", "draft": false,
       "prerelease": false, "published_at": "2026-12-01T09:00:00Z",
       "assets": [{"name": "ChareInk-14.cpfont", "size": 9, "browser_download_url": "f"},
                  {"name": "all-fonts.zip", "size": 9, "browser_download_url": "z"}]},
      {"tag_name": "1.5.0.40-cpr-vcodex", "name": "", "draft": false,
       "prerelease": false, "published_at": "2026-11-20T08:00:00Z",
       "assets": [{"name": "1.5.0.40-cpr-vcodex.bin", "size": 5,
                   "browser_download_url": "u"}]}
    ]"#;

    #[test]
    fn releases_and_their_firmware_for_each_reader() {
        let r = parse_releases(REPLY).unwrap();
        assert_eq!(r.len(), 2, "no drafts, and no releases without firmware");
        assert!(r[0].prerelease && !r[1].prerelease);
        assert_eq!(r[0].date(), "2026-12-02");
        let x4 = r[0].firmware_for(Model::X4).unwrap();
        assert_eq!(x4.name, "1.6.0.2-cpr-vcodex.bin");
        assert_eq!(x4.sha256.as_deref(), Some("abcdef"));
        assert_eq!(r[0].firmware_for(Model::X3), Some(x4));
        let pro = r[0].firmware_for(Model::X4Pro).unwrap();
        assert_eq!(pro.name, "1.6.0.2-cpr-vcodex-x4pro.bin");
        assert_eq!(pro.sha256, None);
        // An older release without X4 Pro firmware has none for it.
        assert!(r[1].firmware_for(Model::X4Pro).is_none());
        assert!(r[1].firmware_for(Model::X4).is_some());
    }
}
