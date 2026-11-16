//! What each screen shows, and the device drawn round it.

use std::path::Path;
use std::time::Duration;

use iced::border::Border;
use iced::widget::{
    Space, button, canvas, column, container, image, mouse_area, opaque, pin, row, space, stack,
    tooltip,
};
use iced::{Alignment, Background, Element, Length, Padding, Shadow, mouse};
use xwa_fetch::size_text;

use crate::app::{
    App, Choice, Job, Message, Popup, SEARCH_ID, Screen, ZOOMS, ZimInfo, conversion_steps, group,
};
use crate::device::{Body, Finish, Glass, Key, MARGIN, Model};
use crate::eink::{
    self, BODY, DARK, Face, INK, Kind, LIGHT, PAD, PAPER, RADIUS, Row, SIDE, SMALL, TITLE, fit,
    label, paragraph,
};
use crate::graphics::Graphics;
use crate::icons;
use crate::library::{self, Choice as _, View, Wiki};

/// The key held for shortcuts.
const MODIFIER: &str = if cfg!(target_os = "macos") {
    "⌘"
} else {
    "Ctrl"
};

/// What a screen holds: its header, the room it keeps above and below its
/// list, and the list.
pub struct Page {
    pub title: String,
    pub subtitle: Option<String>,
    pub back: bool,
    pub top: f32,
    pub bottom: f32,
    pub rows: Vec<Row<Message>>,
    /// Said under the list, when there is nothing (or not yet anything) in it.
    pub note: Option<String>,
    /// Labels for the four front keys.
    pub hints: [&'static str; 4],
}

const LIST_HINTS: [&str; 4] = ["« Back", "Select", "Up", "Down"];
const HOME_TOP: f32 = 192.0;
const TILE_TOP: f32 = 150.0;
const FIELD_TOP: f32 = 58.0;
const SUMMARY_TOP: f32 = 112.0;
const COPY_TOP: f32 = 96.0;

fn on_off(on: bool) -> &'static str {
    if on { "ON" } else { "OFF" }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn percent(fraction: f32) -> String {
    format!("{:.0}%", (fraction * 100.0).floor())
}

fn time_left(left: Duration) -> String {
    let minutes = left.as_secs().div_ceil(60);
    match minutes {
        0..=1 => "About a minute left".into(),
        2..=59 => format!("About {minutes} min left"),
        _ => format!("About {} h {} min left", minutes / 60, minutes % 60),
    }
}

impl App {
    pub fn page(&self) -> Page {
        match self.screen {
            Screen::Home => self.home_page(),
            Screen::Library => self.library_page(),
            Screen::Languages => self.languages_page(),
            Screen::Book => self.book_page(),
            Screen::Convert => self.convert_page(),
            Screen::Card => self.card_page(),
            Screen::Settings => self.settings_page(),
            Screen::Filter => self.filter_page(),
        }
    }

    fn home_page(&self) -> Page {
        let short = self.prefs.model.short_name();
        let get = match (&self.downloading, self.download.fraction()) {
            (Some(b), Some(f)) => format!("Downloading {}… {}", b.title, percent(f)),
            _ => "From the Kiwix library".into(),
        };
        let convert = match (self.conversion.fraction(), &self.zim) {
            (Some(f), _) => format!("Converting… {}", percent(f)),
            (None, Some(zim)) => file_name(zim),
            (None, None) => format!("Make a ZIM file readable on the {short}"),
        };
        let copy = match (self.copy.fraction(), &self.archive) {
            (Some(f), _) => format!("Copying… {}", percent(f)),
            (None, Some(a)) => file_name(a),
            (None, None) => "Put an archive on your e-reader's card".into(),
        };
        let settings = format!(
            "{short} · {} case · {:.0}% size",
            self.prefs.finish().name(),
            self.zoom * 100.0
        );
        Page {
            title: "xwa-tool".into(),
            subtitle: Some(self.prefs.model.name().into()),
            back: false,
            top: HOME_TOP,
            bottom: 0.0,
            rows: vec![
                Row::new(Kind::Menu, "Get Wikis")
                    .icon(&icons::DOWNLOAD)
                    .subtitle(get)
                    .on_press(Message::Open(Screen::Library)),
                Row::new(Kind::Menu, "Convert")
                    .icon(&icons::CONVERT)
                    .subtitle(convert)
                    .on_press(Message::Open(Screen::Convert)),
                Row::new(Kind::Menu, "Copy to SD card")
                    .icon(&icons::TRANSFER)
                    .subtitle(copy)
                    .on_press(Message::Open(Screen::Card)),
                Row::new(Kind::Menu, "Settings")
                    .icon(&icons::SETTINGS)
                    .subtitle(settings)
                    .on_press(Message::Open(Screen::Settings)),
            ],
            note: None,
            hints: ["", "Select", "Up", "Down"],
        }
    }

