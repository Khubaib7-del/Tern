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
