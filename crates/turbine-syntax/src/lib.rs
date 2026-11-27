//! Syntax highlighting with tree-sitter.
//!
//! Seventeen grammars are built in; any other language Helix has (~250) is
//! loaded from Helix's runtime, or fetched once from Helix's release and
//! unpacked when first needed (`grammars`, from Tessera). Parsing runs on a
//! worker thread: the editor asks with [`Highlighter::request`] and picks up
//! results on its ticks, so a big file never stalls typing.

pub mod grammars;

use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};
use streaming_iterator::StreamingIterator;
use tree_sitter::{Language, Parser, Query, QueryCursor};

/// Highlight kinds, in the order their indexes are used everywhere.
pub const KINDS: &[&str] = &[
    "keyword", "string", "comment", "number", "constant", "function", "operator", "property", "variable", "embedded",
    "type", "constructor", "punctuation", "escape", "attribute", "tag", "label", "namespace", "special",
];

pub fn kind_index(name: &str) -> Option<u8> {
    KINDS.iter().position(|k| *k == name).map(|i| i as u8)
}

/// Built-in languages: (name, label).
pub const BUILTIN: &[(&str, &str)] = &[
    ("bash", "Shell"),
    ("c", "C"),
    ("cpp", "C++"),
    ("css", "CSS"),
    ("go", "Go"),
    ("html", "HTML"),
    ("java", "Java"),
    ("javascript", "JavaScript"),
    ("json", "JSON"),
    ("lua", "Lua"),
    ("markdown", "Markdown"),
    ("python", "Python"),
    ("ruby", "Ruby"),
    ("rust", "Rust"),
    ("toml", "TOML"),
    ("typescript", "TypeScript"),
    ("tsx", "TSX"),
    ("yaml", "YAML"),
];

/// The language of a file, from its name, extension or first line.
pub fn detect(path: Option<&Path>, first_line: &str) -> Option<String> {
    if let Some(p) = path {
        let name = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        let by_name = match name.as_str() {
            "makefile" | "gnumakefile" => Some("make"),
            "dockerfile" | "containerfile" => Some("dockerfile"),
            "cmakelists.txt" => Some("cmake"),
            "cargo.lock" | "flake.lock" => Some(if name == "flake.lock" { "json" } else { "toml" }),
            ".bashrc" | ".bash_profile" | ".profile" | ".zshrc" | ".zprofile" => Some("bash"),
            _ => None,
        };
        if let Some(l) = by_name {
            return Some(l.into());
        }
        let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        let by_ext = match ext.as_str() {
            "rs" => "rust",
            "py" | "pyw" | "pyi" => "python",
            "js" | "mjs" | "cjs" | "jsx" => "javascript",
            "ts" | "mts" | "cts" => "typescript",
            "tsx" => "tsx",
            "c" | "h" => "c",
            "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" | "ipp" => "cpp",
            "go" => "go",
            "java" => "java",
            "rb" => "ruby",
            "lua" => "lua",
            "json" | "jsonc" => "json",
            "yaml" | "yml" => "yaml",
            "toml" => "toml",
            "html" | "htm" => "html",
            "css" => "css",
            "md" | "markdown" => "markdown",
            "sh" | "bash" | "zsh" => "bash",
            "nix" => "nix",
            "zig" => "zig",
            "hs" => "haskell",
            "ml" | "mli" => "ocaml",
            "ex" | "exs" => "elixir",
            "erl" => "erlang",
            "kt" | "kts" => "kotlin",
            "swift" => "swift",
            "cs" => "c-sharp",
            "fs" | "fsx" => "fsharp",
            "php" => "php",
            "pl" | "pm" => "perl",
            "r" => "r",
            "scala" => "scala",
            "sql" => "sql",
            "vim" => "vim",
            "el" => "elisp",
            "clj" | "cljs" | "edn" => "clojure",
            "scm" | "ss" => "scheme",
            "dart" => "dart",
            "jl" => "julia",
            "qml" => "qml",
            "xml" | "svg" => "xml",
            "proto" => "protobuf",
            "bas" | "bi" | "bm" => "basic",
            "pas" | "pp" | "inc" => "pascal",
            "asm" | "s" => "nasm",
            "f" | "f90" | "for" => "fortran",
            "cob" | "cbl" => "cobol",
            "bat" | "cmd" => "batch",
            "ps1" => "powershell",
            _ => "",
        };
        if !by_ext.is_empty() {
            return Some(by_ext.into());
        }
    }
    let l = first_line.trim();
    if let Some(shebang) = l.strip_prefix("#!") {
        let prog = shebang.split_whitespace().map(|w| w.rsplit('/').next().unwrap_or(w)).find(|w| *w != "env")?;
        let lang = match prog {
            p if p.starts_with("python") => "python",
            "sh" | "bash" | "zsh" | "dash" | "ksh" => "bash",
            "node" | "deno" | "bun" => "javascript",
            "ruby" => "ruby",
            "lua" => "lua",
            "perl" => "perl",
            _ => return None,
        };
        return Some(lang.into());
    }
    None
}