    fn library_page(&self) -> Page {
        let wiki = self.prefs.wiki;
        let view = self.prefs.view;
        let mut rows = vec![
            Row::new(Kind::Setting, "Wiki")
                .value(wiki.name())
                .on_press(Message::Choose(Choice::Wiki)),
            Row::new(Kind::Setting, "Language")
                .value(
                    self.language
                        .as_ref()
                        .map(|l| l.name.clone())
                        .unwrap_or_else(|| "…".into()),
                )
                .opens()
                .on_press_maybe(
                    (!self.languages.is_empty()).then_some(Message::Open(Screen::Languages)),
                ),
            Row::new(Kind::Item, "Filter and sort")
                .subtitle(view.summary())
                .opens()
                .on_press(Message::Open(Screen::Filter)),
        ];
        debug_assert_eq!(rows.len(), LIBRARY_ROWS);
        let downloading = self.downloading.as_ref().map(|b| &b.id);
        let shown = view.arrange(&self.books, &self.search);
        for b in &shown {
            let mut line = format!("{} · {}", b.flavour_description(), b.date);
            if downloading == Some(&b.id) {
                line = format!(
                    "Downloading… {}",
                    percent(self.download.fraction().unwrap_or(0.0))
                );
            }
            rows.push(
                Row::new(Kind::Item, b.title.clone())
                    .subtitle(line)
                    .value(size_text(b.size))
                    .on_press(Message::OpenBook((*b).clone())),
            );
        }
        let mut note = self.library_status.clone();
        if note.is_none() && !self.books.is_empty() && shown.is_empty() {
            note = Some(if self.search.is_empty() {
                "Nothing here matches the filters.".into()
            } else {
                "Nothing matches the search.".into()
            });
        }
        if self.library_failed {
            rows.push(Row::new(Kind::Action, "Try again").on_press(Message::LoadLibrary));
        }
        Page {
            title: "Get Wikis".into(),
            subtitle: self.language.as_ref().map(|l| l.name.clone()),
            back: true,
            top: FIELD_TOP,
            bottom: 0.0,
            rows,
            note,
            hints: ["« Back", "Open", "Up", "Down"],
        }
    }

    fn filter_page(&self) -> Page {
        let view = self.prefs.view;
        let shown = view.arrange(&self.books, "").len();
        Page {
            title: "Filter and sort".into(),
            subtitle: Some(self.prefs.wiki.name().into()),
            back: true,
            top: 0.0,
            bottom: 0.0,
            rows: vec![
                Row::new(Kind::Setting, "Pictures")
                    .value(view.pictures.name())
                    .on_press(Message::Choose(Choice::Pictures)),
                Row::new(Kind::Setting, "Selection")
                    .value(view.selection.name())
                    .on_press(Message::Choose(Choice::Selection)),
                Row::new(Kind::Setting, "Articles")
                    .value(view.articles.name())
                    .on_press(Message::Choose(Choice::Articles)),
                Row::new(Kind::Setting, "Sort by")
                    .value(view.sort.name())
                    .on_press(Message::Choose(Choice::Sort)),
                Row::new(Kind::Item, "Show everything").on_press_maybe(view.is_filtered().then(
                    || {
                        Message::SetView(View {
                            sort: view.sort,
                            ..View::default()
                        })
                    },
                )),
            ],
            note: (!self.books.is_empty())
                .then(|| format!("{shown} of the {} files are shown.", self.books.len())),
            hints: ["« Back", "Change", "Up", "Down"],
        }
    }

    fn languages_page(&self) -> Page {
        let needle = self.language_search.to_lowercase();
        let rows: Vec<_> = self
            .languages
            .iter()
            .filter(|l| {
                needle.is_empty()
                    || l.name.to_lowercase().contains(&needle)
                    || l.code.starts_with(&needle)
            })
            .map(|l| {
                let row = Row::new(Kind::Item, l.name.clone())
                    .on_press(Message::LanguageChosen(l.clone()));
                if self.language.as_ref() == Some(l) {
                    row.value(SELECTED)
                } else {
                    row
                }
            })
            .collect();
        let note = rows
            .is_empty()
            .then(|| "No language matches the search.".to_string());
        Page {
            title: "Language".into(),
            subtitle: Some(format!("{} languages", self.languages.len())),
            back: true,
            top: FIELD_TOP,
            bottom: 0.0,
            rows,
            note,
            hints: LIST_HINTS,
        }
    }

    fn book_page(&self) -> Page {
        let Some(b) = &self.book else {
            return self.home_page();
        };
        let this = self.downloading.as_ref().is_some_and(|d| d.id == b.id);
        let other = self.download.running() && !this;
        let page = |top: f32, bottom: f32, rows| Page {
            title: b.title.clone(),
            subtitle: (!b.flavour.is_empty()).then(|| b.flavour.clone()),
            back: true,
            top,
            bottom,
            rows,
            note: None,
            hints: LIST_HINTS,
        };
        if this && self.download.running() {
            return page(
                SUMMARY_TOP + progress_height(None),
                0.0,
                vec![Row::new(Kind::Item, "Pause").on_press(Message::CancelDownload)],
            );
        }
        let mut rows = vec![
            Row::new(Kind::Info, "Size").value(size_text(b.size)),
            Row::new(Kind::Info, "Articles").value(group(b.article_count)),
            Row::new(Kind::Info, "Pictures").value(if b.has_pictures() {
                group(b.media_count)
            } else {
                "None".into()
            }),
            Row::new(Kind::Info, "Published").value(b.date.clone()),
            Row::new(Kind::Item, "Save in")
                .icon(&icons::FOLDER)
                .subtitle(self.download_dir().display().to_string())
                .opens()
                .on_press(Message::ChooseDownloadDir),
        ];
        rows.push(
            Row::new(Kind::Action, "Download")
                .on_press_maybe((!other).then_some(Message::Download)),
        );
        let bottom = if other || matches!(self.download, Job::Finished(Err(_))) {
            96.0
        } else {
            0.0
        };
        page(SUMMARY_TOP, bottom, rows)
    }

