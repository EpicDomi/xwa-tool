#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Desktop front end, drawn as the e-reader it makes archives for: the
//! window is the device (an X4 Pro, X4 or X3, black or white), its screen
//! looks like e-ink and shows the firmware's own menus, and its keys work.
//! From the home screen: get Wikipedia from the Kiwix library, convert a
//! ZIM file, copy the archive to the SD card, and settings. A ZIM or XWA
//! file can also be opened with the app, or dropped onto it.

mod app;
mod crash;
mod device;
mod eink;
mod graphics;
mod icons;
mod library;
mod prefs;
mod screens;
#[cfg(test)]
mod tests;

use std::path::PathBuf;

use iced::{Color, Theme, theme, window};

use app::App;
use prefs::Prefs;

fn main() -> iced::Result {
    let open = std::env::args_os().nth(1).map(PathBuf::from);
    let prefs = Prefs::load();
    // Before anything else runs: the renderer is picked through the
    // environment, and failures from here on are reported.
    let graphics = graphics::choose(prefs.graphics);
    crash::install(graphics);
    // The real size is set once the monitor's is known; until then the
    // window stays hidden.
    let zoom = prefs.zoom.unwrap_or(app::FIRST_ZOOM);
    let window = prefs.model.spec().window();
    iced::application(
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
    .scale_factor(|app: &App| app.zoom)
    .font(eink::UBUNTU_REGULAR)
    .font(eink::UBUNTU_BOLD)
    .font(eink::NOTO_SANS)
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
        transparent: device::TRANSPARENT,
        icon: device::window_icon(),
        platform_specific: platform(),
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
fn platform() -> window::settings::PlatformSpecific {
    // Windows cannot show a transparent window here, so it rounds the
    // corners and draws the shadow itself.
    window::settings::PlatformSpecific {
        undecorated_shadow: true,
        corner_preference: window::settings::platform::CornerPreference::Round,
        ..Default::default()
    }
}

#[cfg(target_os = "linux")]
fn platform() -> window::settings::PlatformSpecific {
    window::settings::PlatformSpecific {
        application_id: "xwa-tool".into(),
        ..Default::default()
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
fn platform() -> window::settings::PlatformSpecific {
    window::settings::PlatformSpecific::default()
}
