//! More languages for the editor, loaded when needed: Helix's tree-sitter
//! collection (a compiled grammar and highlight queries for each of ~250
//! languages). From Helix's runtime when Helix is installed; otherwise the
//! first language that's needed fetches Helix's release archive once (about
//! 16 MB, kept compressed in the cache) and each language is unpacked from
//! it only when it's used.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tree_sitter::Language;

/// Helix runtime folders that hold grammars and queries.
pub fn runtime_dirs() -> &'static [PathBuf] {
    static DIRS: OnceLock<Vec<PathBuf>> = OnceLock::new();
    DIRS.get_or_init(|| {
        let mut dirs: Vec<PathBuf> = Vec::new();
        if let Some(r) = std::env::var_os("HELIX_RUNTIME") {
            dirs.push(PathBuf::from(r));
        }
        if let Some(h) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(&h).join(".config/helix/runtime"));
        }
        // Nix wraps hx with its runtime's path: read it from the wrapper.
        if let Some(path) = std::env::var_os("PATH") {
            for d in std::env::split_paths(&path) {
                let hx = d.join("hx");
                if let Ok(bytes) = std::fs::read(&hx) {
                    if let Some(i) = bytes.windows(14).position(|w| w == b"HELIX_RUNTIME=") {
                        let rest = &bytes[i + 14..];
                        let end = rest.iter().position(|&b| b == 0 || b == b' ' || b == b'\n' || b == b'\'' || b == b'"').unwrap_or(rest.len());
                        dirs.push(PathBuf::from(String::from_utf8_lossy(&rest[..end]).into_owned()));
                    }
                    if let Ok(real) = std::fs::canonicalize(&hx) {
                        if let Some(prefix) = real.parent().and_then(Path::parent) {
                            dirs.push(prefix.join("lib/runtime"));
                            dirs.push(prefix.join("share/helix/runtime"));
                        }
                    }
                }
            }
        }
        dirs.push(PathBuf::from("/usr/lib/helix/runtime"));
        dirs.push(PathBuf::from("/usr/share/helix/runtime"));
        dirs.retain(|d| d.join("grammars").is_dir());
        // Turbine's own (languages unpacked on demand) always counts.
        let own = own_dir();
        if !dirs.contains(&own) {
            dirs.push(own);
        }
        dirs.dedup();
        dirs
    })
}

fn grammar_file(name: &str) -> Option<PathBuf> {
    let ext = if cfg!(windows) { "dll" } else if cfg!(target_os = "macos") { "dylib" } else { "so" };
    runtime_dirs().iter().map(|d| d.join("grammars").join(format!("{name}.{ext}"))).find(|p| p.is_file())
        .or_else(|| runtime_dirs().iter().map(|d| d.join("grammars").join(format!("{name}.so"))).find(|p| p.is_file()))
}

/// Where languages unpacked on demand go.
fn own_dir() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(std::env::temp_dir)
        .join("turbine/helix-runtime")
}

/// Every language Helix has a grammar for (whether it's here yet or not).
pub fn known() -> Vec<String> {
    let mut out: Vec<String> = include_str!("helix_languages.txt").lines().map(str::to_string).collect();
    for n in installed() {
        if !out.contains(&n) {
            out.push(n);
        }
    }
    out.sort();
    out
}

/// Whether a language can be had (here, or from Helix's archive).
pub fn is_known(name: &str) -> bool {
    include_str!("helix_languages.txt").lines().any(|l| l == name) || grammar_file(name).is_some()
}

/// The languages whose grammar is here now, with its highlight rules (and
/// those they inherit).
pub fn available() -> Vec<String> {
    installed().into_iter().filter(|n| queries_here(query_name(n), 0)).collect()
}

/// The folder of a grammar's queries in Helix's runtime: its language's
/// name, which for these isn't the grammar's (QML's grammar is qmljs, its
/// queries are qml's: looked for as qmljs, QML was never highlighted).
fn query_name(grammar: &str) -> &str {
    match grammar {
        "qmljs" => "qml",
        "gitattributes" => "git-attributes",
        "gitcommit" => "git-commit",
        "gitignore" => "git-ignore",
        "janet-simple" => "janet",
        "jinja2" => "jinja",
        "markdown_inline" => "markdown.inline",
        "proto" => "protobuf",
        "ql" => "codeql",
        "query" => "tsq",
        "embedded-template" => "erb",
        other => other,
    }
}