    fn convert_page(&self) -> Page {
        let short = self.prefs.model.short_name();
        let mut page = Page {
            title: "Convert".into(),
            subtitle: Some(format!("for the {short}")),
            back: true,
            top: TILE_TOP,
            bottom: 0.0,
            rows: Vec::new(),
            note: None,
            hints: LIST_HINTS,
        };
        let Some(zim) = &self.zim else {
            page.top = 190.0;
            page.rows = vec![
                Row::new(Kind::Action, "Choose a ZIM file").on_press(Message::ChooseZim),
                Row::new(Kind::Item, "Get Wikis")
                    .subtitle("Download one from the Kiwix library")
                    .opens()
                    .on_press(Message::Open(Screen::Library)),
            ];
            return page;
        };
        if self.conversion.running() {
            let steps = conversion_steps(self.converting_images, self.converting_sharper);
            page.top += progress_height(Some(steps.len()));
            page.rows =
                vec![Row::new(Kind::Item, "Stop converting").on_press(Message::AskStopConversion)];
            return page;
        }
        if let Job::Finished(Ok(_)) = &self.conversion {
            page.bottom = 150.0;
            page.rows = vec![
                Row::new(Kind::Action, "Copy to SD card").on_press(Message::Open(Screen::Card)),
                Row::new(Kind::Item, "Convert again")
                    .subtitle("With other settings, or for another device")
                    .on_press(Message::ConvertAgain),
                Row::new(Kind::Item, "Convert another file")
                    .opens()
                    .on_press(Message::ChooseZim),
            ];
            return page;
        }
        let readable = matches!(self.zim_info, Some(Ok(_)));
        page.rows = vec![
            Row::new(Kind::Item, "File")
                .subtitle(file_name(zim))
                .opens()
                .on_press(Message::ChooseZim),
            Row::new(Kind::Setting, "Made for")
                .value(short)
                .on_press(Message::Choose(Choice::Device)),
            Row::new(Kind::Setting, "Pictures")
                .value(on_off(self.images))
                .on_press(Message::ToggleImages),
            Row::new(Kind::Setting, "Sharper diagrams")
                .subtitle("From Wikipedia, over the internet")
                .value(on_off(self.images && self.sharper))
                .on_press_maybe(self.images.then_some(Message::ToggleSharper)),
            Row::new(
                Kind::Action,
                if matches!(self.conversion, Job::Finished(Err(_))) {
                    "Try again"
                } else {
                    "Convert"
                },
            )
            .on_press_maybe(readable.then_some(Message::Convert)),
        ];
        if matches!(self.conversion, Job::Finished(Err(_))) {
            page.bottom = 96.0;
        }
        page
    }

    fn card_page(&self) -> Page {
        let mut page = Page {
            title: "Copy to SD card".into(),
            subtitle: None,
            back: true,
            top: 0.0,
            bottom: 0.0,
            rows: Vec::new(),
            note: None,
            hints: LIST_HINTS,
        };
        if self.copy.running() {
            page.top = COPY_TOP + progress_height(None);
            page.rows = vec![Row::new(Kind::Item, "Stop copying").on_press(Message::CancelCopy)];
            return page;
        }
        page.rows.push(
            Row::new(Kind::Item, "Archive")
                .subtitle(
                    self.archive
                        .as_deref()
                        .map(file_name)
                        .unwrap_or_else(|| "Choose an .xwa file".into()),
                )
                .opens()
                .on_press(Message::ChooseArchive),
        );
        for d in &self.drives {
            let name = if d.name.is_empty() {
                "Removable drive".to_string()
            } else {
                d.name.clone()
            };
            let row = Row::new(Kind::Item, name)
                .icon(&icons::CARD)
                .subtitle(drive_line(d))
                .on_press(Message::DriveChosen(d.clone()));
            page.rows.push(if self.drive.as_ref() == Some(d) {
                row.value(SELECTED)
            } else {
                row
            });
        }
        page.rows
            .push(Row::new(Kind::Item, "Look for cards again").on_press(Message::LookForCards));
        page.rows
            .push(Row::new(Kind::Action, "Copy").on_press_maybe(
                (self.archive.is_some() && self.drive.is_some()).then_some(Message::CopyToCard),
            ));
        if self.drives.is_empty() || matches!(self.copy, Job::Finished(_)) {
            page.bottom = 120.0;
        }
        page
    }

    fn settings_page(&self) -> Page {
        let converting = self.conversion.running();
        Page {
            title: "Settings".into(),
            subtitle: None,
            back: true,
            top: 0.0,
            bottom: 170.0,
            rows: vec![
                Row::new(Kind::Setting, "Device")
                    .value(self.prefs.model.short_name())
                    .on_press_maybe((!converting).then_some(Message::Choose(Choice::Device))),
                Row::new(Kind::Setting, "Case")
                    .value(self.prefs.finish().name())
                    .on_press(Message::Choose(Choice::Case)),
                Row::new(Kind::Setting, "Display size")
                    .value(format!("{:.0}%", self.zoom * 100.0))
                    .on_press(Message::Choose(Choice::Size)),
                {
                    let row = Row::new(Kind::Setting, "Graphics")
                        .value(match self.prefs.graphics {
                            Graphics::Auto => self.graphics.name().to_string(),
                            g => g.name().to_string(),
                        })
                        .on_press(Message::Choose(Choice::Graphics));
                    if self.prefs.graphics != Graphics::Auto && self.prefs.graphics != self.graphics
                    {
                        row.subtitle("From the next start")
                    } else {
                        row
                    }
                },
                Row::new(Kind::Setting, "Refresh flash")
                    .value(on_off(self.prefs.refresh_flash))
                    .on_press(Message::ToggleFlash),
                Row::new(Kind::Item, "Download folder")
                    .icon(&icons::FOLDER)
                    .subtitle(self.download_dir().display().to_string())
                    .opens()
                    .on_press(Message::ChooseDownloadDir),
                Row::new(Kind::Info, "Version").value(env!("CARGO_PKG_VERSION")),
                Row::new(Kind::Info, "Archive format").value(format!(
                    "XWA {}.{}",
                    xwa_format::VERSION_MAJOR,
                    xwa_format::VERSION_MINOR
                )),
            ],
            note: None,
            hints: ["« Back", "Change", "Up", "Down"],
        }
    }

