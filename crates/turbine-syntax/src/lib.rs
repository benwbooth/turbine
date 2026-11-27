//! Syntax highlighting with tree-sitter.
//!
//! Seventeen grammars are built in; any other language Helix has (~250) is
//! loaded from Helix's runtime, or fetched once from Helix's release and
//! unpacked when first needed (`grammars`, from Tessera). Parsing runs on a
//! worker thread: the editor asks with [`Highlighter::request`] and picks up
//! results on its ticks, so a big file never stalls typing.

pub mod grammars;
pub mod plain;

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

/// A language's line comment, if it has one.
pub fn line_comment(lang: &str) -> Option<&'static str> {
    Some(match lang {
        "rust" | "c" | "cpp" | "java" | "javascript" | "typescript" | "tsx" | "go" | "c-sharp" | "kotlin" | "swift" | "dart" | "zig" | "php" | "qml" | "scala"
        | "jsonc" | "glsl" | "wgsl" | "proto" => "//",
        "python" | "bash" | "ruby" | "yaml" | "toml" | "nix" | "make" | "cmake" | "perl" | "r" | "elixir" | "julia" | "dockerfile" | "fish" | "tessh"
        | "nim" | "powershell" | "gdscript" => "#",
        "lua" | "haskell" | "sql" | "elm" | "ada" => "--",
        "lisp" | "scheme" | "clojure" | "commonlisp" | "racket" | "fennel" | "ini" | "asm" | "nasm" => ";",
        "erlang" | "latex" | "matlab" => "%",
        "vim" => "\"",
        "ocaml" | "html" | "css" | "markdown" | "json" | "xml" => return None,
        _ => return None,
    })
}

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
    ("sql", "SQL"),
    ("tessh", "tessh"),
    ("toml", "TOML"),
    ("typescript", "TypeScript"),
    ("tsx", "TSX"),
    ("yaml", "YAML"),
];

