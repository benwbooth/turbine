//! tessh's highlighting, as Tessera does it (and Grove, from here): the
//! highlight query's captures, then "…" strings split so what they
//! interpolate (`$name`, `$(form)`, `${expr}`) reads as code. Spans are
//! byte ranges with a kind: a capture name's first part ("keyword",
//! "string", "operator", "embedded", "escape", …), later over earlier.

use std::sync::OnceLock;
use tree_sitter::{Node, Parser, Query, QueryCursor, StreamingIterator};

fn query() -> &'static Query {
    static Q: OnceLock<Query> = OnceLock::new();
    Q.get_or_init(|| Query::new(&crate::LANGUAGE.into(), crate::HIGHLIGHTS_QUERY).expect("tessh's highlight query"))
}

/// A whole text's spans: (start, end, kind).
pub fn spans(text: &str) -> Vec<(usize, usize, &'static str)> {
    let mut p = Parser::new();
    if p.set_language(&crate::LANGUAGE.into()).is_err() {
        return vec![];
    }
    let Some(tree) = p.parse(text, None) else { return vec![] };
    split_strings(text, captures(tree.root_node(), text))
}

/// The query's captures in a tree of `text`: (start, end, kind).
pub fn captures(root: Node, text: &str) -> Vec<(usize, usize, &'static str)> {
    let q = query();
    let names = q.capture_names();
    let mut out = vec![];
    let mut cursor = QueryCursor::new();
    let mut caps = cursor.captures(q, root, text.as_bytes());
    while let Some((m, i)) = caps.next() {
        let c = m.captures()[*i];
        let (s, e) = (c.node.start_byte(), c.node.end_byte().min(text.len()));
        if e > s {
            out.push((s, e, names[c.index as usize].split('.').next().unwrap_or("")));
        }
    }
    out
}