    // -----------------------------------------------------------------------
    // The space above and below the lists

    fn top_view(&self) -> Element<'_, Message> {
        let w = self.spec().screen.width;
        match self.screen {
            Screen::Home => self.home_tile(w),
            Screen::Library => search(eink::field(
                "Search titles",
                &self.search,
                Message::Search,
                Some(Message::Submit),
                SEARCH_ID,
            )),
            Screen::Languages => search(eink::field(
                "Search languages",
                &self.language_search,
                Message::LanguageSearch,
                Some(Message::Submit),
                SEARCH_ID,
            )),
            Screen::Book => {
                let summary = self.book.as_ref().map(|b| b.summary.as_str()).unwrap_or("");
                let summary = container(paragraph(clip_words(summary, 180), Face::Ui, BODY))
                    .padding([10.0, SIDE + PAD])
                    .height(SUMMARY_TOP);
                if self.download.running() && self.downloading.as_ref().map(|b| &b.id) == self.book.as_ref().map(|b| &b.id) {
                    column![summary, progress_view(&self.download, None)].into()
                } else {
                    summary.into()
                }
            }
            Screen::Convert => match &self.zim {
                Some(zim) if self.conversion.running() => {
                    let steps = conversion_steps(self.converting_images, self.converting_sharper);
                    column![
                        container(zim_tile(zim, self.zim_info.as_ref(), w)).height(TILE_TOP),
                        progress_view(&self.conversion, Some(&steps)),
                    ]
                    .into()
                }
                Some(zim) => zim_tile(zim, self.zim_info.as_ref(), w),
                None => container(
                    column![
                        image(eink::wikipedia_cover()).width(96).height(96),
                        paragraph(
                            "Choose a Kiwix ZIM file to make an archive the e-reader can open, or drop one onto the device.",
                            Face::Ui,
                            BODY,
                        ),
                    ]
                    .spacing(16)
                    .align_x(Alignment::Center),
                )
                .padding([18.0, SIDE + PAD])
                .center_x(Length::Fill)
                .into(),
            },
            Screen::Card if self.copy.running() => {
                let what = self.archive.as_deref().map(file_name).unwrap_or_default();
                let to = self.drive.as_ref().map(drive_line).unwrap_or_default();
                column![
                    container(
                        column![
                            label(fit(&what, Face::UiBold, BODY, w - 2.0 * (SIDE + PAD)).into_owned(), Face::UiBold, BODY),
                            label(format!("to {}", fit(&to, Face::Small, SMALL, w - 2.0 * (SIDE + PAD) - 20.0)), Face::Small, SMALL),
                        ]
                        .spacing(4),
                    )
                    .padding(Padding {
                        top: 20.0,
                        right: SIDE + PAD,
                        bottom: 0.0,
                        left: SIDE + PAD,
                    })
                    .height(COPY_TOP),
                    progress_view(&self.copy, None),
                ]
                .into()
            }
            _ => Space::new().into(),
        }
    }