/// The language of a file, from its name, extension or first line (the
/// names Tessera and Helix use).
pub fn detect(path: Option<&Path>, first_line: &str) -> Option<String> {
    if let Some(p) = path {
        let name = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        let by_name = match name.as_str() {
            "makefile" | "gnumakefile" => Some("make"),
            "dockerfile" | "containerfile" => Some("dockerfile"),
            "cmakelists.txt" => Some("cmake"),
            "cargo.lock" => Some("toml"),
            "flake.lock" | "package-lock.json" => Some("json"),
            ".bashrc" | ".bash_profile" | ".profile" | ".zshrc" | ".zprofile" | ".zshenv" | "pkgbuild" | ".envrc" => Some("bash"),
            "login.tsh" | "rc.tsh" | "logout.tsh" => Some("tessh"),
            "justfile" | ".justfile" => Some("just"),
            "meson.build" => Some("meson"),
            "go.mod" => Some("gomod"),
            "go.work" => Some("gowork"),
            ".gitignore" | ".dockerignore" => Some("gitignore"),
            ".gitattributes" => Some("gitattributes"),
            ".gitconfig" => Some("git-config"),
            "commit_editmsg" => Some("gitcommit"),
            "git-rebase-todo" => Some("git-rebase"),
            "nginx.conf" => Some("nginx"),
            "caddyfile" => Some("caddyfile"),
            "build.zig.zon" => Some("zig"),
            _ if name.starts_with("dockerfile.") => Some("dockerfile"),
            _ if name.ends_with(".env") || name == ".env" => Some("bash"),
            _ => None,
        };
        if let Some(l) = by_name {
            return Some(l.into());
        }
        let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        let by_ext = match ext.as_str() {
            "rs" => "rust",
            "py" | "pyw" | "pyi" | "xsh" => "python",
            "js" | "mjs" | "cjs" | "jsx" => "javascript",
            "ts" | "mts" | "cts" => "typescript",
            "tsx" => "tsx",
            "c" | "h" => "c",
            "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" | "ipp" | "c++" | "h++" | "ino" => "cpp",
            "go" => "go",
            "java" => "java",
            "rb" | "rake" | "gemspec" => "ruby",
            "lua" => "lua",
            "json" | "jsonc" | "json5" => "json",
            "yaml" | "yml" => "yaml",
            "toml" => "toml",
            "html" | "htm" | "xhtml" => "html",
            "css" => "css",
            "scss" => "scss",
            "md" | "markdown" | "mdx" => "markdown",
            "sh" | "bash" | "zsh" | "ksh" | "dash" => "bash",
            "fish" => "fish",
            "tsh" | "tessh" => "tessh",
            "nu" => "nu",
            "ps1" | "psm1" | "psd1" => "powershell",
            "nix" => "nix",
            "zig" => "zig",
            "hs" | "lhs" => "haskell",
            "ml" => "ocaml",
            "mli" => "ocaml_interface",
            "ex" | "exs" => "elixir",
            "heex" => "heex",
            "eex" => "eex",
            "erl" | "hrl" => "erlang",
            "kt" | "kts" => "kotlin",
            "swift" => "swift",
            "cs" => "c-sharp",
            "fs" | "fsx" | "fsi" => "fsharp",
            "php" => "php",
            "pl" | "pm" | "t" => "perl",
            "r" | "rmd" => "r",
            "scala" | "sc" | "sbt" => "scala",
            "sql" => "sql",
            "el" => "elisp",
            "clj" | "cljs" | "cljc" | "edn" => "clojure",
            "scm" | "ss" | "rkt" => "scheme",
            "fnl" => "fennel",
            "janet" => "janet-simple",
            "dart" => "dart",
            "jl" => "julia",
            "qml" => "qmljs",
            "xml" | "svg" | "xaml" | "xsd" | "xsl" | "xslt" | "plist" | "csproj" | "vbproj" | "fsproj" | "vcxproj" | "props" | "targets" | "resx"
            | "nuspec" | "wsdl" | "rss" | "atom" | "kml" | "gpx" | "ui" | "qrc" => "xml",
            "proto" => "proto",
            "pas" | "pp" | "inc" | "dpr" => "pascal",
            "asm" | "s" | "nasm" => "nasm",
            "f" | "f90" | "f95" | "f03" | "for" => "fortran",
            "ini" | "cfg" | "conf" | "desktop" | "service" | "timer" | "socket" | "mount" | "target" => "ini",
            "properties" => "properties",
            "make" | "mk" => "make",
            "cmake" => "cmake",
            "dockerfile" => "dockerfile",
            "tex" | "sty" | "cls" | "ltx" => "latex",
            "bib" => "bibtex",
            "typ" => "typst",
            "rst" => "rst",
            "org" => "org",
            "csv" | "tsv" => "csv",
            "diff" | "patch" => "diff",
            "graphql" | "gql" => "graphql",
            "svelte" => "svelte",
            "vue" => "vue",
            "astro" => "astro",
            "gleam" => "gleam",
            "odin" => "odin",
            "v" | "vv" => "v",
            "nim" | "nims" | "nimble" => "nim",
            "d" | "di" => "d",
            "cr" => "crystal",
            "elm" => "elm",
            "purs" => "purescript",
            "hcl" | "tf" | "tfvars" => "hcl",
            "glsl" | "vert" | "frag" | "geom" | "comp" => "glsl",
            "wgsl" => "wgsl",
            "hlsl" => "hlsl",
            "sol" => "solidity",
            "vhd" | "vhdl" => "vhdl",
            "sv" | "svh" | "vh" => "verilog",
            "tcl" => "tcl",
            "awk" => "awk",
            "jq" => "jq",
            "jsonnet" | "libsonnet" => "jsonnet",
            "kdl" => "kdl",
            "ron" => "ron",
            "dhall" => "dhall",
            "cue" => "cue",
            "prisma" => "prisma",
            "wat" => "wat",
            "wast" => "wast",
            "wit" => "wit",
            "ll" => "llvm",
            "lean" => "lean",
            "agda" => "agda",
            "sml" => "sml",
            "ada" | "adb" | "ads" => "ada",
            "groovy" | "gradle" => "groovy",
            "vala" => "vala",
            "hare" => "hare",
            "mojo" => "mojo",
            "slint" => "slint",
            "templ" => "templ",
            "twig" => "twig",
            "jinja" | "jinja2" | "j2" => "jinja2",
            "tera" => "tera",
            "pug" => "pug",
            "mermaid" | "mmd" => "mermaid",
            "dot" | "gv" => "dot",
            "log" => "log",
            "po" | "pot" => "po",
            "gd" => "gdscript",
            "tscn" | "tres" => "godot_resource",
            "pest" => "pest",
            "ungram" => "ungrammar",
            "thrift" => "thrift",
            "capnp" => "capnp",
            "smali" => "smali",
            "robot" => "robot",
            "feature" => "gherkin",
            "hurl" => "hurl",
            "just" => "just",
            "bas" | "bi" | "bm" => "basic",
            "cob" | "cbl" => "cobol",
            "bat" | "cmd" => "batch",
            "vim" => "vim",
            _ => "",
        };
        if !by_ext.is_empty() {
            return Some(by_ext.into());
        }
    }
    let l = first_line.trim_start_matches('\u{feff}').trim();
    if let Some(shebang) = l.strip_prefix("#!") {
        let mut words = shebang.split_whitespace().map(|w| w.rsplit('/').next().unwrap_or(w));
        let mut prog = words.next()?;
        if prog == "env" {
            prog = words.find(|w| !w.starts_with('-'))?;
        }
        let prog = prog.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
        let lang = match prog {
            "python" | "pypy" => "python",
            "sh" | "bash" | "zsh" | "dash" | "ksh" => "bash",
            "fish" => "fish",
            "tessh" | "tessh-static" => "tessh",
            "nu" => "nu",
            "node" | "bun" => "javascript",
            "deno" | "ts-node" | "tsx" => "typescript",
            "ruby" => "ruby",
            "lua" | "luajit" => "lua",
            "perl" => "perl",
            "php" => "php",
            "pwsh" => "powershell",
            "Rscript" => "r",
            "julia" => "julia",
            "elixir" => "elixir",
            "escript" => "erlang",
            "runghc" | "runhaskell" => "haskell",
            "awk" | "gawk" => "awk",
            "make" => "make",
            "nix-shell" => "bash",
            _ => return None,
        };
        return Some(lang.into());
    }
    if l.starts_with("<?xml") {
        return Some("xml".into());
    }
    if l.to_ascii_lowercase().starts_with("<!doctype html") || l.starts_with("<html") {
        return Some("html".into());
    }
    // Then the desktop's MIME database (shared-mime-info, as grove and
    // the file managers use): its globs for the name, its magic for the
    // first line, and each type's parents in turn.
    by_mime(path, first_line)
}

