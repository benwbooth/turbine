//! Highlighting for text no grammar covers (notes, logs, configuration
//! of every kind, files of unknown type): what most text shares, found by
//! looking. Comment lines, [sections], keys before = or :, quoted strings,
//! numbers, dates and times, URLs, and a log's levels (errors red,
//! warnings and good news in their colours), with indentation as folds.

use crate::{LineSpans, kind_index};

fn k(name: &str) -> u8 {
    kind_index(name).unwrap_or(0)
}

/// Spans for every line (none past a size where looking would be slow).
pub fn spans(text: &str) -> Vec<LineSpans> {
    if text.len() > 4 << 20 {
        return vec![Vec::new(); text.lines().count().max(1)];
    }
    text.split('\n').map(line).collect()
}

fn is_word(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

fn line(l: &str) -> LineSpans {
    let b = l.as_bytes();
    let mut out: LineSpans = Vec::new();
    let t = l.trim_start();
    let lead = (l.len() - t.len()) as u32;
    let end = l.trim_end().len() as u32;
    // A comment line.
    if t.starts_with('#') || t.starts_with("//") || t.starts_with(';') || t.starts_with("-- ") || t.starts_with("REM ") || t.starts_with("rem ") || t.starts_with("! ") {
        out.push((lead, end, k("comment")));
        return out;
    }
    // [section] (ini, desktop files, TOML-ish).
    if t.starts_with('[') && t.trim_end().ends_with(']') && t.len() > 2 && !t.contains("](") {
        out.push((lead, end, k("type")));
        return out;
    }
    // key = value, key: value (the key a word with dots or dashes).
    let key_end = t.bytes().position(|c| !(is_word(c) || matches!(c, b'.' | b'-' | b'@' | b'/'))).unwrap_or(t.len());
    if key_end > 0 && key_end < t.len() {
        let rest = &t[key_end..];
        let r = rest.trim_start();
        let gap = rest.len() - r.len();
        if r.starts_with('=') || (r.starts_with(':') && (r.len() == 1 || r.as_bytes()[1] == b' ')) && !t[..key_end].bytes().all(|c| c.is_ascii_digit()) {
            let a = lead;
            out.push((a, a + key_end as u32, k("property")));
            let op = a + (key_end + gap) as u32;
            out.push((op, op + 1, k("operator")));
        }
    }
    // Words and marks along the line.
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let prev_word = i > 0 && is_word(b[i - 1]);
        // URLs.
        if (l[i..].starts_with("http://") || l[i..].starts_with("https://") || l[i..].starts_with("file://")) && !prev_word {
            let e = l[i..].find(|ch: char| ch.is_whitespace() || matches!(ch, '"' | '\'' | '>' | ')' | ']')).map_or(b.len(), |n| i + n);
            out.push((i as u32, e as u32, k("label")));
            i = e;
            continue;
        }
        // Quoted strings (a quote that's a word's apostrophe isn't one).
        if (c == b'"' || (c == b'\'' && !prev_word) || c == b'`') && !prev_word {
            if let Some(n) = l[i + 1..].find(c as char) {
                let e = i + 1 + n + 1;
                if c != b'\'' || e >= b.len() || !is_word(b[e]) {
                    out.push((i as u32, e as u32, k("string")));
                    i = e;
                    continue;
                }
            }
        }
        if c.is_ascii_digit() && !prev_word {
            // Dates and times: 2026-11-27, 12:34:56(.789), with T between.
            let mut e = i;
            while e < b.len() && (b[e].is_ascii_digit() || matches!(b[e], b'-' | b':' | b'.' | b'T' | b'Z' | b'/' | b',') && e + 1 < b.len() && b[e + 1].is_ascii_digit()) {
                e += 1;
            }
            let s = &l[i..e];
            let date = s.len() >= 8 && (s.matches('-').count() >= 2 || s.matches(':').count() >= 1 || s.matches('/').count() >= 2);
            if date {
                out.push((i as u32, e as u32, k("constant")));
                i = e;
                continue;
            }
            // Numbers: 42, 3.14, 0x1F, 1e9, 64KB.
            let mut e = i;
            if l[i..].starts_with("0x") || l[i..].starts_with("0X") {
                e += 2;
                while e < b.len() && b[e].is_ascii_hexdigit() {
                    e += 1;
                }
            } else {
                while e < b.len() && (b[e].is_ascii_digit() || b[e] == b'.' && e + 1 < b.len() && b[e + 1].is_ascii_digit() || b[e] == b'_') {
                    e += 1;
                }
                if e < b.len() && matches!(b[e], b'e' | b'E') && e + 1 < b.len() && b[e + 1].is_ascii_digit() {
                    e += 2;
                    while e < b.len() && b[e].is_ascii_digit() {
                        e += 1;
                    }
                }
            }
            // (Units stuck on are part of it; a word with digits first isn't.)
            let mut u = e;
            while u < b.len() && b[u].is_ascii_alphabetic() {
                u += 1;
            }
            if u - e <= 3 && (u == b.len() || !is_word(b[u])) {
                out.push((i as u32, u as u32, k("number")));
                i = u;
                continue;
            }
            while i < b.len() && is_word(b[i]) {
                i += 1;
            }
            continue;
        }
        if c.is_ascii_alphabetic() && !prev_word {
            let mut e = i;
            while e < b.len() && is_word(b[e]) {
                e += 1;
            }
            let w = &l[i..e];
            let kind = match w {
                "ERROR" | "Error" | "error" | "FATAL" | "Fatal" | "fatal" | "CRITICAL" | "CRIT" | "FAIL" | "FAILED" | "Failed" | "FAILURE" | "panic" | "PANIC" | "panicked" | "Exception" | "EXCEPTION" | "Traceback" | "EMERG" | "ALERT" => Some("escape"),
                "WARN" | "WARNING" | "Warning" | "warning" | "warn" | "DEPRECATED" | "deprecated" => Some("type"),
                "INFO" | "NOTICE" | "OK" | "ok" | "PASS" | "PASSED" | "SUCCESS" | "Success" | "DONE" | "true" | "false" | "True" | "False" | "yes" | "no" | "on" | "off" | "null" | "None" | "nil" => Some("constant"),
                "DEBUG" | "TRACE" | "VERBOSE" | "debug" | "trace" => Some("comment"),
                "TODO" | "FIXME" | "XXX" | "HACK" | "NOTE" => Some("special"),
                _ => None,
            };
            if let Some(kind) = kind {
                out.push((i as u32, e as u32, k(kind)));
            }
            i = e;
            continue;
        }
        i += c.max(1).leading_ones().max(1) as usize;
        while i < b.len() && !l.is_char_boundary(i) {
            i += 1;
        }
    }
    out.sort_by_key(|s| s.0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(l: &str) -> Vec<(&str, &'static str)> {
        line(l).into_iter().map(|(a, b, kd)| (&l[a as usize..b as usize], crate::KINDS[kd as usize])).collect()
    }

    #[test]
    fn plain_text_has_colours() {
        assert_eq!(kinds("# a comment"), vec![("# a comment", "comment")]);
        assert_eq!(kinds("[Desktop Entry]"), vec![("[Desktop Entry]", "type")]);
        assert_eq!(kinds("Name=Turbine 2"), vec![("Name", "property"), ("=", "operator"), ("2", "number")]);
        assert_eq!(
            kinds("2026-11-27 12:34:56 ERROR can't open \"x.txt\" (see https://x.org/a)"),
            vec![("2026-11-27", "constant"), ("12:34:56", "constant"), ("ERROR", "escape"), ("\"x.txt\"", "string"), ("https://x.org/a", "label")]
        );
        assert_eq!(kinds("took 64ms, v2 and abc123"), vec![("64ms", "number")]);
        assert_eq!(kinds("  timeout: 30"), vec![("timeout", "property"), (":", "operator"), ("30", "number")]);
        assert!(kinds("Just some words, don't worry.").is_empty());
        assert!(kinds("ünïcödé 1 ok").iter().any(|k| k.0 == "1"));
    }
}
