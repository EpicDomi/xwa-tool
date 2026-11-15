#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Desktop front end. For now: pick a ZIM, see what it contains, choose the
//! target device. Conversion arrives with the converter pipeline.

use std::fmt;
use std::path::PathBuf;

use iced::widget::{button, column, pick_list, row, text};
use iced::{Element, Task};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Device {
    #[default]
    X4Pro,
    X4OrX3,
}

impl Device {
    const ALL: [Device; 2] = [Device::X4Pro, Device::X4OrX3];
}

impl fmt::Display for Device {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Device::X4Pro => "Xteink X4 Pro",
            Device::X4OrX3 => "Xteink X4 / X3",
        })
    }
}

#[derive(Default)]
struct App {
    zim: Option<PathBuf>,
    details: Option<Result<String, String>>,
    device: Device,
    loading: bool,
}

#[derive(Debug, Clone)]
enum Message {
    ChooseZim,
    ZimChosen(Option<PathBuf>),
    DetailsLoaded(Result<String, String>),
    DeviceChosen(Device),
}

fn update(app: &mut App, message: Message) -> Task<Message> {
    match message {
        Message::ChooseZim => Task::perform(
            async {
                rfd::AsyncFileDialog::new()
                    .add_filter("Kiwix ZIM archive", &["zim"])
                    .pick_file()
                    .await
                    .map(|f| f.path().to_path_buf())
            },
            Message::ZimChosen,
        ),
        Message::ZimChosen(Some(path)) => {
            app.zim = Some(path.clone());
            app.details = None;
            app.loading = true;
            Task::perform(async move { describe(&path) }, Message::DetailsLoaded)
        }
        Message::ZimChosen(None) => Task::none(),
        Message::DetailsLoaded(details) => {
            app.details = Some(details);
            app.loading = false;
            Task::none()
        }
        Message::DeviceChosen(device) => {
            app.device = device;
            Task::none()
        }
    }
}

/// A short summary read from the ZIM header and metadata (no full scan).
fn describe(path: &PathBuf) -> Result<String, String> {
    let zim = xwa_zim::Archive::open(path).map_err(|e| e.to_string())?;
    let meta = |name: &str| -> String {
        zim.metadata(name)
            .ok()
            .flatten()
            .map(|v| String::from_utf8_lossy(&v).into_owned())
            .unwrap_or_else(|| "?".into())
    };
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    Ok(format!(
        "{}\n{} · {} · {}\n{} entries · {:.1} GB",
        meta("Title"),
        meta("Language"),
        meta("Flavour"),
        meta("Date"),
        zim.entry_count(),
        size as f64 / 1e9
    ))
}

fn view(app: &App) -> Element<'_, Message> {
    let file = app
        .zim
        .as_ref()
        .map_or_else(|| "No file chosen".to_string(), |p| p.display().to_string());
    let details = match (&app.details, app.loading) {
        (_, true) => text("Reading…"),
        (Some(Ok(d)), _) => text(d.clone()),
        (Some(Err(e)), _) => text(format!("Could not read this file: {e}")),
        (None, _) => text(""),
    };
    column![
        text("Convert a Kiwix ZIM for your Xteink").size(22),
        row![
            button("Choose ZIM file…").on_press(Message::ChooseZim),
            text(file)
        ]
        .spacing(12),
        details,
        row![
            text("Device"),
            pick_list(&Device::ALL[..], Some(app.device), Message::DeviceChosen)
        ]
        .spacing(12),
        button("Convert"),
    ]
    .spacing(16)
    .padding(24)
    .into()
}

fn main() -> iced::Result {
    iced::application(App::default, update, view)
        .title("xwa-tool")
        .window_size((680.0, 420.0))
        .run()
}
