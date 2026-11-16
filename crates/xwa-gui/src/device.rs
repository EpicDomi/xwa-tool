//! The e-reader drawn round the screen: its case, the glass over the
//! display, and its keys, which work.
//!
//! Everything is measured in the device's own screen pixels, so the window
//! shows the screen at its real resolution, scaled by the zoom. The cases
//! are measured from Xteink's product photos against each display's known
//! resolution:
//!
//! - X4 Pro: a 480 × 800 display at 219 ppi in a 69 × 111 mm case, a page
//!   key on each side, the power key above the right one, and a round touch
//!   Home key under the screen.
//! - X4: the same display in a taller case, the power key and a page rocker
//!   on the right, and two rockers (Back and Confirm, Left and Right) low on
//!   the face, under the labels the firmware draws for them.
//! - X3: a 528 × 792 display at about 250 ppi, a page key on each side, the
//!   power key on top, and two rockers in a tray under the screen.

use iced::widget::canvas::{self, Fill, Frame, Geometry, Path, Stroke, fill, gradient, path};
use iced::widget::tooltip;
use iced::{Color, Point, Rectangle, Renderer, Size, Theme, Vector, mouse};

/// Whether windows can be transparent here. Windows cannot show a
/// transparent window with iced's renderers, so there the case fills a
/// rectangular window, which the system rounds, and the keys stay within it.
pub const TRANSPARENT: bool = !cfg!(target_os = "windows");

/// Room round the case for keys that stick out of it.
pub const MARGIN: f32 = if TRANSPARENT { 6.0 } else { 0.0 };

/// How far a key on an edge stands out of the case.
const STANDOUT: f32 = 4.0;

/// How far into the case a key on an edge can be clicked: its edge alone is
/// too thin to aim at.
const EDGE_REACH: f32 = 26.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Model {
    #[default]
    X4Pro,
    X4,
    X3,
}

impl Model {
    pub const ALL: [Model; 3] = [Model::X4Pro, Model::X4, Model::X3];

    pub fn name(self) -> &'static str {
        self.converter().name()
    }

    pub fn short_name(self) -> &'static str {
        match self {
            Model::X4Pro => "X4 Pro",
            Model::X4 => "X4",
            Model::X3 => "X3",
        }
    }

    /// How the model is written in the preferences file.
    pub fn key(self) -> &'static str {
        match self {
            Model::X4Pro => "x4pro",
            Model::X4 => "x4",
            Model::X3 => "x3",
        }
    }

    pub fn converter(self) -> xwa_convert::Device {
        match self {
            Model::X4Pro => xwa_convert::Device::X4Pro,
            Model::X4 => xwa_convert::Device::X4,
            Model::X3 => xwa_convert::Device::X3,
        }
    }

    pub fn usual_finish(self) -> Finish {
        match self {
            Model::X4Pro => Finish::Black,
            Model::X4 | Model::X3 => Finish::White,
        }
    }

    pub fn spec(self) -> &'static Spec {
        match self {
            Model::X4Pro => &X4_PRO,
            Model::X4 => &X4,
            Model::X3 => &X3,
        }
    }

    /// The case's colours, sampled from the photos: each model's black and
    /// white differ (the X4 Pro's white is a cool grey, the X3's a blue grey).
    pub fn look(self, finish: Finish) -> &'static Look {
        match (self, finish) {
            (Model::X4Pro, Finish::Black) => &X4_PRO_BLACK,
            (Model::X4Pro, Finish::White) => &X4_PRO_WHITE,
            (Model::X4, Finish::Black) => &X4_BLACK,
            (Model::X4, Finish::White) => &X4_WHITE,
            (Model::X3, Finish::Black) => &X3_BLACK,
            (Model::X3, Finish::White) => &X3_WHITE,
        }
    }
}

/// The colour of the case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Finish {
    Black,
    White,
}

impl Finish {
    pub const ALL: [Finish; 2] = [Finish::Black, Finish::White];

    pub fn key(self) -> &'static str {
        match self {
            Finish::Black => "black",
            Finish::White => "white",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Finish::Black => "Black",
            Finish::White => "White",
        }
    }
}

/// A physical control. `Up` and `Down` are the page keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Power,
    Home,
    Back,
    Confirm,
    Left,
    Right,
    Up,
    Down,
}

