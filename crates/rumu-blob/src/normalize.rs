//! Image normalization: once at ingest, for storage and display, and once
//! per image for the model.
//!
//! Ingest ([`normalize`]):
//! - accepted inputs: PNG, JPEG, GIF (first frame) and WebP, sniffed from
//!   the bytes, at most [`MAX_INPUT_BYTES`];
//! - EXIF orientation is applied to the pixels; re-encoding drops EXIF and
//!   other metadata;
//! - the long edge is scaled down to [`MAX_EDGE`];
//! - JPEG input stays JPEG; everything else (screenshots, diagrams, alpha)
//!   becomes PNG, which keeps text crisp.
//!
//! The model's copy ([`for_model`]) is what every request sends, replayed
//! each turn, so it is small: scaled to [`MODEL_MAX_EDGE`] and
//! [`MODEL_MAX_PIXELS`] (what Anthropic downscales to anyway) and encoded as
//! JPEG, alpha flattened onto white. Deterministic, so the projection — and
//! the prompt cache — stays byte-identical.

use std::io::Cursor;

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits, RgbImage};

use crate::{Error, Result};

/// Largest accepted input.
pub const MAX_INPUT_BYTES: usize = 20 << 20;
/// Longest stored edge, in pixels.
pub const MAX_EDGE: u32 = 2000;
/// Stored images at most this large go to the model as they are.
pub const MODEL_PASSTHROUGH_BYTES: u64 = 128 << 10;
/// Longest edge of a model copy, in pixels.
pub const MODEL_MAX_EDGE: u32 = 1568;
/// Most pixels of a model copy (about 1.15 MP).
pub const MODEL_MAX_PIXELS: u32 = 1_150_000;
/// Largest decoded dimension; guards against decompression bombs.
const MAX_DECODE_EDGE: u32 = 16_384;
const JPEG_QUALITY: u8 = 90;
const MODEL_JPEG_QUALITY: u8 = 85;

pub(crate) struct Normalized {
    pub bytes: Vec<u8>,
    pub mime: &'static str,
    pub width: u32,
    pub height: u32,
}

pub(crate) fn normalize(bytes: &[u8]) -> Result<Normalized> {
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(Error::TooLarge {
            size: bytes.len(),
            limit: MAX_INPUT_BYTES,
        });
    }
    let format = accepted_format(bytes).ok_or(Error::Unsupported)?;
    let mut img = decode(bytes, format)?;
    if img.width().max(img.height()) > MAX_EDGE {
        img = img.resize(MAX_EDGE, MAX_EDGE, FilterType::Triangle);
    }
    let jpeg = format == ImageFormat::Jpeg;
    let mut out = Cursor::new(Vec::new());
    let result = if jpeg {
        JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY).encode_image(&img.to_rgb8())
    } else if img.color().has_alpha() {
        img.to_rgba8().write_to(&mut out, ImageFormat::Png)
    } else {
        img.to_rgb8().write_to(&mut out, ImageFormat::Png)
    };
    result.map_err(|e| Error::Encode(e.to_string()))?;
    Ok(Normalized {
        bytes: out.into_inner(),
        mime: if jpeg { "image/jpeg" } else { "image/png" },
        width: img.width(),
        height: img.height(),
    })
}

/// The JPEG the model sees of a stored image.
pub(crate) fn for_model(stored: &[u8]) -> Result<Normalized> {
    let format = accepted_format(stored).ok_or(Error::Unsupported)?;
    let mut img = decode(stored, format)?;
    let (w, h) = (f64::from(img.width()), f64::from(img.height()));
    let scale =
        (f64::from(MODEL_MAX_EDGE) / w.max(h)).min((f64::from(MODEL_MAX_PIXELS) / (w * h)).sqrt());
    if scale < 1.0 {
        let fit = |d: f64| ((d * scale) as u32).max(1);
        img = img.resize_exact(fit(w), fit(h), FilterType::Lanczos3);
    }
    let rgb = if img.color().has_alpha() {
        let rgba = img.to_rgba8();
        RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
            let [r, g, b, a] = rgba.get_pixel(x, y).0;
            let over =
                |c: u8| ((u16::from(c) * u16::from(a) + 255 * (255 - u16::from(a))) / 255) as u8;
            image::Rgb([over(r), over(g), over(b)])
        })
    } else {
        img.to_rgb8()
    };
    let mut out = Cursor::new(Vec::new());
    JpegEncoder::new_with_quality(&mut out, MODEL_JPEG_QUALITY)
        .encode_image(&rgb)
        .map_err(|e| Error::Encode(e.to_string()))?;
    Ok(Normalized {
        bytes: out.into_inner(),
        mime: "image/jpeg",
        width: rgb.width(),
        height: rgb.height(),
    })
}

/// Decode with bomb limits, applying EXIF orientation.
fn decode(bytes: &[u8], format: ImageFormat) -> Result<DynamicImage> {
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DECODE_EDGE);
    limits.max_image_height = Some(MAX_DECODE_EDGE);
    reader.limits(limits);
    let decode = |e: image::ImageError| Error::Decode(e.to_string());
    let mut decoder = reader.into_decoder().map_err(decode)?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(decoder).map_err(decode)?;
    img.apply_orientation(orientation);
    Ok(img)
}

/// The format of an accepted input, sniffed from its magic bytes.
pub(crate) fn accepted_format(bytes: &[u8]) -> Option<ImageFormat> {
    image::guess_format(bytes).ok().filter(|f| {
        matches!(
            f,
            ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Gif | ImageFormat::WebP
        )
    })
}

/// MIME type and dimensions of a stored (already normalized) image, read
/// from its header only.
pub(crate) fn probe(bytes: &[u8]) -> Result<(&'static str, u32, u32)> {
    let mime = match image::guess_format(bytes) {
        Ok(ImageFormat::Png) => "image/png",
        Ok(ImageFormat::Jpeg) => "image/jpeg",
        _ => return Err(Error::Unsupported),
    };
    let (width, height) = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| Error::Decode(e.to_string()))?
        .into_dimensions()
        .map_err(|e| Error::Decode(e.to_string()))?;
    Ok((mime, width, height))
}
