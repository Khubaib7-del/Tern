//! Thumbnails for pictures waiting in the composer: the cache key and the
//! decoded bytes. GPUI-free so it is unit tested; `view` turns the bytes
//! into a GPUI image once and keeps it.

use tern_agent::ImageInput;

/// The picture formats Tern sends (`tern_agent::attach`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Png,
    Jpeg,
    Gif,
    Webp,
}

/// A key that changes when the picture does: its name, its length, and
/// the start of its data.
pub fn key(img: &ImageInput) -> String {
    let head: String = img.data.chars().take(48).collect();
    format!("{}:{}:{head}", img.name, img.data.len())
}

/// The picture's format and bytes, or `None` for anything else.
pub fn decode(img: &ImageInput) -> Option<(Format, Vec<u8>)> {
    let format = match img.media_type.as_str() {
        "image/png" => Format::Png,
        "image/jpeg" | "image/jpg" => Format::Jpeg,
        "image/gif" => Format::Gif,
        "image/webp" => Format::Webp,
        _ => return None,
    };
    Some((format, tern_agent::attach::image_bytes(img)?))
}

/// The format of picture `bytes` from their first bytes (#71: a scanned
/// PDF's page picture for its preview card).
pub fn sniff(bytes: &[u8]) -> Option<Format> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some(Format::Png)
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        Some(Format::Jpeg)
    } else if bytes.starts_with(b"GIF8") {
        Some(Format::Gif)
    } else if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(Format::Webp)
    } else {
        None
    }
}

/// A picture's width and height in pixels, read from its header (#76:
/// the focus view fits a sent picture to the window). PNG, GIF, JPEG and
/// WebP; `None` when the header is not one of those.
pub fn image_size(bytes: &[u8]) -> Option<(f32, f32)> {
    let be16 = |i: usize| Some(u16::from_be_bytes([*bytes.get(i)?, *bytes.get(i + 1)?]) as f32);
    let le16 = |i: usize| Some(u16::from_le_bytes([*bytes.get(i)?, *bytes.get(i + 1)?]) as f32);
    let le24 = |i: usize| {
        Some(u32::from_le_bytes([*bytes.get(i)?, *bytes.get(i + 1)?, *bytes.get(i + 2)?, 0]) as f32)
    };
    match sniff(bytes)? {
        Format::Png => {
            let w = u32::from_be_bytes(bytes.get(16..20)?.try_into().ok()?);
            let h = u32::from_be_bytes(bytes.get(20..24)?.try_into().ok()?);
            Some((w as f32, h as f32))
        }
        Format::Gif => Some((le16(6)?, le16(8)?)),
        Format::Jpeg => {
            // Walk the segments to the frame header (SOF0..SOF15 but not
            // DHT, JPG or DAC).
            let mut i = 2;
            while i + 9 < bytes.len() {
                if bytes[i] != 0xFF {
                    i += 1;
                    continue;
                }
                let m = bytes[i + 1];
                if (0xC0..=0xCF).contains(&m) && !matches!(m, 0xC4 | 0xC8 | 0xCC) {
                    return Some((be16(i + 7)?, be16(i + 5)?));
                }
                if m == 0xFF || m == 0x01 || (0xD0..=0xD9).contains(&m) {
                    i += if m == 0xFF { 1 } else { 2 };
                    continue;
                }
                i += 2 + be16(i + 2)? as usize;
            }
            None
        }
        Format::Webp => match bytes.get(12..16)? {
            b"VP8X" => Some((le24(24)? + 1., le24(27)? + 1.)),
            b"VP8L" => {
                let b = bytes.get(21..25)?;
                let v = u32::from_le_bytes(b.try_into().ok()?);
                Some(((v & 0x3FFF) as f32 + 1., ((v >> 14) & 0x3FFF) as f32 + 1.))
            }
            b"VP8 " => Some((
                (le16(26)? as u32 & 0x3FFF) as f32,
                (le16(28)? as u32 & 0x3FFF) as f32,
            )),
            _ => None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0];

    #[test]
    fn page_pictures_are_sniffed() {
        assert_eq!(sniff(PNG), Some(Format::Png));
        assert_eq!(sniff(&[0xFF, 0xD8, 0xFF, 0xE0]), Some(Format::Jpeg));
        assert_eq!(sniff(b"%PDF-1.4"), None);
    }

    #[test]
    fn picture_sizes_come_from_their_headers() {
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13];
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&640u32.to_be_bytes());
        png.extend_from_slice(&480u32.to_be_bytes());
        assert_eq!(image_size(&png), Some((640., 480.)));
        let gif = [b'G', b'I', b'F', b'8', b'9', b'a', 0x20, 0x03, 0x58, 0x02];
        assert_eq!(image_size(&gif), Some((800., 600.)));
        // SOI, an APP0 of 4 bytes, then SOF0: precision, height 2, width 3.
        let jpeg = [
            0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00, 0xFF, 0xC0, 0x00, 0x11, 0x08, 0x00,
            0x02, 0x00, 0x03, 0x03, 0x01, 0x22,
        ];
        assert_eq!(image_size(&jpeg), Some((3., 2.)));
        assert_eq!(image_size(b"%PDF-1.4"), None);
    }

    #[test]
    fn pictures_decode_and_key_by_content() {
        let a = tern_agent::Attachment::image("shot.png", PNG).unwrap();
        let img = &a.images[0];
        let (f, bytes) = decode(img).unwrap();
        assert_eq!(f, Format::Png);
        assert_eq!(bytes, PNG);
        let mut other = img.clone();
        other.data.push_str("AAAA");
        assert_ne!(key(img), key(&other));
        let mut svg = img.clone();
        svg.media_type = "image/svg+xml".into();
        assert!(decode(&svg).is_none());
    }
}
