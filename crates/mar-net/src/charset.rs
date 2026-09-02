//! Charset detection and decoding.
//!
//! A surprising share of the web is still not UTF-8, and getting this wrong
//! silently corrupts every extracted string. The order below follows the HTML
//! spec's encoding sniffing algorithm, minus the parts that need a live parser.

use encoding_rs::{Encoding, UTF_8};

/// Decode `raw` to a `String`, returning the text and the charset used.
pub fn decode_body(raw: &[u8], content_type: &str) -> (String, String) {
    let encoding = sniff_charset(raw, content_type);
    let (text, _, _) = encoding.decode(raw);
    (text.into_owned(), encoding.name().to_ascii_lowercase())
}

/// Pick the encoding for a document body.
pub fn sniff_charset(raw: &[u8], content_type: &str) -> &'static Encoding {
    // 1. A byte order mark is authoritative and outranks every declaration.
    if let Some((encoding, _)) = Encoding::for_bom(raw) {
        return encoding;
    }

    // 2. The Content-Type header.
    if let Some(label) = charset_from_content_type(content_type)
        && let Some(encoding) = Encoding::for_label(label.as_bytes())
    {
        return encoding;
    }

    // 3. A <meta> declaration in the first kilobytes of the document. The spec
    //    scans 1024 bytes; real pages sometimes push it past that, so scan more.
    let window = &raw[..raw.len().min(8192)];
    if let Some(encoding) = charset_from_meta(window) {
        return encoding;
    }

    // 4. Nothing said anything. If the bytes are valid UTF-8 they almost
    //    certainly are UTF-8; otherwise fall back to the legacy default, which
    //    at least never fails and keeps byte values recoverable.
    if std::str::from_utf8(window).is_ok() {
        UTF_8
    } else {
        encoding_rs::WINDOWS_1252
    }
}

fn charset_from_content_type(content_type: &str) -> Option<String> {
    let lower = content_type.to_ascii_lowercase();
    let idx = lower.find("charset")?;
    let rest = &lower[idx + "charset".len()..];
    let rest = rest.trim_start();
    let rest = rest.strip_prefix('=')?.trim();
    let value = rest
        .trim_start_matches(['"', '\''])
        .split(['"', '\'', ';', ' '])
        .next()?;
    (!value.is_empty()).then(|| value.to_owned())
}

/// Look for `<meta charset=...>` or `<meta http-equiv=content-type content=...>`.
fn charset_from_meta(window: &[u8]) -> Option<&'static Encoding> {
    // Work on a lossy view: the declaration itself is always ASCII, and a
    // malformed multi-byte sequence elsewhere must not abort the scan.
    let text = String::from_utf8_lossy(window).to_ascii_lowercase();
    let mut cursor = 0usize;
    while let Some(found) = text[cursor..].find("<meta") {
        let start = cursor + found;
        let end = text[start..]
            .find('>')
            .map(|e| start + e)
            .unwrap_or(text.len());
        let tag = &text[start..end];

        // <meta charset="utf-8">
        if let Some(value) = attr_value(tag, "charset")
            && let Some(encoding) = Encoding::for_label(value.as_bytes())
        {
            return Some(encoding);
        }
        // <meta http-equiv="content-type" content="text/html; charset=...">
        if attr_value(tag, "http-equiv").is_some_and(|v| v.contains("content-type"))
            && let Some(content) = attr_value(tag, "content")
            && let Some(label) = charset_from_content_type(&content)
            && let Some(encoding) = Encoding::for_label(label.as_bytes())
        {
            return Some(encoding);
        }
        cursor = end.max(start + 5);
    }
    None
}