struct Lang {
    language: Language,
    query: Query,
}

fn make(name: &str) -> Option<Lang> {
    let (language, query): (Language, String) = match name {
        "bash" => (tree_sitter_bash::LANGUAGE.into(), tree_sitter_bash::HIGHLIGHT_QUERY.into()),
        "python" => (tree_sitter_python::LANGUAGE.into(), tree_sitter_python::HIGHLIGHTS_QUERY.into()),
        "javascript" => (tree_sitter_javascript::LANGUAGE.into(), tree_sitter_javascript::HIGHLIGHT_QUERY.into()),
        "typescript" => (
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            format!("{}\n{}", tree_sitter_javascript::HIGHLIGHT_QUERY, tree_sitter_typescript::HIGHLIGHTS_QUERY),
        ),
        "tsx" => (
            tree_sitter_typescript::LANGUAGE_TSX.into(),
            format!("{}\n{}", tree_sitter_javascript::HIGHLIGHT_QUERY, tree_sitter_typescript::HIGHLIGHTS_QUERY),
        ),
        "rust" => (tree_sitter_rust::LANGUAGE.into(), tree_sitter_rust::HIGHLIGHTS_QUERY.into()),
        "go" => (tree_sitter_go::LANGUAGE.into(), tree_sitter_go::HIGHLIGHTS_QUERY.into()),
        "c" => (tree_sitter_c::LANGUAGE.into(), tree_sitter_c::HIGHLIGHT_QUERY.into()),
        "cpp" => (tree_sitter_cpp::LANGUAGE.into(), format!("{}\n{}", tree_sitter_c::HIGHLIGHT_QUERY, tree_sitter_cpp::HIGHLIGHT_QUERY)),
        "java" => (tree_sitter_java::LANGUAGE.into(), tree_sitter_java::HIGHLIGHTS_QUERY.into()),
        "ruby" => (tree_sitter_ruby::LANGUAGE.into(), tree_sitter_ruby::HIGHLIGHTS_QUERY.into()),
        "lua" => (tree_sitter_lua::LANGUAGE.into(), tree_sitter_lua::HIGHLIGHTS_QUERY.into()),
        "json" => (tree_sitter_json::LANGUAGE.into(), tree_sitter_json::HIGHLIGHTS_QUERY.into()),
        "yaml" => (tree_sitter_yaml::LANGUAGE.into(), tree_sitter_yaml::HIGHLIGHTS_QUERY.into()),
        "toml" => (tree_sitter_toml_ng::LANGUAGE.into(), tree_sitter_toml_ng::HIGHLIGHTS_QUERY.into()),
        "html" => (tree_sitter_html::LANGUAGE.into(), tree_sitter_html::HIGHLIGHTS_QUERY.into()),
        "css" => (tree_sitter_css::LANGUAGE.into(), tree_sitter_css::HIGHLIGHTS_QUERY.into()),
        "markdown" => (tree_sitter_md::LANGUAGE.into(), tree_sitter_md::HIGHLIGHT_QUERY_BLOCK.into()),
        // Anything else: Helix's grammar for it, if there is one.
        other => {
            grammars::ensure(other).ok()?;
            (grammars::load(other)?, grammars::highlights(other))
        }
    };
    let query = salvaged_query(&language, &query).or_else(|| Query::new(&language, "").ok())?;
    Some(Lang { language, query })
}

