//! The screen: an e-ink panel showing the firmware's Lyra theme. Sizes are
//! the firmware's, in screen pixels: a header 84 high with a 3 pixel rule,
//! list rows of 40 (60 with a second line) on a light grey pill when
//! selected, 20 pixels of side padding, labels for the front keys 80 × 40
//! along the bottom. Text is set in the firmware's fonts: Ubuntu at 10 and
//! 12 pt and Noto Sans at 8 pt, at the 150 dpi its font converter uses.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use iced::border::{self, Border};
use iced::widget::{
    Space, button, column, container, image, mouse_area, progress_bar, row, space, stack, text,
    text_input,
};
use iced::{Alignment, Background, Color, Element, Font, Length, Padding, Shadow, font};

use crate::icons::{self, Icon};

/// The panel's white, which on e-ink is a light warm grey.
pub const PAPER: Color = Color::from_rgb8(0xE8, 0xE7, 0xE2);
/// The panel's black.
pub const INK: Color = Color::from_rgb8(0x1F, 0x1F, 0x1E);
/// The firmware's light grey, for selections.
pub const LIGHT: Color = Color::from_rgb8(0xC2, 0xC1, 0xBC);
/// The firmware's dark grey, for what cannot be chosen.
pub const DARK: Color = Color::from_rgb8(0x6E, 0x6D, 0x69);
/// The unlit edge of the panel round the active area.
pub const PANEL_BORDER: Color = Color::from_rgb8(0xD6, 0xD5, 0xD0);

pub const UI: Font = Font::with_name("Ubuntu");
pub const UI_BOLD: Font = Font {
    weight: font::Weight::Bold,
    ..UI
};
pub const SMALL_FONT: Font = Font::with_name("Noto Sans");

pub const UBUNTU_REGULAR: &[u8] = include_bytes!("../fonts/Ubuntu-Regular.ttf");
pub const UBUNTU_BOLD: &[u8] = include_bytes!("../fonts/Ubuntu-Bold.ttf");
pub const NOTO_SANS: &[u8] = include_bytes!("../fonts/NotoSans-Regular.ttf");

/// Ubuntu 10 pt, for list rows and body text.
pub const BODY: f32 = 10.0 * 150.0 / 72.0;
/// Ubuntu 12 pt, for titles.
pub const TITLE: f32 = 12.0 * 150.0 / 72.0;
/// Noto Sans 8 pt, for second lines, labels and the status bar.
pub const SMALL: f32 = 8.0 * 150.0 / 72.0;

pub const SIDE: f32 = 20.0;
pub const PAD: f32 = 8.0;
pub const RADIUS: f32 = 6.0;
pub const HEADER: f32 = 84.0;
const STATUS: f32 = 40.0;
const RULE: f32 = 3.0;
pub const ROW: f32 = 40.0;
pub const ROW_TWO_LINES: f32 = 60.0;
pub const MENU_ROW: f32 = 64.0;
pub const MENU_GAP: f32 = 8.0;
pub const ACTION_ROW: f32 = 60.0;
pub const HINTS: f32 = 40.0;
const HINT_WIDTH: f32 = 80.0;
const ICON: f32 = 32.0;
const LIST_ICON: f32 = 24.0;

// ---------------------------------------------------------------------------
// Text

#[derive(Clone, Copy)]
pub enum Face {
    Ui,
    UiBold,
    Small,
}

impl Face {
    fn font(self) -> Font {
        match self {
            Face::Ui => UI,
            Face::UiBold => UI_BOLD,
            Face::Small => SMALL_FONT,
        }
    }
}

struct Faces {
    ui: ttf_parser::Face<'static>,
    ui_bold: ttf_parser::Face<'static>,
    small: ttf_parser::Face<'static>,
}

fn faces() -> &'static Faces {
    static FACES: OnceLock<Faces> = OnceLock::new();
    FACES.get_or_init(|| Faces {
        ui: ttf_parser::Face::parse(UBUNTU_REGULAR, 0).expect("bundled font"),
        ui_bold: ttf_parser::Face::parse(UBUNTU_BOLD, 0).expect("bundled font"),
        small: ttf_parser::Face::parse(NOTO_SANS, 0).expect("bundled font"),
    })
}

