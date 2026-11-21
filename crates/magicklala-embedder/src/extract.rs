use std::io::Read;

use crate::{DocumentKind, EmbedderError};

const MAX_STREAM_BYTES: usize = 4 * 1024 * 1024;
const MAX_DOCUMENT_BYTES: usize = 16 * 1024 * 1024;

const SKIPPED_ELEMENTS: [&str; 7] = ["script", "style", "noscript", "template", "svg", "nav", "iframe"];
const BLOCK_ELEMENTS: [&str; 33] = [
    "p",
    "div",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "li",
    "ul",
    "ol",
    "section",
    "article",
    "header",
    "footer",
    "main",
    "aside",
    "blockquote",
    "pre",
    "table",
    "tr",
    "title",
    "hr",
    "dl",
    "dt",
    "dd",
    "figure",
    "figcaption",
    "form",
    "body",
    "html",
    "address",
    "details",
];

pub fn extract_text(kind: DocumentKind, bytes: &[u8]) -> Result<String, EmbedderError> {
    match kind {
        DocumentKind::Text => Ok(tidy(utf8(bytes)?)),
        DocumentKind::Markdown => Ok(markdown_to_text(utf8(bytes)?)),
        DocumentKind::Html => Ok(html_to_text(&String::from_utf8_lossy(bytes))),
        DocumentKind::Pdf => pdf_to_text(bytes),
    }
}

fn utf8(bytes: &[u8]) -> Result<&str, EmbedderError> {
    let text = std::str::from_utf8(bytes).map_err(|_| EmbedderError::Encoding)?;
    Ok(text.strip_prefix('\u{feff}').unwrap_or(text))
}

pub fn tidy(text: &str) -> String {
    let mut paragraphs: Vec<String> = Vec::new();
    let mut lines: Vec<String> = Vec::new();
    for line in text.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        if words.is_empty() {
            if !lines.is_empty() {
                paragraphs.push(lines.join("\n"));
                lines.clear();
            }
        } else {
            lines.push(words.join(" "));
        }
    }
    if !lines.is_empty() {
        paragraphs.push(lines.join("\n"));
    }
    paragraphs.join("\n\n")
}

fn markdown_to_text(markdown: &str) -> String {
    let mut out = String::new();
    let mut in_fence = false;
    for line in markdown.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            out.push('\n');
            continue;
        }
        if in_fence {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if is_horizontal_rule(trimmed) {
            out.push('\n');
            continue;
        }
        let body = strip_block_markers(trimmed);
        out.push_str(&strip_inline_markup(body));
        out.push('\n');
    }
    tidy(&out)
}

fn is_horizontal_rule(line: &str) -> bool {
    let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    compact.len() >= 3 && ['-', '*', '_'].iter().any(|marker| compact.chars().all(|c| c == *marker))
}

fn strip_block_markers(line: &str) -> &str {
    let mut body = line;
    loop {
        let before = body;
        body = body.trim_start_matches('>').trim_start();
        let hashes = body.len() - body.trim_start_matches('#').len();
        if (1..=6).contains(&hashes) && body[hashes..].starts_with([' ', '\t']) {
            body = body[hashes..].trim_start();
        }
        for marker in ["- [ ] ", "- [x] ", "- ", "* ", "+ "] {
            if let Some(rest) = body.strip_prefix(marker) {
                body = rest;
            }
        }
        let digits = body.len() - body.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        if digits > 0 && (body[digits..].starts_with(". ") || body[digits..].starts_with(") ")) {
            body = &body[digits + 2..];
        }
        if body == before {
            return body;
        }
    }
}

fn strip_inline_markup(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::new();
    let mut index = 0;
    while index < chars.len() {
        let current = chars[index];
        let label_open = match current {
            '!' if chars.get(index + 1) == Some(&'[') => Some(index + 1),
            '[' => Some(index),
            _ => None,
        };
        if let Some((label, next)) = label_open.and_then(|open| link_label(&chars, open)) {
            out.push_str(&strip_inline_markup(&label));
            index = next;
            continue;
        }
        let previous = index.checked_sub(1).and_then(|at| chars.get(at)).copied();
        let next = chars.get(index + 1).copied();
        let keep = match current {
            '*' | '`' => false,
            '~' => !(previous == Some('~') || next == Some('~')),
            '_' => previous.is_some_and(char::is_alphanumeric) && next.is_some_and(char::is_alphanumeric),
            _ => true,
        };
        if keep {
            out.push(current);
        }
        index += 1;
    }
    out
}

fn link_label(chars: &[char], open: usize) -> Option<(String, usize)> {
    let close = open + chars[open..].iter().position(|c| *c == ']')?;
    if chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let paren_close = close + 1 + chars[close + 1..].iter().position(|c| *c == ')')?;
    Some((chars[open + 1..close].iter().collect(), paren_close + 1))
}

