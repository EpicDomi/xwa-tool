//! Choices the app remembers between runs, kept as `key=value` lines in
//! `xwa-tool/app.txt` in the user's configuration folder.

use std::path::{Path, PathBuf};

use crate::device::{Finish, Model};
use crate::graphics::Graphics;

#[derive(Debug, Clone, PartialEq)]
pub struct Prefs {
    /// The e-reader: what conversions are made for, and what the window
    /// looks like.
    pub model: Model,
    /// The case colour; `None` is the model's usual one.
    pub finish: Option<Finish>,
    /// How big the device is drawn, in desktop pixels per screen pixel;
    /// `None` fits it to the monitor.
    pub zoom: Option<f32>,
    /// Where downloads go; `None` is the Downloads folder.
    pub download_dir: Option<PathBuf>,
    /// The Kiwix library language last shown (ISO 639-3).
    pub language: Option<String>,
    /// Flash the screen black and white when it changes, as e-ink does on a
    /// full refresh.
    pub refresh_flash: bool,
    /// How to draw the window; automatic unless chosen in Settings.
    pub graphics: Graphics,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            model: Model::default(),
            finish: None,
            zoom: None,
            download_dir: None,
            language: None,
            refresh_flash: true,
            graphics: Graphics::Auto,
        }
    }
}

impl Prefs {
    pub fn finish(&self) -> Finish {
        self.finish.unwrap_or(self.model.usual_finish())
    }

    /// Reads the remembered choices; anything missing or unreadable keeps
    /// its default.
    pub fn load() -> Prefs {
        file()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .map(|text| Prefs::parse(&text))
            .unwrap_or_default()
    }

    /// Remembers the choices. Failing to is not worth bothering anyone about:
    /// the app works the same, it just forgets.
    pub fn save(&self) {
        // Tests never touch the real file.
        if cfg!(test) {
            return;
        }
        if let Some(path) = file() {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(path, self.to_text());
        }
    }

    pub fn parse(text: &str) -> Prefs {
        let mut prefs = Prefs::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            match key.trim() {
                "device" => {
                    if let Some(m) = Model::ALL.into_iter().find(|m| m.key() == value) {
                        prefs.model = m;
                    }
                }
                "case" => prefs.finish = Finish::ALL.into_iter().find(|f| f.key() == value),
                "zoom" => prefs.zoom = value.parse().ok().filter(|z: &f32| z.is_finite()),
                "downloads" if !value.is_empty() => {
                    prefs.download_dir = Some(PathBuf::from(value));
                }
                "language" if !value.is_empty() => prefs.language = Some(value.to_string()),
                "refresh_flash" => prefs.refresh_flash = value != "off",
                "graphics" => {
                    if let Some(g) = Graphics::from_key(value) {
                        prefs.graphics = g;
                    }
                }
                _ => {}
            }
        }
        prefs
    }

    pub fn to_text(&self) -> String {
        let mut out = format!("device={}\n", self.model.key());
        if let Some(f) = self.finish {
            out += &format!("case={}\n", f.key());
        }
        if let Some(z) = self.zoom {
            out += &format!("zoom={z}\n");
        }
        if let Some(d) = &self.download_dir {
            out += &format!("downloads={}\n", d.display());
        }
        if let Some(l) = &self.language {
            out += &format!("language={l}\n");
        }
        out += &format!(
            "refresh_flash={}\n",
            if self.refresh_flash { "on" } else { "off" }
        );
        if self.graphics != Graphics::Auto {
            out += &format!("graphics={}\n", self.graphics.key());
        }
        out
    }
}

fn file() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("xwa-tool").join("app.txt"))
}

/// The folder downloads go to when none was chosen.
pub fn default_download_dir() -> PathBuf {
    dirs::download_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| Path::new(".").to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let prefs = Prefs {
            model: Model::X3,
            finish: Some(Finish::Black),
            zoom: Some(0.85),
            download_dir: Some(PathBuf::from("/data/zim files")),
            language: Some("fra".into()),
            refresh_flash: false,
            graphics: Graphics::Software,
        };
        assert_eq!(Prefs::parse(&prefs.to_text()), prefs);
        assert_eq!(Prefs::parse(&Prefs::default().to_text()), Prefs::default());
    }

    #[test]
    fn ignores_what_it_does_not_know() {
        let prefs = Prefs::parse("device=x9\nzoom=big\nnonsense\ncolour=red\nlanguage=\n");
        assert_eq!(prefs, Prefs::default());
    }
}