/// How wide `s` is set in `face` at `size`, from the font's advances. Letters
/// the font lacks come from a system font; they are counted as a full em
/// when wide (CJK) and 0.6 em otherwise.
pub fn width(s: &str, face: Face, size: f32) -> f32 {
    let f = match face {
        Face::Ui => &faces().ui,
        Face::UiBold => &faces().ui_bold,
        Face::Small => &faces().small,
    };
    let scale = size / f32::from(f.units_per_em());
    s.chars()
        .map(
            |c| match f.glyph_index(c).and_then(|g| f.glyph_hor_advance(g)) {
                Some(a) => f32::from(a) * scale,
                None if is_wide(c) => size,
                None => size * 0.6,
            },
        )
        .sum()
}

fn is_wide(c: char) -> bool {
    matches!(c as u32, 0x1100..=0x115F | 0x2E80..=0xA4CF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0xFE30..=0xFE4F | 0xFF00..=0xFF60 | 0x20000..=0x3FFFD)
}

/// `s` cut to fit `max` with an ellipsis, as the firmware truncates.
pub fn fit(s: &str, face: Face, size: f32, max: f32) -> Cow<'_, str> {
    if width(s, face, size) <= max {
        return Cow::Borrowed(s);
    }
    let room = max - width("…", face, size);
    let mut used = 0.0;
    let mut end = 0;
    for (i, c) in s.char_indices() {
        let w = width(c.encode_utf8(&mut [0; 4]), face, size);
        if used + w > room {
            break;
        }
        used += w;
        end = i + c.len_utf8();
    }
    Cow::Owned(format!("{}…", s[..end].trim_end()))
}

/// A line of text in the screen's fonts.
pub fn label<'a>(s: impl text::IntoFragment<'a>, face: Face, size: f32) -> text::Text<'a> {
    text(s)
        .font(face.font())
        .size(size)
        .line_height(text::LineHeight::Relative(1.25))
        .wrapping(text::Wrapping::None)
}

/// A paragraph that wraps.
pub fn paragraph<'a>(s: impl text::IntoFragment<'a>, face: Face, size: f32) -> text::Text<'a> {
    text(s)
        .font(face.font())
        .size(size)
        .line_height(text::LineHeight::Relative(1.3))
        .wrapping(text::Wrapping::WordOrGlyph)
}

// ---------------------------------------------------------------------------
// Pictures