fn mime_db() -> &'static std::sync::Mutex<xdg_mime::SharedMimeInfo> {
    static DB: std::sync::OnceLock<std::sync::Mutex<xdg_mime::SharedMimeInfo>> = std::sync::OnceLock::new();
    DB.get_or_init(|| std::sync::Mutex::new(xdg_mime::SharedMimeInfo::new()))
}

/// Load the MIME database ahead (off the UI thread) so the first look is quick.
pub fn warm_mime_db() {
    std::thread::spawn(|| {
        let _ = mime_db();
    });
}

/// A language for a MIME type: the names that differ, else the type's
/// own name when a grammar goes by it (text/x-toml: toml).
fn lang_for_mime(m: &str) -> Option<String> {
    let named = match m {
        "application/x-shellscript" | "text/x-shellscript" | "application/x-sh" => "bash",
        "text/x-python" | "text/x-python3" | "application/x-python" => "python",
        "text/x-csrc" | "text/x-chdr" => "c",
        "text/x-c++src" | "text/x-c++hdr" => "cpp",
        "text/x-csharp" => "c-sharp",
        "application/json" | "application/x-ipynb+json" | "application/geo+json" | "application/ld+json" => "json",
        "application/xml" | "text/xml" | "image/svg+xml" | "application/x-desktop" => {
            if m == "application/x-desktop" { "ini" } else { "xml" }
        }
        "application/javascript" | "text/javascript" | "application/x-javascript" => "javascript",
        "application/x-ruby" | "text/x-ruby" => "ruby",
        "text/x-makefile" => "make",
        "application/yaml" | "application/x-yaml" | "text/x-yaml" => "yaml",
        "application/toml" | "text/x-toml" => "toml",
        "text/markdown" | "text/x-markdown" => "markdown",
        "text/x-tex" | "text/x-latex" => "latex",
        "text/x-patch" | "text/x-diff" => "diff",
        "application/sql" | "text/x-sql" => "sql",
        "text/x-dsrc" => "d",
        "text/x-objcsrc" => "objc",
        "text/x-gettext-translation" | "text/x-gettext-translation-template" => "po",
        "application/x-php" => "php",
        "application/x-perl" | "text/x-perl" => "perl",
        "text/x-systemd-unit" | "text/x-ini" | "text/x-ini-file" => "ini",
        _ => "",
    };
    if !named.is_empty() {
        return Some(named.into());
    }
    let sub = m.split_once('/')?.1;
    let name = sub.trim_start_matches("x-").trim_end_matches("src").to_ascii_lowercase();
    let known = BUILTIN.iter().any(|(n, _)| *n == name) || grammars::known().iter().any(|n| *n == name);
    known.then_some(name)
}

fn by_mime(path: Option<&Path>, first_line: &str) -> Option<String> {
    let db = mime_db().lock().ok()?;
    let mut g = db.guess_mime_type();
    if let Some(name) = path.and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()) {
        g.file_name(&name);
    }
    if !first_line.is_empty() {
        g.data(first_line.as_bytes());
    }
    let guess = g.guess();
    let mut todo = vec![guess.mime_type().clone()];
    let mut seen = 0;
    while let Some(m) = todo.pop() {
        let s = m.essence_str().to_string();
        if s == "text/plain" || s == "application/octet-stream" {
            continue;
        }
        if let Some(l) = lang_for_mime(&s) {
            return Some(l);
        }
        seen += 1;
        if seen > 12 {
            break;
        }
        if let Some(ps) = db.get_parents(&m) {
            todo.extend(ps);
        }
    }
    None
}