impl Key {
    /// What it does here, with the keyboard key that does the same.
    pub fn tooltip(self) -> &'static str {
        match self {
            Key::Power => "Power: sleep or turn off",
            Key::Home => "Home",
            Key::Back => "Back (Esc)",
            Key::Confirm => "Select (Enter)",
            Key::Left => "Up (↑)",
            Key::Right => "Down (↓)",
            Key::Up => "Previous page (Page Up)",
            Key::Down => "Next page (Page Down)",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Top,
}

/// Where a key is, in case coordinates.
#[derive(Debug, Clone, Copy)]
pub enum Place {
    /// On an edge, from one point along it to another.
    Edge(Side, f32, f32),
    /// Half of a pill-shaped rocker on the face.
    Face(Rectangle),
    /// A round touch key on the face: its middle and diameter.
    Round(Point, f32),
}

#[derive(Debug, Clone, Copy)]
pub struct KeySpec {
    pub key: Key,
    pub place: Place,
    /// The two halves of a rocker share a nonzero number.
    pub rocker: u8,
}

impl KeySpec {
    /// Where clicking presses the key, in window coordinates.
    pub fn hit(&self, spec: &Spec) -> Rectangle {
        let b = spec.body;
        let r = match self.place {
            Place::Edge(Side::Left, from, to) => Rectangle::new(
                Point::new(-MARGIN, from),
                Size::new(MARGIN + EDGE_REACH, to - from),
            ),
            Place::Edge(Side::Right, from, to) => Rectangle::new(
                Point::new(b.width - EDGE_REACH, from),
                Size::new(EDGE_REACH + MARGIN, to - from),
            ),
            Place::Edge(Side::Top, from, to) => Rectangle::new(
                Point::new(from, -MARGIN),
                Size::new(to - from, MARGIN + EDGE_REACH),
            ),
            Place::Face(r) => Rectangle::new(
                Point::new(r.x, r.y - 6.0),
                Size::new(r.width, r.height + 12.0),
            ),
            Place::Round(c, d) => Rectangle::new(
                Point::new(c.x - d / 2.0 - 4.0, c.y - d / 2.0 - 4.0),
                Size::new(d + 8.0, d + 8.0),
            ),
        };
        Rectangle::new(r.position() + Vector::new(MARGIN, MARGIN), r.size())
    }

    /// Where its tooltip goes: inwards, onto the device.
    pub fn tip_position(&self) -> tooltip::Position {
        match self.place {
            Place::Edge(Side::Left, ..) => tooltip::Position::Right,
            Place::Edge(Side::Right, ..) => tooltip::Position::Left,
            Place::Edge(Side::Top, ..) => tooltip::Position::Bottom,
            Place::Face(_) | Place::Round(..) => tooltip::Position::Top,
        }
    }
}

pub struct Spec {
    /// The case, in screen pixels.
    pub body: Size,
    pub radius: f32,
    /// The display's active area, in case coordinates.
    pub screen: Rectangle,
    /// How round the opening over the display is.
    pub opening_radius: f32,
    /// The display's unlit border showing round the active area.
    pub panel_border: f32,
    /// Where the face meets the rim: insets from the case's edge (top,
    /// right, bottom, left).
    pub face: [f32; 4],
    pub face_radius: f32,
    /// A tray cut into the bottom of the face for the front keys.
    pub tray: Option<Rectangle>,
    pub keys: &'static [KeySpec],
    /// Left edges of the four front keys' labels along the bottom of the
    /// screen (screen coordinates), as the firmware draws them, on models
    /// with front keys.
    pub hints: Option<[f32; 4]>,
}

impl Spec {
    /// The window: the case and the room round it.
    pub fn window(&self) -> Size {
        Size::new(
            self.body.width + 2.0 * MARGIN,
            self.body.height + 2.0 * MARGIN,
        )
    }