/// An icon, scaled up four times pixel by pixel so it stays crisp when the
/// device is drawn at any size.
pub fn icon<'a, M: 'a>(icon: &'static Icon, size: f32) -> Element<'a, M> {
    image(icon_handle(icon))
        .width(size)
        .height(size)
        .filter_method(image::FilterMethod::Linear)
        .into()
}

fn icon_handle(icon: &'static Icon) -> image::Handle {
    static HANDLES: OnceLock<Mutex<HashMap<usize, image::Handle>>> = OnceLock::new();
    let mut handles = HANDLES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    handles
        .entry(icon as *const Icon as usize)
        .or_insert_with(|| {
            const UP: u32 = 4;
            let n = icon.size;
            let stride = n.div_ceil(8) as usize;
            let side = n * UP;
            let mut rgba = vec![0u8; (side * side * 4) as usize];
            for y in 0..side {
                for x in 0..side {
                    let (sx, sy) = ((x / UP) as usize, (y / UP) as usize);
                    let byte = icon.bits[sy * stride + sx / 8];
                    let ink = byte & (0x80 >> (sx % 8)) == 0;
                    if ink {
                        let i = ((y * side + x) * 4) as usize;
                        rgba[i..i + 4].copy_from_slice(&[0x1F, 0x1F, 0x1E, 0xFF]);
                    }
                }
            }
            image::Handle::from_rgba(side, side, rgba)
        })
        .clone()
}

/// The grain of the panel: faint specks of ink and of light, one per screen
/// pixel.
pub fn grain(width: u32, height: u32) -> image::Handle {
    static GRAINS: OnceLock<Mutex<HashMap<(u32, u32), image::Handle>>> = OnceLock::new();
    let mut grains = GRAINS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    grains
        .entry((width, height))
        .or_insert_with(|| {
            let mut state = 0x9E37_79B9_7F4A_7C15u64;
            let mut next = move || {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state
            };
            let mut rgba = vec![0u8; (width * height * 4) as usize];
            for px in rgba.as_chunks_mut::<4>().0 {
                let r = next();
                let strength = (r & 0xFF) as u8;
                // Most pixels carry a trace; a few are visibly darker.
                let alpha = match strength {
                    0..=200 => strength / 25,
                    201..=250 => 10 + (strength - 200) / 5,
                    _ => 24,
                };
                if (r >> 8) & 3 == 0 {
                    px.copy_from_slice(&[0xFF, 0xFF, 0xFF, alpha * 2]);
                } else {
                    px.copy_from_slice(&[0x1F, 0x1F, 0x1E, alpha]);
                }
            }
            image::Handle::from_rgba(width, height, rgba)
        })
        .clone()
}

/// A small picture (a ZIM file's 48 × 48 illustration) as the panel shows
/// it: over its white, in its four greys, each pixel drawn 2 × 2.
pub fn panel_picture(encoded: &[u8]) -> Option<image::Handle> {
    let img = ::image::load_from_memory(encoded).ok()?;
    let img = img.resize_exact(48, 48, ::image::imageops::FilterType::Triangle);
    let luma: Vec<u8> = img
        .to_rgba8()
        .pixels()
        .map(|p| {
            let [r, g, b, a] = p.0;
            let y = 0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b);
            let a = f32::from(a) / 255.0;
            (y * a + 255.0 * (1.0 - a)).round() as u8
        })
        .collect();
    Some(dithered(&luma, 48, 48, 2))
}

/// Wikipedia's globe, for when there is no ZIM file to show.
pub fn wikipedia_cover() -> image::Handle {
    static COVER: OnceLock<image::Handle> = OnceLock::new();
    COVER
        .get_or_init(|| {
            panel_picture(include_bytes!("../assets/wikipedia-48.png")).expect("bundled picture")
        })
        .clone()
}

/// A picture in the panel's four greys, dithered as the firmware shows
/// pictures. `luma` is one byte per pixel; each source pixel becomes
/// `scale` × `scale` screen pixels before dithering.
pub fn dithered(luma: &[u8], w: u32, h: u32, scale: u32) -> image::Handle {
    const BAYER: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
    let levels = [INK, DARK, LIGHT, PAPER].map(|c| c.into_rgba8());
    let (ow, oh) = (w * scale, h * scale);
    let mut rgba = vec![0u8; (ow * oh * 4) as usize];
    for y in 0..oh {
        for x in 0..ow {
            let l = f32::from(luma[((y / scale) * w + x / scale) as usize]) / 255.0;
            let threshold = (f32::from(BAYER[(y % 4) as usize][(x % 4) as usize]) + 0.5) / 16.0;
            let level = ((l * 3.0 + threshold - 0.5).round()).clamp(0.0, 3.0) as usize;
            let i = ((y * ow + x) * 4) as usize;
            rgba[i..i + 4].copy_from_slice(&levels[level]);
        }
    }
    image::Handle::from_rgba(ow, oh, rgba)
}

// ---------------------------------------------------------------------------
// Rows

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A big row with an icon on the home screen.
    Menu,
    /// A list row, which may open something.
    Item,
    /// A setting: its name, and its value on the right.
    Setting,
    /// The thing to do on a screen, drawn as a button.
    Action,
    /// A fact, which cannot be chosen.
    Info,
}

