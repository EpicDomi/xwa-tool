//! Which renderer draws the window, and a way out for machines whose
//! graphics driver fails.
//!
//! wgpu picks Vulkan wherever a Vulkan driver is installed, and some
//! Windows drivers crash on the first frame: the window stays white, then
//! the app is gone. So on Windows DirectX 12 comes first. And whatever the
//! system, each launch is recorded before the window opens and confirmed
//! once the app has run for a few seconds (or quit normally): a launch that
//! died before that makes the next one try the next way of drawing, down to
//! software, which works everywhere. Settings can also pick one outright.
//!
//! iced reads its choice from the environment (`ICED_BACKEND` for GPU or
//! software, `WGPU_BACKEND` for the GPU interface); someone who has set
//! either keeps their own.

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Graphics {
    Auto,
    Dx12,
    Vulkan,
    Metal,
    OpenGl,
    Software,
}

impl Graphics {
    /// The ways of drawing on this system, best first.
    pub fn chain() -> &'static [Graphics] {
        if cfg!(target_os = "windows") {
            &[
                Graphics::Dx12,
                Graphics::Vulkan,
                Graphics::OpenGl,
                Graphics::Software,
            ]
        } else if cfg!(target_os = "macos") {
            &[Graphics::Metal, Graphics::Software]
        } else {
            &[Graphics::Vulkan, Graphics::OpenGl, Graphics::Software]
        }
    }

    /// What Settings offers.
    pub fn choices() -> Vec<Graphics> {
        let mut all = vec![Graphics::Auto];
        all.extend_from_slice(Graphics::chain());
        all
    }

    pub fn key(self) -> &'static str {
        match self {
            Graphics::Auto => "auto",
            Graphics::Dx12 => "dx12",
            Graphics::Vulkan => "vulkan",
            Graphics::Metal => "metal",
            Graphics::OpenGl => "gl",
            Graphics::Software => "software",
        }
    }

    pub fn from_key(key: &str) -> Option<Graphics> {
        [
            Graphics::Auto,
            Graphics::Dx12,
            Graphics::Vulkan,
            Graphics::Metal,
            Graphics::OpenGl,
            Graphics::Software,
        ]
        .into_iter()
        .find(|g| g.key() == key)
    }

    pub fn name(self) -> &'static str {
        match self {
            Graphics::Auto => "Automatic",
            Graphics::Dx12 => "DirectX 12",
            Graphics::Vulkan => "Vulkan",
            Graphics::Metal => "Metal",
            Graphics::OpenGl => "OpenGL",
            Graphics::Software => "Software",
        }
    }

    /// The next way to try after this one failed.
    fn after(self) -> Graphics {
        let chain = Graphics::chain();
        chain
            .iter()
            .position(|g| *g == self)
            .and_then(|i| chain.get(i + 1))
            .copied()
            .unwrap_or(Graphics::Software)
    }
}

/// The launch record: the way of drawing last tried, and whether that
/// launch got going.
fn record() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("xwa-tool").join("graphics.txt"))
}

fn read_record() -> Option<(Graphics, bool)> {
    let text = std::fs::read_to_string(record()?).ok()?;
    let (key, state) = text.trim().split_once(' ')?;
    Some((Graphics::from_key(key)?, state == "ok"))
}

fn write_record(g: Graphics, ok: bool) {
    if cfg!(test) {
        return;
    }
    if let Some(path) = record() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let state = if ok { "ok" } else { "starting" };
        let _ = std::fs::write(path, format!("{} {state}\n", g.key()));
    }
}

/// What to draw with this launch, given the choice in Settings and how the
/// last launch went.
pub fn decide(preferred: Graphics, last: Option<(Graphics, bool)>) -> Graphics {
    if preferred != Graphics::Auto {
        return preferred;
    }
    match last {
        Some((g, true)) if g != Graphics::Auto => g,
        // The last launch died before it got going.
        Some((g, false)) if g != Graphics::Auto => g.after(),
        _ => Graphics::chain()[0],
    }
}

/// Picks the renderer for this launch, records it, and tells iced. Must be
/// called before any other thread starts.
pub fn choose(preferred: Graphics) -> Graphics {
    let chosen = decide(preferred, read_record());
    write_record(chosen, false);
    let user_set =
        std::env::var_os("ICED_BACKEND").is_some() || std::env::var_os("WGPU_BACKEND").is_some();
    if !user_set {
        let (iced, wgpu) = match chosen {
            Graphics::Software => ("tiny-skia", None),
            // Should the GPU not start at all, iced falls back to software.
            g => ("wgpu,tiny-skia", Some(g.key())),
        };
        // SAFETY: called at the start of `main`, before any thread exists.
        unsafe {
            std::env::set_var("ICED_BACKEND", iced);
            if let Some(w) = wgpu {
                std::env::set_var("WGPU_BACKEND", w);
            }
        }
    }
    chosen
}

/// The launch got going: draw the same way next time.
pub fn confirm(g: Graphics) {
    write_record(g, true);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_launch_moves_down_the_chain() {
        let chain = Graphics::chain();
        assert_eq!(decide(Graphics::Auto, None), chain[0]);
        assert_eq!(decide(Graphics::Auto, Some((chain[0], true))), chain[0]);
        assert_eq!(decide(Graphics::Auto, Some((chain[0], false))), chain[1]);
        let last = *chain.last().unwrap();
        assert_eq!(last, Graphics::Software);
        assert_eq!(
            decide(Graphics::Auto, Some((last, false))),
            Graphics::Software
        );
        // A choice in Settings wins.
        assert_eq!(
            decide(Graphics::Software, Some((chain[0], true))),
            Graphics::Software
        );
    }

    #[test]
    fn windows_tries_directx_first() {
        if cfg!(target_os = "windows") {
            assert_eq!(Graphics::chain()[0], Graphics::Dx12);
        }
        for g in Graphics::choices() {
            assert_eq!(Graphics::from_key(g.key()), Some(g));
        }
    }
}