/// What an untitled text is written in, when it says so clearly (a
/// shebang, JSON, a doctype, or lines with unmistakable marks): Tessera's
/// input's guess, a few more languages added.
pub fn guess(text: &str) -> Option<&'static str> {
    let t = text.trim_start_matches('\u{feff}').trim_start();
    if let Some(l) = t.lines().next().filter(|l| l.starts_with("#!")) {
        return detect(None, l).and_then(|l| BUILTIN.iter().map(|b| b.0).chain(["fish", "nu", "perl", "php", "powershell", "r", "julia", "elixir", "erlang", "haskell", "awk", "make"]).find(|b| *b == l));
    }
    if (t.starts_with('{') || t.starts_with('[')) && serde_json::from_str::<serde_json::Value>(t.trim()).is_ok() {
        return Some("json");
    }
    let low = t.get(..t.len().min(64)).unwrap_or("").to_ascii_lowercase();
    if low.starts_with("<!doctype html") || low.starts_with("<html") {
        return Some("html");
    }
    if low.starts_with("<?xml") {
        return Some("xml");
    }
    let lines: Vec<&str> = t.lines().filter(|l| !l.trim().is_empty()).take(200).collect();
    if lines.len() < 2 {
        let l = t.trim();
        for (marks, lang) in [
            (&["print(", "import ", "from ", "def ", "lambda "][..], "python"),
            (&["console.log(", "const ", "function "][..], "javascript"),
            (&["println!(", "let mut ", "fn main", "use std::"][..], "rust"),
            (&["fmt.Print", "package main"][..], "go"),
            (&["System.out.print", "public class "][..], "java"),
            (&["printf(\""][..], "c"),
            (&["std::cout", "#include", "using namespace std", "auto main("][..], "cpp"),
            (&["SELECT ", "select ", "CREATE TABLE ", "INSERT INTO "][..], "sql"),
        ] {
            if marks.iter().any(|m| l.starts_with(m)) {
                return Some(lang);
            }
        }
        return None;
    }
    let has = |pats: &[&str]| lines.iter().filter(|l| pats.iter().any(|p| l.trim_start().starts_with(p))).count();
    let score = [
        ("python", has(&["def ", "import ", "from ", "class ", "elif ", "print(", "if __name__"]) * 2 + lines.iter().filter(|l| l.trim_end().ends_with(':')).count()),
        ("rust", has(&["fn ", "let mut ", "use ", "impl ", "pub fn ", "struct ", "#[derive", "mod ", "pub struct "]) * 2),
        ("go", has(&["func ", "package ", "import (", "type "]) * 2 + lines.iter().filter(|l| l.contains(" := ")).count()),
        ("javascript", has(&["const ", "function ", "console.log", "export ", "import {", "let "]) * 2 + lines.iter().filter(|l| l.contains("=> ")).count()),
        ("typescript", has(&["interface ", "type ", "export interface ", "export type "]) * 2 + lines.iter().filter(|l| l.contains(": string") || l.contains(": number")).count() * 2),
        ("sql", lines.iter().filter(|l| { let u = l.trim_start().to_uppercase(); ["SELECT ", "INSERT ", "UPDATE ", "DELETE ", "CREATE ", "FROM ", "WHERE ", "JOIN ", "ALTER "].iter().any(|k| u.starts_with(k)) }).count() * 2),
        ("c", has(&["#include <stdio", "#include <stdlib", "int main", "printf(", "#define ", "typedef "]) * 2),
        ("cpp", has(&["#include <iostream", "#include <vector", "std::", "namespace ", "template<", "template <", "class "]) * 2),
        ("java", has(&["public class ", "import java.", "public static void", "private ", "package "]) * 2),
        ("html", has(&["<!doctype", "<html", "<div", "<p>", "<body", "<head"]) * 2),
        ("css", lines.iter().filter(|l| { let x = l.trim(); x.ends_with('{') && !x.contains('(') || (x.contains(": ") && x.ends_with(';') && !x.contains('(')) }).count()),
        ("markdown", has(&["# ", "## ", "### ", "- [ ] ", "```"]) * 2 + lines.iter().filter(|l| l.trim_start().starts_with("* ") || l.contains("](")).count()),
        ("bash", has(&["echo ", "if [", "fi", "then", "done", "export ", "for ", "while "]) + lines.iter().filter(|l| l.contains("$(") || l.contains("${")).count()),
        ("yaml", lines.iter().filter(|l| { let x = l.trim_start(); (x.starts_with("- ") || x.contains(": ")) && !x.contains('=') && !x.contains('|') && !x.ends_with(';') && !x.ends_with('{') }).count()),
        ("toml", lines.iter().filter(|l| { let x = l.trim(); (x.starts_with('[') && x.ends_with(']')) || x.contains(" = \"") }).count()),
        ("lua", has(&["local ", "function ", "end", "require("]) + lines.iter().filter(|l| l.contains(" then")).count()),
        ("ruby", has(&["def ", "end", "require '", "puts ", "class "]) + has(&["end"])),
    ];
    let best = score.iter().max_by_key(|(_, s)| *s).unwrap();
    // Clear marks on at least half the lines.
    if best.1 >= 3 && best.1 * 2 >= lines.len() { Some(best.0) } else { None }
}