    fn bottom_view(&self) -> Element<'_, Message> {
        match self.screen {
            Screen::Convert => match &self.conversion {
                Job::Finished(Ok(s)) => note(&format!("Done. {s}")),
                Job::Finished(Err(e)) => note(&format!("It did not work: {e}")),
                _ => Space::new().into(),
            },
            Screen::Card => match &self.copy {
                Job::Finished(Ok(s)) => note(s),
                Job::Finished(Err(e)) => note(&format!("The copy did not work: {e}")),
                Job::Idle if self.drives.is_empty() => note(
                    "No card found. Put the card in a card reader, or connect your e-reader by USB, then look again.",
                ),
                _ => Space::new().into(),
            },
            Screen::Book => match &self.download {
                Job::Running { .. } => {
                    note("Another file is downloading. It can be downloaded once that one is done.")
                }
                Job::Finished(Err(e)) => note(e),
                _ => Space::new().into(),
            },
            Screen::Settings => note(&format!(
                "On the keyboard: the arrows move, Enter chooses, Esc goes back, Page Up and Page Down turn pages. {MODIFIER} with + and − changes the size, {MODIFIER} 0 fits it to the screen."
            )),
            _ => Space::new().into(),
        }
    }

    fn home_tile(&self, w: f32) -> Element<'_, Message> {
        let wide = w - 2.0 * SIDE;
        let zim_cover = || match &self.zim_info {
            Some(Ok(ZimInfo { cover: Some(c), .. })) => image_cover(c.clone()),
            _ => image_cover(eink::wikipedia_cover()),
        };
        let zim_title = || match &self.zim_info {
            Some(Ok(info)) => info.title.clone(),
            _ => self.zim.as_deref().map(file_name).unwrap_or_default(),
        };
        let (cover, title, lines, open): (
            Element<'_, Message>,
            String,
            Vec<Element<'_, Message>>,
            Screen,
        ) = if let Job::Running { step, .. } = &self.conversion {
            let steps = conversion_steps(self.converting_images, self.converting_sharper);
            (
                zim_cover(),
                zim_title(),
                job_lines(
                    &self.conversion,
                    steps.get(*step).copied().unwrap_or("Converting"),
                ),
                Screen::Convert,
            )
        } else if let (Job::Running { .. }, Some(b)) = (&self.download, &self.downloading) {
            (
                image_cover(eink::wikipedia_cover()),
                b.title.clone(),
                job_lines(&self.download, b.flavour_description()),
                Screen::Library,
            )
        } else if self.copy.running() {
            (
                icon_cover(&icons::BOOK),
                self.archive.as_deref().map(file_name).unwrap_or_default(),
                job_lines(&self.copy, "Copying to the card"),
                Screen::Card,
            )
        } else if let Some(a) = &self.archive {
            (
                icon_cover(&icons::BOOK),
                file_name(a),
                vec![small_line(match &self.copy {
                    Job::Finished(Ok(_)) => "Copied to the card",
                    _ => "Ready to copy to the card",
                })],
                Screen::Card,
            )
        } else if self.zim.is_some() {
            (
                zim_cover(),
                zim_title(),
                vec![small_line("Ready to convert")],
                Screen::Convert,
            )
        } else {
            (
                    image_cover(eink::wikipedia_cover()),
                    "Wikis, offline".into(),
                    vec![
                        paragraph(
                            "Download Wikipedia or another wiki from Kiwix, convert it here, then copy it to your e-reader's card.",
                            Face::Small,
                            SMALL,
                        )
                        .into(),
                    ],
                    Screen::Library,
                )
        };
        let words = column![paragraph(title, Face::UiBold, TITLE)]
            .extend(lines)
            .spacing(10)
            .width(Length::Fill);
        let tile = button(
            container(row![cover, words].spacing(16).align_y(Alignment::Center))
                .padding([0.0, 16.0])
                .center_y(Length::Fill),
        )
        .width(wide)
        .height(HOME_TOP - 24.0)
        .padding(0)
        .on_press(Message::Open(open))
        .style(|_, status| button::Style {
            background: match status {
                button::Status::Pressed => Some(Background::Color(LIGHT)),
                _ => None,
            },
            text_color: INK,
            border: Border {
                color: INK,
                width: 1.0,
                radius: RADIUS.into(),
            },
            shadow: Shadow::default(),
            snap: true,
        });
        container(tile).padding([12.0, SIDE]).into()
    }

    // -----------------------------------------------------------------------
    // The whole

    pub fn view(&self) -> Element<'_, Message> {
        let spec = self.spec();
        let look = self.prefs.model.look(self.prefs.finish());
        let window = spec.window();
        let screen = spec.screen_in_window();
        let mut layers: Vec<Element<'_, Message>> = vec![
            canvas(Body {
                spec,
                look,
                hovered: self.hovered_key,
                pressed: self.shown_pressed(),
            })
            .width(window.width)
            .height(window.height)
            .into(),
            pin(
                mouse_area(Space::new().width(spec.body.width).height(spec.body.height))
                    .on_press(Message::Drag)
                    .on_right_press(Message::SystemMenu),
            )
            .x(MARGIN)
            .y(MARGIN)
            .into(),
            pin(opaque(self.screen_view()))
                .x(screen.x)
                .y(screen.y)
                .into(),
        ];
        // Handles on the edges and corners resize the window; macOS does
        // that itself at the edges of a borderless window.
        if !cfg!(target_os = "macos") {
            for (r, direction, cursor) in resize_handles(window) {
                layers.push(
                    pin(mouse_area(Space::new().width(r.width).height(r.height))
                        .on_press(Message::ResizeFrom(direction))
                        .interaction(cursor))
                    .x(r.x)
                    .y(r.y)
                    .into(),
                );
            }
        }
        for k in spec.keys {
            let hit = k.hit(spec);
            let area = mouse_area(Space::new().width(hit.width).height(hit.height))
                .on_enter(Message::KeyHover(Some(k.key)))
                .on_exit(Message::KeyHover(None))
                .on_press(Message::KeyDown(k.key))
                .on_release(Message::KeyUp(k.key))
                .interaction(mouse::Interaction::Pointer);
            layers.push(
                pin(tooltip(area, tip(k.key.tooltip()), k.tip_position()).gap(4))
                    .x(hit.x)
                    .y(hit.y)
                    .into(),
            );
        }
        layers.push(
            canvas(Glass {
                spec,
                look,
                flash: self.flash_colour(),
            })
            .width(window.width)
            .height(window.height)
            .into(),
        );
        container(stack(layers).width(window.width).height(window.height))
            .center(Length::Fill)
            .into()
    }

    fn screen_view(&self) -> Element<'_, Message> {
        let spec = self.spec();
        let (w, h) = (spec.screen.width, spec.screen.height);
        let page = self.page();
        let room = self.list_room(&page);
        let pages = eink::paginate(&page.rows, room);
        let current = pages
            .iter()
            .position(|p| p.contains(&self.focus))
            .unwrap_or(0);
        let list_w = w - 2.0 * SIDE;
        let selecting = self.popup.is_none();
        let mut list = column![];
        if let Some(range) = pages.get(current) {
            for i in range.clone() {
                list = list.push(eink::row_view(
                    &page.rows[i],
                    selecting && i == self.focus,
                    list_w,
                    Message::Focus(i),
                ));
            }
        }
        if let Some(n) = &page.note {
            list = list.push(container(paragraph(n.clone(), Face::Ui, BODY)).padding([12.0, PAD]));
        }
        let multi = pages.len() > 1;
        let mut list_area: Element<'_, Message> = container(list)
            .padding([0.0, SIDE])
            .width(w)
            .height(if multi {
                Length::Fixed(room)
            } else {
                Length::Shrink
            })
            .into();
        if multi {
            list_area = stack![
                list_area,
                pin(canvas(eink::ScrollBar {
                    pages: pages.len(),
                    page: current,
                    on_page: Message::ShowPage,
                })
                .width(16)
                .height(room - 8.0))
                .x(w - 20.0)
                .y(4.0),
            ]
            .into();
        }
        let list_area = mouse_area(list_area).on_scroll(Message::Scroll);

        let status = self.status();
        let mut content = column![eink::header(
            &page.title,
            page.subtitle.as_deref(),
            page.back.then_some(Message::Back),
            status.as_deref(),
            &self.clock,
            [Message::Sleep, Message::CloseRequested],
            w,
        )];
        if page.top > 0.0 {
            content = content.push(container(self.top_view()).width(w).height(page.top));
        }
        content = content.push(list_area);
        if page.bottom > 0.0 {
            content = content.push(container(self.bottom_view()).width(w).height(page.bottom));
        }
        content = content.push(space().height(Length::Fill));
        if let Some(xs) = spec.hints {
            content = content.push(eink::hints(
                xs,
                page.hints,
                [Key::Back, Key::Confirm, Key::Left, Key::Right].map(|k| Some(Message::Press(k))),
            ));
        }

        let mut layers: Vec<Element<'_, Message>> = vec![
            image(eink::grain(w as u32, h as u32))
                .width(w)
                .height(h)
                .filter_method(image::FilterMethod::Nearest)
                .into(),
            content.width(w).height(h).into(),
        ];
        if let Some(p) = self.popup {
            layers.push(opaque(
                mouse_area(Space::new().width(w).height(h)).on_press(Message::ClosePopup),
            ));
            layers.push(
                pin(self.popup_view(p, w - 2.0 * SIDE))
                    .x(SIDE - 2.0)
                    .y((h * 0.165).round())
                    .into(),
            );
        }
        container(stack(layers).width(w).height(h))
            .style(|_| container::Style {
                background: Some(Background::Color(PAPER)),
                text_color: Some(INK),
                ..container::Style::default()
            })
            .into()
    }

    fn popup_view(&self, p: Popup, width: f32) -> Element<'_, Message> {
        let (title, words) = match p {
            Popup::Power => ("Power", None),
            Popup::TurnOffBusy => (
                "Still working",
                Some(self.status().unwrap_or_default() + ". Turning off now stops it."),
            ),
            Popup::StopConversion => (
                "Stop converting?",
                Some(
                    "The archive will not be finished. Converting again starts from the beginning."
                        .to_string(),
                ),
            ),
            Popup::Choose(Choice::Device) => ("Device", None),
            Popup::Choose(Choice::Case) => ("Case", None),
            Popup::Choose(Choice::Size) => ("Display size", None),
            Popup::Choose(Choice::Wiki) => ("Wiki", None),
            Popup::Choose(Choice::Pictures) => ("Pictures", None),
            Popup::Choose(Choice::Selection) => ("Selection", None),
            Popup::Choose(Choice::Articles) => ("Articles", None),
            Popup::Choose(Choice::Sort) => ("Sort by", None),
            Popup::Choose(Choice::Graphics) => (
                "Graphics",
                Some(
                    "How the window is drawn. A change takes effect when xwa-tool next starts."
                        .to_string(),
                ),
            ),
        };
        let inner = width - 2.0 * 16.0;
        let mut content = column![label(title.to_string(), Face::UiBold, TITLE)].spacing(8);
        if let Some(w) = words {
            content = content.push(paragraph(w, Face::Ui, BODY));
        }
        let rows = self.popup_rows(p);
        let mut list = column![];
        for (i, r) in rows.iter().enumerate() {
            list = list.push(eink::row_view(
                r,
                i == self.popup_focus,
                inner,
                Message::PopupFocus(i),
            ));
        }
        eink::popup(content.push(list).into(), width)
    }
}

