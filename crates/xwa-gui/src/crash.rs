//! When the app fails, it says so: a report goes to `xwa-tool/crash.txt`
//! in the configuration folder, and a dialog tells the user what happened
//! and where the report is. Without this a failure on Windows is silent,
//! as the app has no console there.

use std::backtrace::Backtrace;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::graphics::{self, Launch};

/// What a failure kept back to say, while the app starts again to draw
/// another way (see `main`).
static HELD: Mutex<Option<String>> = Mutex::new(None);

pub fn report_file() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("xwa-tool").join("crash.txt"))
}

pub fn install(launch: Launch) {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default(info);
        let what = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown error".into());
        let place = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_default();
        let report = format!(
            "xwa-tool {} stopped.\n\n{what}\n\nWhere: {place}\nSystem: {} {}\nDrawing with: {}\n\n{}\n",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH,
            launch.graphics.name(),
            Backtrace::force_capture(),
        );
        let saved = report_file().and_then(|path| {
            std::fs::create_dir_all(path.parent()?).ok()?;
            std::fs::write(&path, &report).ok()?;
            Some(path)
        });
        let mut text = format!("xwa-tool stopped because of an error:\n\n{what}\n\n");
        if let Some(path) = &saved {
            text += &format!("A report was saved to {}.\n\n", path.display());
        }
        text += "If this happens when it starts, the next start draws the window another way.";
        // Failing to draw while starting, `main` starts the app again the
        // next way, and says nothing unless it cannot.
        if std::thread::current().name() == Some("main") && graphics::can_retry(&launch) {
            if let Ok(mut held) = HELD.lock() {
                *held = Some(text);
            }
            return;
        }
        tell("xwa-tool stopped", text);
    }));
}

/// Says what was kept back, when starting again did not happen.
pub fn tell_held() {
    if let Some(text) = HELD.lock().ok().and_then(|mut h| h.take()) {
        tell("xwa-tool stopped", text);
    }
}

/// An error dialog.
pub fn tell(title: &str, text: String) {
    let title = title.to_string();
    let show = move || {
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title(title)
            .set_description(text)
            .set_buttons(rfd::MessageButtons::Ok)
            .show();
    };
    if cfg!(target_os = "macos") {
        // AppKit shows dialogs from the main thread, which waiting here for
        // another thread would block.
        show();
    } else {
        // Elsewhere the dialog gets its own thread: the one that failed may
        // be in the middle of handling the window's events.
        let _ = std::thread::spawn(show).join();
    }
}