fn html_to_text(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    while let Some(open) = rest.find('<') {
        out.push_str(&decode_entities(&rest[..open]));
        let tail = &rest[open..];
        if tail.starts_with("<!--") {
            rest = tail.find("-->").map_or("", |end| &tail[end + 3..]);
            continue;
        }
        let Some(close) = tail.find('>') else {
            rest = "";
            break;
        };
        let tag = &tail[1..close];
        let closing = tag.starts_with('/');
        let name = tag_name(tag);
        rest = &tail[close + 1..];
        if !closing && !tag.ends_with('/') && SKIPPED_ELEMENTS.contains(&name.as_str()) {
            rest = after_closing_tag(rest, &name);
            continue;
        }
        if BLOCK_ELEMENTS.contains(&name.as_str()) {
            out.push_str("\n\n");
        } else if name == "br" {
            out.push('\n');
        } else if name == "td" || name == "th" || name == "img" {
            out.push(' ');
        }
    }
    out.push_str(&decode_entities(rest));
    tidy(&out)
}

fn tag_name(tag: &str) -> String {
    tag.trim_start_matches('/').chars().take_while(char::is_ascii_alphanumeric).collect::<String>().to_ascii_lowercase()
}

fn after_closing_tag<'a>(rest: &'a str, name: &str) -> &'a str {
    let needle = format!("</{name}");
    let lowered = rest.to_ascii_lowercase();
    let Some(start) = lowered.find(&needle) else {
        return "";
    };
    let tail = &rest[start..];
    tail.find('>').map_or("", |end| &tail[end + 1..])
}

fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        let decoded =
            tail.get(1..tail.len().min(12)).and_then(|window| window.find(';')).and_then(|semi| entity_char(&tail[1..semi + 1]).map(|c| (c, semi + 2)));
        match decoded {
            Some((character, consumed)) => {
                out.push(character);
                rest = &tail[consumed..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn entity_char(name: &str) -> Option<char> {
    if let Some(number) = name.strip_prefix('#') {
        let code = match number.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => number.parse().ok()?,
        };
        return char::from_u32(code);
    }
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => ' ',
        "mdash" => '—',
        "ndash" => '–',
        "hellip" => '…',
        "copy" => '©',
        "reg" => '®',
        "rsquo" => '’',
        "lsquo" => '‘',
        "rdquo" => '”',
        "ldquo" => '“',
        _ => return None,
    })
}

fn pdf_to_text(bytes: &[u8]) -> Result<String, EmbedderError> {
    if !bytes.starts_with(b"%PDF-") {
        return Err(EmbedderError::Extraction("not a PDF: missing %PDF- header".to_owned()));
    }
    let mut pieces = Vec::new();
    let mut cursor = 0;
    let mut inflated_so_far = 0;
    while let Some(found) = find_bytes(bytes, b"stream", cursor) {
        if found >= 3 && &bytes[found - 3..found] == b"end" {
            cursor = found + 6;
            continue;
        }
        let mut body_start = found + 6;
        if bytes.get(body_start) == Some(&b'\r') {
            body_start += 1;
        }
        if bytes.get(body_start) == Some(&b'\n') {
            body_start += 1;
        }
        let Some(body_end) = find_bytes(bytes, b"endstream", body_start) else {
            return Err(EmbedderError::Extraction("corrupted PDF: stream without endstream".to_owned()));
        };
        let dictionary = &bytes[found.saturating_sub(1024)..found];
        let dictionary = rfind_bytes(dictionary, b"<<").map_or(dictionary, |start| &dictionary[start..]);
        if let Some(content) = stream_content(dictionary, &bytes[body_start..body_end], &mut inflated_so_far)? {
            let text = text_from_content_stream(&content);
            if !text.trim().is_empty() {
                pieces.push(text);
            }
        }
        cursor = body_end + 9;
    }
    let text = tidy(&pieces.join("\n\n"));
    if text.is_empty() {
        return Err(EmbedderError::Extraction("PDF contains no extractable text".to_owned()));
    }
    Ok(text)
}

fn stream_content(dictionary: &[u8], raw: &[u8], inflated_so_far: &mut usize) -> Result<Option<Vec<u8>>, EmbedderError> {
    if [b"/Image".as_slice(), b"/Length1", b"/FontFile", b"/XRef"].iter().any(|marker| find_bytes(dictionary, marker, 0).is_some()) {
        return Ok(None);
    }
    if find_bytes(dictionary, b"/FlateDecode", 0).is_some() {
        return inflate_within_limits(raw, inflated_so_far);
    }
    if find_bytes(dictionary, b"/Filter", 0).is_some() {
        return Ok(None);
    }
    Ok(Some(raw.to_vec()))
}

