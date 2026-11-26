//! Encoding detection and conversion.
//!
//! Files are decoded on open and encoded back on save in the encoding they
//! came in. Detection order: BOM, valid UTF-8, DOS code page heuristic
//! (box drawing), then `chardetng` for everything the web knows about.

use encoding_rs::Encoding;

/// A text encoding Turbine can read and write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextEncoding {
    Utf8,
    Utf16Le,
    Utf16Be,
    /// Any encoding `encoding_rs` supports (Windows-125x, ISO-8859-x,
    /// Shift_JIS, GBK, Big5, EUC-KR, KOI8, IBM866, ...).
    Web(&'static Encoding),
    /// A DOS OEM code page that `encoding_rs` doesn't cover.
    Dos(CodePage),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodePage {
    Cp437,
    Cp850,
}

impl TextEncoding {
    pub fn name(&self) -> &'static str {
        match self {
            TextEncoding::Utf8 => "UTF-8",
            TextEncoding::Utf16Le => "UTF-16LE",
            TextEncoding::Utf16Be => "UTF-16BE",
            TextEncoding::Web(e) => e.name(),
            TextEncoding::Dos(CodePage::Cp437) => "CP437",
            TextEncoding::Dos(CodePage::Cp850) => "CP850",
        }
    }

    /// Look an encoding up by label ("cp437", "latin1", "shift_jis", ...).
    pub fn for_label(label: &str) -> Option<TextEncoding> {
        let l = label.trim().to_ascii_lowercase();
        match l.as_str() {
            "utf-8" | "utf8" => Some(TextEncoding::Utf8),
            "utf-16le" | "utf16le" | "utf-16" => Some(TextEncoding::Utf16Le),
            "utf-16be" | "utf16be" => Some(TextEncoding::Utf16Be),
            "cp437" | "ibm437" | "437" | "dos" => Some(TextEncoding::Dos(CodePage::Cp437)),
            "cp850" | "ibm850" | "850" => Some(TextEncoding::Dos(CodePage::Cp850)),
            _ => Encoding::for_label(l.as_bytes()).map(TextEncoding::Web),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    CrLf,
    Cr,
}

impl LineEnding {
    pub fn as_str(&self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::CrLf => "\r\n",
            LineEnding::Cr => "\r",
        }
    }
    pub fn name(&self) -> &'static str {
        match self {
            LineEnding::Lf => "LF",
            LineEnding::CrLf => "CRLF",
            LineEnding::Cr => "CR",
        }
    }
}

/// The result of decoding a file.
#[derive(Clone, Debug)]
pub struct Decoded {
    /// Text with line endings normalised to `\n`.
    pub text: String,
    pub encoding: TextEncoding,
    pub bom: bool,
    pub line_ending: LineEnding,
    /// True when some bytes could not be decoded and were replaced.
    pub lossy: bool,
}

/// Detect the encoding of `bytes` without decoding.
pub fn detect(bytes: &[u8]) -> (TextEncoding, bool) {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return (TextEncoding::Utf8, true);
    }
    if bytes.starts_with(&[0xFF, 0xFE]) {
        return (TextEncoding::Utf16Le, true);
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        return (TextEncoding::Utf16Be, true);
    }
    if let Some(e) = utf16_without_bom(bytes) {
        return (e, false);
    }
    if std::str::from_utf8(bytes).is_ok() {
        return (TextEncoding::Utf8, false);
    }
    if looks_like_dos(bytes) {
        return (TextEncoding::Dos(CodePage::Cp437), false);
    }
    let mut det = chardetng::EncodingDetector::new();
    det.feed(bytes, true);
    (TextEncoding::Web(det.guess(None, true)), false)
}

/// UTF-16 text without a BOM: mostly ASCII means every other byte is zero.
fn utf16_without_bom(bytes: &[u8]) -> Option<TextEncoding> {
    if bytes.len() < 8 || bytes.len() % 2 != 0 {
        return None;
    }
    let sample = &bytes[..bytes.len().min(4096)];
    let (mut even0, mut odd0) = (0usize, 0usize);
    for (i, b) in sample.iter().enumerate() {
        if *b == 0 {
            if i % 2 == 0 { even0 += 1 } else { odd0 += 1 }
        }
    }
    let half = sample.len() / 2;
    if odd0 * 10 > half * 7 && even0 * 10 < half {
        Some(TextEncoding::Utf16Le)
    } else if even0 * 10 > half * 7 && odd0 * 10 < half {
        Some(TextEncoding::Utf16Be)
    } else {
        None
    }
}