impl App {
    /// The choices in a popup.
    pub fn popup_rows(&self, p: Popup) -> Vec<Row<Message>> {
        let pick = |title: String, chosen: bool, m: Message| {
            let row = Row::new(Kind::Item, title).on_press(m);
            if chosen { row.value(SELECTED) } else { row }
        };
        match p {
            Popup::Choose(Choice::Device) => Model::ALL
                .into_iter()
                .map(|m| pick(m.name().into(), m == self.prefs.model, Message::SetModel(m)))
                .collect(),
            Popup::Choose(Choice::Case) => Finish::ALL
                .into_iter()
                .map(|f| {
                    pick(
                        f.name().into(),
                        f == self.prefs.finish(),
                        Message::SetFinish(f),
                    )
                })
                .collect(),
            Popup::Choose(Choice::Wiki) => Wiki::ALL
                .into_iter()
                .map(|w| {
                    pick(w.name().into(), w == self.prefs.wiki, Message::SetWiki(w))
                        .subtitle(w.about())
                })
                .collect(),
            Popup::Choose(Choice::Pictures) => {
                let v = self.prefs.view;
                choices(v.pictures, |pictures| View { pictures, ..v })
            }
            Popup::Choose(Choice::Selection) => {
                let v = self.prefs.view;
                choices(v.selection, |selection| View { selection, ..v })
            }
            Popup::Choose(Choice::Articles) => {
                let v = self.prefs.view;
                choices(v.articles, |articles| View { articles, ..v })
            }
            Popup::Choose(Choice::Sort) => {
                let v = self.prefs.view;
                choices(v.sort, |sort| View { sort, ..v })
            }
            Popup::Choose(Choice::Graphics) => Graphics::choices()
                .into_iter()
                .map(|g| {
                    pick(
                        g.name().into(),
                        g == self.prefs.graphics,
                        Message::SetGraphics(g),
                    )
                })
                .collect(),
            Popup::Choose(Choice::Size) => {
                let max = self.max_zoom();
                let mut rows: Vec<_> = ZOOMS
                    .into_iter()
                    .filter(|&z| z <= max + 1e-3)
                    .map(|z| {
                        let chosen = self.prefs.zoom.is_some() && (z - self.zoom).abs() < 1e-3;
                        pick(
                            format!("{:.0}%", z * 100.0),
                            chosen,
                            Message::SetZoom(Some(z)),
                        )
                    })
                    .collect();
                rows.push(
                    pick(
                        "Fit to the screen".into(),
                        self.prefs.zoom.is_none(),
                        Message::SetZoom(None),
                    )
                    .subtitle(format!("{:.0}% here", max.min(1.0) * 100.0)),
                );
                rows
            }
            _ => popup_actions(p),
        }
    }
}

