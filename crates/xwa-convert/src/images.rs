//! Image conversion (spec 9): greyscale JPEG for photos, lossless LA2 for
//! drawings and small icons.

use image::imageops::FilterType;
use image::{DynamicImage, GrayImage};
use xwa_format::Profile;
use xwa_format::la2;
use xwa_format::records::{image_encoding, image_kind};
use xwa_format::writer::{ImageInfo, image_flags};

/// How an image is used, which decides its encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A photo or painting: JPEG.
    Photo,
    /// A diagram, chart or map drawn from vector graphics: LA2.
    Drawing,
    /// A small inline image (flag, symbol): LA2.
    Icon,
}

#[derive(Debug, Clone)]
pub struct Settings {
    pub profile: Profile,
    /// JPEG quality for photos, 1-100.
    pub jpeg_quality: u8,
    /// Longest side in pixels; larger images are scaled down.
    pub max_side: u32,
}

impl Settings {
    pub fn new(profile: Profile) -> Self {
        Self {
            profile,
            jpeg_quality: 75,
            max_side: 800,
        }
    }
}

/// An image ready for the archive.
#[derive(Debug, Clone)]
pub struct Encoded {
    pub info: ImageInfo,
    pub data: Vec<u8>,
}

/// Decodes `bytes` (WebP, PNG, JPEG or GIF) and encodes it for the archive.
pub fn convert(bytes: &[u8], kind: Kind, s: &Settings) -> Result<Encoded, String> {
    let img = image::load_from_memory(bytes).map_err(|e| e.to_string())?;
    let (grey, transparent) = to_grey(&img);
    let grey = fit(grey, s.max_side);
    let (width, height) = (grey.width() as u16, grey.height() as u16);
    let flags = if transparent {
        image_flags::TRANSPARENCY
    } else {
        0
    };
    // Many "bitmaps" are diagrams saved as PNG (reaction schemes, plots):
    // they are crisper and smaller as line art.
    let kind = if kind == Kind::Photo && looks_like_line_art(&grey) {
        Kind::Drawing
    } else {
        kind
    };
    match kind {
        Kind::Photo => {
            let mut data = Vec::new();
            let mut enc = jpeg_encoder::Encoder::new(&mut data, s.jpeg_quality.clamp(1, 100));
            enc.set_optimized_huffman_tables(true);
            enc.encode(grey.as_raw(), width, height, jpeg_encoder::ColorType::Luma)
                .map_err(|e| e.to_string())?;
            let info = ImageInfo {
                encoding: image_encoding::JPEG,
                width,
                height,
                kind: image_kind::PHOTO,
                flags,
            };
            Ok(Encoded { info, data })
        }
        Kind::Drawing | Kind::Icon => {
            let data = encode_la2(&grey, s.profile)?;
            let kind = if kind == Kind::Icon {
                image_kind::ICON
            } else {
                image_kind::LINE_ART
            };
            Ok(Encoded {
                info: ImageInfo {
                    encoding: image_encoding::LINE_ART,
                    width,
                    height,
                    kind,
                    flags,
                },
                data,
            })
        }
    }
}

/// A 1x1 white line-art image, standing in for one that could not be read
/// (articles already refer to its id).
pub fn placeholder(profile: Profile) -> Encoded {
    let grey = GrayImage::from_pixel(1, 1, image::Luma([255]));
    let data = encode_la2(&grey, profile).expect("a 1x1 image always encodes");
    Encoded {
        info: ImageInfo {
            encoding: image_encoding::LINE_ART,
            width: 1,
            height: 1,
            kind: image_kind::ICON,
            flags: 0,
        },
        data,
    }
}

