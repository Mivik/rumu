use std::collections::HashSet;
use std::io::Cursor;
use std::path::PathBuf;
use std::time::Duration;

use image::{DynamicImage, ImageFormat, Rgb, RgbImage, Rgba, RgbaImage};

use super::*;

fn temp_dir(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("rumu-blob-{tag}-{}", uuid::Uuid::new_v4()))
}

fn encoded(img: DynamicImage, format: ImageFormat) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, format).unwrap();
    out.into_inner()
}

fn rgb(w: u32, h: u32) -> DynamicImage {
    DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
        Rgb([(x % 256) as u8, (y % 256) as u8, 128])
    }))
}

#[test]
fn png_round_trips_as_png() {
    let dir = temp_dir("png");
    let blobs = Blobs::open(&dir).unwrap();
    let a = blobs
        .ingest_image(
            &encoded(rgb(40, 30), ImageFormat::Png),
            Some("a.png".into()),
        )
        .unwrap();
    assert_eq!(a.mime, "image/png");
    assert_eq!((a.width, a.height), (40, 30));
    assert_eq!(a.name.as_deref(), Some("a.png"));
    let stored = blobs.get(&a.hash).unwrap();
    assert_eq!(stored.len() as u64, a.size);
    assert_eq!(hash_of(&stored), a.hash);
    // Ingesting the same image again is the same blob.
    let b = blobs
        .ingest_image(&encoded(rgb(40, 30), ImageFormat::Png), None)
        .unwrap();
    assert_eq!(a.hash, b.hash);
    // A wire reference is rebuilt from the blob, keeping only the name.
    assert_eq!(blobs.describe(&a.hash, Some("a.png".into())).unwrap(), a);
    // A stored non-image cannot be described.
    let text = blobs.put(b"not an image").unwrap();
    assert!(matches!(
        blobs.describe(&text, None),
        Err(Error::Unsupported)
    ));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn jpeg_stays_jpeg_and_alpha_stays_png() {
    let dir = temp_dir("formats");
    let blobs = Blobs::open(&dir).unwrap();
    let jpeg = blobs
        .ingest_image(&encoded(rgb(16, 16), ImageFormat::Jpeg), None)
        .unwrap();
    assert_eq!(jpeg.mime, "image/jpeg");
    let alpha = DynamicImage::ImageRgba8(RgbaImage::from_pixel(8, 8, Rgba([1, 2, 3, 4])));
    let png = blobs
        .ingest_image(&encoded(alpha, ImageFormat::Png), None)
        .unwrap();
    assert_eq!(png.mime, "image/png");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn long_edge_is_scaled_down() {
    let dir = temp_dir("resize");
    let blobs = Blobs::open(&dir).unwrap();
    let a = blobs
        .ingest_image(&encoded(rgb(3000, 600), ImageFormat::Png), None)
        .unwrap();
    assert_eq!((a.width, a.height), (MAX_EDGE, 400));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn rejects_non_images() {
    let dir = temp_dir("reject");
    let blobs = Blobs::open(&dir).unwrap();
    assert!(matches!(
        blobs.ingest_image(b"plain text, not an image", None),
        Err(Error::Unsupported)
    ));
    // A recognizable but unaccepted format (a BMP header).
    let mut bmp = b"BM".to_vec();
    bmp.resize(64, 0);
    assert!(matches!(
        blobs.ingest_image(&bmp, None),
        Err(Error::Unsupported)
    ));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn model_copy_is_the_blob_when_small_else_a_scaled_jpeg() {
    let dir = temp_dir("model");
    let blobs = Blobs::open(&dir).unwrap();
    let small = blobs
        .ingest_image(&encoded(rgb(40, 30), ImageFormat::Png), None)
        .unwrap();
    let (mime, bytes) = blobs.for_model(&small).unwrap();
    assert_eq!(mime, "image/png");
    assert_eq!(bytes, blobs.get(&small.hash).unwrap());

    // Noise does not compress, so this PNG is over the passthrough size.
    let mut seed = 1u32;
    let noise = RgbaImage::from_fn(1800, 1000, |_, _| {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let [a, b, c, d] = seed.to_le_bytes();
        Rgba([a, b, c, d])
    });
    let big = blobs
        .ingest_image(
            &encoded(DynamicImage::ImageRgba8(noise), ImageFormat::Png),
            None,
        )
        .unwrap();
    assert!(big.size > MODEL_PASSTHROUGH_BYTES);
    let (mime, bytes) = blobs.for_model(&big).unwrap();
    assert_eq!(mime, "image/jpeg");
    let (_, w, h) = normalize::probe(&bytes).unwrap();
    assert!(w <= MODEL_MAX_EDGE && w * h <= MODEL_MAX_PIXELS, "{w}×{h}");
    assert_eq!((w, h), (1438, 799));
    // Kept, and the same bytes next time.
    assert!(dir.join("model").join(&big.hash).is_file());
    assert_eq!(blobs.for_model(&big).unwrap().1, bytes);

    // The copy goes with its blob.
    assert_eq!(blobs.gc(&HashSet::new(), Duration::ZERO).unwrap(), 3);
    assert!(!dir.join("model").join(&big.hash).exists());
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn hashes_are_validated() {
    let dir = temp_dir("hash");
    let blobs = Blobs::open(&dir).unwrap();
    assert!(matches!(
        blobs.path("../etc/passwd"),
        Err(Error::InvalidHash(_))
    ));
    assert!(matches!(
        blobs.path(&"A".repeat(64)),
        Err(Error::InvalidHash(_))
    ));
    assert!(matches!(
        blobs.get(&"0".repeat(64)),
        Err(Error::NotFound(_))
    ));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn copy_to_creates_parents_and_keeps_an_existing_file() {
    let dir = temp_dir("copy");
    let blobs = Blobs::open(&dir.join("blobs")).unwrap();
    let hash = blobs.put(b"bytes").unwrap();
    let dest = dir.join("scratch/attachments/x.png");
    blobs.copy_to(&hash, &dest).unwrap();
    assert_eq!(fs::read(&dest).unwrap(), b"bytes");

    // A file already there (maybe edited by a tool) is left alone.
    fs::write(&dest, b"edited").unwrap();
    blobs.copy_to(&hash, &dest).unwrap();
    assert_eq!(fs::read(&dest).unwrap(), b"edited");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn gc_keeps_live_and_recent_blobs() {
    let dir = temp_dir("gc");
    let blobs = Blobs::open(&dir).unwrap();
    let live = blobs.put(b"live").unwrap();
    let orphan = blobs.put(b"orphan").unwrap();
    fs::write(dir.join(".stale.1.0.tmp"), b"partial").unwrap();
    fs::write(dir.join("unrelated"), b"kept").unwrap();
    let refs: HashSet<String> = [live.clone()].into();

    // Within the grace period nothing goes.
    assert_eq!(blobs.gc(&refs, Duration::from_secs(3600)).unwrap(), 0);
    assert!(blobs.contains(&orphan));

    // Past it, the orphan and the temp file go; the live blob and files
    // that are not blobs stay.
    assert_eq!(blobs.gc(&refs, Duration::ZERO).unwrap(), 2);
    assert!(blobs.contains(&live));
    assert!(!blobs.contains(&orphan));
    assert!(dir.join("unrelated").exists());
    fs::remove_dir_all(&dir).ok();
}