#[derive(Debug, Clone)]
pub struct Row<M> {
    pub kind: Kind,
    pub icon: Option<&'static Icon>,
    pub title: String,
    pub subtitle: Option<String>,
    pub value: Option<String>,
    /// Opens another screen.
    pub chevron: bool,
    /// `None` shows it greyed out.
    pub on_press: Option<M>,
}

impl<M> Row<M> {
    pub fn new(kind: Kind, title: impl Into<String>) -> Self {
        Row {
            kind,
            icon: None,
            title: title.into(),
            subtitle: None,
            value: None,
            chevron: false,
            on_press: None,
        }
    }

    pub fn icon(mut self, icon: &'static Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn subtitle(mut self, s: impl Into<String>) -> Self {
        self.subtitle = Some(s.into());
        self
    }

    pub fn value(mut self, s: impl Into<String>) -> Self {
        self.value = Some(s.into());
        self
    }

    pub fn opens(mut self) -> Self {
        self.chevron = true;
        self
    }

    pub fn on_press(mut self, m: M) -> Self {
        self.on_press = Some(m);
        self
    }

    pub fn on_press_maybe(mut self, m: Option<M>) -> Self {
        self.on_press = m;
        self
    }

    pub fn height(&self) -> f32 {
        match self.kind {
            Kind::Menu => MENU_ROW + MENU_GAP,
            Kind::Action => ACTION_ROW,
            _ if self.subtitle.is_some() => ROW_TWO_LINES,
            _ => ROW,
        }
    }

    /// Whether the selection can rest on it.
    pub fn focusable(&self) -> bool {
        self.kind != Kind::Info && self.on_press.is_some()
    }
}

/// Splits rows into pages that fit `room`, as the firmware pages its lists.
pub fn paginate<M>(rows: &[Row<M>], room: f32) -> Vec<std::ops::Range<usize>> {
    let mut pages = Vec::new();
    let mut start = 0;
    let mut used = 0.0;
    for (i, r) in rows.iter().enumerate() {
        let h = r.height();
        if used + h > room + 0.5 && i > start {
            pages.push(start..i);
            start = i;
            used = 0.0;
        }
        used += h;
    }
    if start < rows.len() || pages.is_empty() {
        pages.push(start..rows.len());
    }
    pages
}

/// A row of a list, `width` wide.
pub fn row_view<'a, M: Clone + 'a>(
    r: &Row<M>,
    selected: bool,
    width: f32,
    on_hover: M,
) -> Element<'a, M> {
    let enabled = r.on_press.is_some() || r.kind == Kind::Info;
    let content: Element<'a, M> = match r.kind {
        Kind::Action => {
            let face = Face::UiBold;
            let title = fit(&r.title, face, BODY, width - 2.0 * SIDE);
            container(label(title.into_owned(), face, BODY))
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .into()
        }
        Kind::Menu => menu_content(r, width),
        _ => list_content(r, width),
    };
    let kind = r.kind;
    let height = match kind {
        Kind::Menu => MENU_ROW,
        Kind::Action => ACTION_ROW - 16.0,
        _ => r.height(),
    };
    let body = button(content)
        .width(width)
        .height(height)
        .padding(0)
        .on_press_maybe(r.on_press.clone())
        .style(move |_, status| row_style(kind, selected, enabled, status));
    let body: Element<'a, M> = if kind == Kind::Action {
        container(body).padding([8, 0]).into()
    } else if kind == Kind::Menu {
        container(body)
            .padding(Padding::default().bottom(MENU_GAP))
            .into()
    } else {
        body.into()
    };
    if r.focusable() {
        // Only a moving pointer takes the selection, so a pointer resting
        // where a new screen puts a row does not steal it from the keys.
        mouse_area(body).on_move(move |_| on_hover.clone()).into()
    } else {
        body
    }
}

