//! A file's icon (a monochrome Nerd Font glyph, one cell) and its colour
//! by type, in EGA's sixteen: the Explorer, the file manager and the tabs
//! share them. Colours are the light halves; lists darken them on a light
//! background (turbine_ui::list::readable).

use turbine_ui::theme::{Color, vga};

fn ext_of(name: &str) -> String {
    let n = name.to_ascii_lowercase();
    match n.rsplit_once('.') {
        Some((stem, e)) if !stem.is_empty() => e.to_string(),
        _ => String::new(),
    }
}

/// A file's (or folder's) icon.
pub fn icon(name: &str, dir: bool, open: bool) -> &'static str {
    if dir {
        return match name.to_ascii_lowercase().as_str() {
            ".git" => "\u{e5fb}",
            "src" | "lib" => if open { "\u{eaf7}" } else { "\u{ea83}" },
            _ => if open { "\u{eaf7}" } else { "\u{ea83}" },
        };
    }
    let lower = name.to_ascii_lowercase();
    match lower.as_str() {
        "makefile" | "gnumakefile" | "justfile" => return "\u{e779}",
        "dockerfile" | "containerfile" => return "\u{f308}",
        "license" | "licence" | "copying" => return "\u{e60a}",
        "readme" | "readme.md" => return "\u{eb33}",
        "cargo.toml" | "cargo.lock" => return "\u{e7a8}",
        "flake.nix" | "flake.lock" => return "\u{f313}",
        ".gitignore" | ".gitattributes" | ".gitmodules" => return "\u{e702}",
        _ => {}
    }
    match ext_of(name).as_str() {
        "rs" => "\u{e7a8}",
        "py" | "pyi" | "pyw" => "\u{e73c}",
        "md" | "markdown" | "mdx" => "\u{e73e}",
        "js" | "mjs" | "cjs" | "jsx" => "\u{e74e}",
        "ts" | "mts" | "tsx" => "\u{e628}",
        "json" | "jsonc" | "json5" => "\u{eb0f}",
        "toml" => "\u{e6b2}",
        "nix" => "\u{f313}",
        "yaml" | "yml" => "\u{e6a8}",
        "ini" | "cfg" | "conf" | "properties" | "env" => "\u{e615}",
        "lock" => "\u{f023}",
        "c" | "h" => "\u{e61e}",
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" | "ino" => "\u{e61d}",
        "cs" => "\u{f81a}",
        "go" => "\u{e626}",
        "java" | "jar" | "class" => "\u{e738}",
        "kt" | "kts" => "\u{e634}",
        "swift" => "\u{e755}",
        "rb" | "gemspec" => "\u{e739}",
        "php" => "\u{e73d}",
        "lua" => "\u{e620}",
        "zig" => "\u{e6a9}",
        "hs" | "lhs" => "\u{e777}",
        "ml" | "mli" => "\u{e67a}",
        "ex" | "exs" | "heex" => "\u{e62d}",
        "erl" | "hrl" => "\u{e7b1}",
        "clj" | "cljs" | "cljc" | "edn" => "\u{e768}",
        "scala" | "sc" => "\u{e737}",
        "dart" => "\u{e798}",
        "r" => "\u{f25d}",
        "jl" => "\u{e624}",
        "pl" | "pm" => "\u{e769}",
        "sh" | "bash" | "zsh" | "fish" | "ksh" | "tsh" | "tessh" | "nu" | "ps1" | "bat" | "cmd" => "\u{f489}",
        "html" | "htm" | "xhtml" => "\u{e736}",
        "css" | "scss" | "sass" | "less" => "\u{e749}",
        "vue" => "\u{e6a0}",
        "svelte" => "\u{e697}",
        "xml" | "xsd" | "xsl" | "plist" | "csproj" | "ui" | "qrc" => "\u{e619}",
        "qml" => "\u{f375}",
        "sql" | "db" | "sqlite" => "\u{e706}",
        "txt" | "text" | "log" | "rst" | "org" | "tex" | "typ" => "\u{f15c}",
        "pdf" => "\u{f1c1}",
        "doc" | "docx" | "odt" | "rtf" => "\u{f1c2}",
        "xls" | "xlsx" | "ods" | "csv" | "tsv" => "\u{f1c3}",
        "ppt" | "pptx" | "odp" => "\u{f1c4}",
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "svg" | "ico" | "tif" | "tiff" | "avif" | "heic" | "xcf" | "psd" => "\u{f1c5}",
        "mp3" | "flac" | "ogg" | "opus" | "wav" | "m4a" | "aac" | "mid" | "midi" => "\u{f1c7}",
        "mp4" | "mkv" | "webm" | "avi" | "mov" | "wmv" | "m4v" => "\u{f1c8}",
        "zip" | "tar" | "gz" | "tgz" | "xz" | "bz2" | "zst" | "7z" | "rar" | "deb" | "rpm" | "iso" => "\u{f1c6}",
        "exe" | "com" | "msi" | "appimage" | "so" | "dll" | "o" | "a" => "\u{f013}",
        "ttf" | "otf" | "woff" | "woff2" => "\u{f031}",
        "patch" | "diff" => "\u{f440}",
        "wasm" | "wat" => "\u{e6a1}",
        "glsl" | "vert" | "frag" | "wgsl" | "hlsl" => "\u{f1fc}",
        "bas" | "bi" | "bm" => "\u{f121}",
        "pas" | "pp" => "\u{f121}",
        "asm" | "s" | "nasm" => "\u{f2db}",
        _ => "\u{ea7b}",
    }
}