/// Read one attribute's value out of a start tag.
///
/// Every occurrence of the name is tried, not just the first: in
/// `http-equiv="content-type" content="..."` the substring `content` appears
/// inside the previous attribute's value, and stopping there finds nothing.
fn attr_value(tag: &str, name: &str) -> Option<String> {
    let mut from = 0usize;
    while let Some(found) = tag[from..].find(name) {
        let idx = from + found;
        from = idx + name.len();

        // The name must start an attribute, so what precedes it is whitespace
        // or the tag itself, never part of a longer name like "data-charset".
        let starts_attribute = idx == 0 || {
            let prev = tag.as_bytes()[idx - 1];
            prev.is_ascii_whitespace() || prev == b'<'
        };
        if !starts_attribute {
            continue;
        }

        let rest = tag[from..].trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        let rest = rest.trim_start();
        let value = match rest.as_bytes().first() {
            Some(b'"') => rest[1..].split('"').next(),
            Some(b'\'') => rest[1..].split('\'').next(),
            _ => rest.split([' ', '\t', '\n', '/', '>']).next(),
        };
        if let Some(value) = value
            && !value.trim().is_empty()
        {
            return Some(value.trim().to_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bom_wins_over_every_declaration() {
        let mut raw = vec![0xEF, 0xBB, 0xBF];
        raw.extend_from_slice(br#"<meta charset="windows-1251">x"#);
        let (_, charset) = decode_body(&raw, "text/html; charset=iso-8859-5");
        assert_eq!(charset, "utf-8");
    }

    #[test]
    fn header_wins_over_meta() {
        let raw = br#"<html><meta charset="utf-8"><body>x"#;
        assert_eq!(
            sniff_charset(raw, "text/html; charset=windows-1251").name(),
            "windows-1251"
        );
    }

    #[test]
    fn meta_is_used_when_the_header_is_silent() {
        // "Привет" in windows-1251.
        let mut raw = br#"<html><head><meta charset="windows-1251"></head><body>"#.to_vec();
        raw.extend_from_slice(&[0xCF, 0xF0, 0xE8, 0xE2, 0xE5, 0xF2]);
        let (text, charset) = decode_body(&raw, "text/html");
        assert_eq!(charset, "windows-1251");
        assert!(text.contains("Привет"), "decoded: {text}");
    }

    #[test]
    fn http_equiv_form_is_understood() {
        let raw = br#"<meta http-equiv="Content-Type" content="text/html; charset=koi8-r">"#;
        assert_eq!(sniff_charset(raw, "").name(), "KOI8-R");
    }

    #[test]
    fn a_similar_attribute_name_is_not_mistaken_for_charset() {
        let raw = br#"<meta data-charset="koi8-r" name="x">"#;
        // Falls through to the UTF-8 default rather than trusting data-charset.
        assert_eq!(sniff_charset(raw, "").name(), "UTF-8");
    }

    #[test]
    fn a_koi8_body_is_decoded_once_not_twice() {
        // Regression: with the HTTP client's own charset support enabled, the
        // body arrived already transcoded to UTF-8 and this decoded it a second
        // time, turning "Библиотека" into "п▒п╦п╠п╩п╦п╬я┌п╣п╨п╟". The client
        // now hands over raw bytes.
        let mut raw = b"<html><body>".to_vec();
        raw.extend_from_slice(&[
            0xE2, 0xC9, 0xC2, 0xCC, 0xC9, 0xCF, 0xD4, 0xC5, 0xCB, 0xC1,
        ]); // "Библиотека" in koi8-r
        let (text, charset) = decode_body(&raw, "text/html; charset=koi8-r");
        assert_eq!(charset, "koi8-r");
        assert!(text.contains("Библиотека"), "decoded: {text}");

        // The same bytes already in UTF-8 must not be re-decoded as koi8-r.
        let utf8 = "<html><body>Библиотека".as_bytes();
        let (text, _) = decode_body(utf8, "text/html; charset=utf-8");
        assert!(text.contains("Библиотека"), "decoded: {text}");
    }

    #[test]
    fn windows_1251_pages_decode() {
        let mut raw = br#"<meta charset="windows-1251">"#.to_vec();
        raw.extend_from_slice(&[0xCC, 0xEE, 0xF1, 0xEA, 0xE2, 0xE0]); // "Москва"
        let (text, charset) = decode_body(&raw, "text/html");
        assert_eq!(charset, "windows-1251");
        assert!(text.contains("Москва"), "decoded: {text}");
    }

    #[test]
    fn undeclared_utf8_is_detected_and_invalid_bytes_fall_back() {
        assert_eq!(sniff_charset("Привет".as_bytes(), "").name(), "UTF-8");
        assert_eq!(sniff_charset(&[0xCF, 0xF0, 0xE8], "").name(), "windows-1252");
    }
}