fn row_style(kind: Kind, selected: bool, enabled: bool, status: button::Status) -> button::Style {
    let pressed = status == button::Status::Pressed;
    let ink = if enabled { INK } else { DARK };
    match kind {
        Kind::Action => {
            let filled = (selected || pressed) && enabled;
            button::Style {
                background: Some(Background::Color(if filled { ink } else { PAPER })),
                text_color: if filled { PAPER } else { ink },
                border: Border {
                    color: ink,
                    width: 2.0,
                    radius: RADIUS.into(),
                },
                shadow: Shadow::default(),
                snap: true,
            }
        }
        _ => button::Style {
            // A tap flashes the row black, as the firmware's touch screens do.
            background: if pressed && enabled {
                Some(Background::Color(INK))
            } else if selected && enabled {
                Some(Background::Color(LIGHT))
            } else {
                None
            },
            text_color: if pressed && enabled { PAPER } else { ink },
            border: Border {
                radius: RADIUS.into(),
                ..Border::default()
            },
            shadow: Shadow::default(),
            snap: true,
        },
    }
}

fn list_content<'a, M: 'a>(r: &Row<M>, width: f32) -> Element<'a, M> {
    let mut room = width - 2.0 * PAD;
    let mut line = row![].align_y(Alignment::Center).spacing(PAD);
    if let Some(i) = r.icon {
        line = line.push(icon(i, LIST_ICON));
        room -= LIST_ICON + PAD;
    }
    let value = r
        .value
        .as_deref()
        .map(|v| fit(v, Face::Ui, BODY, 200.0).into_owned());
    if let Some(v) = &value {
        room -= width_of(v, Face::Ui, BODY) + PAD;
    }
    if r.chevron {
        room -= LIST_ICON + PAD;
    }
    let title = fit(&r.title, Face::Ui, BODY, room).into_owned();
    let words: Element<'a, M> = match &r.subtitle {
        Some(s) => {
            let s = fit(s, Face::Small, SMALL, room).into_owned();
            column![label(title, Face::Ui, BODY), label(s, Face::Small, SMALL)]
                .spacing(1)
                .into()
        }
        None => label(title, Face::Ui, BODY).into(),
    };
    line = line.push(container(words).width(Length::Fill));
    if let Some(v) = value {
        line = line.push(label(v, Face::Ui, BODY));
    }
    if r.chevron {
        line = line.push(icon(&icons::NEXT, LIST_ICON));
    }
    container(line)
        .padding([0.0, PAD])
        .center_y(Length::Fill)
        .width(Length::Fill)
        .into()
}

fn width_of(s: &str, face: Face, size: f32) -> f32 {
    width(s, face, size).ceil()
}

fn menu_content<'a, M: 'a>(r: &Row<M>, width: f32) -> Element<'a, M> {
    let room = width - 16.0 - ICON - PAD - 2.0 - 16.0;
    let title = fit(&r.title, Face::Ui, TITLE, room).into_owned();
    let mut words = column![label(title, Face::Ui, TITLE)].spacing(0);
    if let Some(s) = &r.subtitle {
        words = words.push(label(
            fit(s, Face::Small, SMALL, room).into_owned(),
            Face::Small,
            SMALL,
        ));
    }
    let mut line = row![].align_y(Alignment::Center);
    if let Some(i) = r.icon {
        line = line.push(icon(i, ICON)).push(space().width(PAD + 2.0));
    }
    container(line.push(words))
        .padding(Padding::default().left(16.0))
        .center_y(Length::Fill)
        .width(Length::Fill)
        .into()
}

// ---------------------------------------------------------------------------
// Chrome

