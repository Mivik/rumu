//! Ingest-time image normalization.
//!
//! Done exactly once per image, before it is stored: the stored bytes are
//! what every later request sends, so the model's view (and the prompt
//! cache) never changes. Re-encoding also drops EXIF and other metadata.
//!
//! - accepted inputs: PNG, JPEG, GIF (first frame) and WebP, sniffed from
//!   the bytes, at most [`MAX_INPUT_BYTES`];
//! - EXIF orientation is applied to the pixels;
//! - the long edge is scaled down to [`MAX_EDGE`];
//! - JPEG input stays JPEG; everything else (screenshots, diagrams, alpha)
//!   becomes PNG, which keeps text crisp;
//! - a result over [`MAX_STORED_BYTES`] is re-encoded as JPEG when it has no
//!   alpha, then scaled down until it fits.

use std::io::Cursor;

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits};

use crate::{Error, Result};

/// Largest accepted input.
pub const MAX_INPUT_BYTES: usize = 20 << 20;
/// Longest stored edge, in pixels.
pub const MAX_EDGE: u32 = 2000;
/// Largest stored image: 5 MiB once base64-encoded (Anthropic's per-image
/// limit).
pub const MAX_STORED_BYTES: usize = 3_932_160;
/// Largest decoded dimension; guards against decompression bombs.
const MAX_DECODE_EDGE: u32 = 16_384;
const JPEG_QUALITY: u8 = 90;

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

    let mut jpeg = format == ImageFormat::Jpeg;
    let mut edge = MAX_EDGE;
    loop {
        if img.width().max(img.height()) > edge {
            img = img.resize(edge, edge, FilterType::Triangle);
        }
        let bytes = encode(&img, jpeg)?;
        if bytes.len() <= MAX_STORED_BYTES {
            return Ok(Normalized {
                bytes,
                mime: if jpeg { "image/jpeg" } else { "image/png" },
                width: img.width(),
                height: img.height(),
            });
        }
        if !jpeg && !img.color().has_alpha() {
            jpeg = true;
        } else {
            edge = img.width().max(img.height()) * 3 / 4;
        }
    }
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

fn encode(img: &DynamicImage, jpeg: bool) -> Result<Vec<u8>> {
    let mut out = Cursor::new(Vec::new());
    let result = if jpeg {
        JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY).encode_image(&img.to_rgb8())
    } else if img.color().has_alpha() {
        img.to_rgba8().write_to(&mut out, ImageFormat::Png)
    } else {
        img.to_rgb8().write_to(&mut out, ImageFormat::Png)
    };
    result.map_err(|e| Error::Encode(e.to_string()))?;
    Ok(out.into_inner())
}
