#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Desktop front end, drawn as the e-reader it makes archives for: the
//! window is the device (an X4 Pro, X4 or X3, black or white), its screen
//! looks like e-ink and shows the firmware's own menus, and its keys work.
//! From the home screen: get Wikipedia or another wiki from the Kiwix
//! library (downloads queue up on their own screen), convert a ZIM file,
//! copy the archive to the SD card, and settings. A ZIM or XWA file can also
//! be opened with the app, or dropped onto it.

mod app;
mod crash;
mod device;
mod downloads;
mod eink;
mod graphics;
mod icons;
mod library;
mod prefs;
mod read;
mod screens;
#[cfg(test)]
mod tests;

use std::panic::AssertUnwindSafe;
use std::path::PathBuf;

use iced::{Color, Theme, theme, window};

use app::App;
use graphics::{Graphics, Launch};
use prefs::Prefs;

fn main() -> iced::Result {
    let open = std::env::args_os().nth(1).map(PathBuf::from);
    let prefs = Prefs::load();
    // Before anything else runs: the renderer is picked through the
    // environment, and failures from here on are reported.
    let launch = graphics::choose(prefs.graphics);
    device::set_shaped(launch.shaped);
    crash::install(launch);
    let chosen = prefs.graphics;
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| run(prefs, open, launch)));
    // Failing to draw while starting (an error from iced, or a failure on
    // its thread), the app starts again the next way.
    let failed = matches!(
        result,
        Err(_) | Ok(Err(iced::Error::GraphicsCreationFailed(_)))
    );
    if failed && graphics::can_retry(&launch) {
        if chosen != Graphics::Auto {
            // The way chosen in Settings does not work here.
            let mut prefs = Prefs::load();
            prefs.graphics = Graphics::Auto;
            prefs.save();
        }
        if graphics::retry(&launch) {
            return Ok(());
        }
    }
    match result {
        Ok(Err(e)) => {
            crash::tell(
                "xwa-tool could not start",
                format!("xwa-tool could not open its window: {e}"),
            );
            Err(e)
        }
        Ok(ok) => ok,
        Err(failure) => {
            crash::tell_held();
            std::panic::resume_unwind(failure)
        }
    }
}

fn run(prefs: Prefs, open: Option<PathBuf>, launch: Launch) -> iced::Result {
    // The real size is set once the monitor's is known; until then the
    // window stays hidden.
    let zoom = app::start_zoom(&prefs);
    let window = prefs.model.spec().window();
    let graphics = launch.graphics;
    let app = iced::application(
        move || App::new(prefs.clone(), zoom, open.clone(), graphics),
        App::update,
        App::view,
    )
    .subscription(App::subscription)
    .title("xwa-tool")
    .theme(|_: &App| eink_theme())
    .style(|_: &App, _: &Theme| theme::Style {
        background_color: Color::TRANSPARENT,
        text_color: eink::INK,
    })
    .scale_factor(|app: &App| app.zoom);
    eink::FONTS
        .iter()
        .fold(app, |app, &f| app.font(f))
        .default_font(eink::UI)
        .exit_on_close_request(false)
        .window(window::Settings {
            size: window,
            position: window::Position::Centered,
            visible: false,
            // Resizable, so the system takes size changes (Wayland pins the size
            // of a window that is not), and the device can be dragged bigger.
            resizable: true,
            decorations: false,
            transparent: launch.shaped,
            icon: device::window_icon(),
            platform_specific: platform(launch.shaped),
            exit_on_close_request: false,
            ..window::Settings::default()
        })
        .run()
}

/// Everything on screen is styled by hand; the theme only matters for its
/// clear background.
pub fn eink_theme() -> Theme {
    Theme::custom(
        "E-ink".to_string(),
        theme::Palette {
            background: Color::TRANSPARENT,
            text: eink::INK,
            primary: eink::INK,
            success: eink::INK,
            warning: eink::INK,
            danger: eink::INK,
        },
    )
}

#[cfg(target_os = "windows")]
fn platform(shaped: bool) -> window::settings::PlatformSpecific {
    use window::settings::platform::CornerPreference;
    if shaped {
        // The window is the case's shape, with its own corners.
        window::settings::PlatformSpecific {
            undecorated_shadow: false,
            corner_preference: CornerPreference::DoNotRound,
            ..Default::default()
        }
    } else {
        // A rectangular window, whose corners and shadow the system draws.
        window::settings::PlatformSpecific {
            undecorated_shadow: true,
            corner_preference: CornerPreference::Round,
            ..Default::default()
        }
    }
}

#[cfg(target_os = "linux")]
fn platform(_shaped: bool) -> window::settings::PlatformSpecific {
    window::settings::PlatformSpecific {
        application_id: "xwa-tool".into(),
        ..Default::default()
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
fn platform(_shaped: bool) -> window::settings::PlatformSpecific {
    window::settings::PlatformSpecific::default()
}