/// The header: a status line (what is running on the left, the time on the
/// right), the title in bold with its subtitle on the right, and the rule.
pub fn header<'a, M: Clone + 'a>(
    title: &str,
    subtitle: Option<&str>,
    back: Option<M>,
    status: Option<&str>,
    clock: &str,
    window: [M; 2],
    width: f32,
) -> Element<'a, M> {
    // The time on the left, what is running beside it, and the window's
    // minimise and close on the right.
    let [minimise, close] = window;
    let room = width - 2.0 * SIDE - width_of(clock, Face::Small, SMALL) - 12.0 - 2.0 * 34.0;
    let status_line = row![
        label(clock.to_string(), Face::Small, SMALL),
        space().width(12),
        label(
            status
                .map(|s| fit(s, Face::Small, SMALL, room).into_owned())
                .unwrap_or_default(),
            Face::Small,
            SMALL
        ),
        space().width(Length::Fill),
        glyph_button("–", minimise),
        glyph_button("×", close),
    ]
    .align_y(Alignment::Center)
    .padding(Padding {
        top: 0.0,
        right: 6.0,
        bottom: 4.0,
        left: SIDE,
    })
    .height(STATUS);

    let subtitle_text =
        subtitle.map(|s| fit(s, Face::Small, SMALL, (width - 3.0 * SIDE) / 2.0).into_owned());
    let mut room = width - 2.0 * SIDE;
    if let Some(s) = &subtitle_text {
        room -= width_of(s, Face::Small, SMALL) + SIDE;
    }
    let mut title_line = row![]
        .align_y(Alignment::Center)
        .height(HEADER - STATUS - RULE);
    match back {
        Some(back) => {
            room -= 28.0;
            title_line = title_line.push(space().width(SIDE - 14.0)).push(
                button(icon(&icons::BACK, LIST_ICON))
                    .padding(4)
                    .on_press(back)
                    .style(|_, status| button::Style {
                        background: match status {
                            button::Status::Pressed => Some(Background::Color(INK)),
                            button::Status::Hovered => Some(Background::Color(LIGHT)),
                            _ => None,
                        },
                        border: Border {
                            radius: RADIUS.into(),
                            ..Border::default()
                        },
                        ..button::Style::default()
                    }),
            );
            title_line = title_line.push(space().width(2));
        }
        None => title_line = title_line.push(space().width(SIDE)),
    }
    title_line = title_line.push(
        container(label(
            fit(title, Face::UiBold, TITLE, room).into_owned(),
            Face::UiBold,
            TITLE,
        ))
        .width(Length::Fill),
    );
    if let Some(s) = subtitle_text {
        title_line = title_line
            .push(label(s, Face::Small, SMALL))
            .push(space().width(SIDE));
    } else {
        title_line = title_line.push(space().width(SIDE));
    }

    column![status_line, title_line, rule(RULE)]
        .width(width)
        .height(HEADER)
        .into()
}

/// A small button showing a glyph, which inverts when pressed like a tapped
/// row.
fn glyph_button<'a, M: Clone + 'a>(glyph: &str, on_press: M) -> Element<'a, M> {
    button(
        container(label(glyph.to_string(), Face::Ui, TITLE))
            .center_x(Length::Fill)
            .center_y(Length::Fill),
    )
    .width(34)
    .height(30)
    .padding(0)
    .on_press(on_press)
    .style(|_, status| button::Style {
        background: match status {
            button::Status::Pressed => Some(Background::Color(INK)),
            button::Status::Hovered => Some(Background::Color(LIGHT)),
            _ => None,
        },
        text_color: if status == button::Status::Pressed {
            PAPER
        } else {
            INK
        },
        border: Border {
            radius: RADIUS.into(),
            ..Border::default()
        },
        ..button::Style::default()
    })
    .into()
}

/// A full-width black line.
pub fn rule<'a, M: 'a>(thickness: f32) -> Element<'a, M> {
    container(Space::new())
        .width(Length::Fill)
        .height(thickness)
        .style(|_| container::Style::default().background(INK))
        .into()
}