struct Lang {
    language: Language,
    query: Query,
}

fn make(name: &str) -> Option<Lang> {
    // (Plain text has no grammar to look for: plain.rs colours it.)
    if name == "text" {
        return None;
    }
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
        "sql" => (tree_sitter_sequel::LANGUAGE.into(), tree_sitter_sequel::HIGHLIGHTS_QUERY.into()),
        "tessh" => (tree_sitter_tessh::LANGUAGE.into(), tree_sitter_tessh::HIGHLIGHTS_QUERY.into()),
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
/// A language's grammar and query, made once per thread.
fn cached(lang: &str) -> Option<std::rc::Rc<Lang>> {
    thread_local! {
        static CACHE: std::cell::RefCell<HashMap<String, Option<std::rc::Rc<Lang>>>> = Default::default();
    }
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if let Some(l) = c.get(lang) {
            return l.clone();
        }
        let l = make(lang).map(std::rc::Rc::new);
        c.insert(lang.to_string(), l.clone());
        l
    })
}

/// A text's shape, from its syntax tree: what folds, what selections grow
/// through, and where definitions start.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Structure {
    /// Foldable regions: (first line, last line hidden), by first line.
    pub folds: Vec<(usize, usize)>,
    /// Named nodes' byte ranges (selection grows through them), sorted by
    /// start, the outer one first.
    pub nodes: Vec<(u32, u32)>,
    /// Lines where definitions (functions, types, ...) start.
    pub defs: Vec<usize>,
}

pub fn highlight(lang: &str, text: &str, deadline: Duration) -> Option<Vec<LineSpans>> {
    analyze(lang, text, deadline).map(|(h, _)| h)
}

/// The structure of a parsed tree.
fn structure(tree: &tree_sitter::Tree, text: &str, end: Instant) -> Structure {
    let bytes = text.as_bytes();
    let mut st = Structure::default();
    let mut folds: HashMap<usize, usize> = HashMap::new();
    let mut cursor = tree.walk();
    let root = tree.root_node().id();
    'walk: loop {
        let node = cursor.node();
        if node.is_named() && node.id() != root {
            if st.nodes.len() < 200_000 {
                st.nodes.push((node.start_byte() as u32, node.end_byte() as u32));
            }
            let (a, b) = (node.start_position(), node.end_position());
            // (A body that starts below its header folds with the header.)
            let body_below = (node.kind() == "block" || node.kind().contains("body")) && node.parent().is_some_and(|p| p.start_position().row < a.row);
            if b.row > a.row && !body_below {
                // A closing bracket's line stays shown (`fn main() {⋯` then `}`).
                let line_start = bytes[..node.end_byte().min(bytes.len())].iter().rposition(|&c| c == b'\n').map(|i| i + 1).unwrap_or(0);
                let before = &text[line_start..node.end_byte().min(text.len())];
                let last = if !before.trim().is_empty() && before.trim().chars().all(|c| matches!(c, '}' | ')' | ']' | ';' | ',')) || before.trim().starts_with("end") && before.trim().len() <= 4 {
                    b.row - 1
                } else if before.trim().is_empty() {
                    // (Ends at the start of the next line.)
                    b.row - 1
                } else {
                    b.row
                };
                if last > a.row {
                    let e = folds.entry(a.row).or_insert(last);
                    *e = (*e).max(last);
                }
            }
            if symbol_kind(node.kind()).is_some() && st.defs.last() != Some(&a.row) {
                st.defs.push(a.row);
            }
        }
        if cursor.goto_first_child() {
            continue;
        }
        if Instant::now() > end {
            break;
        }
        loop {
            if cursor.goto_next_sibling() {
                continue 'walk;
            }
            if !cursor.goto_parent() {
                break 'walk;
            }
        }
    }
    st.folds = folds.into_iter().collect();
    st.folds.sort();
    st.nodes.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
    st.nodes.dedup();
    st.defs.sort();
    st.defs.dedup();
    st
}