/// Rows for a filter's or the sort's choices, the one in use marked.
fn choices<C: library::Choice>(current: C, view: impl Fn(C) -> View) -> Vec<Row<Message>> {
    C::ALL
        .iter()
        .map(|&c| {
            let row = Row::new(Kind::Item, c.name()).on_press(Message::SetView(view(c)));
            if c == current {
                row.value(SELECTED)
            } else {
                row
            }
        })
        .collect()
}

/// The rows above the books on the library screen.
pub const LIBRARY_ROWS: usize = 3;

/// The value shown beside the choice in use, as the firmware marks it.
pub const SELECTED: &str = "Selected";

/// Where the window can be grabbed to resize it: its edges and corners,
/// in the room round the case where there is some, else the case's rim.
fn resize_handles(
    window: iced::Size,
) -> [(iced::Rectangle, iced::window::Direction, mouse::Interaction); 8] {
    use iced::window::Direction::*;
    use mouse::Interaction::*;
    let t = if crate::device::TRANSPARENT {
        MARGIN + 4.0
    } else {
        6.0
    };
    let c = 22.0;
    let (w, h) = (window.width, window.height);
    let r = |x: f32, y: f32, width: f32, height: f32| iced::Rectangle {
        x,
        y,
        width,
        height,
    };
    [
        (r(c, 0.0, w - 2.0 * c, t), North, ResizingVertically),
        (r(c, h - t, w - 2.0 * c, t), South, ResizingVertically),
        (r(0.0, c, t, h - 2.0 * c), West, ResizingHorizontally),
        (r(w - t, c, t, h - 2.0 * c), East, ResizingHorizontally),
        (r(0.0, 0.0, c, c), NorthWest, ResizingDiagonallyDown),
        (r(w - c, h - c, c, c), SouthEast, ResizingDiagonallyDown),
        (r(w - c, 0.0, c, c), NorthEast, ResizingDiagonallyUp),
        (r(0.0, h - c, c, c), SouthWest, ResizingDiagonallyUp),
    ]
}

/// The choices in a popup that asks what to do.
fn popup_actions(p: Popup) -> Vec<Row<Message>> {
    match p {
        Popup::Power => vec![
            Row::new(Kind::Item, "Sleep")
                .subtitle("Minimise the window")
                .on_press(Message::Sleep),
            Row::new(Kind::Item, "Turn off")
                .subtitle("Quit xwa-tool")
                .on_press(Message::CloseRequested),
            Row::new(Kind::Item, "Cancel").on_press(Message::ClosePopup),
        ],
        Popup::TurnOffBusy => vec![
            Row::new(Kind::Item, "Keep working").on_press(Message::ClosePopup),
            Row::new(Kind::Item, "Turn off anyway").on_press(Message::TurnOff),
        ],
        Popup::StopConversion => vec![
            Row::new(Kind::Item, "Keep converting").on_press(Message::ClosePopup),
            Row::new(Kind::Item, "Stop").on_press(Message::CancelConversion),
        ],
        Popup::Choose(_) => Vec::new(),
    }
}

fn drive_line(d: &xwa_fetch::drives::Drive) -> String {
    let mut line = format!("{} · {} free", d.mount.display(), size_text(d.free));
    if !d.file_system.is_empty() {
        line += &format!(" · {}", d.file_system);
    }
    line
}

/// At most `max` characters, cut at a word.
fn clip_words(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max).collect();
    match cut.rfind(' ') {
        Some(i) => format!("{}…", cut[..i].trim_end_matches([',', ';', ':', '.'])),
        None => format!("{cut}…"),
    }
}

fn search<'a>(field: Element<'a, Message>) -> Element<'a, Message> {
    container(field).padding([9.0, SIDE]).into()
}

fn note<'a>(s: &str) -> Element<'a, Message> {
    container(paragraph(s.to_string(), Face::Ui, BODY))
        .padding([10.0, SIDE + PAD])
        .into()
}

fn small_line<'a>(s: &str) -> Element<'a, Message> {
    label(s.to_string(), Face::Small, SMALL).into()
}