/// DOS-era text: high bytes are mostly box drawing / block characters
/// (0xB0..=0xDF in CP437), which would be accented letters in Latin-1.
fn looks_like_dos(bytes: &[u8]) -> bool {
    let (mut high, mut boxy) = (0usize, 0usize);
    for &b in bytes {
        if b >= 0x80 {
            high += 1;
            if (0xB0..=0xDF).contains(&b) {
                boxy += 1;
            }
        }
    }
    high >= 4 && boxy * 10 >= high * 6
}

/// Decode a file's bytes, detecting the encoding.
pub fn decode(bytes: &[u8]) -> Decoded {
    let (enc, bom) = detect(bytes);
    decode_as(bytes, enc, bom)
}

/// Decode with a chosen encoding ("Reopen with Encoding").
pub fn decode_as(bytes: &[u8], enc: TextEncoding, bom: bool) -> Decoded {
    let (raw, lossy) = match enc {
        TextEncoding::Utf8 => {
            let b = if bom && bytes.starts_with(&[0xEF, 0xBB, 0xBF]) { &bytes[3..] } else { bytes };
            let s = String::from_utf8_lossy(b);
            let lossy = matches!(s, std::borrow::Cow::Owned(_));
            (s.into_owned(), lossy)
        }
        TextEncoding::Utf16Le | TextEncoding::Utf16Be => {
            let e = if enc == TextEncoding::Utf16Le { encoding_rs::UTF_16LE } else { encoding_rs::UTF_16BE };
            let (s, had_errors) = e.decode_with_bom_removal(bytes);
            (s.into_owned(), had_errors)
        }
        TextEncoding::Web(e) => {
            let (s, _, had_errors) = e.decode(bytes);
            (s.into_owned(), had_errors)
        }
        TextEncoding::Dos(cp) => (bytes.iter().map(|&b| dos_char(cp, b)).collect(), false),
    };
    let line_ending = detect_line_ending(&raw);
    let text = if raw.contains('\r') { normalize_newlines(&raw) } else { raw };
    Decoded { text, encoding: enc, bom, line_ending, lossy }
}

/// Encode text (with `\n` line endings) for saving.
pub fn encode(text: &str, enc: TextEncoding, bom: bool, le: LineEnding) -> Vec<u8> {
    let text: std::borrow::Cow<str> =
        if le == LineEnding::Lf { text.into() } else { text.replace('\n', le.as_str()).into() };
    match enc {
        TextEncoding::Utf8 => {
            let mut out = Vec::with_capacity(text.len() + 3);
            if bom {
                out.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
            }
            out.extend_from_slice(text.as_bytes());
            out
        }
        TextEncoding::Utf16Le | TextEncoding::Utf16Be => {
            let le16 = enc == TextEncoding::Utf16Le;
            let mut out = Vec::with_capacity(text.len() * 2 + 2);
            let mut push = |u: u16| {
                let b = if le16 { u.to_le_bytes() } else { u.to_be_bytes() };
                out.extend_from_slice(&b);
            };
            if bom {
                push(0xFEFF);
            }
            for u in text.encode_utf16() {
                push(u);
            }
            out
        }
        TextEncoding::Web(e) => e.encode(&text).0.into_owned(),
        TextEncoding::Dos(cp) => text.chars().map(|c| dos_byte(cp, c)).collect(),
    }
}

fn detect_line_ending(s: &str) -> LineEnding {
    let b = s.as_bytes();
    for (i, &c) in b.iter().enumerate() {
        match c {
            b'\r' => return if b.get(i + 1) == Some(&b'\n') { LineEnding::CrLf } else { LineEnding::Cr },
            b'\n' => return LineEnding::Lf,
            _ => {}
        }
    }
    if cfg!(windows) { LineEnding::CrLf } else { LineEnding::Lf }
}

fn normalize_newlines(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\r' {
            if it.peek() == Some(&'\n') {
                it.next();
            }
            out.push('\n');
        } else {
            out.push(c);
        }
    }
    out
}

fn dos_char(cp: CodePage, b: u8) -> char {
    if b < 0x80 {
        return b as char;
    }
    let table = match cp {
        CodePage::Cp437 => &CP437_HIGH,
        CodePage::Cp850 => &CP850_HIGH,
    };
    table[(b - 0x80) as usize]
}

fn dos_byte(cp: CodePage, c: char) -> u8 {
    if (c as u32) < 0x80 {
        return c as u8;
    }
    let table = match cp {
        CodePage::Cp437 => &CP437_HIGH,
        CodePage::Cp850 => &CP850_HIGH,
    };
    table.iter().position(|&t| t == c).map(|i| i as u8 + 0x80).unwrap_or(b'?')
}