/// Its highlights query and every one it inherits are here. (A grammar
/// alone highlights nothing: one unpacked while another language's was
/// clobbered its queries, and the language stayed plain for good.)
fn queries_here(name: &str, depth: usize) -> bool {
    if depth > 6 {
        return true;
    }
    let Some(text) = runtime_dirs().iter().find_map(|d| std::fs::read_to_string(d.join("queries").join(name).join("highlights.scm")).ok()) else {
        return false;
    };
    text.lines().filter_map(|l| l.trim().strip_prefix("; inherits:")).all(|ps| ps.split(',').all(|p| queries_here(p.trim(), depth + 1)))
}

fn installed() -> Vec<String> {
    {
        let mut out: Vec<String> = Vec::new();
        for d in runtime_dirs() {
            if let Ok(rd) = std::fs::read_dir(d.join("grammars")) {
                for e in rd.flatten() {
                    let p = e.path();
                    if let Some(stem) = p.file_stem().map(|s| s.to_string_lossy().into_owned()) {
                        if !out.contains(&stem) {
                            out.push(stem);
                        }
                    }
                }
            }
        }
        out.sort();
        out
    }
}

pub fn count() -> usize {
    known().len()
}

// ---- on demand ---------------------------------------------------------------------

fn cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
        .join("turbine")
}

/// Helix's release archive for this computer, fetched once and kept.
fn archive() -> Result<PathBuf, String> {
    let (os, ext) = if cfg!(windows) { ("windows", "zip") } else if cfg!(target_os = "macos") { ("macos", "tar.xz") } else { ("linux", "tar.xz") };
    let arch = if cfg!(target_arch = "aarch64") { "aarch64" } else { "x86_64" };
    let dir = cache_dir();
    if let Some(have) = std::fs::read_dir(&dir).ok().and_then(|rd| {
        rd.flatten().map(|e| e.path()).find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("helix-") && n.to_string_lossy().ends_with(ext)))
    }) {
        return Ok(have);
    }
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(std::time::Duration::from_secs(600))).build().into();
    let release: serde_json::Value = agent
        .get("https://api.github.com/repos/helix-editor/helix/releases/latest")
        .header("User-Agent", "turbine")
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .read_json()
        .map_err(|e| e.to_string())?;
    let (name, url) = release["assets"]
        .as_array()
        .and_then(|a| a.iter().find_map(|x| {
            let n = x["name"].as_str()?;
            (n.contains(&format!("{arch}-{os}")) && n.ends_with(ext)).then(|| Some((n.to_string(), x["browser_download_url"].as_str()?.to_string()))).flatten()
        }))
        .ok_or("no Helix release for this computer")?;
    let _ = std::fs::create_dir_all(&dir);
    let dest = dir.join(&name);
    let part = dir.join(format!("{name}.part"));
    let mut resp = agent.get(&url).call().map_err(|e| e.to_string())?;
    let mut f = std::fs::File::create(&part).map_err(|e| e.to_string())?;
    std::io::copy(&mut resp.body_mut().as_reader(), &mut f).map_err(|e| e.to_string())?;
    drop(f);
    std::fs::rename(&part, &dest).map_err(|e| e.to_string())?;
    Ok(dest)
}

/// The archive's file list (cached next to it).
fn archive_index(archive: &Path) -> Result<Vec<String>, String> {
    let idx = archive.with_extension("index");
    if let Ok(t) = std::fs::read_to_string(&idx) {
        return Ok(t.lines().map(str::to_string).collect());
    }
    let out = std::process::Command::new("tar").arg("-tf").arg(archive).output().map_err(|e| format!("couldn't run tar: {e}"))?;
    if !out.status.success() {
        return Err("couldn't read Helix's archive".into());
    }
    let list: Vec<String> = String::from_utf8_lossy(&out.stdout).lines().filter(|l| l.contains("/runtime/grammars/") || l.contains("/runtime/queries/")).map(str::to_string).collect();
    let _ = std::fs::write(&idx, list.join("\n"));
    Ok(list)
}