/// A file's colour by type (folders bright cyan, the only ones that are,
/// and bold in lists; dotfiles dim).
pub fn colour(name: &str, dir: bool) -> Color {
    if name.starts_with('.') {
        return vga::DARK_GRAY;
    }
    if dir {
        return vga::LIGHT_CYAN;
    }
    let lower = name.to_ascii_lowercase();
    if matches!(lower.as_str(), "makefile" | "dockerfile" | "justfile" | "containerfile") {
        return vga::LIGHT_GREEN;
    }
    match ext_of(name).as_str() {
        "rs" | "java" | "jar" | "class" | "scala" | "rb" | "erl" | "swift" => vga::LIGHT_RED,
        "py" | "pyi" | "pyw" | "sh" | "bash" | "zsh" | "fish" | "ksh" | "tsh" | "tessh" | "nu" | "ps1" | "bat" | "cmd" | "pl" | "pm" | "lua" => vga::LIGHT_GREEN,
        "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" | "cs" | "zig" | "dart" | "bas" | "bi" | "pas" | "asm" | "s" => vga::LIGHT_BLUE,
        "go" | "md" | "markdown" | "mdx" | "txt" | "rst" | "org" | "tex" | "typ" | "html" | "htm" | "css" | "scss" | "sass" | "vue" | "svelte" | "qml" => vga::WHITE,
        "js" | "mjs" | "cjs" | "jsx" | "ts" | "mts" | "tsx" | "kt" | "kts" | "php" | "hs" | "ml" | "mli" | "ex" | "exs" | "clj" | "cljs" | "jl" | "r" => vga::YELLOW,
        "json" | "jsonc" | "json5" | "toml" | "yaml" | "yml" | "ini" | "cfg" | "conf" | "nix" | "xml" | "lock" | "env" | "properties" | "sql" => vga::LIGHT_MAGENTA,
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "svg" | "ico" | "mp3" | "flac" | "ogg" | "wav" | "mp4" | "mkv" | "webm" | "mov" | "pdf" | "ttf" | "otf" => vga::MAGENTA,
        "zip" | "tar" | "gz" | "tgz" | "xz" | "bz2" | "zst" | "7z" | "rar" | "deb" | "rpm" | "iso" => vga::RED,
        "exe" | "com" | "msi" | "appimage" | "so" | "dll" | "o" | "a" | "wasm" => vga::GREEN,
        "log" | "patch" | "diff" => vga::LIGHT_GRAY,
        _ => vga::LIGHT_GRAY,
    }
}

/// Git's status letter for a path and its colour, as VS Code marks them.
pub fn git_badge(code: char) -> (&'static str, Color) {
    match code {
        'M' => ("M", vga::YELLOW),
        'A' => ("A", vga::LIGHT_GREEN),
        'U' => ("U", vga::LIGHT_GREEN),
        'D' => ("D", vga::LIGHT_RED),
        'R' => ("R", vga::LIGHT_CYAN),
        'C' => ("C", vga::LIGHT_RED),
        _ => ("•", vga::YELLOW),
    }
}

/// `git status --porcelain=v1 -z` → (path from the repository, letter):
/// U untracked, A added, M changed, D deleted, R renamed, C conflict.
pub fn parse_git_status(out: &str) -> Vec<(String, char)> {
    let mut v = Vec::new();
    let mut parts = out.split('\0').peekable();
    while let Some(e) = parts.next() {
        if e.len() < 4 {
            continue;
        }
        let (xy, path) = e.split_at(3);
        let xy: Vec<char> = xy.chars().collect();
        let (x, y) = (xy[0], xy[1]);
        let code = match (x, y) {
            ('?', '?') => 'U',
            ('!', '!') => 'I',
            ('U', _) | (_, 'U') | ('A', 'A') | ('D', 'D') => 'C',
            ('R', _) => 'R',
            ('A', _) => 'A',
            ('D', _) | (_, 'D') => 'D',
            _ => 'M',
        };
        if x == 'R' || x == 'C' {
            parts.next(); // (the old name)
        }
        v.push((path.trim_end_matches('/').to_string(), code));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn types_and_git() {
        assert_eq!(colour("main.rs", false), vga::LIGHT_RED);
        assert_eq!(colour("src", true), vga::LIGHT_CYAN);
        assert_eq!(colour(".env", false), vga::DARK_GRAY);
        assert_ne!(icon("a.py", false, false), icon("a.rs", false, false));
        assert_ne!(icon("x", true, true), icon("x", true, false));
        let st = parse_git_status(" M src/a.rs\0?? new.txt\0R  b.rs\0old.rs\0A  c/\0!! target/\0");
        assert_eq!(st, vec![("src/a.rs".into(), 'M'), ("new.txt".into(), 'U'), ("b.rs".into(), 'R'), ("c".into(), 'A'), ("target".into(), 'I')]);
    }
}
