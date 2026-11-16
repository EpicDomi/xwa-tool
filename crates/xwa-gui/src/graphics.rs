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
//! A launch that fails to draw before it has got going starts the app again
//! straight away, the next way (see `retry`), so the user meets a working
//! window rather than an error.
//!
//! The window is the device's shape where it can be transparent. On Windows
//! that takes DirectX 12 presenting through DirectComposition (iced is
//! patched to allow it, see `vendor/README.md`); drawn any other way the
//! window there is a rectangle.
//!
//! iced reads its choice from the environment (`ICED_BACKEND` for GPU or
//! software, `WGPU_BACKEND` for the GPU interface,
//! `WGPU_DX12_PRESENTATION_SYSTEM` for how DirectX 12 presents); someone who
//! has set any of them keeps their own.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

/// The variables iced and wgpu read the choice from.
const VARS: [&str; 3] = [
    "ICED_BACKEND",
    "WGPU_BACKEND",
    "WGPU_DX12_PRESENTATION_SYSTEM",
];

/// How many times in a row the app has started again after failing to draw.
const TRIES: &str = "XWA_GUI_TRIES";

/// Set once the launch has got going.
static STARTED: AtomicBool = AtomicBool::new(false);

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

/// How this launch draws.
#[derive(Debug, Clone, Copy)]
pub struct Launch {
    pub graphics: Graphics,
    /// Whether the window can be transparent, and so the device's shape.
    pub shaped: bool,
    /// Whether the app chose, rather than the environment.
    picked: bool,
}

/// Picks the renderer for this launch, records it, and tells iced. Must be
/// called before any other thread starts.
pub fn choose(preferred: Graphics) -> Launch {
    let graphics = decide(preferred, read_record());
    write_record(graphics, false);
    let windows = cfg!(target_os = "windows");
    if VARS.iter().any(|v| std::env::var_os(v).is_some()) {
        // Their own choice, which on Windows may not make a window that
        // can be transparent.
        return Launch {
            graphics,
            shaped: !windows,
            picked: false,
        };
    }
    let shaped = !windows || graphics == Graphics::Dx12;
    let iced = match graphics {
        Graphics::Software => "tiny-skia",
        // Software drawing cannot show through a transparent window on
        // Windows, so there the shaped window keeps to DirectX 12: should
        // it not start, `retry` starts the app again the next way.
        _ if windows && shaped => "wgpu",
        // Should the GPU not start at all, iced falls back to software.
        _ => "wgpu,tiny-skia",
    };
    // SAFETY: called at the start of `main`, before any thread exists.
    unsafe {
        std::env::set_var("ICED_BACKEND", iced);
        if graphics != Graphics::Software {
            std::env::set_var("WGPU_BACKEND", graphics.key());
        }
        if windows && shaped {
            std::env::set_var("WGPU_DX12_PRESENTATION_SYSTEM", "visual");
        }
    }
    Launch {
        graphics,
        shaped,
        picked: true,
    }
}

/// The launch got going: draw the same way next time.
pub fn confirm(g: Graphics) {
    STARTED.store(true, Ordering::Relaxed);
    write_record(g, true);
}

/// Whether a failure to draw now can be met by starting again the next way:
/// before the launch has got going, when the app chose how to draw, and
/// while there is another way left.
pub fn can_retry(launch: &Launch) -> bool {
    launch.picked
        && !STARTED.load(Ordering::Relaxed)
        && launch.graphics != Graphics::Software
        && tries() < Graphics::chain().len()
}

/// Starts the app again, to draw the next way (the launch record says this
/// one failed); gives whether it did.
pub fn retry(launch: &Launch) -> bool {
    if !can_retry(launch) {
        return false;
    }
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let mut again = std::process::Command::new(exe);
    again
        .args(std::env::args_os().skip(1))
        .env(TRIES, (tries() + 1).to_string());
    // The next launch chooses afresh.
    for v in VARS {
        again.env_remove(v);
    }
    again.spawn().is_ok()
}

fn tries() -> usize {
    std::env::var(TRIES)
        .ok()
        .and_then(|t| t.parse().ok())
        .unwrap_or(0)
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
    fn only_the_apps_own_choice_starts_again_and_not_past_software() {
        let launch = |graphics, picked| Launch {
            graphics,
            shaped: true,
            picked,
        };
        let first = Graphics::chain()[0];
        assert!(can_retry(&launch(first, true)));
        assert!(
            !can_retry(&launch(first, false)),
            "a choice from the environment is left alone"
        );
        assert!(
            !can_retry(&launch(Graphics::Software, true)),
            "nothing comes after software"
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