/// The labels of the four front keys along the bottom of the screen, which
/// can be clicked like the keys under them.
pub fn hints<'a, M: Clone + 'a>(
    xs: [f32; 4],
    labels: [&str; 4],
    presses: [Option<M>; 4],
) -> Element<'a, M> {
    let mut line = row![].align_y(Alignment::End).height(HINTS);
    let mut x = 0.0;
    for ((left, text_), press) in xs.into_iter().zip(labels).zip(presses) {
        line = line.push(space().width(left - x));
        x = left + HINT_WIDTH;
        let full = !text_.is_empty();
        let tab = button(
            container(label(
                fit(text_, Face::Small, SMALL, HINT_WIDTH - 6.0).into_owned(),
                Face::Small,
                SMALL,
            ))
            .center_x(Length::Fill)
            .padding(Padding::default().top(7.0)),
        )
        .width(HINT_WIDTH)
        .height(if full { HINTS } else { 15.0 })
        .padding(0)
        .on_press_maybe(if full { press } else { None })
        .style(|_, status| {
            let pressed = status == button::Status::Pressed;
            button::Style {
                background: Some(Background::Color(if pressed { INK } else { PAPER })),
                text_color: if pressed { PAPER } else { INK },
                border: Border {
                    color: INK,
                    width: 1.0,
                    radius: border::top(RADIUS),
                },
                shadow: Shadow::default(),
                snap: true,
            }
        });
        line = line.push(tab);
    }
    line.into()
}

/// The scroll mark beside a list of several pages: a line, and a block
/// showing which part of it is on screen.
pub fn scroll_mark<'a, M: 'a>(pages: usize, page: usize, height: f32) -> Element<'a, M> {
    let pages = pages.max(1);
    let thumb = (height / pages as f32).max(12.0);
    let top = if pages > 1 {
        (height - thumb) * page as f32 / (pages - 1) as f32
    } else {
        0.0
    };
    let bar = |w: f32, h: f32| {
        container(Space::new())
            .width(w)
            .height(h)
            .style(|_| container::Style::default().background(INK))
    };
    row![
        column![space().height(top), bar(4.0, thumb)],
        bar(1.0, height)
    ]
    .into()
}

/// A dialog in a black frame, as the firmware draws popups.
pub fn popup<'a, M: 'a>(content: Element<'a, M>, width: f32) -> Element<'a, M> {
    container(
        container(content)
            .width(width)
            .padding([12.0, 16.0])
            .style(|_| container::Style {
                background: Some(Background::Color(PAPER)),
                border: Border {
                    color: INK,
                    width: 2.0,
                    radius: RADIUS.into(),
                },
                ..container::Style::default()
            }),
    )
    .padding(2)
    .style(|_| container::Style {
        background: Some(Background::Color(PAPER)),
        border: Border {
            radius: (RADIUS + 2.0).into(),
            ..Border::default()
        },
        ..container::Style::default()
    })
    .into()
}

/// A progress bar in the firmware's style: a black outline filling with
/// black.
pub fn progress<'a, M: 'a>(fraction: f32) -> Element<'a, M> {
    progress_bar(0.0..=1.0, fraction.clamp(0.0, 1.0))
        .length(Length::Fill)
        .girth(10)
        .style(|_| progress_bar::Style {
            background: Background::Color(PAPER),
            bar: Background::Color(INK),
            border: Border {
                color: INK,
                width: 1.0,
                radius: 5.0.into(),
            },
        })
        .into()
}

/// A text field: a box with a black outline, thicker while typing.
pub fn field<'a, M: Clone + 'a>(
    placeholder: &str,
    value: &str,
    on_input: impl Fn(String) -> M + 'a,
    on_submit: Option<M>,
    id: &'static str,
) -> Element<'a, M> {
    let input = text_input(placeholder, value)
        .id(id)
        .on_input(on_input)
        .on_submit_maybe(on_submit)
        .font(UI)
        .size(BODY)
        .padding(Padding {
            top: 5.0,
            right: PAD,
            bottom: 5.0,
            left: PAD + LIST_ICON + PAD,
        })
        .style(|_, status| {
            let focused = matches!(status, text_input::Status::Focused { .. });
            text_input::Style {
                background: Background::Color(PAPER),
                border: Border {
                    color: INK,
                    width: if focused { 2.0 } else { 1.0 },
                    radius: RADIUS.into(),
                },
                icon: INK,
                placeholder: DARK,
                value: INK,
                selection: LIGHT,
            }
        });
    stack![
        input,
        container(icon(&icons::SEARCH, LIST_ICON))
            .padding(Padding::default().left(PAD))
            .center_y(Length::Fill),
    ]
    .into()
}