    /// The screen, in window coordinates.
    pub fn screen_in_window(&self) -> Rectangle {
        Rectangle::new(
            self.screen.position() + Vector::new(MARGIN, MARGIN),
            self.screen.size(),
        )
    }
}

const fn rect(x: f32, y: f32, w: f32, h: f32) -> Rectangle {
    Rectangle {
        x,
        y,
        width: w,
        height: h,
    }
}

const fn key(key: Key, place: Place, rocker: u8) -> KeySpec {
    KeySpec { key, place, rocker }
}

static X4_PRO: Spec = Spec {
    body: Size::new(596.0, 963.0),
    radius: 27.0,
    screen: rect(57.0, 49.0, 480.0, 800.0),
    opening_radius: 8.0,
    panel_border: 0.0,
    face: [12.0, 12.0, 12.0, 12.0],
    face_radius: 16.0,
    tray: None,
    keys: &[
        key(Key::Power, Place::Edge(Side::Right, 89.0, 149.0), 0),
        key(Key::Up, Place::Edge(Side::Left, 222.0, 354.0), 0),
        key(Key::Down, Place::Edge(Side::Right, 222.0, 354.0), 0),
        key(Key::Home, Place::Round(Point::new(298.0, 902.0), 68.0), 0),
    ],
    hints: None,
};

static X4: Spec = Spec {
    body: Size::new(598.0, 1012.0),
    radius: 22.0,
    screen: rect(59.0, 53.0, 480.0, 800.0),
    opening_radius: 0.0,
    panel_border: 3.0,
    face: [7.0, 7.0, 20.0, 7.0],
    face_radius: 15.0,
    tray: Some(rect(91.0, 961.0, 414.0, 40.0)),
    keys: &[
        key(Key::Back, Place::Face(rect(103.0, 972.0, 94.0, 22.0)), 1),
        key(Key::Confirm, Place::Face(rect(197.0, 972.0, 94.0, 22.0)), 1),
        key(Key::Left, Place::Face(rect(306.0, 972.0, 94.0, 22.0)), 2),
        key(Key::Right, Place::Face(rect(400.0, 972.0, 93.0, 22.0)), 2),
        key(Key::Power, Place::Edge(Side::Right, 166.0, 250.0), 0),
        key(Key::Up, Place::Edge(Side::Right, 387.0, 475.0), 3),
        key(Key::Down, Place::Edge(Side::Right, 475.0, 563.0), 3),
    ],
    hints: Some([58.0, 146.0, 254.0, 342.0]),
};

static X3: Spec = Spec {
    body: Size::new(642.0, 984.0),
    radius: 25.0,
    screen: rect(55.0, 51.0, 528.0, 792.0),
    opening_radius: 0.0,
    panel_border: 2.0,
    face: [9.0, 9.0, 9.0, 9.0],
    face_radius: 16.0,
    tray: Some(rect(96.0, 891.0, 443.0, 90.0)),
    keys: &[
        key(Key::Back, Place::Face(rect(110.0, 904.0, 96.0, 18.0)), 1),
        key(Key::Confirm, Place::Face(rect(206.0, 904.0, 97.0, 18.0)), 1),
        key(Key::Left, Place::Face(rect(331.0, 904.0, 97.0, 18.0)), 2),
        key(Key::Right, Place::Face(rect(428.0, 904.0, 97.0, 18.0)), 2),
        key(Key::Up, Place::Edge(Side::Left, 166.0, 324.0), 0),
        key(Key::Down, Place::Edge(Side::Right, 166.0, 324.0), 0),
        key(Key::Power, Place::Edge(Side::Top, 478.0, 555.0), 0),
    ],
    hints: Some([65.0, 157.0, 291.0, 383.0]),
};

/// The colours of a case.
pub struct Look {
    rim_top: Color,
    rim_bottom: Color,
    face_top: Color,
    face_bottom: Color,
    seam: Color,
    /// Light caught along the top of the rim.
    light: Color,
    key: Color,
    key_light: Color,
    key_dark: Color,
    pill: Color,
    pill_edge: Color,
    pill_light: Color,
    home: Color,
    home_edge: Color,
}

const fn rgb(hex: u32) -> Color {
    Color::from_rgb8((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

const fn white(a: f32) -> Color {
    Color::from_rgba(1.0, 1.0, 1.0, a)
}

static X4_PRO_BLACK: Look = Look {
    rim_top: rgb(0x1A1A1C),
    rim_bottom: rgb(0x0B0B0C),
    face_top: rgb(0x2D2D31),
    face_bottom: rgb(0x212123),
    seam: rgb(0x0E0E10),
    light: white(0.16),
    key: rgb(0x303033),
    key_light: white(0.20),
    key_dark: rgb(0x0A0A0B),
    pill: rgb(0x303033),
    pill_edge: rgb(0x0A0A0B),
    pill_light: white(0.16),
    home: rgb(0x37383C),
    home_edge: rgb(0x18181A),
};

static X4_PRO_WHITE: Look = Look {
    rim_top: rgb(0xCDD0D2),
    rim_bottom: rgb(0xB2B5B8),
    face_top: rgb(0xC3C7CA),
    face_bottom: rgb(0xB6BABE),
    seam: rgb(0xA0A4A8),
    light: white(0.75),
    key: rgb(0x9DA2A6),
    key_light: white(0.45),
    key_dark: rgb(0x7B8084),
    pill: rgb(0xB6BABE),
    pill_edge: rgb(0x8E9397),
    pill_light: white(0.6),
    home: rgb(0xAEB2B6),
    home_edge: rgb(0x9B9FA3),
};

static X4_BLACK: Look = Look {
    rim_top: rgb(0x3C3C3F),
    rim_bottom: rgb(0x1B1B1D),
    face_top: rgb(0x18181B),
    face_bottom: rgb(0x0E0E10),
    seam: rgb(0x3A3A3D),
    light: white(0.28),
    key: rgb(0x202023),
    key_light: white(0.24),
    key_dark: rgb(0x050506),
    pill: rgb(0x232326),
    pill_edge: rgb(0x050506),
    pill_light: white(0.22),
    home: rgb(0x232326),
    home_edge: rgb(0x050506),
};

static X4_WHITE: Look = Look {
    rim_top: rgb(0xE8E9E8),
    rim_bottom: rgb(0xCDCFCE),
    face_top: rgb(0xE3E5E4),
    face_bottom: rgb(0xD2D4D3),
    seam: rgb(0xB6B8B7),
    light: white(0.9),
    key: rgb(0xD3D5D4),
    key_light: white(0.8),
    key_dark: rgb(0xA6A8A7),
    pill: rgb(0xD9DBDA),
    pill_edge: rgb(0x9C9E9D),
    pill_light: white(0.85),
    home: rgb(0xD9DBDA),
    home_edge: rgb(0x9C9E9D),
};

static X3_BLACK: Look = Look {
    rim_top: rgb(0x303030),
    rim_bottom: rgb(0x212121),
    face_top: rgb(0x3B3B3B),
    face_bottom: rgb(0x303030),
    seam: rgb(0x1D1D1D),
    light: white(0.13),
    key: rgb(0x2B2B2B),
    key_light: white(0.16),
    key_dark: rgb(0x141414),
    pill: rgb(0x343434),
    pill_edge: rgb(0x191919),
    pill_light: white(0.13),
    home: rgb(0x343434),
    home_edge: rgb(0x191919),
};

static X3_WHITE: Look = Look {
    rim_top: rgb(0xC3CBD1),
    rim_bottom: rgb(0xA9B1B8),
    face_top: rgb(0xBBC1C8),
    face_bottom: rgb(0xACB2B9),
    seam: rgb(0x949BA2),
    light: white(0.55),
    key: rgb(0xA7AEB5),
    key_light: white(0.5),
    key_dark: rgb(0x878F96),
    pill: rgb(0xBAC0C6),
    pill_edge: rgb(0x939AA1),
    pill_light: white(0.6),
    home: rgb(0xBAC0C6),
    home_edge: rgb(0x939AA1),
};

/// A key's state, as drawn.
#[derive(Clone, Copy, PartialEq, Eq)]
enum KeyLook {
    Rest,
    Hovered,
    Pressed,
}

/// The case, drawn behind the screen. It only draws: the keys and the drag
/// area are widgets laid over it.
pub struct Body {
    pub spec: &'static Spec,
    pub look: &'static Look,
    pub hovered: Option<Key>,
    pub pressed: Option<Key>,
}

/// The case drawn without its keys, kept until the model or finish changes.
#[derive(Default)]
pub struct BodyState {
    cache: canvas::Cache,
    made_for: std::cell::Cell<Option<(*const Spec, *const Look)>>,
}

impl<Message> canvas::Program<Message> for Body {
    type State = BodyState;

    fn draw(
        &self,
        state: &BodyState,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let made_for = Some((self.spec as *const Spec, self.look as *const Look));
        if state.made_for.get() != made_for {
            state.cache.clear();
            state.made_for.set(made_for);
        }
        let case = state.cache.draw(renderer, bounds.size(), |frame| {
            frame.translate(Vector::new(MARGIN, MARGIN));
            self.draw_case(frame);
        });
        let mut keys = Frame::new(renderer, bounds.size());
        keys.translate(Vector::new(MARGIN, MARGIN));
        self.draw_keys(&mut keys);
        // Keys standing out of the edges go behind the case.
        let mut behind = Frame::new(renderer, bounds.size());
        behind.translate(Vector::new(MARGIN, MARGIN));
        if TRANSPARENT {
            for k in self.spec.keys {
                if let Place::Edge(side, from, to) = k.place {
                    self.draw_edge_key(&mut behind, side, from, to, self.key_look(k.key));
                }
            }
        }
        vec![behind.into_geometry(), case, keys.into_geometry()]
    }
}

impl Body {
    fn key_look(&self, key: Key) -> KeyLook {
        if self.pressed == Some(key) {
            KeyLook::Pressed
        } else if self.hovered == Some(key) {
            KeyLook::Hovered
        } else {
            KeyLook::Rest
        }
    }

    fn draw_case(&self, frame: &mut Frame) {
        let (spec, look) = (self.spec, self.look);
        let size = spec.body;
        let radius = if TRANSPARENT { spec.radius } else { 0.0 };
        let outline = Path::rounded_rectangle(Point::ORIGIN, size, radius.into());
        // The rim.
        frame.fill(
            &outline,
            gradient::Linear::new(Point::ORIGIN, Point::new(0.0, size.height))
                .add_stop(0.0, look.rim_top)
                .add_stop(1.0, look.rim_bottom),
        );
        // Light along the top of the rim.
        let lit = Path::rounded_rectangle(
            Point::new(0.75, 0.75),
            Size::new(size.width - 1.5, size.height - 1.5),
            (radius - 0.75).max(0.0).into(),
        );
        frame.stroke(
            &lit,
            Stroke {
                style: canvas::Style::Gradient(
                    gradient::Linear::new(Point::ORIGIN, Point::new(0.0, size.height))
                        .add_stop(0.0, look.light)
                        .add_stop(
                            0.3,
                            Color {
                                a: 0.0,
                                ..look.light
                            },
                        )
                        .into(),
                ),
                width: 1.5,
                ..Stroke::default()
            },
        );
        // The face, with its seam.
        let face = face_path(spec);
        frame.fill(&face, face_fill(spec, look));
        frame.stroke(
            &face,
            Stroke::default().with_width(1.0).with_color(look.seam),
        );

        // The display: its unlit border, then the panel.
        let s = spec.screen;
        let b = spec.panel_border;
        if b > 0.0 {
            frame.fill_rectangle(
                Point::new(s.x - b, s.y - b),
                Size::new(s.width + 2.0 * b, s.height + 2.0 * b),
                crate::eink::PANEL_BORDER,
            );
        }
        frame.fill_rectangle(s.position(), s.size(), crate::eink::PAPER);

        // Without transparency the edge keys sit within the case.
        if !TRANSPARENT {
            for k in spec.keys {
                if let Place::Edge(side, from, to) = k.place {
                    draw_inset_key(frame, size, side, from, to, look);
                }
            }
        }
    }

    fn draw_keys(&self, frame: &mut Frame) {
        let look = self.look;
        // Rockers on the face: each cap whole, then each half's state.
        let mut caps: Vec<(u8, Rectangle)> = Vec::new();
        for k in self.spec.keys {
            if let Place::Face(r) = k.place {
                match caps.iter_mut().find(|(n, _)| *n == k.rocker) {
                    Some((_, cap)) => *cap = cap.union(&r),
                    None => caps.push((k.rocker, r)),
                }
            }
        }
        for (_, cap) in &caps {
            draw_pill(frame, *cap, look);
        }
        for k in self.spec.keys {
            let state = self.key_look(k.key);
            match k.place {
                Place::Face(r) => {
                    let cap = caps
                        .iter()
                        .find(|(n, _)| *n == k.rocker)
                        .map_or(r, |(_, c)| *c);
                    shade_pill_half(frame, cap, r, state, look);
                }
                Place::Round(c, d) => draw_round_key(frame, c, d, state, look),
                Place::Edge(side, from, to) if !TRANSPARENT && state != KeyLook::Rest => {
                    let tint = if state == KeyLook::Pressed {
                        Color {
                            a: 0.35,
                            ..look.key_dark
                        }
                    } else {
                        Color {
                            a: 0.5,
                            ..look.key_light
                        }
                    };
                    let r = inset_rect(self.spec.body, side, from, to);
                    frame.fill(
                        &Path::rounded_rectangle(r.position(), r.size(), 3.0.into()),
                        tint,
                    );
                }
                Place::Edge(..) => {}
            }
        }
    }

    /// A key standing out of an edge; pressing pushes it in.
    fn draw_edge_key(&self, frame: &mut Frame, side: Side, from: f32, to: f32, state: KeyLook) {
        let look = self.look;
        let out = match state {
            KeyLook::Pressed => STANDOUT - 2.0,
            _ => STANDOUT,
        };
        let size = self.spec.body;
        // The key reaches a little under the case so no gap shows.
        let r = match side {
            Side::Left => rect(-out, from, out + 4.0, to - from),
            Side::Right => rect(size.width - 4.0, from, out + 4.0, to - from),
            Side::Top => rect(from, -out, to - from, out + 4.0),
        };
        let path = Path::rounded_rectangle(r.position(), r.size(), 2.5.into());
        let colour = match state {
            KeyLook::Rest => look.key,
            KeyLook::Hovered => mix(look.key, look.key_light, 0.5),
            KeyLook::Pressed => mix(look.key, look.key_dark, 0.5),
        };
        frame.fill(&path, colour);
        frame.stroke(
            &path,
            Stroke::default().with_width(1.0).with_color(look.key_dark),
        );
        // Light along the key's outer face.
        let (a, b) = match side {
            Side::Left => (
                Point::new(r.x + 1.0, r.y + 2.0),
                Point::new(r.x + 1.0, r.y + r.height - 2.0),
            ),
            Side::Right => (
                Point::new(r.x + r.width - 1.0, r.y + 2.0),
                Point::new(r.x + r.width - 1.0, r.y + r.height - 2.0),
            ),
            Side::Top => (
                Point::new(r.x + 2.0, r.y + 1.0),
                Point::new(r.x + r.width - 2.0, r.y + 1.0),
            ),
        };
        frame.stroke(
            &Path::line(a, b),
            Stroke::default().with_width(1.0).with_color(look.key_light),
        );
    }
}

/// The face's outline: a rounded rectangle inset from the case, with a tray
/// cut out of its bottom for the front keys where there is one.
fn face_path(spec: &Spec) -> Path {
    let [top, right, bottom, left] = spec.face;
    let (l, t) = (left, top);
    let (r, b) = (spec.body.width - right, spec.body.height - bottom);
    let rad = spec.face_radius;
    Path::new(|p: &mut path::Builder| {
        p.move_to(Point::new(l + rad, t));
        p.line_to(Point::new(r - rad, t));
        p.arc_to(Point::new(r, t), Point::new(r, t + rad), rad);
        p.line_to(Point::new(r, b - rad));
        p.arc_to(Point::new(r, b), Point::new(r - rad, b), rad);
        if let Some(tray) = spec.tray {
            let (tl, tr, tt) = (tray.x, tray.x + tray.width, tray.y);
            let corner = 9.0;
            let foot = 4.0;
            p.line_to(Point::new(tr + foot, b));
            p.arc_to(Point::new(tr, b), Point::new(tr, b - foot), foot);
            p.line_to(Point::new(tr, tt + corner));
            p.arc_to(Point::new(tr, tt), Point::new(tr - corner, tt), corner);
            p.line_to(Point::new(tl + corner, tt));
            p.arc_to(Point::new(tl, tt), Point::new(tl, tt + corner), corner);
            p.line_to(Point::new(tl, b - foot));
            p.arc_to(Point::new(tl, b), Point::new(tl - foot, b), foot);
        }
        p.line_to(Point::new(l + rad, b));
        p.arc_to(Point::new(l, b), Point::new(l, b - rad), rad);
        p.line_to(Point::new(l, t + rad));
        p.arc_to(Point::new(l, t), Point::new(l + rad, t), rad);
        p.close();
    })
}

/// The face's shading, top to bottom.
fn face_fill(spec: &Spec, look: &Look) -> gradient::Linear {
    gradient::Linear::new(Point::ORIGIN, Point::new(0.0, spec.body.height))
        .add_stop(0.0, look.face_top)
        .add_stop(1.0, look.face_bottom)
}

fn mix(a: Color, b: Color, t: f32) -> Color {
    let b = Color {
        r: b.r * b.a + a.r * (1.0 - b.a),
        g: b.g * b.a + a.g * (1.0 - b.a),
        b: b.b * b.a + a.b * (1.0 - b.a),
        a: 1.0,
    };
    Color {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a,
    }
}

/// A pill-shaped rocker cap, in a slight recess.
fn draw_pill(frame: &mut Frame, cap: Rectangle, look: &Look) {
    let radius = cap.height / 2.0;
    let well = Path::rounded_rectangle(
        Point::new(cap.x - 2.0, cap.y - 2.0),
        Size::new(cap.width + 4.0, cap.height + 4.0),
        (radius + 2.0).into(),
    );
    frame.fill(
        &well,
        Color {
            a: 0.55,
            ..look.pill_edge
        },
    );
    let path = Path::rounded_rectangle(cap.position(), cap.size(), radius.into());
    frame.fill(
        &path,
        gradient::Linear::new(cap.position(), Point::new(cap.x, cap.y + cap.height))
            .add_stop(0.0, mix(look.pill, look.pill_light, 0.35))
            .add_stop(1.0, look.pill),
    );
    frame.stroke(
        &path,
        Stroke::default().with_width(1.0).with_color(look.pill_edge),
    );
    // A highlight along the top of the cap.
    frame.stroke(
        &Path::line(
            Point::new(cap.x + radius, cap.y + 1.5),
            Point::new(cap.x + cap.width - radius, cap.y + 1.5),
        ),
        Stroke::default()
            .with_width(1.0)
            .with_color(look.pill_light),
    );
}

/// One half of a rocker, shaded for its state, with the seam between the
/// halves.
fn shade_pill_half(
    frame: &mut Frame,
    cap: Rectangle,
    half: Rectangle,
    state: KeyLook,
    look: &Look,
) {
    let radius = cap.height / 2.0;
    if state != KeyLook::Rest {
        let tint = if state == KeyLook::Pressed {
            Color {
                a: 0.45,
                ..look.pill_edge
            }
        } else {
            Color {
                a: 0.45,
                ..look.pill_light
            }
        };
        frame.with_clip(half, |f| {
            let path = Path::rounded_rectangle(
                Point::new(cap.x - half.x, cap.y - half.y),
                cap.size(),
                radius.into(),
            );
            f.fill(&path, tint);
        });
    }
    if half.x > cap.x + 0.5 {
        frame.stroke(
            &Path::line(
                Point::new(half.x, cap.y + 4.0),
                Point::new(half.x, cap.y + cap.height - 4.0),
            ),
            Stroke::default().with_width(1.0).with_color(look.pill_edge),
        );
    }
}

/// The X4 Pro's round Home key: a shallow dish in the face.
fn draw_round_key(frame: &mut Frame, centre: Point, diameter: f32, state: KeyLook, look: &Look) {
    let r = diameter / 2.0;
    let disc = Path::circle(centre, r);
    let (top, bottom) = match state {
        KeyLook::Rest => (mix(look.home, look.home_edge, 0.45), look.home),
        KeyLook::Hovered => (look.home, mix(look.home, look.light, 0.6)),
        KeyLook::Pressed => (
            mix(look.home, look.home_edge, 0.8),
            mix(look.home, look.home_edge, 0.3),
        ),
    };
    frame.fill(
        &disc,
        gradient::Linear::new(
            Point::new(centre.x, centre.y - r),
            Point::new(centre.x, centre.y + r),
        )
        .add_stop(0.0, top)
        .add_stop(1.0, bottom),
    );
    frame.stroke(
        &disc,
        Stroke::default().with_width(1.0).with_color(look.home_edge),
    );
    // Light catching the far lip of the dish.
    frame.stroke(
        &Path::new(|p| {
            p.arc(path::Arc {
                center: centre,
                radius: r - 1.0,
                start_angle: iced::Radians(0.25 * std::f32::consts::PI),
                end_angle: iced::Radians(0.75 * std::f32::consts::PI),
            });
        }),
        Stroke::default().with_width(1.0).with_color(Color {
            a: look.light.a * 0.6,
            ..look.light
        }),
    );
}

/// Where a key on an edge is drawn when it cannot stand out of the case.
fn inset_rect(size: Size, side: Side, from: f32, to: f32) -> Rectangle {
    const DEPTH: f32 = 5.0;
    match side {
        Side::Left => rect(1.0, from, DEPTH, to - from),
        Side::Right => rect(size.width - 1.0 - DEPTH, from, DEPTH, to - from),
        Side::Top => rect(from, 1.0, to - from, DEPTH),
    }
}

fn draw_inset_key(frame: &mut Frame, size: Size, side: Side, from: f32, to: f32, look: &Look) {
    let r = inset_rect(size, side, from, to);
    let groove = Path::rounded_rectangle(
        Point::new(r.x - 1.0, r.y - 1.0),
        Size::new(r.width + 2.0, r.height + 2.0),
        4.0.into(),
    );
    frame.fill(&groove, look.key_dark);
    frame.fill(
        &Path::rounded_rectangle(r.position(), r.size(), 3.0.into()),
        look.key,
    );
}

/// The glass over the screen: the opening's rounded corners, shade where
/// the panel sits below the face, and the black and white flash of a full
/// refresh.
pub struct Glass {
    pub spec: &'static Spec,
    pub look: &'static Look,
    pub flash: Option<Color>,
}

impl<Message> canvas::Program<Message> for Glass {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        frame.translate(Vector::new(MARGIN, MARGIN));
        let s = self.spec.screen;
        if let Some(colour) = self.flash {
            frame.fill_rectangle(s.position(), s.size(), colour);
        }
        let shade = Color::from_rgba(0.0, 0.0, 0.0, 0.13);
        let top = 7.0;
        frame.fill_rectangle(
            s.position(),
            Size::new(s.width, top),
            gradient::Linear::new(s.position(), Point::new(s.x, s.y + top))
                .add_stop(0.0, shade)
                .add_stop(1.0, Color::TRANSPARENT),
        );
        let side = 4.0;
        let faint = Color { a: 0.07, ..shade };
        frame.fill_rectangle(
            s.position(),
            Size::new(side, s.height),
            gradient::Linear::new(s.position(), Point::new(s.x + side, s.y))
                .add_stop(0.0, faint)
                .add_stop(1.0, Color::TRANSPARENT),
        );
        let right = Point::new(s.x + s.width - side, s.y);
        frame.fill_rectangle(
            right,
            Size::new(side, s.height),
            gradient::Linear::new(Point::new(s.x + s.width, s.y), right)
                .add_stop(0.0, faint)
                .add_stop(1.0, Color::TRANSPARENT),
        );
        // The face covers the screen's corners where the opening is round.
        let r = self.spec.opening_radius;
        if r > 0.0 {
            let corners = Path::new(|p| {
                p.rectangle(s.position(), s.size());
                p.rounded_rectangle(s.position(), s.size(), r.into());
            });
            frame.fill(
                &corners,
                Fill {
                    style: canvas::Style::Gradient(face_fill(self.spec, self.look).into()),
                    rule: fill::Rule::EvenOdd,
                },
            );
        }
        vec![frame.into_geometry()]
    }
}

/// A small picture of the device for the window and taskbar icon.
pub fn window_icon() -> Option<iced::window::Icon> {
    const N: u32 = 64;
    let mut rgba = vec![0u8; (N * N * 4) as usize];
    let body = (14.0f32, 2.0f32, 50.0f32, 62.0f32);
    let screen = (17.5f32, 5.5f32, 46.5f32, 53.0f32);
    let radius = 4.0f32;
    let home = (32.0f32, 57.5f32, 2.5f32);
    for y in 0..N {
        for x in 0..N {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let inside_body = {
                let cx = px.clamp(body.0 + radius, body.2 - radius);
                let cy = py.clamp(body.1 + radius, body.3 - radius);
                (px - cx).powi(2) + (py - cy).powi(2) <= radius * radius
            };
            if !inside_body {
                continue;
            }
            let in_screen = px >= screen.0 && px < screen.2 && py >= screen.1 && py < screen.3;
            let line = in_screen
                && px >= screen.0 + 4.0
                && py >= screen.1 + 5.0
                && py < screen.3 - 4.0
                && ((py - screen.1 - 5.0) as u32 % 6) < 2
                && px
                    < screen.2
                        - if ((py - screen.1) as u32 / 6) % 3 == 2 {
                            12.0
                        } else {
                            4.0
                        };
            let on_home = (px - home.0).powi(2) + (py - home.1).powi(2) <= home.2 * home.2;
            let colour = if line {
                [0x2A, 0x2A, 0x29]
            } else if in_screen {
                [0xE8, 0xE7, 0xE2]
            } else if on_home {
                [0x48, 0x49, 0x4D]
            } else {
                [0x26, 0x26, 0x29]
            };
            let i = ((y * N + x) * 4) as usize;
            rgba[i..i + 4].copy_from_slice(&[colour[0], colour[1], colour[2], 255]);
        }
    }
    iced::window::icon::from_rgba(rgba, N, N).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screens_and_keys_fit_their_cases() {
        for model in Model::ALL {
            let spec = model.spec();
            let s = spec.screen;
            assert!(
                s.x >= 0.0
                    && s.y >= 0.0
                    && s.x + s.width <= spec.body.width
                    && s.y + s.height <= spec.body.height,
                "{model:?}"
            );
            let window = Rectangle::new(Point::ORIGIN, spec.window());
            for k in spec.keys {
                let hit = k.hit(spec);
                assert!(
                    window.contains(hit.position())
                        && hit.x + hit.width <= window.width + 0.01
                        && hit.y + hit.height <= window.height + 0.01,
                    "{model:?} {:?}",
                    k.key
                );
                // No key can be pressed through the screen, nor through
                // another key.
                assert!(
                    hit.intersection(&spec.screen_in_window()).is_none(),
                    "{model:?} {:?}",
                    k.key
                );
                for other in spec.keys.iter().filter(|o| o.key != k.key) {
                    let overlap = hit.intersection(&other.hit(spec));
                    assert!(
                        overlap.is_none_or(|o| o.width < 0.5 || o.height < 0.5),
                        "{model:?} {:?} {:?}",
                        k.key,
                        other.key
                    );
                }
            }
        }
    }

    #[test]
    fn front_keys_sit_under_their_labels() {
        for model in [Model::X4, Model::X3] {
            let spec = model.spec();
            let hints = spec.hints.unwrap();
            for (i, key) in [Key::Back, Key::Confirm, Key::Left, Key::Right]
                .iter()
                .enumerate()
            {
                let k = spec.keys.iter().find(|k| k.key == *key).unwrap();
                let Place::Face(r) = k.place else {
                    panic!("{model:?} {key:?} is not on the face");
                };
                let label_middle = spec.screen.x + hints[i] + 40.0;
                assert!(
                    r.x <= label_middle && label_middle <= r.x + r.width,
                    "{model:?} {key:?}"
                );
            }
        }
    }

    #[test]
    fn the_x4_pro_has_a_home_key_and_no_front_keys() {
        let keys: Vec<Key> = Model::X4Pro.spec().keys.iter().map(|k| k.key).collect();
        assert!(!keys.contains(&Key::Back) && !keys.contains(&Key::Confirm));
        assert!(keys.contains(&Key::Home));
    }

    #[test]
    fn page_keys_are_on_the_sides_the_firmware_expects() {
        // The X4 Pro and X3 have one on each side, previous on the left; the
        // X4 has both on the right, previous above.
        for model in [Model::X4Pro, Model::X3] {
            let side = |key| {
                model.spec().keys.iter().find_map(|k| match k.place {
                    Place::Edge(s, ..) if k.key == key => Some(s),
                    _ => None,
                })
            };
            assert_eq!(side(Key::Up), Some(Side::Left), "{model:?}");
            assert_eq!(side(Key::Down), Some(Side::Right), "{model:?}");
        }
        let at = |key| {
            Model::X4.spec().keys.iter().find_map(|k| match k.place {
                Place::Edge(Side::Right, from, _) if k.key == key => Some(from),
                _ => None,
            })
        };
        assert!(at(Key::Up).unwrap() < at(Key::Down).unwrap());
    }
}
