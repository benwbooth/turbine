//! What a file is, as tessh's viewer tells (by its first bytes, then its
//! name): text Turbine edits, or pictures, PDFs, media, office documents,
//! archives and other binaries the desktop's own app shows (xdg-open).

use std::io::Read;
use std::path::Path;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Text,
    Image,
    Pdf,
    Archive,
    /// Media, office documents, fonts…: an app of their own shows them.
    Outside,
    Binary,
}

fn head(p: &Path) -> Vec<u8> {
    let mut buf = vec![0u8; 8192];
    let Ok(mut f) = std::fs::File::open(p) else { return vec![] };
    let mut n = 0;
    while n < buf.len() {
        match f.read(&mut buf[n..]) {
            Ok(0) | Err(_) => break,
            Ok(k) => n += k,
        }
    }
    buf.truncate(n);
    buf
}

/// Text in any encoding: no NUL bytes (but UTF-16), and few control
/// characters. (Unlike tessh's rule, bytes that aren't UTF-8 are fine:
/// they're a code page Turbine decodes.)
pub fn is_text(b: &[u8]) -> bool {
    if b.starts_with(&[0xff, 0xfe]) || b.starts_with(&[0xfe, 0xff]) {
        return true;
    }
    if matches!(turbine_text::encoding::detect(b).0, turbine_text::encoding::TextEncoding::Utf16Le | turbine_text::encoding::TextEncoding::Utf16Be) {
        return true;
    }
    if b.contains(&0) {
        return false;
    }
    let ctl = b.iter().filter(|&&c| c < 0x20 && !matches!(c, b'\n' | b'\r' | b'\t' | 0x0c | 0x1b | 0x08 | 0x1a)).count();
    ctl * 20 <= b.len().max(1)
}

const OUTSIDE: &[&str] = &[
    "mp3", "flac", "ogg", "oga", "opus", "wav", "m4a", "aac", "wma", "aiff", "mid", "midi", "mp4", "m4v", "mkv", "webm", "avi", "mov", "wmv", "flv", "mpg",
    "mpeg", "3gp", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt", "ods", "odp", "odg", "rtf", "epub", "mobi", "djvu", "ttf", "otf", "woff", "woff2", "tif",
    "tiff", "heic", "heif", "avif", "ico", "psd", "xcf", "kra", "blend", "dmg", "exe", "msi", "sqlite", "db", "png", "jpg", "jpeg", "gif", "webp", "bmp",
];

pub fn classify(p: &Path) -> Kind {
    let b = head(p);
    let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if b.starts_with(b"\x89PNG") || b.starts_with(&[0xff, 0xd8, 0xff]) || b.starts_with(b"GIF8") || (b.starts_with(b"RIFF") && b.get(8..12) == Some(b"WEBP")) || (b.starts_with(b"BM") && ext == "bmp") {
        return Kind::Image;
    }
    if b.starts_with(b"%PDF") {
        return Kind::Pdf;
    }
    if b.starts_with(b"PK\x03\x04") || b.starts_with(b"PK\x05\x06") {
        return if OUTSIDE.contains(&ext.as_str()) { Kind::Outside } else { Kind::Archive };
    }
    let compressed = b.starts_with(&[0x1f, 0x8b]) || b.starts_with(b"BZh") || b.starts_with(&[0xfd, b'7', b'z', b'X', b'Z', 0]) || b.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]);
    if compressed || b.starts_with(&[b'7', b'z', 0xbc, 0xaf, 0x27, 0x1c]) || b.starts_with(b"Rar!") || b.get(257..262) == Some(b"ustar") {
        return Kind::Archive;
    }
    if matches!(ext.as_str(), "iso" | "7z" | "rar" | "cab" | "deb" | "rpm" | "cpio" | "tar" | "tgz" | "txz") {
        return Kind::Archive;
    }
    if is_text(&b) {
        return Kind::Text;
    }
    if OUTSIDE.contains(&ext.as_str()) { Kind::Outside } else { Kind::Binary }
}

/// Open a file in the desktop's app for it, detached (it outlives Turbine).
pub fn open_outside(p: &Path) -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;
    let prog = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    let p = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let mut c = std::process::Command::new(prog);
    c.arg(&p).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    unsafe {
        c.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    c.spawn().map(|_| ())
}

/// A hex dump (offset, 16 bytes, and what they read as), the first 1 MB.
pub fn hex_dump(p: &Path) -> String {
    let bytes = std::fs::read(p).unwrap_or_default();
    let shown = &bytes[..bytes.len().min(1 << 20)];
    let mut out = String::with_capacity(shown.len() * 4 + 64);
    for (i, row) in shown.chunks(16).enumerate() {
        out.push_str(&format!("{:08x}  ", i * 16));
        for j in 0..16 {
            match row.get(j) {
                Some(b) => out.push_str(&format!("{b:02x} ")),
                None => out.push_str("   "),
            }
            if j == 7 {
                out.push(' ');
            }
        }
        out.push(' ');
        out.extend(row.iter().map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '·' }));
        out.push('\n');
    }
    if bytes.len() > shown.len() {
        out.push_str(&format!("… {} more bytes\n", bytes.len() - shown.len()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds() {
        let dir = std::env::temp_dir().join(format!("turbine-kind-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let w = |n: &str, b: &[u8]| {
            std::fs::write(dir.join(n), b).unwrap();
            classify(&dir.join(n))
        };
        assert_eq!(w("a.png", b"\x89PNG\r\n\x1a\n\0\0"), Kind::Image);
        assert_eq!(w("a.pdf", b"%PDF-1.7\n"), Kind::Pdf);
        assert_eq!(w("a.docx", b"PK\x03\x04rest"), Kind::Outside);
        assert_eq!(w("a.zip", b"PK\x03\x04rest"), Kind::Archive);
        assert_eq!(w("dos.txt", b"\xc9\xcd\xcd\xbb hi \xba\r\n"), Kind::Text);
        assert_eq!(w("u16.txt", "hi there you".encode_utf16().flat_map(|u| u.to_le_bytes()).collect::<Vec<_>>().as_slice()), Kind::Text);
        assert_eq!(w("a.bin", &[0, 1, 2, 3, 0, 0, 7, 8, 0, 0]), Kind::Binary);
        assert!(hex_dump(&dir.join("a.bin")).starts_with("00000000  00 01 02 03"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