/// Blocks by indentation (a header line, then lines indented under it):
/// folds, and tessh's definitions (def, class, defmacro, defop).
fn indent_blocks(text: &str) -> (Vec<(usize, usize)>, Vec<Symbol>) {
    let lines: Vec<&str> = text.lines().collect();
    let indent = |l: &str| l.len() - l.trim_start().len();
    let mut folds = Vec::new();
    let mut defs = Vec::new();
    let mut open: Vec<(usize, usize)> = Vec::new(); // (line, indent) of headers
    for (i, l) in lines.iter().enumerate() {
        if l.trim().is_empty() || l.trim_start().starts_with('#') {
            continue;
        }
        let ind = indent(l);
        while let Some(&(h, hi)) = open.last() {
            if ind > hi {
                break;
            }
            open.pop();
            // The block ends at its last non-blank line.
            let mut last = i - 1;
            while last > h && lines[last].trim().is_empty() {
                last -= 1;
            }
            if last > h {
                folds.push((h, last));
            }
        }
        let depth = open.len();
        let mut w = l.split_whitespace();
        let kind = match w.next() {
            Some("def") => Some("function"),
            Some("class") => Some("class"),
            Some("defmacro") | Some("defop") => Some("function"),
            Some("async") if l.split_whitespace().nth(1) == Some("def") => {
                w.next();
                Some("function")
            }
            _ => None,
        };
        if let (Some(kind), Some(name)) = (kind, w.next()) {
            let name: String = name.chars().take_while(|c| !matches!(c, '(' | ':' | ' ')).collect();
            defs.push(Symbol { name, kind, line: i, depth });
        }
        if lines.get(i + 1..).and_then(|r| r.iter().find(|x| !x.trim().is_empty())).is_some_and(|n| indent(n) > ind) {
            open.push((i, ind));
        }
    }
    let n = lines.len();
    while let Some((h, _)) = open.pop() {
        let mut last = n.saturating_sub(1);
        while last > h && lines[last].trim().is_empty() {
            last -= 1;
        }
        if last > h {
            folds.push((h, last));
        }
    }
    folds.sort();
    (folds, defs)
}

/// Highlight spans per line, and the text's structure. None when there's
/// no grammar.
pub fn analyze(lang: &str, text: &str, deadline: Duration) -> Option<(Vec<LineSpans>, Structure)> {
    let l = cached(lang)?;
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
    let mut st = structure(&tree, text, end + Duration::from_millis(500));
    if lang == "tessh" {
        // (tessh's grammar is a line at a time: its strings' $x and $(…)
        // come from its own splitter, its blocks from indentation.)
        lines = vec![Vec::new(); line_starts.len()];
        // (Its query's first match wins; here later spans win: reversed.)
        for (s0, e0, k) in tree_sitter_tessh::highlight::spans(text).into_iter().rev() {
            let Some(kind) = kind_index(kind_of(k)) else { continue };
            let mut li = line_starts.partition_point(|&x| x <= s0).saturating_sub(1);
            while li < line_starts.len() && line_starts[li] < e0 {
                let ls = line_starts[li];
                let le = line_starts.get(li + 1).map(|x| x - 1).unwrap_or(text.len());
                let (a, b) = (s0.max(ls) - ls, e0.min(le).saturating_sub(ls));
                if b > a {
                    lines[li].push((a as u32, b as u32, kind));
                }
                li += 1;
            }
        }
        let (folds, defs) = indent_blocks(text);
        st.folds = folds;
        st.defs = defs.into_iter().map(|d| d.line).collect();
    }
    Some((lines, st))
}

/// A named definition in a file (the Outline): functions, types, classes,
/// modules, ... `depth` is how many other symbols it sits inside.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    /// "function", "method", "class", "struct", "enum", "trait", "interface",
    /// "impl", "module", "type", "constant", "heading".
    pub kind: &'static str,
    /// 0-based line of the definition.
    pub line: usize,
    pub depth: usize,
}

/// What kind of symbol a tree-sitter node is, from its node type (grammars
/// name their definitions alike: function_item, function_definition,
/// class_declaration, ...).
fn symbol_kind(node_kind: &str) -> Option<&'static str> {
    let k = node_kind;
    if k.ends_with("_call") || k.contains("call_expression") || k.ends_with("_reference") || k.ends_with("_identifier") {
        return None;
    }
    let def = k.ends_with("_item") || k.ends_with("_definition") || k.ends_with("_declaration") || k.ends_with("_specifier") || k.ends_with("_statement");
    if !def && !matches!(k, "atx_heading" | "setext_heading" | "section") {
        return None;
    }
    Some(match () {
        _ if k.starts_with("function") || k.starts_with("func_") || k == "function_signature_item" => "function",
        _ if k.starts_with("method") => "method",
        _ if k.starts_with("class") => "class",
        _ if k.starts_with("struct") => "struct",
        _ if k.starts_with("enum") => "enum",
        _ if k.starts_with("trait") => "trait",
        _ if k.starts_with("interface") || k.starts_with("protocol") => "interface",
        _ if k.starts_with("impl") => "impl",
        _ if k.starts_with("mod") || k.starts_with("namespace") || k.starts_with("module") => "module",
        _ if k.starts_with("type") || k.starts_with("union") => "type",
        _ if k.starts_with("const") || k.starts_with("static") => "constant",
        _ if k.contains("heading") || k == "section" => "heading",
        _ => return None,
    })
}

