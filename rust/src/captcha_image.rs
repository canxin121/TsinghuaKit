//! Bounded raster headers for a human-visible CAPTCHA. This is not an image
//! decoder: the UI must still refuse submission when rendering fails.
//! A MIME label alone must not pass HTML, SVG or unbounded pixel canvases.

pub(crate) fn is_bounded_raster(content_type: &str, bytes: &[u8]) -> bool {
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let dimensions = match mime.as_str() {
        "image/png"
            if bytes.starts_with(b"\x89PNG\r\n\x1a\n")
                && bytes.len() >= 33
                && bytes[8..16] == *b"\0\0\0\rIHDR" =>
        {
            Some((
                u32::from_be_bytes(bytes[16..20].try_into().unwrap()),
                u32::from_be_bytes(bytes[20..24].try_into().unwrap()),
            ))
        }
        "image/gif"
            if bytes.len() >= 13
                && (bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a")) =>
        {
            Some((
                u16::from_le_bytes(bytes[6..8].try_into().unwrap()) as u32,
                u16::from_le_bytes(bytes[8..10].try_into().unwrap()) as u32,
            ))
        }
        "image/jpeg" => jpeg_dimensions(bytes),
        "image/webp" => webp_dimensions(bytes),
        _ => None,
    };
    dimensions.is_some_and(|(width, height)| {
        width > 0
            && height > 0
            && width <= 4096
            && height <= 4096
            && u64::from(width) * u64::from(height) <= 2 * 1024 * 1024
    })
}

pub(crate) fn jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if !bytes.starts_with(b"\xff\xd8") {
        return None;
    }
    let mut cursor = 2;
    while cursor < bytes.len() {
        if bytes[cursor] != 0xff {
            return None;
        }
        while bytes.get(cursor) == Some(&0xff) {
            cursor += 1;
        }
        let marker = *bytes.get(cursor)?;
        cursor += 1;
        if matches!(marker, 0x00 | 0xd8..=0xda) {
            return None;
        }
        if matches!(marker, 0x01 | 0xd0..=0xd7) {
            continue;
        }
        let length = u16::from_be_bytes(bytes.get(cursor..cursor + 2)?.try_into().ok()?) as usize;
        if length < 2 {
            return None;
        }
        let segment = bytes.get(cursor..cursor.checked_add(length)?)?;
        if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
            if length < 8 {
                return None;
            }
            return Some((
                u16::from_be_bytes(segment[5..7].try_into().ok()?) as u32,
                u16::from_be_bytes(segment[3..5].try_into().ok()?) as u32,
            ));
        }
        cursor += length;
    }
    None
}

fn webp_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 25 || !bytes.starts_with(b"RIFF") || &bytes[8..12] != b"WEBP" {
        return None;
    }
    let length = u32::from_le_bytes(bytes[4..8].try_into().ok()?) as usize;
    if length.checked_add(8)? != bytes.len() {
        return None;
    }
    match &bytes[12..16] {
        b"VP8X" if bytes.len() >= 30 => {
            let le24 =
                |s: &[u8]| u32::from(s[0]) | (u32::from(s[1]) << 8) | (u32::from(s[2]) << 16);
            Some((le24(&bytes[24..27]) + 1, le24(&bytes[27..30]) + 1))
        }
        b"VP8L" if bytes[20] == 0x2f => {
            let bits = u32::from_le_bytes(bytes[21..25].try_into().ok()?);
            Some(((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1))
        }
        b"VP8 " if bytes.len() >= 30 && &bytes[23..26] == b"\x9d\x01\x2a" => Some((
            (u16::from_le_bytes(bytes[26..28].try_into().ok()?) & 0x3fff) as u32,
            (u16::from_le_bytes(bytes[28..30].try_into().ok()?) & 0x3fff) as u32,
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_repair_captcha_raster_headers_reject_spoofed_unbounded_and_truncated_images() {
        let png = crate::reference_test_support::captcha_png();
        assert!(is_bounded_raster("Image/PNG; charset=binary", &png));
        for bytes in [b"<html>login</html>".as_slice(), b"<svg/>", &png[..24]] {
            assert!(!is_bounded_raster("image/png", bytes));
        }
        for mime in [
            "image/svg+xml",
            "text/html",
            "image/jpeg",
            "image/gif",
            "image/webp",
        ] {
            assert!(!is_bounded_raster(mime, &png));
        }
        for (width, height) in [(0_u32, 1_u32), (1, 0), (4097, 1), (2048, 2048)] {
            let mut huge = png.clone();
            huge[16..20].copy_from_slice(&width.to_be_bytes());
            huge[20..24].copy_from_slice(&height.to_be_bytes());
            assert!(!is_bounded_raster("image/png", &huge));
        }
    }

    #[test]
    fn backend_repair_captcha_raster_supports_bounded_jpeg_gif_and_webp_headers() {
        let gif = b"GIF89a\x02\0\x03\0\x80\0\0";
        let jpeg = b"\xff\xd8\xff\xe0\0\x02\xff\xc0\0\x08\x08\0\x03\0\x02\0";
        let webp = b"RIFF\x16\0\0\0WEBPVP8X\x0a\0\0\0\0\0\0\0\x01\0\0\x02\0\0";
        for (mime, bytes) in [
            ("image/gif", gif.as_slice()),
            ("image/jpeg", jpeg.as_slice()),
            ("image/webp", webp.as_slice()),
        ] {
            assert!(is_bounded_raster(mime, bytes));
            for end in 0..bytes.len() {
                assert!(!is_bounded_raster(mime, &bytes[..end]));
            }
        }
        assert!(!is_bounded_raster("image/jpeg", b"\xff\xd8\xff\xe0\0\0"));
    }
}