/// CP437 bytes 0x80..=0xFF.
pub const CP437_HIGH: [char; 128] = [
    'Ç', 'ü', 'é', 'â', 'ä', 'à', 'å', 'ç', 'ê', 'ë', 'è', 'ï', 'î', 'ì', 'Ä', 'Å',
    'É', 'æ', 'Æ', 'ô', 'ö', 'ò', 'û', 'ù', 'ÿ', 'Ö', 'Ü', '¢', '£', '¥', '₧', 'ƒ',
    'á', 'í', 'ó', 'ú', 'ñ', 'Ñ', 'ª', 'º', '¿', '⌐', '¬', '½', '¼', '¡', '«', '»',
    '░', '▒', '▓', '│', '┤', '╡', '╢', '╖', '╕', '╣', '║', '╗', '╝', '╜', '╛', '┐',
    '└', '┴', '┬', '├', '─', '┼', '╞', '╟', '╚', '╔', '╩', '╦', '╠', '═', '╬', '╧',
    '╨', '╤', '╥', '╙', '╘', '╒', '╓', '╫', '╪', '┘', '┌', '█', '▄', '▌', '▐', '▀',
    'α', 'ß', 'Γ', 'π', 'Σ', 'σ', 'µ', 'τ', 'Φ', 'Θ', 'Ω', 'δ', '∞', 'φ', 'ε', '∩',
    '≡', '±', '≥', '≤', '⌠', '⌡', '÷', '≈', '°', '∙', '·', '√', 'ⁿ', '²', '■', '\u{a0}',
];

/// CP850 bytes 0x80..=0xFF.
pub const CP850_HIGH: [char; 128] = [
    'Ç', 'ü', 'é', 'â', 'ä', 'à', 'å', 'ç', 'ê', 'ë', 'è', 'ï', 'î', 'ì', 'Ä', 'Å',
    'É', 'æ', 'Æ', 'ô', 'ö', 'ò', 'û', 'ù', 'ÿ', 'Ö', 'Ü', 'ø', '£', 'Ø', '×', 'ƒ',
    'á', 'í', 'ó', 'ú', 'ñ', 'Ñ', 'ª', 'º', '¿', '®', '¬', '½', '¼', '¡', '«', '»',
    '░', '▒', '▓', '│', '┤', 'Á', 'Â', 'À', '©', '╣', '║', '╗', '╝', '¢', '¥', '┐',
    '└', '┴', '┬', '├', '─', '┼', 'ã', 'Ã', '╚', '╔', '╩', '╦', '╠', '═', '╬', '¤',
    'ð', 'Ð', 'Ê', 'Ë', 'È', 'ı', 'Í', 'Î', 'Ï', '┘', '┌', '█', '▄', '¦', 'Ì', '▀',
    'Ó', 'ß', 'Ô', 'Ò', 'õ', 'Õ', 'µ', 'þ', 'Þ', 'Ú', 'Û', 'Ù', 'ý', 'Ý', '¯', '´',
    '\u{ad}', '±', '‗', '¾', '¶', '§', '÷', '¸', '°', '¨', '·', '¹', '³', '²', '■', '\u{a0}',
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_and_bom() {
        let d = decode("héllo\r\nworld".as_bytes());
        assert_eq!(d.encoding, TextEncoding::Utf8);
        assert_eq!(d.text, "héllo\nworld");
        assert_eq!(d.line_ending, LineEnding::CrLf);
        let d = decode(b"\xEF\xBB\xBFhi");
        assert!(d.bom);
        assert_eq!(d.text, "hi");
    }

    #[test]
    fn dos_box_drawing() {
        let bytes = b"\xC9\xCD\xCD\xBB\r\n\xBA hi \xBA\r\n\xC8\xCD\xCD\xBC";
        let d = decode(bytes);
        assert_eq!(d.encoding, TextEncoding::Dos(CodePage::Cp437));
        assert!(d.text.starts_with("╔══╗"));
        assert_eq!(encode(&d.text, d.encoding, false, d.line_ending), bytes.to_vec());
    }

    #[test]
    fn latin1_guess() {
        let d = decode("Grüße aus München, schöne Grüße".encode_utf16().map(|u| u as u8).collect::<Vec<_>>().as_slice());
        assert!(d.text.contains("München"), "{:?} {}", d.encoding, d.text);
    }

    #[test]
    fn utf16_no_bom() {
        let bytes: Vec<u8> = "hello world".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        assert_eq!(decode(&bytes).text, "hello world");
    }
}