fn tip<'a>(s: &str) -> Element<'a, Message> {
    container(label(s.to_string(), Face::Small, SMALL))
        .padding([3.0, 8.0])
        .style(|_| container::Style {
            background: Some(Background::Color(PAPER)),
            text_color: Some(INK),
            border: Border {
                color: INK,
                width: 1.0,
                radius: 4.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

/// A cover in a black frame, the size of the firmware's thumbnails.
fn framed<'a>(content: Element<'a, Message>) -> Element<'a, Message> {
    container(content)
        .width(100)
        .height(132)
        .center_x(100)
        .center_y(132)
        .style(|_| container::Style {
            background: Some(Background::Color(PAPER)),
            border: Border {
                color: INK,
                width: 1.0,
                radius: 2.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

fn image_cover<'a>(handle: image::Handle) -> Element<'a, Message> {
    framed(
        image(handle)
            .width(96)
            .height(96)
            .filter_method(image::FilterMethod::Nearest)
            .into(),
    )
}

fn icon_cover<'a>(icon: &'static icons::Icon) -> Element<'a, Message> {
    framed(eink::icon(icon, 64.0))
}

/// What a running job says on the home tile.
fn job_lines<'a>(job: &Job, what: &str) -> Vec<Element<'a, Message>> {
    let fraction = job.fraction().unwrap_or(0.0);
    let mut lines = vec![
        small_line(what),
        row![
            eink::progress(fraction),
            label(percent(fraction), Face::UiBold, SMALL)
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into(),
    ];
    if let Some(left) = job.time_left() {
        lines.push(small_line(&time_left(left)));
    }
    lines
}

fn progress_height(steps: Option<usize>) -> f32 {
    20.0 + steps.map_or(0.0, |n| n as f32 * 30.0 + 8.0) + 22.0 + 10.0 + 28.0 + 20.0
}

/// A running job: its steps (for conversions), a bar, how far along it is
/// and how long is left.
fn progress_view<'a>(job: &Job, steps: Option<&[&str]>) -> Element<'a, Message> {
    let Job::Running {
        label: what,
        fraction,
        step,
        ..
    } = job
    else {
        return Space::new().into();
    };
    let mut col = column![].spacing(0).padding([10.0, SIDE + PAD]);
    if let Some(steps) = steps {
        for (i, name) in steps.iter().enumerate() {
            let (face, colour) = match i.cmp(step) {
                std::cmp::Ordering::Less => (Face::Ui, INK),
                std::cmp::Ordering::Equal => (Face::UiBold, INK),
                std::cmp::Ordering::Greater => (Face::Ui, DARK),
            };
            col = col.push(
                row![
                    step_mark(i.cmp(step)),
                    label(name.to_string(), face, BODY).color(colour)
                ]
                .spacing(12)
                .align_y(Alignment::Center)
                .height(30),
            );
        }
        col = col.push(space().height(8));
    }
    col = col.push(
        label(
            fit(what, Face::Small, SMALL, 400.0).into_owned(),
            Face::Small,
            SMALL,
        )
        .height(22),
    );
    col = col.push(space().height(4));
    col = col.push(
        row![
            eink::progress(*fraction),
            label(percent(*fraction), Face::UiBold, BODY)
        ]
        .spacing(10)
        .align_y(Alignment::Center),
    );
    let left = job
        .time_left()
        .map(time_left)
        .unwrap_or_else(|| "Working out how long it will take…".into());
    col = col.push(label(left, Face::Small, SMALL).height(28));
    col.into()
}

/// Done, doing, or still to do.
fn step_mark<'a>(state: std::cmp::Ordering) -> Element<'a, Message> {
    let (fill, border) = match state {
        std::cmp::Ordering::Less => (INK, INK),
        std::cmp::Ordering::Equal => (LIGHT, INK),
        std::cmp::Ordering::Greater => (PAPER, DARK),
    };
    container(Space::new())
        .width(12)
        .height(12)
        .style(move |_| container::Style {
            background: Some(Background::Color(fill)),
            border: Border {
                color: border,
                width: if state == std::cmp::Ordering::Equal {
                    2.0
                } else {
                    1.0
                },
                radius: 6.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

/// The ZIM file at the top of the Convert screen.
fn zim_tile<'a>(
    zim: &Path,
    info: Option<&Result<ZimInfo, String>>,
    w: f32,
) -> Element<'a, Message> {
    let room = w - 2.0 * SIDE - 100.0 - 16.0 - PAD;
    let (cover, words): (Element<'a, Message>, Element<'a, Message>) = match info {
        None => (image_cover(eink::wikipedia_cover()), small_line("Reading…")),
        Some(Err(e)) => (
            image_cover(eink::wikipedia_cover()),
            column![
                label(
                    fit(&file_name(zim), Face::UiBold, BODY, room).into_owned(),
                    Face::UiBold,
                    BODY
                ),
                paragraph(format!("Could not read this file: {e}"), Face::Small, SMALL),
            ]
            .spacing(6)
            .into(),
        ),
        Some(Ok(info)) => {
            let cover = match &info.cover {
                Some(c) => image_cover(c.clone()),
                None => image_cover(eink::wikipedia_cover()),
            };
            let mut facts = Vec::new();
            for s in [&info.language, &info.flavour, &info.date] {
                if !s.is_empty() {
                    facts.push(s.clone());
                }
            }
            (
                cover,
                column![
                    paragraph(info.title.clone(), Face::UiBold, TITLE),
                    small_line(&facts.join(" · ")),
                    small_line(&format!(
                        "{} entries · {}",
                        group(u64::from(info.entries)),
                        size_text(info.size)
                    )),
                ]
                .spacing(6)
                .into(),
            )
        }
    };
    container(
        row![cover, container(words).width(Length::Fill)]
            .spacing(16)
            .align_y(Alignment::Center),
    )
    .padding(Padding {
        top: 9.0,
        right: SIDE,
        bottom: 9.0,
        left: SIDE + PAD,
    })
    .into()
}