/// A highlights query, less the rules this tree-sitter can't read (Helix's
/// are written for its own tree-sitter version).
fn salvaged_query(language: &Language, src: &str) -> Option<Query> {
    let mut src = src.to_string();
    for _ in 0..40 {
        match Query::new(language, &src) {
            Ok(q) => return Some(q),
            Err(e) => {
                let (start, end) = top_level_form(&src, e.offset)?;
                src.replace_range(start..end, "");
            }
        }
    }
    None
}

fn top_level_form(src: &str, at: usize) -> Option<(usize, usize)> {
    let b = src.as_bytes();
    let (mut depth, mut start, mut i, mut in_str) = (0i32, None::<usize>, 0usize, false);
    let mut forms: Vec<(usize, usize)> = vec![];
    while i < b.len() {
        let c = b[i];
        if in_str {
            if c == b'\\' {
                i += 1;
            } else if c == b'"' {
                in_str = false;
            }
        } else if c == b';' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if c == b'"' {
            in_str = true;
        } else if c == b'(' || c == b'[' {
            if depth == 0 && start.is_none() {
                start = Some(i);
            }
            depth += 1;
        } else if c == b')' || c == b']' {
            depth -= 1;
            if depth == 0 {
                let mut j = i + 1;
                while j < b.len() && b[j] != b'(' && b[j] != b'[' && b[j] != b';' && b[j] != b'\n' {
                    j += 1;
                }
                if let Some(s) = start.take() {
                    forms.push((s, j));
                }
            }
        }
        i += 1;
    }
    forms
        .iter()
        .copied()
        .find(|&(s, e)| s <= at && at < e.max(s + 1))
        .or_else(|| forms.iter().copied().filter(|&(s, _)| s <= at).last())
        .or(forms.first().copied())
}

/// A capture's kind: its first part ("keyword.control" is a keyword), with
/// prose markup read as the code kind it looks like.
fn kind_of(name: &str) -> &str {
    match name {
        "text.title" | "markup.heading" => "keyword",
        "text.literal" | "markup.raw" | "markup.raw.block" => "string",
        "text.uri" | "text.reference" | "markup.link" | "markup.link.url" | "markup.link.label" => "function",
        "text.emphasis" | "text.strong" | "markup.italic" | "markup.bold" => "attribute",
        "markup.list" | "markup.quote" => "punctuation",
        _ => name.split('.').next().unwrap_or(""),
    }
}

/// Spans for one line: (start byte, end byte, kind index), later over earlier.
pub type LineSpans = Vec<(u32, u32, u8)>;

/// Highlight a whole text: spans per line. None when there's no grammar.
pub fn highlight(lang: &str, text: &str, deadline: Duration) -> Option<Vec<LineSpans>> {
    thread_local! {
        static CACHE: std::cell::RefCell<HashMap<String, Option<std::rc::Rc<Lang>>>> = Default::default();
    }
    let l = CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if let Some(l) = c.get(lang) {
            return l.clone();
        }
        let l = make(lang).map(std::rc::Rc::new);
        c.insert(lang.to_string(), l.clone());
        l
    })?;
    let end = Instant::now() + deadline;
    let mut p = Parser::new();
    p.set_language(&l.language).ok()?;
    let mut over = |_: &tree_sitter::ParseState| {
        if Instant::now() > end { std::ops::ControlFlow::Break(()) } else { std::ops::ControlFlow::Continue(()) }
    };
    let bytes = text.as_bytes();
    let mut read = |i: usize, _: tree_sitter::Point| -> &[u8] { if i < bytes.len() { &bytes[i..] } else { b"" } };
    let tree = p.parse_with_options(&mut read, None, Some(tree_sitter::ParseOptions::new().progress_callback(&mut over)))?;

    let line_starts: Vec<usize> =
        std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect();
    let mut lines: Vec<LineSpans> = vec![Vec::new(); line_starts.len()];
    let names = l.query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut caps = cursor.captures(&l.query, tree.root_node(), bytes);
    let mut n = 0usize;
    while let Some((m, i)) = caps.next() {
        n += 1;
        if n % 4096 == 0 && Instant::now() > end {
            break;
        }
        let c = m.captures()[*i];
        let Some(kind) = kind_index(kind_of(names[c.index as usize])) else { continue };
        let (s, e) = (c.node.start_byte(), c.node.end_byte().min(text.len()));
        if e <= s {
            continue;
        }
        // Split across lines.
        let mut li = line_starts.partition_point(|&x| x <= s).saturating_sub(1);
        while li < line_starts.len() && line_starts[li] < e {
            let ls = line_starts[li];
            let le = line_starts.get(li + 1).map(|x| x - 1).unwrap_or(text.len());
            let a = s.max(ls) - ls;
            let b = e.min(le).saturating_sub(ls);
            if b > a {
                lines[li].push((a as u32, b as u32, kind));
            }
            li += 1;
        }
    }
    Some(lines)
}