/// Greyscale on white paper; also reports whether any pixel was transparent.
fn to_grey(img: &DynamicImage) -> (GrayImage, bool) {
    if !img.color().has_alpha() {
        return (img.to_luma8(), false);
    }
    let la = img.to_luma_alpha8();
    let mut transparent = false;
    let grey = GrayImage::from_fn(la.width(), la.height(), |x, y| {
        let [l, a] = la.get_pixel(x, y).0;
        if a < 255 {
            transparent = true;
        }
        image::Luma([((l as u32 * a as u32 + 255 * (255 - a as u32)) / 255) as u8])
    });
    (grey, transparent)
}

fn fit(img: GrayImage, max_side: u32) -> GrayImage {
    let (w, h) = img.dimensions();
    let long = w.max(h);
    if long <= max_side.max(1) && long <= u16::MAX as u32 {
        return img;
    }
    let scale = max_side as f64 / long as f64;
    let (nw, nh) = (
        ((w as f64 * scale).round() as u32).max(1),
        ((h as f64 * scale).round() as u32).max(1),
    );
    image::imageops::resize(&img, nw, nh, FilterType::Lanczos3)
}

/// Whether nearly every pixel already sits on one of the four ink levels.
/// Measured on the chemistry ZIM: reaction schemes score 0.90 and above,
/// shaded renderings and photos below (those look better as JPEG, which
/// the device can dither).
fn looks_like_line_art(img: &GrayImage) -> bool {
    let pixels = img.as_raw();
    if pixels.is_empty() {
        return false;
    }
    let near = pixels
        .iter()
        .filter(|&&v| {
            [0i16, 85, 170, 255]
                .iter()
                .any(|&c| (v as i16 - c).abs() <= 8)
        })
        .count();
    near as f64 >= 0.90 * pixels.len() as f64
}

/// Ink level of a grey value: 0 paper, 3 black.
fn level(v: u8) -> u8 {
    3 - ((v as u16 + 42) / 85).min(3) as u8
}

