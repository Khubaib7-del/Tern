//! Getting a picture into a shape every reader takes: PNG or JPEG, at most
//! [`MAX_EDGE`] pixels on the long edge and [`MAX_BYTES`] in size. WebP,
//! GIF (first frame), oversized PNGs and big JPEGs are converted; HEIC and
//! anything unreadable is refused with a plain reason. Used before a
//! picture goes to a provider (the chat model or a describer) and before
//! a local model reads it.

use std::io::Cursor;

use base64::Engine as _;
use image::{DynamicImage, ImageFormat, imageops::FilterType};
use tern_providers::ImageInput;

/// The long edge pictures are sent at, at most.
pub const MAX_EDGE: u32 = 2048;
/// The largest picture sent, in bytes (providers allow 5 MB to 20 MB).
pub const MAX_BYTES: usize = 5_000_000;

fn is_heic(bytes: &[u8]) -> bool {
    bytes.len() > 12
        && &bytes[4..8] == b"ftyp"
        && matches!(
            &bytes[8..12],
            b"heic" | b"heix" | b"hevc" | b"hevx" | b"heim" | b"heis" | b"mif1" | b"msf1" | b"avif"
        )
}

/// `bytes` as PNG or JPEG within the limits, with its media type.
pub fn prepare_bytes(name: &str, bytes: &[u8]) -> Result<(Vec<u8>, &'static str), String> {
    if is_heic(bytes) {
        return Err(format!(
            "{name} is a HEIC/AVIF picture, which Tern cannot read yet; save it as JPEG or PNG"
        ));
    }
    let format = image::guess_format(bytes)
        .map_err(|_| format!("{name} is not a picture format Tern can read"))?;
    let reader = image::ImageReader::with_format(Cursor::new(bytes), format);
    let (w, h) = reader
        .into_dimensions()
        .map_err(|e| format!("{name} could not be read: {e}"))?;
    let fits = w.max(h) <= MAX_EDGE && bytes.len() <= MAX_BYTES;
    match format {
        ImageFormat::Png if fits => return Ok((bytes.to_vec(), "image/png")),
        ImageFormat::Jpeg if fits => return Ok((bytes.to_vec(), "image/jpeg")),
        _ => {}
    }
    let img = image::load_from_memory_with_format(bytes, format)
        .map_err(|e| format!("{name} could not be decoded: {e}"))?;
    shrink(
        name,
        img,
        format == ImageFormat::Png || format == ImageFormat::WebP,
    )
}

/// Scales `img` into the limits and encodes it: PNG when it has
/// transparency and came from a lossless format, else JPEG.
fn shrink(
    name: &str,
    img: DynamicImage,
    lossless: bool,
) -> Result<(Vec<u8>, &'static str), String> {
    let mut img = if img.width().max(img.height()) > MAX_EDGE {
        img.resize(MAX_EDGE, MAX_EDGE, FilterType::Triangle)
    } else {
        img
    };
    let png = lossless && img.color().has_alpha();
    for _ in 0..5 {
        let mut out = Cursor::new(Vec::new());
        let r = if png {
            img.write_to(&mut out, ImageFormat::Png)
        } else {
            let rgb = DynamicImage::ImageRgb8(img.to_rgb8());
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85).encode_image(&rgb)
        };
        r.map_err(|e| format!("{name} could not be converted: {e}"))?;
        let out = out.into_inner();
        if out.len() <= MAX_BYTES {
            return Ok((out, if png { "image/png" } else { "image/jpeg" }));
        }
        let (w, h) = (img.width() * 3 / 4, img.height() * 3 / 4);
        img = img.resize(w.max(1), h.max(1), FilterType::Triangle);
    }
    Err(format!("{name} is too large to send even when scaled down"))
}

/// [`prepare_bytes`] for a picture attached to a message (base64).
pub fn prepare(img: &ImageInput) -> Result<ImageInput, String> {
    let b64 = base64::engine::general_purpose::STANDARD;
    let bytes = b64
        .decode(img.data.trim())
        .map_err(|e| format!("{} is not readable: {e}", img.name))?;
    let (out, media_type) = prepare_bytes(&img.name, &bytes)?;
    if out == bytes && media_type == img.media_type {
        return Ok(img.clone());
    }
    let mut p = img.clone();
    p.data = b64.encode(out);
    p.media_type = media_type.into();
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(img: &DynamicImage, f: ImageFormat) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, f).unwrap();
        out.into_inner()
    }

    fn dims(bytes: &[u8]) -> (u32, u32) {
        let img = image::load_from_memory(bytes).unwrap();
        (img.width(), img.height())
    }

    #[test]
    fn small_png_and_jpeg_pass_untouched() {
        let img = DynamicImage::new_rgb8(64, 32);
        let png = encode(&img, ImageFormat::Png);
        assert_eq!(prepare_bytes("a.png", &png).unwrap(), (png, "image/png"));
        let jpg = encode(&img, ImageFormat::Jpeg);
        assert_eq!(prepare_bytes("a.jpg", &jpg).unwrap(), (jpg, "image/jpeg"));
    }

    #[test]
    fn a_huge_picture_is_scaled_to_the_long_edge() {
        let img = DynamicImage::new_rgb8(4096, 1024);
        let (out, ty) = prepare_bytes("wide.png", &encode(&img, ImageFormat::Png)).unwrap();
        assert_eq!(ty, "image/jpeg");
        assert_eq!(dims(&out), (2048, 512));
    }

    #[test]
    fn webp_and_gif_become_png_or_jpeg() {
        let img = DynamicImage::new_rgba8(40, 30);
        let (out, ty) = prepare_bytes("s.webp", &encode(&img, ImageFormat::WebP)).unwrap();
        assert_eq!(ty, "image/png");
        assert_eq!(dims(&out), (40, 30));
        let rgb = DynamicImage::new_rgb8(40, 30);
        let (_, ty) = prepare_bytes("s.gif", &encode(&rgb, ImageFormat::Gif)).unwrap();
        assert!(ty == "image/jpeg" || ty == "image/png", "{ty}");
    }

    #[test]
    fn heic_and_junk_are_refused_in_plain_words() {
        let mut heic = vec![0, 0, 0, 24];
        heic.extend(b"ftypheic\0\0\0\0mif1heic");
        let e = prepare_bytes("IMG_1.HEIC", &heic).unwrap_err();
        assert!(e.contains("HEIC") && e.contains("JPEG or PNG"), "{e}");
        let e = prepare_bytes("x.bin", b"not a picture").unwrap_err();
        assert!(e.contains("not a picture format"), "{e}");
    }

    #[test]
    fn attached_pictures_are_rewritten_only_when_needed() {
        let b64 = base64::engine::general_purpose::STANDARD;
        let img = DynamicImage::new_rgb8(8, 8);
        let jpg = ImageInput {
            name: "a.jpg".into(),
            media_type: "image/jpeg".into(),
            data: b64.encode(encode(&img, ImageFormat::Jpeg)),
        };
        assert_eq!(prepare(&jpg).unwrap(), jpg);
        let webp = ImageInput {
            name: "b.webp".into(),
            media_type: "image/webp".into(),
            data: b64.encode(encode(&DynamicImage::new_rgba8(8, 8), ImageFormat::WebP)),
        };
        let p = prepare(&webp).unwrap();
        assert_eq!(p.media_type, "image/png");
        assert_eq!(p.name, "b.webp");
    }
}