/// The result for one request.
#[derive(Debug)]
pub struct Highlights {
    pub id: u64,
    pub version: u64,
    /// None: no grammar for that language here.
    pub lines: Option<Vec<LineSpans>>,
}

struct Job {
    id: u64,
    version: u64,
    lang: String,
    text: String,
}

/// A worker thread that highlights; only the newest request per document
/// is done.
pub struct Highlighter {
    tx: Sender<Job>,
    rx: Receiver<Highlights>,
}

impl Default for Highlighter {
    fn default() -> Self {
        Self::new()
    }
}

impl Highlighter {
    pub fn new() -> Self {
        let (tx, jobs) = mpsc::channel::<Job>();
        let (done, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("turbine-syntax".into())
            .spawn(move || {
                while let Ok(first) = jobs.recv() {
                    let mut latest: HashMap<u64, Job> = HashMap::new();
                    latest.insert(first.id, first);
                    for j in jobs.try_iter() {
                        latest.insert(j.id, j);
                    }
                    for (_, j) in latest {
                        let lines = highlight(&j.lang, &j.text, Duration::from_secs(2));
                        if done.send(Highlights { id: j.id, version: j.version, lines }).is_err() {
                            return;
                        }
                    }
                }
            })
            .expect("syntax thread");
        Highlighter { tx, rx }
    }

    pub fn request(&self, id: u64, version: u64, lang: &str, text: String) {
        let _ = self.tx.send(Job { id, version, lang: lang.into(), text });
    }

    pub fn results(&self) -> Vec<Highlights> {
        self.rx.try_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_keywords_strings_comments() {
        let text = "fn main() {\n    // hi\n    let s = \"x\";\n}\n";
        let lines = highlight("rust", text, Duration::from_secs(2)).unwrap();
        let kinds = |li: usize| -> Vec<&str> { lines[li].iter().map(|s| KINDS[s.2 as usize]).collect() };
        assert!(kinds(0).contains(&"keyword"), "{:?}", lines[0]);
        assert!(kinds(1).contains(&"comment"));
        assert!(kinds(2).contains(&"string"));
    }

    #[test]
    fn detects_languages() {
        assert_eq!(detect(Some(Path::new("a/b.rs")), "").as_deref(), Some("rust"));
        assert_eq!(detect(None, "#!/usr/bin/env python3").as_deref(), Some("python"));
        assert_eq!(detect(Some(Path::new("Makefile")), "").as_deref(), Some("make"));
        assert_eq!(detect(None, "hello"), None);
    }

    #[test]
    fn worker_answers() {
        let h = Highlighter::new();
        h.request(7, 1, "json", "{\"a\": 1}".into());
        let start = Instant::now();
        loop {
            let r = h.results();
            if let Some(r) = r.into_iter().next() {
                assert_eq!((r.id, r.version), (7, 1));
                assert!(r.lines.is_some());
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