/// LA2 (spec 9.2): 1 bit per pixel when the image has no greys, else 2;
/// raw deflate with a 4 KB window (compact) or zstd (full).
fn encode_la2(img: &GrayImage, profile: Profile) -> Result<Vec<u8>, String> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let levels: Vec<u8> = img.as_raw().iter().map(|&v| level(v)).collect();
    let bpp = if levels.iter().all(|&l| l == 0 || l == 3) {
        1
    } else {
        2
    };
    let stride = (w * bpp).div_ceil(8);
    let mut packed = vec![0u8; stride * h];
    for (i, &l) in levels.iter().enumerate() {
        let (y, x) = (i / w, i % w);
        let v = if bpp == 1 { l / 3 } else { l };
        let bit = x * bpp;
        packed[y * stride + bit / 8] |= v << (8 - bpp - bit % 8);
    }
    let (codec, body) = match profile {
        Profile::Compact => {
            // A 4 KB window: the X4/X3 decode with a 4 KB buffer (spec 9.2).
            let mut c =
                flate2::Compress::new_with_window_bits(flate2::Compression::best(), false, 12);
            let mut out = Vec::with_capacity(packed.len() / 2 + 64);
            loop {
                let input = &packed[c.total_in() as usize..];
                let status = c
                    .compress_vec(input, &mut out, flate2::FlushCompress::Finish)
                    .map_err(|e| e.to_string())?;
                if status == flate2::Status::StreamEnd {
                    break;
                }
                out.reserve(out.capacity().max(1024));
            }
            (la2::codec::DEFLATE, out)
        }
        // Level 15: within 0.2% of level 19 at two thirds of the time
        // (chemistry ZIM: 163.6 MB in 40 s against 163.3 MB in 62 s).
        Profile::Full => (
            la2::codec::ZSTD,
            zstd::bulk::compress(&packed, 15).map_err(|e| e.to_string())?,
        ),
    };
    let mut out = Vec::with_capacity(la2::HEADER + body.len());
    out.extend_from_slice(&(w as u16).to_le_bytes());
    out.extend_from_slice(&(h as u16).to_le_bytes());
    out.extend_from_slice(&[bpp as u8, codec]);
    out.extend_from_slice(&body);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageFormat, Rgba, RgbaImage};

    fn png(img: &RgbaImage) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, ImageFormat::Png).unwrap();
        out.into_inner()
    }

    /// A drawing: black lines, a grey fill, transparent background.
    fn drawing() -> RgbaImage {
        RgbaImage::from_fn(40, 20, |x, y| match (x, y) {
            (_, 0) | (0, _) => Rgba([0, 0, 0, 255]),
            (10..=20, 5..=10) => Rgba([80, 80, 80, 255]),
            _ => Rgba([0, 0, 0, 0]),
        })
    }

    #[test]
    fn drawings_become_lossless_line_art_in_both_profiles() {
        for profile in [Profile::Compact, Profile::Full] {
            let e = convert(&png(&drawing()), Kind::Drawing, &Settings::new(profile)).unwrap();
            assert_eq!(
                (e.info.width, e.info.height, e.info.encoding),
                (40, 20, image_encoding::LINE_ART)
            );
            assert_eq!(e.info.flags, image_flags::TRANSPARENCY);
            let img = la2::decode(&e.data).unwrap();
            assert_eq!(img.header.bpp, 2);
            assert_eq!(img.levels[0], 3, "black line");
            assert_eq!(img.levels[5 * 40 + 15], 2, "grey fill");
            assert_eq!(img.levels[15 * 40 + 30], 0, "transparent becomes paper");
        }
    }

    #[test]
    fn black_and_white_images_use_one_bit() {
        let bw = RgbaImage::from_fn(9, 3, |x, _| {
            if x % 2 == 0 {
                Rgba([0, 0, 0, 255])
            } else {
                Rgba([255, 255, 255, 255])
            }
        });
        let e = convert(&png(&bw), Kind::Icon, &Settings::new(Profile::Compact)).unwrap();
        assert_eq!(e.info.kind, image_kind::ICON);
        let img = la2::decode(&e.data).unwrap();
        assert_eq!(img.header.bpp, 1);
        assert_eq!(&img.levels[..4], &[1, 0, 1, 0]);
    }

    #[test]
    fn bitmaps_that_are_line_art_become_line_art() {
        let scheme = RgbaImage::from_fn(60, 30, |x, y| {
            if y == 15 || x == 30 {
                Rgba([0, 0, 0, 255])
            } else {
                Rgba([255, 255, 255, 255])
            }
        });
        let e = convert(&png(&scheme), Kind::Photo, &Settings::new(Profile::Compact)).unwrap();
        assert_eq!(
            (e.info.encoding, e.info.kind),
            (image_encoding::LINE_ART, image_kind::LINE_ART)
        );
    }

    #[test]
    fn photos_become_greyscale_jpeg_and_large_ones_shrink() {
        let photo = RgbaImage::from_fn(1600, 400, |x, y| {
            Rgba([(x % 256) as u8, (y % 256) as u8, 90, 255])
        });
        let e = convert(&png(&photo), Kind::Photo, &Settings::new(Profile::Full)).unwrap();
        assert_eq!(
            (e.info.width, e.info.height, e.info.kind, e.info.flags),
            (800, 200, image_kind::PHOTO, 0)
        );
        assert!(e.data.starts_with(&[0xFF, 0xD8, 0xFF]) && e.data.ends_with(&[0xFF, 0xD9]));
        let back = image::load_from_memory(&e.data).unwrap();
        assert_eq!(back.color(), image::ColorType::L8);
    }

    #[test]
    fn unreadable_images_report_an_error_and_placeholders_decode() {
        assert!(convert(b"<svg/>", Kind::Photo, &Settings::new(Profile::Full)).is_err());
        for profile in [Profile::Compact, Profile::Full] {
            assert_eq!(
                la2::decode(&placeholder(profile).data).unwrap().levels,
                vec![0]
            );
        }
    }
}