/// Unpack these archive members into Turbine's runtime folder.
fn unpack(archive: &Path, members: &[String]) -> Result<(), String> {
    if members.is_empty() {
        return Ok(());
    }
    // (Its own folder: two languages unpacked at once shared one, and each
    // removed it under the other.)
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let tmp = cache_dir().join(format!("helix-unpack-{}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let ok = std::process::Command::new("tar").arg("-xf").arg(archive).arg("-C").arg(&tmp).args(members).status().map_err(|e| e.to_string())?;
    if !ok.success() {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err("couldn't unpack from Helix's archive".into());
    }
    let own = own_dir();
    for m in members {
        let Some(i) = m.find("/runtime/") else { continue };
        let rel = &m[i + "/runtime/".len()..];
        let (from, to) = (tmp.join(m), own.join(rel));
        if from.is_file() {
            let _ = std::fs::create_dir_all(to.parent().unwrap());
            std::fs::copy(&from, &to).map_err(|e| e.to_string())?;
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(())
}

/// Make a language usable: unpack its grammar and queries (and the queries
/// they inherit) from Helix's archive, fetching that first if need be.
/// Ok(true) when something was added.
pub fn ensure(name: &str) -> Result<bool, String> {
    let r = ensure_files(name);
    // (Tried now, so it's ready, or known to be left alone, when this returns.)
    if grammar_file(name).is_some() {
        checked(name, true);
    }
    r
}

fn ensure_files(name: &str) -> Result<bool, String> {
    if grammar_file(name).is_some() && queries_here(query_name(name), 0) {
        return Ok(false);
    }
    if !is_known(name) {
        return Err(format!("no grammar for {name}"));
    }
    let archive = archive()?;
    let index = archive_index(&archive)?;
    let mut want: Vec<String> = Vec::new();
    let mut queue = vec![query_name(name).to_string()];
    let mut seen = std::collections::HashSet::new();
    while let Some(lang) = queue.pop() {
        if !seen.insert(lang.clone()) {
            continue;
        }
        for m in &index {
            let is_grammar = lang == query_name(name) && (m.ends_with(&format!("/runtime/grammars/{name}.so")) || m.ends_with(&format!("/runtime/grammars/{name}.dll")) || m.ends_with(&format!("/runtime/grammars/{name}.dylib")));
            if is_grammar || (m.contains(&format!("/runtime/queries/{lang}/")) && !m.ends_with('/')) {
                want.push(m.clone());
            }
        }
        // The queries it inherits, too.
        unpack(&archive, &want.iter().filter(|m| m.ends_with(&format!("/queries/{lang}/highlights.scm"))).cloned().collect::<Vec<_>>())?;
        if let Ok(t) = std::fs::read_to_string(own_dir().join("queries").join(&lang).join("highlights.scm")) {
            for l in t.lines() {
                if let Some(ps) = l.trim().strip_prefix("; inherits:") {
                    queue.extend(ps.split(',').map(|p| p.trim().to_string()));
                }
            }
        }
    }
    unpack(&archive, &want)?;
    Ok(grammar_file(name).is_some() && queries_here(query_name(name), 0))
}

/// Everything at once (for working offline): every grammar and query.
pub fn ensure_all() -> Result<usize, String> {
    let archive = archive()?;
    let index = archive_index(&archive)?;
    let members: Vec<String> = index.into_iter().filter(|m| !m.ends_with('/')).collect();
    for chunk in members.chunks(400) {
        unpack(&archive, chunk)?;
    }
    Ok(installed().len())
}

/// A highlights query with Helix's "; inherits: a,b" lines resolved (the
/// parents' rules first).
pub fn highlights(name: &str) -> String {
    fn read(name: &str, depth: usize, out: &mut String) {
        if depth > 6 {
            return;
        }
        let Some(text) = runtime_dirs().iter().find_map(|d| std::fs::read_to_string(d.join("queries").join(name).join("highlights.scm")).ok()) else {
            return;
        };
        for line in text.lines() {
            if let Some(parents) = line.trim().strip_prefix("; inherits:") {
                for p in parents.split(',') {
                    read(p.trim(), depth + 1, out);
                }
            }
        }
        out.push_str(&text);
        out.push('\n');
    }
    let mut out = String::new();
    read(query_name(name), 0, &mut out);
    out
}

/// Load a grammar (kept loaded for the rest of the run).
pub fn load(name: &str) -> Option<Language> {
    // (A grammar is compiled code: one that crashes would take Turbine with
    // it, as Helix's HCL does on any string. Each is tried in a child first.)
    // (Never waits: an untried grammar is tried in the background and its
    // code is plain until then; ensure() waits for it.)
    if !checked(name, false) {
        return None;
    }
    load_unchecked(name)
}

/// The flag a child is started with to try a grammar (`--turbine-check-grammar NAME`).
pub const CHECK_ARG: &str = "--turbine-check-grammar";

/// Call first in main: as that child, try the grammar and exit (0 when it
/// parsed and highlighted the samples).
pub fn check_from_args() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some(CHECK_ARG) {
        std::process::exit(if try_grammar(args.get(2).map(String::as_str).unwrap_or("")) { 0 } else { 3 });
    }
}

/// Text with the things scanners trip on: strings of every kind, comments,
/// numbers, heredocs, tags, interpolation.
const SAMPLE: &str = "a = \"x\" 'y' `z` # c\n// c\n/* c */ -- c ; c\nfn f(x) { return [1, 2.5e3, 0x1F, -7] }\nb = <<EOT\nhi ${x}\nEOT\n<a b=\"c\">d</a>\n$x ${y} @z %w \\n\n\t  end\n";

fn try_grammar(name: &str) -> bool {
    let Some(lang) = load_unchecked(name) else { return false };
    let mut p = tree_sitter::Parser::new();
    if p.set_language(&lang).is_err() {
        return false;
    }
    for text in [SAMPLE, "", "\"", "x", &SAMPLE[..SAMPLE.len() / 2]] {
        let Some(tree) = p.parse(text, None) else { continue };
        if let Ok(q) = tree_sitter::Query::new(&lang, &highlights(name)) {
            use streaming_iterator::StreamingIterator;
            let mut c = tree_sitter::QueryCursor::new();
            let mut caps = c.captures(&q, tree.root_node(), text.as_bytes());
            while caps.next().is_some() {}
        }
    }
    true
}

/// Whether the grammar came through its trial (tried once per grammar
/// file, in a child, 10 s at most; kept in checked.tsv). Not tried yet:
/// `wait` tries it now; otherwise it's tried in the background (false meanwhile).
fn checked(name: &str, wait: bool) -> bool {
    static MEMO: OnceLock<std::sync::Mutex<std::collections::HashMap<String, bool>>> = OnceLock::new();
    static TRYING: OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    let memo = MEMO.get_or_init(Default::default);
    if let Some(&ok) = memo.lock().unwrap().get(name) {
        return ok;
    }
    if !wait {
        // (Already decided on disk: that, no child.)
        if let Some(ok) = kept_verdict(name) {
            memo.lock().unwrap().insert(name.to_string(), ok);
            return ok;
        }
        if TRYING.get_or_init(Default::default).lock().unwrap().insert(name.to_string()) {
            let n = name.to_string();
            std::thread::spawn(move || {
                checked(&n, true);
            });
        }
        return false;
    }
    let Some(file) = grammar_file(name) else { return false };
    let key = verdict_key(name, &file);
    let list = own_dir().join("checked.tsv");
    let kept = std::fs::read_to_string(&list).unwrap_or_default();
    let ok = match kept.lines().rev().find(|l| l.starts_with(&key)) {
        Some(l) => l.ends_with("\tok"),
        None => {
            let ok = trial(name);
            let _ = std::fs::create_dir_all(own_dir());
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&list) {
                let _ = writeln!(f, "{key}{}", if ok { "ok" } else { "crashed" });
            }
            if !ok {
                eprintln!("turbine: the {name} grammar crashed or hung when tried; its code is shown plain");
            }
            ok
        }
    };
    memo.lock().unwrap().insert(name.to_string(), ok);
    ok
}

/// What checked.tsv says of the grammar as it is now, if anything.
fn kept_verdict(name: &str) -> Option<bool> {
    let file = grammar_file(name)?;
    let key = verdict_key(name, &file);
    let kept = std::fs::read_to_string(own_dir().join("checked.tsv")).ok()?;
    kept.lines().rev().find(|l| l.starts_with(&key)).map(|l| l.ends_with("\tok"))
}

fn verdict_key(name: &str, file: &Path) -> String {
    let stamp = std::fs::metadata(file).ok().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
    // (v2: verdicts from before the desktop app ran trials properly said
    // "crashed" for every grammar; those don't count.)
    format!("v2\t{name}\t{}\t{stamp}\t", file.display())
}

/// The grammar tried in a child process: this program again with
/// CHECK_ARG (in tests, the test that does the same).
fn trial(name: &str) -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    let mut cmd = std::process::Command::new(exe);
    if cfg!(test) {
        cmd.args(["--ignored", "--exact", "grammars::trial_child::child", "--test-threads=1"]).env("TURBINE_TRY_GRAMMAR", name);
    } else {
        cmd.args([CHECK_ARG, name]);
    }
    let Ok(mut child) = cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn() else { return false };
    let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if std::time::Instant::now() < end => std::thread::sleep(std::time::Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

#[cfg(test)]
mod trial_child {
    /// The child side of a grammar's trial in tests (TURBINE_TRY_GRAMMAR).
    #[test]
    #[ignore]
    fn child() {
        let name = std::env::var("TURBINE_TRY_GRAMMAR").unwrap_or_default();
        assert!(super::try_grammar(&name));
    }
}

fn load_unchecked(name: &str) -> Option<Language> {
    let file = grammar_file(name)?;
    let lib = unsafe { libloading::Library::new(&file) }.ok()?;
    let lib: &'static libloading::Library = Box::leak(Box::new(lib));
    let sym = format!("tree_sitter_{}", name.replace('-', "_"));
    let f: libloading::Symbol<'static, unsafe extern "C" fn() -> *const ()> = unsafe { lib.get(sym.as_bytes()) }.ok()?;
    let lang: Language = unsafe { tree_sitter_language::LanguageFn::from_raw(*f) }.into();
    // A grammar from a tree-sitter this one can't read is left alone.
    let v = lang.abi_version();
    (tree_sitter::MIN_COMPATIBLE_LANGUAGE_VERSION..=tree_sitter::LANGUAGE_VERSION).contains(&v).then_some(lang)
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore]
    fn helix_runtime() {
        println!("dirs: {:?}", super::runtime_dirs());
        println!("{} grammars", super::available().len());
        for n in ["haskell", "elixir", "zig", "nix", "kotlin"] {
            let l = super::load(n);
            let q = super::highlights(n);
            let ok = l.as_ref().map(|l| tree_sitter::Query::new(l, &q).map(|q| q.capture_names().len()).map_err(|e| e.to_string()));
            println!("{n}: loaded {} query {:?}", l.is_some(), ok);
        }
    }
}

#[cfg(test)]
mod on_demand {
    #[test]
    #[ignore]
    fn fetch_one() {
        let t = std::time::Instant::now();
        let r = super::ensure("haskell");
        println!("ensure haskell: {r:?} in {:?}", t.elapsed());
        let l = super::load("haskell").expect("loads");
        let q = super::highlights("haskell");
        println!("query compiles: {:?}", tree_sitter::Query::new(&l, &q).map(|q| q.capture_names().len()).map_err(|e| e.to_string()));
        let t = std::time::Instant::now();
        println!("ensure typescript (from the kept archive): {:?} in {:?}", super::ensure("tsx"), t.elapsed());
        println!("installed: {:?}", super::available());
    }
}