/// The definitions in a text, in order (the editor's Outline). None when
/// there's no grammar for the language.
pub fn symbols(lang: &str, text: &str, deadline: Duration) -> Option<Vec<Symbol>> {
    if lang == "tessh" {
        return Some(indent_blocks(text).1);
    }
    let l = cached(lang)?;
    let end = Instant::now() + deadline;
    let mut p = Parser::new();
    p.set_language(&l.language).ok()?;
    let mut over = |_: &tree_sitter::ParseState| {
        if Instant::now() > end { std::ops::ControlFlow::Break(()) } else { std::ops::ControlFlow::Continue(()) }
    };
    let bytes = text.as_bytes();
    let mut read = |i: usize, _: tree_sitter::Point| -> &[u8] { if i < bytes.len() { &bytes[i..] } else { b"" } };
    let tree = p.parse_with_options(&mut read, None, Some(tree_sitter::ParseOptions::new().progress_callback(&mut over)))?;
    let mut out = Vec::new();
    let mut cursor = tree.walk();
    // Depth-first, counting the symbols we're inside.
    let mut stack: Vec<bool> = Vec::new();
    let mut depth = 0usize;
    'walk: loop {
        let node = cursor.node();
        let mut is_sym = false;
        if let Some(kind) = symbol_kind(node.kind()) {
            let name = node
                .child_by_field_name("name")
                .or_else(|| node.child_by_field_name("type"))
                .or_else(|| node.child_by_field_name("declarator").map(|d| d.child_by_field_name("declarator").unwrap_or(d)))
                .and_then(|n| n.utf8_text(bytes).ok())
                .map(|s| s.lines().next().unwrap_or("").trim().to_string())
                .or_else(|| {
                    // (Markdown headings: their text.)
                    (kind == "heading").then(|| node.utf8_text(bytes).ok()).flatten().map(|s| s.lines().next().unwrap_or("").trim_start_matches('#').trim().to_string())
                });
            if let Some(name) = name.filter(|n| !n.is_empty()) {
                out.push(Symbol { name, kind, line: node.start_position().row, depth });
                is_sym = true;
            }
        }
        if cursor.goto_first_child() {
            stack.push(is_sym);
            if is_sym {
                depth += 1;
            }
            continue;
        }
        if Instant::now() > end {
            break;
        }
        loop {
            if cursor.goto_next_sibling() {
                continue 'walk;
            }
            if !cursor.goto_parent() {
                break 'walk;
            }
            if stack.pop() == Some(true) {
                depth -= 1;
            }
        }
    }
    Some(out)
}