fn inflate_within_limits(raw: &[u8], inflated_so_far: &mut usize) -> Result<Option<Vec<u8>>, EmbedderError> {
    let allowed = MAX_STREAM_BYTES.min(MAX_DOCUMENT_BYTES.saturating_sub(*inflated_so_far));
    let mut inflated = Vec::new();
    let mut capped = Read::take(flate2::read::ZlibDecoder::new(raw), u64::try_from(allowed + 1).unwrap_or(u64::MAX));
    let finished = capped.read_to_end(&mut inflated).is_ok();
    *inflated_so_far = inflated_so_far.saturating_add(inflated.len());
    if inflated.len() > allowed {
        return Err(EmbedderError::Extraction("pdf stream exceeds decompression limit".into()));
    }
    Ok(finished.then_some(inflated))
}

fn find_bytes(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    haystack.get(from..)?.windows(needle.len()).position(|window| window == needle).map(|at| at + from)
}

fn rfind_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).rposition(|window| window == needle)
}

fn text_from_content_stream(content: &[u8]) -> String {
    let mut out = String::new();
    let mut in_text = false;
    let mut index = 0;
    while index < content.len() {
        let byte = content[index];
        match byte {
            b'%' => {
                while index < content.len() && content[index] != b'\n' && content[index] != b'\r' {
                    index += 1;
                }
            }
            b'(' => {
                let (literal, next) = literal_string(content, index + 1);
                if in_text {
                    out.push_str(&decode_pdf_string(&literal));
                }
                index = next;
                continue;
            }
            b'<' if content.get(index + 1) == Some(&b'<') => index += 1,
            b'<' => {
                let end = content[index..].iter().position(|b| *b == b'>').map_or(content.len(), |at| index + at);
                if in_text {
                    out.push_str(&decode_pdf_string(&hex_string(&content[index + 1..end])));
                }
                index = end;
            }
            b'[' | b']' | b'>' => {}
            _ if byte.is_ascii_alphabetic() || byte == b'\'' || byte == b'"' || byte == b'*' => {
                let start = index;
                while index < content.len() && (content[index].is_ascii_alphanumeric() || matches!(content[index], b'*' | b'\'' | b'"')) {
                    index += 1;
                }
                match &content[start..index] {
                    b"BT" => in_text = true,
                    b"ET" => {
                        in_text = false;
                        out.push('\n');
                    }
                    b"Td" | b"TD" | b"T*" | b"'" | b"\"" | b"Tm" => out.push('\n'),
                    _ => {}
                }
                continue;
            }
            _ => {}
        }
        index += 1;
    }
    out
}

fn literal_string(content: &[u8], start: usize) -> (Vec<u8>, usize) {
    let mut out = Vec::new();
    let mut depth = 1;
    let mut index = start;
    while index < content.len() {
        let byte = content[index];
        match byte {
            b'\\' => {
                index += 1;
                match content.get(index).copied() {
                    Some(b'n') => out.push(b'\n'),
                    Some(b'r') => out.push(b'\r'),
                    Some(b't') => out.push(b'\t'),
                    Some(b'b' | b'f') => {}
                    Some(digit @ b'0'..=b'7') => {
                        let mut value = u32::from(digit - b'0');
                        for _ in 0..2 {
                            match content.get(index + 1).copied() {
                                Some(next @ b'0'..=b'7') => {
                                    value = value * 8 + u32::from(next - b'0');
                                    index += 1;
                                }
                                _ => break,
                            }
                        }
                        out.push(u8::try_from(value & 0xff).unwrap_or(b'?'));
                    }
                    Some(b'\r' | b'\n') => {}
                    Some(other) => out.push(other),
                    None => {}
                }
            }
            b'(' => {
                depth += 1;
                out.push(byte);
            }
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return (out, index + 1);
                }
                out.push(byte);
            }
            _ => out.push(byte),
        }
        index += 1;
    }
    (out, index)
}

fn hex_string(digits: &[u8]) -> Vec<u8> {
    let clean: Vec<u8> = digits.iter().copied().filter(u8::is_ascii_hexdigit).collect();
    clean
        .chunks(2)
        .filter_map(|pair| {
            let high = char::from(pair[0]).to_digit(16)?;
            let low = pair.get(1).map_or(Some(0), |low| char::from(*low).to_digit(16))?;
            u8::try_from(high * 16 + low).ok()
        })
        .collect()
}

fn decode_pdf_string(bytes: &[u8]) -> String {
    if let Some(utf16) = bytes.strip_prefix(&[0xfe, 0xff]) {
        let units: Vec<u16> = utf16.chunks(2).map(|pair| u16::from_be_bytes([pair[0], pair.get(1).copied().unwrap_or(0)])).collect();
        return String::from_utf16_lossy(&units);
    }
    bytes.iter().map(|byte| char::from(*byte)).collect()
}