/// Spans with each "string" one split as a tessh string reads.
pub fn split_strings(text: &str, spans: Vec<(usize, usize, &'static str)>) -> Vec<(usize, usize, &'static str)> {
    spans.into_iter().flat_map(|(s, e, k)| if k == "string" { tessh_string(text, s, e) } else { vec![(s, e, k)] }).collect()
}

/// tessh code inside a string (text[s..e]), highlighted on its own.
fn tessh_code(text: &str, s: usize, e: usize) -> Vec<(usize, usize, &'static str)> {
    spans(&text[s..e]).into_iter().map(|(a, b, k)| (a + s, b + s, k)).collect()
}

/// Where the `close` that ends a group opened just before `from` is, in
/// b[..end]: brackets nest and quotes hide.
fn group_end(b: &[u8], from: usize, end: usize, close: u8) -> Option<usize> {
    let (mut depth, mut quote, mut i) = (0i32, None::<u8>, from);
    while i < end {
        let c = b[i];
        if let Some(q) = quote {
            if c == b'\\' {
                i += 1;
            } else if c == q {
                quote = None;
            }
        } else if c == close && depth == 0 {
            return Some(i);
        } else if c == b'"' || c == b'\'' {
            quote = Some(c);
        } else if b"([{".contains(&c) {
            depth += 1;
        } else if b")]}".contains(&c) {
            depth -= 1;
        }
        i += 1;
    }
    None
}

/// Whether the heredoc body starting at `s` (its first newline) fills in
/// `$`: its end word, on the line before, isn't quoted (<<END, not <<'END').
fn heredoc_interpolates(text: &str, s: usize) -> bool {
    let line = &text[text[..s].rfind('\n').map_or(0, |i| i + 1)..s];
    let mut k = 0;
    let mut last = None;
    while let Some(i) = line[k..].find("<<") {
        let at = k + i;
        if !line[at + 2..].starts_with('<') && (at == 0 || !line[..at].ends_with('<')) {
            last = Some(at + 2);
        }
        k = at + 2;
    }
    match last {
        Some(a) => !line[a..].trim_start_matches('-').starts_with(['\'', '"']),
        None => false,
    }
}

/// A tessh string's spans. Double quotes (no prefix) interpolate: `$name`,
/// `$name.attr[i]`, `$(form)` and `${expr!r:spec}` are code there, their
/// `$(` `)` `${` `}` marked (embedded), and escapes are escapes. So does a
/// heredoc's body (<<END, not <<'END'), where a backslash is only special
/// before `$`, `` ` `` and itself, as in bash.
fn tessh_string(text: &str, s: usize, e: usize) -> Vec<(usize, usize, &'static str)> {
    let t = &text[s..e];
    let heredoc = t.starts_with('\n');
    let q = if t.len() >= 6 && t.starts_with("\"\"\"") && t.ends_with("\"\"\"") {
        3
    } else if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        1
    } else if heredoc && heredoc_interpolates(text, s) {
        0
    } else {
        return vec![(s, e, "string")];
    };
    let b = text.as_bytes();
    let end = e - q;
    let word = |j: usize| j < end && (b[j].is_ascii_alphabetic() || b[j] == b'_');
    let word_end = |mut j: usize| {
        while j < end && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
            j += 1;
        }
        j
    };
    let mut out = vec![];
    let mut lit = s; // (where the text being read began)
    let mut i = s + q;
    while i < end {
        let start = i;
        let mut code: Vec<(usize, usize, &'static str)> = vec![];
        if b[i] == b'\\' && heredoc && !matches!(b.get(i + 1), Some(b'$' | b'`' | b'\\')) {
            i += 1;
            continue;
        } else if b[i] == b'\\' {
            let n = text[i + 1..].chars().next().map_or(0, |c| c.len_utf8());
            i = (i + 1 + n).min(end);
            code.push((start, i, "escape"));
        } else if b[i] == b'$' && i + 1 < end && (b[i + 1] == b'(' || b[i + 1] == b'{') {
            let braced = b[i + 1] == b'{';
            let Some(j) = group_end(b, i + 2, end, if braced { b'}' } else { b')' }) else {
                i += 1;
                continue;
            };
            code.push((i, i + 2, "embedded"));
            let mut x = j;
            if braced {
                // (As the reader splits it: the expression, then !r and :spec.)
                let body = &text[i + 2..j];
                let mut cut = body.len();
                let mut depth = 0;
                let mut quote = None;
                for (k, c) in body.char_indices() {
                    match (quote, c) {
                        (Some(qc), c) if c == qc => quote = None,
                        (Some(_), _) => {}
                        (None, '"' | '\'') => quote = Some(c),
                        (None, '(' | '[' | '{') => depth += 1,
                        (None, ')' | ']' | '}') => depth -= 1,
                        (None, ':') if depth == 0 && !body[k + 1..].starts_with('=') => {
                            cut = k;
                            break;
                        }
                        _ => {}
                    }
                }
                let expr = body[..cut].trim_end();
                let expr = match expr.len().checked_sub(2).map(|k| &expr[k..]) {
                    Some("!r" | "!s" | "!a") => &expr[..expr.len() - 2],
                    _ => expr,
                };
                x = i + 2 + expr.len();
                // (A name alone is the variable's value; anything else a form.)
                let lead = expr.len() - expr.trim_start().len();
                let name = expr.trim();
                if !name.is_empty() && name.split('.').all(|p| p.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')) {
                    code.push((i + 2 + lead, i + 2 + lead + name.len(), "variable"));
                } else {
                    code.extend(tessh_code(text, i + 2, x));
                }
                let tail = x + (j - x - text[x..j].trim_start().len());
                if tail < j {
                    code.push((tail, j, "attribute"));
                }
            } else {
                code.extend(tessh_code(text, i + 2, x));
            }
            code.push((j, j + 1, "embedded"));
            i = j + 1;
        } else if b[i] == b'$' {
            // $name, then .attr and [index] glued on; $? and $%….
            let mut j = i + 1;
            if j < end && b[j] == b'?' {
                j += 1;
            } else if j < end && b[j] == b'%' {
                j += 1;
                if j < end && b"123456789&".contains(&b[j]) {
                    j += 1;
                }
            } else if word(j) {
                j = word_end(j);
                loop {
                    if j < end && b[j] == b'.' && word(j + 1) {
                        j = word_end(j + 1);
                    } else if j < end && b[j] == b'[' {
                        let Some(k) = group_end(b, j + 1, end, b']') else { break };
                        code.push((i, j, "variable"));
                        code.push((j, j + 1, "punctuation"));
                        code.extend(tessh_code(text, j + 1, k));
                        code.push((k, k + 1, "punctuation"));
                        i = k + 1;
                        j = i;
                    } else {
                        break;
                    }
                }
            }
            if j == start + 1 {
                i += 1;
                continue;
            }
            if j > i {
                code.push((i, j, "variable"));
            }
            i = j;
        } else {
            i += 1;
            continue;
        }
        if start > lit {
            out.push((lit, start, "string"));
        }
        out.extend(code);
        lit = i;
    }
    if e > lit {
        out.push((lit, e, "string"));
    }
    out
}