/// The result for one request.
#[derive(Debug)]
pub struct Highlights {
    pub id: u64,
    pub version: u64,
    /// None: no grammar for that language here.
    pub lines: Option<Vec<LineSpans>>,
    pub structure: Option<Structure>,
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
                let mut waited: std::collections::HashSet<String> = std::collections::HashSet::new();
                while let Ok(first) = jobs.recv() {
                    let mut latest: HashMap<u64, Job> = HashMap::new();
                    latest.insert(first.id, first);
                    for j in jobs.try_iter() {
                        latest.insert(j.id, j);
                    }
                    for (_, j) in latest {
                        let got = analyze(&j.lang, &j.text, Duration::from_secs(2));
                        // (A grammar not tried or unpacked yet: plain for now,
                        // and the real colours once it's ready, once per language.)
                        if got.is_none() && j.lang != "text" && grammars::is_known(&j.lang) && waited.insert(j.lang.clone()) {
                            let done = done.clone();
                            let (id, version, lang, text) = (j.id, j.version, j.lang.clone(), j.text.clone());
                            std::thread::spawn(move || {
                                if grammars::ensure(&lang).is_ok() {
                                    if let Some((l, s)) = analyze(&lang, &text, Duration::from_secs(2)) {
                                        let _ = done.send(Highlights { id, version, lines: Some(l), structure: Some(s) });
                                    }
                                }
                            });
                        }
                        let (lines, structure) = match got {
                            Some((l, s)) => (Some(l), Some(s)),
                            // No grammar (or plain text): what all text
                            // shares, and indentation's folds.
                            None => {
                                let folds = if j.text.len() < 4 << 20 { indent_blocks(&j.text).0 } else { Vec::new() };
                                (Some(plain::spans(&j.text)), Some(Structure { folds, ..Structure::default() }))
                            }
                        };
                        if done.send(Highlights { id: j.id, version: j.version, lines, structure }).is_err() {
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
    #[test]
    fn guesses_untitled_text() {
        use super::guess;
        assert_eq!(guess("#!/usr/bin/env python3\nx = 1\n"), Some("python"));
        assert_eq!(guess("{\"a\": [1, 2]}"), Some("json"));
        assert_eq!(guess("def f(x):\n    return x\n\nimport os\n"), Some("python"));
        assert_eq!(guess("fn main() {\n    let mut x = 1;\n}\nuse std::fmt;\n"), Some("rust"));
        assert_eq!(guess("SELECT a\nFROM t\nWHERE b = 1\n"), Some("sql"));
        assert_eq!(guess("# Title\n\nSome text with a [link](x).\n## More\n"), Some("markdown"));
        assert_eq!(guess("hello there\nhow are you\n"), None);
        assert_eq!(guess("#!/usr/bin/env tessh\nls\n"), Some("tessh"));
    }

    #[test]
    fn tessh_highlights_folds_outline() {
        let src = "# tools\ndef greet name\n    print \"hi $name\"\n    if name\n        echo yes\n\nclass Box\n    def size self\n        return 1\nls -la | grep x\n";
        assert_eq!(super::detect(Some(std::path::Path::new("a.tsh")), ""), Some("tessh".into()));
        assert_eq!(super::detect(None, "#!/usr/bin/env tessh"), Some("tessh".into()));
        let (lines, st) = super::analyze("tessh", src, std::time::Duration::from_secs(2)).unwrap();
        let kind = |line: usize, word: &str| {
            let l = src.lines().nth(line).unwrap();
            let b = l.find(word).unwrap() as u32;
            lines[line].iter().rev().find(|(s, e, _)| *s <= b && b < *e).map(|x| super::KINDS[x.2 as usize])
        };
        assert_eq!(kind(1, "def"), Some("keyword"));
        assert_eq!(kind(2, "$name"), Some("variable"));
        assert_eq!(kind(0, "tools"), Some("comment"));
        assert_eq!(kind(9, "-la"), Some("attribute"));
        assert_eq!(st.folds, vec![(1, 4), (3, 4), (6, 8), (7, 8)]);
        assert_eq!(st.defs, vec![1, 6, 7]);
        let syms = super::symbols("tessh", src, std::time::Duration::from_secs(1)).unwrap();
        let got: Vec<_> = syms.iter().map(|s| (s.name.as_str(), s.kind, s.depth)).collect();
        assert_eq!(got, vec![("greet", "function", 0), ("Box", "class", 0), ("size", "function", 1)]);
    }

    #[test]
    fn structure_folds_nodes_defs() {
        let src = "fn main() {\n    let x = 1;\n    if x > 0 {\n        go();\n    }\n}\n\nfn go() {}\n";
        let (_, st) = super::analyze("rust", src, std::time::Duration::from_secs(2)).unwrap();
        // `fn main() {` folds to before its `}`; the if too.
        assert!(st.folds.contains(&(0, 4)), "{:?}", st.folds);
        assert!(st.folds.contains(&(2, 3)), "{:?}", st.folds);
        assert_eq!(st.defs, vec![0, 7]);
        // `1` sits inside `let x = 1;` inside the block inside main.
        let one = src.find('1').unwrap() as u32;
        let around: Vec<_> = st.nodes.iter().filter(|(a, b)| *a <= one && *b > one).collect();
        assert!(around.len() >= 4, "{around:?}");
        let py = "def f(a):\n    if a:\n        return 1\n    return 2\n";
        let (_, st) = super::analyze("python", py, std::time::Duration::from_secs(2)).unwrap();
        assert!(st.folds.contains(&(0, 3)) && st.folds.contains(&(1, 2)), "{:?}", st.folds);
    }

    #[test]
    fn rust_symbols_nest() {
        let src = "mod m {\n    pub struct S;\n    impl S {\n        fn go(&self) {}\n    }\n}\nfn main() {}\nconst N: u8 = 1;\n";
        let syms = super::symbols("rust", src, std::time::Duration::from_secs(1)).unwrap();
        let got: Vec<(String, &str, usize, usize)> = syms.into_iter().map(|s| (s.name, s.kind, s.line, s.depth)).collect();
        assert_eq!(
            got,
            vec![
                ("m".into(), "module", 0, 0),
                ("S".into(), "struct", 1, 1),
                ("S".into(), "impl", 2, 1),
                ("go".into(), "function", 3, 2),
                ("main".into(), "function", 6, 0),
                ("N".into(), "constant", 7, 0),
            ]
        );
    }

    #[test]
    fn python_symbols() {
        let src = "class A:\n    def f(self):\n        pass\n\ndef g():\n    pass\n";
        let syms = super::symbols("python", src, std::time::Duration::from_secs(1)).unwrap();
        let got: Vec<(String, &str, usize)> = syms.into_iter().map(|s| (s.name, s.kind, s.depth)).collect();
        assert_eq!(got, vec![("A".into(), "class", 0), ("f".into(), "function", 1), ("g".into(), "function", 0)]);
    }

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
