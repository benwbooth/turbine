//! Building, checking and running: the project's tasks (from
//! `.turbine/tasks.json` or `.vscode/tasks.json`, else what its files say:
//! Cargo, make, npm, go, CMake, ...), a per-language check of one file,
//! running a program, and reading compilers' messages as problems.
//!
//! Commands run on threads of their own; their output comes back line by
//! line through a channel the UI polls.

use regex::Regex;
use serde_json::Value;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::sync::mpsc::{Receiver, channel};

/// A task: a shell command run in a folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Task {
    pub label: String,
    pub command: String,
    pub cwd: PathBuf,
    /// "build", "test", "run" or "".
    pub group: String,
    /// The project's default build task.
    pub default: bool,
}

fn task(label: &str, command: &str, cwd: &Path, group: &str) -> Task {
    Task { label: label.into(), command: command.into(), cwd: cwd.to_path_buf(), group: group.into(), default: false }
}

/// Quote a word for `sh -c`.
pub fn sh_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || "-_./=:+,@%".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// The project's tasks: its own file's first, then detected ones.
pub fn tasks(root: &Path) -> Vec<Task> {
    let mut out = Vec::new();
    let file = [".turbine/tasks.json", ".vscode/tasks.json"].iter().find_map(|f| std::fs::read_to_string(root.join(f)).ok());
    if let Some(v) = file.and_then(|t| serde_json::from_str::<Value>(&crate::dap::strip_jsonc(&t)).ok()) {
        for t in v["tasks"].as_array().into_iter().flatten() {
            let Some(cmd) = t["command"].as_str() else { continue };
            let args: Vec<String> = t["args"].as_array().into_iter().flatten().filter_map(|a| a.as_str().map(sh_quote)).collect();
            let command = if args.is_empty() { cmd.to_string() } else { format!("{cmd} {}", args.join(" ")) };
            let command = command.replace("${workspaceFolder}", &root.display().to_string());
            let (group, default) = match &t["group"] {
                Value::String(g) => (g.clone(), false),
                Value::Object(o) => (o.get("kind").and_then(Value::as_str).unwrap_or("").to_string(), o.get("isDefault").and_then(Value::as_bool).unwrap_or(false)),
                _ => (String::new(), false),
            };
            let cwd = t["options"]["cwd"].as_str().map(|c| PathBuf::from(c.replace("${workspaceFolder}", &root.display().to_string()))).unwrap_or_else(|| root.to_path_buf());
            let label = t["label"].as_str().unwrap_or(cmd).to_string();
            out.push(Task { label, command, cwd, group, default });
        }
    }
    let has = |f: &str| root.join(f).exists();
    if has("Cargo.toml") {
        out.push(task("cargo build", "cargo build", root, "build"));
        out.push(task("cargo build --workspace --all-targets", "cargo build --workspace --all-targets", root, "build-all"));
        out.push(task("cargo test", "cargo test", root, "test"));
        out.push(task("cargo clippy", "cargo clippy", root, "check"));
        out.push(task("cargo run", "cargo run", root, "run"));
    }
    if has("Makefile") || has("makefile") || has("GNUmakefile") {
        out.push(task("make", "make", root, "build"));
        out.push(task("make -B (all)", "make -B", root, "build-all"));
        out.push(task("make test", "make test", root, "test"));
        out.push(task("make clean", "make clean", root, ""));
    }
    if has("CMakeLists.txt") {
        out.push(task("cmake --build build", "cmake -S . -B build && cmake --build build", root, "build"));
        out.push(task("cmake --build build --clean-first", "cmake -S . -B build && cmake --build build --clean-first", root, "build-all"));
        out.push(task("ctest", "ctest --test-dir build", root, "test"));
    }
    if has("meson.build") {
        out.push(task("meson compile", "meson setup build 2>/dev/null; meson compile -C build", root, "build"));
        out.push(task("meson test", "meson test -C build", root, "test"));
    }
    if has("build.zig") {
        out.push(task("zig build", "zig build", root, "build"));
        out.push(task("zig build test", "zig build test", root, "test"));
        out.push(task("zig build run", "zig build run", root, "run"));
    }
    if has("go.mod") {
        out.push(task("go build ./...", "go build ./...", root, "build"));
        out.push(task("go vet ./...", "go vet ./...", root, "check"));
        out.push(task("go test ./...", "go test ./...", root, "test"));
        out.push(task("go run .", "go run .", root, "run"));
    }
    if let Some(pkg) = has("package.json").then(|| std::fs::read_to_string(root.join("package.json")).ok()).flatten().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
        let runner = if has("pnpm-lock.yaml") { "pnpm" } else if has("yarn.lock") { "yarn" } else if has("bun.lockb") || has("bun.lock") { "bun run" } else { "npm run" };
        for (name, _) in pkg["scripts"].as_object().into_iter().flatten() {
            let group = match name.as_str() {
                "build" => "build",
                "test" => "test",
                "lint" | "typecheck" | "check" => "check",
                "start" | "dev" | "serve" => "run",
                _ => "",
            };
            out.push(task(&format!("{runner} {name}"), &format!("{runner} {name}"), root, group));
        }
    }
    if has("pyproject.toml") || has("setup.py") {
        out.push(task("python -m compileall", "python3 -m compileall -q .", root, "build"));
        out.push(task("pytest", "python3 -m pytest", root, "test"));
    }
    if has("flake.nix") {
        out.push(task("nix build", "nix build -L", root, if out.iter().any(|t| t.group == "build") { "" } else { "build" }));
        out.push(task("nix flake check", "nix flake check -L", root, "check"));
    }
    if std::fs::read_dir(root).ok().is_some_and(|rd| rd.flatten().any(|e| e.path().extension().is_some_and(|x| x == "sln" || x == "csproj" || x == "fsproj"))) {
        out.push(task("dotnet build", "dotnet build", root, "build"));
        out.push(task("dotnet test", "dotnet test", root, "test"));
        out.push(task("dotnet run", "dotnet run", root, "run"));
    }
    if has("gradlew") {
        out.push(task("gradle build", "./gradlew build", root, "build"));
        out.push(task("gradle test", "./gradlew test", root, "test"));
    } else if has("build.gradle") || has("build.gradle.kts") {
        out.push(task("gradle build", "gradle build", root, "build"));
    }
    if has("pom.xml") {
        out.push(task("mvn compile", "mvn -q compile", root, "build"));
        out.push(task("mvn test", "mvn -q test", root, "test"));
    }
    if has("mix.exs") {
        out.push(task("mix compile", "mix compile", root, "build"));
        out.push(task("mix test", "mix test", root, "test"));
    }
    if has("stack.yaml") {
        out.push(task("stack build", "stack build", root, "build"));
    } else if std::fs::read_dir(root).ok().is_some_and(|rd| rd.flatten().any(|e| e.path().extension().is_some_and(|x| x == "cabal"))) {
        out.push(task("cabal build", "cabal build", root, "build"));
    }
    if has("dune-project") {
        out.push(task("dune build", "dune build", root, "build"));
    }
    if has("Package.swift") {
        out.push(task("swift build", "swift build", root, "build"));
    }
    if has("pubspec.yaml") {
        out.push(task("dart analyze", "dart analyze", root, "check"));
    }
    out
}

/// The default build task: one marked default, else the first "build".
pub fn default_build(tasks: &[Task]) -> Option<&Task> {
    tasks.iter().find(|t| t.default && t.group == "build").or_else(|| tasks.iter().find(|t| t.group == "build"))
}

/// A tool on the PATH?
pub fn on_path(cmd: &str) -> bool {
    crate::rpc::on_path(cmd)
}

/// A command that checks one file for errors without running it (the
/// first whose tool is installed). `root`: its project.
pub fn check_command(lang: &str, file: &Path, root: &Path) -> Option<String> {
    let f = sh_quote(&file.display().to_string());
    let in_cargo = root.join("Cargo.toml").exists();
    let candidates: Vec<(&str, String)> = match lang {
        "rust" if in_cargo => vec![("cargo", "cargo check --message-format=short".into())],
        "rust" => vec![("rustc", format!("rustc --edition 2024 --emit=metadata -o /dev/null {f}"))],
        "c" => vec![("cc", format!("cc -fsyntax-only -Wall {f}")), ("clang", format!("clang -fsyntax-only -Wall {f}")), ("gcc", format!("gcc -fsyntax-only -Wall {f}"))],
        "cpp" => vec![("c++", format!("c++ -std=c++20 -fsyntax-only -Wall {f}")), ("clang++", format!("clang++ -std=c++20 -fsyntax-only -Wall {f}"))],
        "python" => vec![("ruff", format!("ruff check --output-format=concise {f}")), ("python3", format!("python3 -m py_compile {f}"))],
        "go" => vec![("go", format!("go vet {}", sh_quote(&file.parent().unwrap_or(root).display().to_string())))],
        "javascript" => vec![("node", format!("node --check {f}"))],
        "typescript" | "tsx" => vec![("tsc", format!("tsc --noEmit --pretty false {f}"))],
        "bash" => vec![("shellcheck", format!("shellcheck -f gcc {f}")), ("bash", format!("bash -n {f}"))],
        "fish" => vec![("fish", format!("fish -n {f}"))],
        "zsh" => vec![("zsh", format!("zsh -n {f}"))],
        "tessh" => vec![("tessh", format!("tessh --check {f}"))],
        "lua" => vec![("luac", format!("luac -p {f}")), ("luacheck", format!("luacheck --formatter plain {f}"))],
        "ruby" => vec![("ruby", format!("ruby -wc {f}"))],
        "perl" => vec![("perl", format!("perl -c {f}"))],
        "php" => vec![("php", format!("php -l {f}"))],
        "java" => vec![("javac", format!("javac -Xlint -d /tmp/turbine-javac {f}"))],
        "kotlin" => vec![("kotlinc", format!("kotlinc -d /tmp/turbine-kotlinc {f}"))],
        "zig" => vec![("zig", format!("zig ast-check {f}"))],
        "nix" => vec![("nix-instantiate", format!("nix-instantiate --parse {f} >/dev/null"))],
        "haskell" => vec![("ghc", format!("ghc -fno-code {f}"))],
        "ocaml" => vec![("ocamlfind", format!("ocamlfind ocamlc -c -o /tmp/turbine-ocaml {f}")), ("ocamlc", format!("ocamlc -c -o /tmp/turbine-ocaml {f}"))],
        "elixir" => vec![("elixir", format!("elixir -e 'Code.string_to_quoted!(File.read!(\"{}\"))'", file.display()))],
        "erlang" => vec![("erlc", format!("erlc -o /tmp {f}"))],
        "c-sharp" => vec![("dotnet", "dotnet build --nologo -v q".into())],
        "swift" => vec![("swiftc", format!("swiftc -parse {f}"))],
        "dart" => vec![("dart", format!("dart analyze {f}"))],
        "json" => vec![("jq", format!("jq empty {f}")), ("python3", format!("python3 -m json.tool {f} >/dev/null"))],
        "yaml" => vec![("yamllint", format!("yamllint -f parsable {f}")), ("python3", format!("python3 -c 'import sys,yaml; yaml.safe_load(open(sys.argv[1]))' {f}"))],
        "toml" => vec![("taplo", format!("taplo check {f}")), ("python3", format!("python3 -c 'import sys,tomllib; tomllib.load(open(sys.argv[1],\"rb\"))' {f}"))],
        "html" => vec![("tidy", format!("tidy -q -e {f}"))],
        "css" => vec![("stylelint", format!("stylelint -f unix {f}"))],
        "markdown" => vec![("markdownlint", format!("markdownlint {f}"))],
        "dockerfile" => vec![("hadolint", format!("hadolint {f}"))],
        "sql" => vec![("sqlfluff", format!("sqlfluff lint --format github-annotation-native {f}"))],
        "cmake" => vec![("cmake", format!("cmake -P {f}"))],
        "make" => vec![("make", format!("make -n -f {f} >/dev/null"))],
        "qml" => vec![("qmllint", format!("qmllint {f}"))],
        "scala" => vec![("scalac", format!("scalac -d /tmp {f}"))],
        "julia" => vec![("julia", format!("julia -e 'Meta.parseall(read(ARGS[1],String))' {f}"))],
        "r" => vec![("Rscript", format!("Rscript -e 'invisible(parse(commandArgs(TRUE)[1]))' {f}"))],
        "nim" => vec![("nim", format!("nim check --hints:off {f}"))],
        "d" => vec![("dmd", format!("dmd -o- {f}"))],
        "fortran" => vec![("gfortran", format!("gfortran -fsyntax-only {f}"))],
        "powershell" => vec![("pwsh", format!("pwsh -NoProfile -Command \"\\$null = [System.Management.Automation.Language.Parser]::ParseFile('{}', [ref]\\$null, [ref]\\$e); \\$e\"", file.display()))],
        _ => vec![],
    };
    candidates.into_iter().find(|(tool, _)| on_path(tool)).map(|(_, c)| c)
}

/// A command that runs a file (or its project) as a program: the first
/// whose tool is installed (Tessera's runners, a file at a time).
pub fn run_command(lang: &str, file: &Path, root: &Path) -> Option<String> {
    let f = sh_quote(&file.display().to_string());
    let d = sh_quote(&file.parent().unwrap_or(root).display().to_string());
    let stem = sh_quote(&file.with_extension("").display().to_string());
    let tmp = format!("/tmp/turbine-run-{}", file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
    let c: Vec<(&str, String)> = match lang {
        "rust" if root.join("Cargo.toml").exists() => vec![("cargo", "cargo run".into())],
        "rust" => vec![("rust-script", format!("rust-script {f}")), ("rustc", format!("rustc --edition 2024 -g -o {tmp} {f} && {tmp}"))],
        "c" => vec![("cc", format!("cc -g -Wall -o {stem} {f} && {stem}")), ("tcc", format!("tcc -run {f}"))],
        "cpp" => vec![("c++", format!("c++ -std=c++23 -g -Wall -o {stem} {f} && {stem}")), ("cling", format!("cling -std=c++23 < {f}"))],
        "python" => vec![("python3", format!("python3 {f}")), ("uv", format!("uv run {f}")), ("python", format!("python {f}"))],
        "go" if root.join("go.mod").exists() => vec![("go", "go run .".into())],
        "go" => vec![("go", format!("go run {f}")), ("yaegi", format!("yaegi run {f}"))],
        "javascript" => vec![("node", format!("node {f}")), ("deno", format!("deno run {f}")), ("bun", format!("bun {f}"))],
        "typescript" | "tsx" => vec![("deno", format!("deno run {f}")), ("bun", format!("bun {f}")), ("npx", format!("npx tsx {f}"))],
        "bash" => vec![("bash", format!("bash {f}"))],
        "zsh" => vec![("zsh", format!("zsh {f}"))],
        "fish" => vec![("fish", format!("fish {f}"))],
        "tessh" => vec![("tessh", format!("tessh {f}"))],
        "nu" => vec![("nu", format!("nu {f}"))],
        "elvish" => vec![("elvish", format!("elvish {f}"))],
        "powershell" => vec![("pwsh", format!("pwsh -File {f}"))],
        "lua" => vec![("lua", format!("lua {f}")), ("luajit", format!("luajit {f}"))],
        "luau" => vec![("luau", format!("luau {f}"))],
        "fennel" => vec![("fennel", format!("fennel {f}"))],
        "ruby" => vec![("ruby", format!("ruby {f}"))],
        "crystal" => vec![("crystal", format!("crystal run {f}"))],
        "perl" => vec![("perl", format!("perl {f}"))],
        "php" => vec![("php", format!("php {f}"))],
        "r" => vec![("Rscript", format!("Rscript {f}"))],
        "julia" => vec![("julia", format!("julia {f}"))],
        "octave" | "matlab" => vec![("octave", format!("octave --no-gui {f}"))],
        "elixir" => vec![("elixir", format!("elixir {f}"))],
        "erlang" => vec![("escript", format!("escript {f}"))],
        "gleam" => vec![("gleam", "gleam run".into())],
        "groovy" => vec![("groovy", format!("groovy {f}"))],
        "java" => vec![("java", format!("java {f}")), ("jbang", format!("jbang {f}"))],
        "kotlin" => vec![("kotlin", format!("kotlin {f}")), ("kotlinc", format!("kotlinc -script {f}"))],
        "scala" => vec![("scala-cli", format!("scala-cli run {f}")), ("scala", format!("scala {f}"))],
        "clojure" => vec![("bb", format!("bb {f}")), ("clojure", format!("clojure -M {f}"))],
        "haskell" => vec![("runghc", format!("runghc {f}"))],
        "ocaml" => vec![("ocaml", format!("ocaml {f}"))],
        "racket" => vec![("racket", format!("racket {f}"))],
        "scheme" => vec![("guile", format!("guile {f}")), ("racket", format!("racket {f}"))],
        "commonlisp" | "common-lisp" | "lisp" => vec![("sbcl", format!("sbcl --script {f}"))],
        "elisp" => vec![("emacs", format!("emacs --batch -l {f}"))],
        "janet-simple" | "janet" => vec![("janet", format!("janet {f}"))],
        "c-sharp" if file.extension().is_some_and(|e| e == "csx") => vec![("dotnet-script", format!("dotnet-script {f}"))],
        "c-sharp" => vec![("dotnet", "dotnet run".into())],
        "fsharp" => vec![("dotnet", format!("dotnet fsi {f}"))],
        "swift" => vec![("swift", format!("swift {f}"))],
        "dart" => vec![("dart", format!("dart run {f}"))],
        "zig" => vec![("zig", format!("zig run {f}"))],
        "nim" => vec![("nim", format!("nim r --hints:off {f}"))],
        "d" => vec![("ldc2", format!("ldc2 -run {f}")), ("rdmd", format!("rdmd {f}"))],
        "v" => vec![("v", format!("v run {f}"))],
        "odin" => vec![("odin", format!("odin run {f} -file"))],
        "hare" => vec![("hare", format!("hare run {f}"))],
        "koka" => vec![("koka", format!("koka --console=raw -v0 -e {f}"))],
        "lean" => vec![("lean", format!("lean --run {f}"))],
        "koto" => vec![("koto", format!("koto {f}"))],
        "inko" => vec![("inko", format!("inko run {f}"))],
        "pascal" => vec![("fpc", format!("fpc -v0 -o{tmp} {f} >/dev/null && {tmp}"))],
        "fortran" => vec![("gfortran", format!("gfortran -g -o {stem} {f} && {stem}"))],
        "ada" => vec![("gnatmake", format!("cd {d} && gnatmake -q -o {tmp} {f} && {tmp}"))],
        "sml" => vec![("poly", format!("poly --script {f}")), ("sml", format!("sml {f}"))],
        "prolog" => vec![("swipl", format!("swipl {f}"))],
        "forth" => vec![("gforth", format!("gforth {f} -e bye"))],
        "tcl" => vec![("tclsh", format!("tclsh {f}"))],
        "awk" => vec![("awk", format!("awk -f {f}"))],
        "jq" => vec![("jq", format!("jq -n -f {f}"))],
        "nasm" => vec![("nasm", format!("nasm -felf64 {f} -o {tmp}.o && ld {tmp}.o -o {tmp} && {tmp}"))],
        "gas" => vec![("as", format!("as {f} -o {tmp}.o && ld {tmp}.o -o {tmp} && {tmp}"))],
        "llvm" => vec![("lli", format!("lli {f}"))],
        "uxntal" => vec![("uxncli", format!("uxnasm {f} {tmp}.rom >/dev/null && uxncli {tmp}.rom"))],
        "amber" => vec![("amber", format!("amber run {f}"))],
        "nix" => vec![("nix", format!("nix eval --impure --file {f}"))],
        "jsonnet" => vec![("jsonnet", format!("jsonnet {f}"))],
        "nickel" => vec![("nickel", format!("nickel export {f}"))],
        "dhall" => vec![("dhall", format!("dhall --file {f}"))],
        "cue" => vec![("cue", format!("cue eval {f}"))],
        "pkl" => vec![("pkl", format!("pkl eval {f}"))],
        "rego" => vec![("opa", format!("opa eval --format pretty -d {f} data"))],
        "prql" => vec![("prqlc", format!("prqlc compile {f}"))],
        "scss" => vec![("sass", format!("sass {f}"))],
        "sql" => vec![("duckdb", format!("duckdb < {f}")), ("sqlite3", format!("sqlite3 :memory: < {f}"))],
        "make" => vec![("make", format!("make -f {f}"))],
        "cmake" => vec![("cmake", format!("cmake -P {f}"))],
        "typst" => vec![("typst", format!("typst compile {f} && xdg-open {stem}.pdf"))],
        "latex" => vec![("tectonic", format!("tectonic {f} && xdg-open {stem}.pdf")), ("pdflatex", format!("cd {d} && pdflatex {f} && xdg-open {stem}.pdf"))],
        "dot" => vec![("dot", format!("dot -Tsvg {f} -o {tmp}.svg && xdg-open {tmp}.svg"))],
        "openscad" => vec![("openscad", format!("openscad {f}"))],
        "markdown" | "html" | "svg" => vec![("xdg-open", format!("xdg-open {f}"))],
        _ => vec![],
    };
    c.into_iter().find(|(tool, _)| on_path(tool)).map(|(_, c)| c)
}

/// A problem a tool reported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub path: PathBuf,
    /// 0-based.
    pub line: usize,
    pub col: usize,
    /// 1 error, 2 warning, 3 info.
    pub severity: u8,
    pub message: String,
}

fn severity(word: &str) -> u8 {
    match word.to_ascii_lowercase().as_str() {
        w if w.starts_with("err") || w == "fatal" || w == "fatal error" => 1,
        w if w.starts_with("warn") => 2,
        _ => 3,
    }
}

/// Reads compiler, linter and interpreter output a line at a time:
/// `file:line:col: error: msg` (gcc, clang, go, zig, swift, shellcheck -f
/// gcc, ruff), rustc's `error: msg` then `--> file:line:col`, tsc's
/// `file(line,col): error TS…`, Python's `File "x", line N` tracebacks, and
/// plain `file:line: msg`.
pub struct ProblemParser {
    cwd: PathBuf,
    pending: Option<(u8, String)>,
    py_at: Option<(PathBuf, usize)>,
}

fn res() -> &'static [Regex; 7] {
    static R: OnceLock<[Regex; 7]> = OnceLock::new();
    R.get_or_init(|| {
        [
            // file:line:col: severity: message
            Regex::new(r"^\s*([^\s:][^:]*?):(\d+):(\d+):\s*(fatal error|error|warning|note|info|hint|[EWCF]\d+)?:?\s*(.*)$").unwrap(),
            // rustc: error[E0308]: msg / warning: msg
            Regex::new(r"^(error|warning)(\[\w+\])?: (.*)$").unwrap(),
            // rustc:  --> file:line:col
            Regex::new(r"^\s*--> ([^:]+):(\d+):(\d+)").unwrap(),
            // tsc / msbuild: file(line,col): error TS1234: msg
            Regex::new(r"^\s*([^\s(][^(]*)\((\d+),(\d+)\):\s*(error|warning|info)\s*\w*:?\s*(.*)$").unwrap(),
            // Python: File "x", line N
            Regex::new(r#"^\s*File "([^"]+)", line (\d+)"#).unwrap(),
            // file:line: message (no column)
            Regex::new(r"^\s*([^\s:][^:]*?):(\d+):\s*(.+)$").unwrap(),
            // sh -n: file: line N: message
            Regex::new(r"^([^:\s][^:]*): line (\d+): (.+)$").unwrap(),
        ]
    })
}

impl ProblemParser {
    pub fn new(cwd: &Path) -> ProblemParser {
        ProblemParser { cwd: cwd.to_path_buf(), pending: None, py_at: None }
    }

    fn path(&self, p: &str) -> Option<PathBuf> {
        let p = PathBuf::from(p.trim());
        let full = if p.is_absolute() { p } else { self.cwd.join(p) };
        // (Only real files: not "note: in expansion of…" and the like.)
        full.is_file().then(|| std::fs::canonicalize(&full).unwrap_or(full))
    }

    pub fn line(&mut self, l: &str) -> Option<Problem> {
        let r = res();
        let num = |s: &str| s.parse::<usize>().unwrap_or(1).saturating_sub(1);
        if let Some(c) = r[1].captures(l) {
            self.pending = Some((severity(&c[1]), c[3].to_string()));
            return None;
        }
        if let Some(c) = r[2].captures(l) {
            let (sev, msg) = self.pending.take()?;
            return Some(Problem { path: self.path(&c[1])?, line: num(&c[2]), col: num(&c[3]), severity: sev, message: msg });
        }
        if let Some(c) = r[4].captures(l) {
            self.py_at = self.path(&c[1]).map(|p| (p, num(&c[2])));
            return None;
        }
        if let Some((p, line)) = self.py_at.clone() {
            // The traceback's last line: "SomeError: why".
            let t = l.trim();
            if !t.is_empty() && !l.starts_with(' ') && (t.contains("Error") || t.contains("Exception")) {
                self.py_at = None;
                return Some(Problem { path: p, line, col: 0, severity: 1, message: t.to_string() });
            }
        }
        if let Some(c) = r[0].captures(l) {
            let path = self.path(&c[1])?;
            let word = c.get(4).map(|m| m.as_str()).unwrap_or("error");
            // ruff / flake8 codes: E… W… F… C…
            let sev = if word.len() > 1 && word[1..].chars().all(|ch| ch.is_ascii_digit()) {
                if word.starts_with('W') || word.starts_with('C') { 2 } else { 1 }
            } else {
                severity(word)
            };
            let msg = if word.len() > 1 && word[1..].chars().all(|ch| ch.is_ascii_digit()) { format!("{word} {}", &c[5]) } else { c[5].to_string() };
            return Some(Problem { path, line: num(&c[2]), col: num(&c[3]), severity: sev, message: msg });
        }
        if let Some(c) = r[3].captures(l) {
            return Some(Problem { path: self.path(&c[1])?, line: num(&c[2]), col: num(&c[3]), severity: severity(&c[4]), message: c[5].to_string() });
        }
        if let Some(c) = r[6].captures(l) {
            if let Some(path) = self.path(&c[1]) {
                return Some(Problem { path, line: num(&c[2]), col: 0, severity: 1, message: c[3].trim().to_string() });
            }
        }
        if let Some(c) = r[5].captures(l) {
            let path = self.path(&c[1])?;
            let msg = c[3].trim();
            let sev = if msg.to_ascii_lowercase().starts_with("warning") { 2 } else { 1 };
            return Some(Problem { path, line: num(&c[2]), col: 0, severity: sev, message: msg.to_string() });
        }
        None
    }
}

/// What a running command sends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunEvent {
    Line(String),
    /// It finished: its exit code (None: killed or didn't start).
    Done(Option<i32>, String),
}

/// A command running on its own threads.
pub struct Running {
    pub rx: Receiver<RunEvent>,
    pub child: Option<std::sync::Arc<std::sync::Mutex<std::process::Child>>>,
}

impl Running {
    pub fn kill(&self) {
        if let Some(c) = &self.child {
            if let Ok(mut c) = c.lock() {
                let _ = c.kill();
            }
        }
    }
}

/// Run `sh -c command` in `cwd`; its output (both streams) comes back by
/// lines.
pub fn spawn(command: &str, cwd: &Path) -> Running {
    let (tx, rx) = channel();
    let child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("CARGO_TERM_COLOR", "never")
        .env("NO_COLOR", "1")
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(RunEvent::Done(None, e.to_string()));
            return Running { rx, child: None };
        }
    };
    let (out, err) = (child.stdout.take(), child.stderr.take());
    let child = std::sync::Arc::new(std::sync::Mutex::new(child));
    let readers: Vec<_> = [out.map(|o| Box::new(o) as Box<dyn std::io::Read + Send>), err.map(|e| Box::new(e) as Box<dyn std::io::Read + Send>)]
        .into_iter()
        .flatten()
        .map(|r| {
            let tx = tx.clone();
            std::thread::spawn(move || {
                for l in BufReader::new(r).lines().map_while(Result::ok) {
                    if tx.send(RunEvent::Line(l)).is_err() {
                        break;
                    }
                }
            })
        })
        .collect();
    let waiter = child.clone();
    std::thread::spawn(move || {
        for r in readers {
            let _ = r.join();
        }
        let code = loop {
            match waiter.lock().map(|mut c| c.try_wait()) {
                Ok(Ok(Some(s))) => break s.code(),
                Ok(Ok(None)) => {}
                _ => break None,
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        let _ = tx.send(RunEvent::Done(code, String::new()));
    });
    Running { rx, child: Some(child) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_compiler_messages() {
        let dir = std::env::temp_dir().join(format!("turbine-build-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        for f in ["src/main.rs", "a.c", "b.ts", "c.py"] {
            std::fs::write(dir.join(f), "x\n").unwrap();
        }
        let canon = |f: &str| std::fs::canonicalize(dir.join(f)).unwrap();
        let mut p = ProblemParser::new(&dir);
        assert_eq!(p.line("error[E0308]: mismatched types"), None);
        let e = p.line("  --> src/main.rs:3:9").unwrap();
        assert_eq!((e.path, e.line, e.col, e.severity, e.message.as_str()), (canon("src/main.rs"), 2, 8, 1, "mismatched types"));
        let e = p.line("a.c:10:5: warning: unused variable 'x' [-Wunused-variable]").unwrap();
        assert_eq!((e.line, e.severity), (9, 2));
        let e = p.line("b.ts(4,2): error TS2322: Type 'string' is not assignable").unwrap();
        assert_eq!((e.path, e.line, e.col, e.severity), (canon("b.ts"), 3, 1, 1));
        assert_eq!(p.line(&format!("  File \"{}\", line 7, in <module>", dir.join("c.py").display())), None);
        assert_eq!(p.line("    foo()"), None);
        let e = p.line("NameError: name 'foo' is not defined").unwrap();
        assert_eq!((e.line, e.message.as_str()), (6, "NameError: name 'foo' is not defined"));
        let e = p.line("c.py:2:1: F821 undefined name 'y'").unwrap();
        assert_eq!((e.severity, e.message.as_str()), (1, "F821 undefined name 'y'"));
        assert_eq!(p.line("note: some note about nothing"), None);
        assert_eq!(p.line("nowhere.rs:1:1: error: no such file"), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn tasks_from_files_and_projects() {
        let dir = std::env::temp_dir().join(format!("turbine-tasks-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".vscode")).unwrap();
        std::fs::write(dir.join("Makefile"), "all:\n\ttrue\n").unwrap();
        std::fs::write(
            dir.join(".vscode/tasks.json"),
            "{ // tasks\n \"tasks\": [{\"label\": \"mine\", \"type\": \"shell\", \"command\": \"echo\", \"args\": [\"hi there\"], \"group\": {\"kind\": \"build\", \"isDefault\": true}},]}",
        )
        .unwrap();
        let t = tasks(&dir);
        assert_eq!(t[0].command, "echo 'hi there'");
        assert_eq!(default_build(&t).unwrap().label, "mine");
        assert!(t.iter().any(|t| t.label == "make" && t.group == "build"));
        let r = spawn("echo one; echo two >&2; exit 3", &dir);
        let mut got = Vec::new();
        while let Ok(e) = r.rx.recv() {
            let done = matches!(e, RunEvent::Done(..));
            got.push(e);
            if done {
                break;
            }
        }
        assert!(got.contains(&RunEvent::Line("one".into())) && got.contains(&RunEvent::Line("two".into())));
        assert_eq!(got.last(), Some(&RunEvent::Done(Some(3), String::new())));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[cfg(test)]
mod tessh_tests {
    use super::*;

    /// tessh's checker and guides, when tessh is installed.
    #[test]
    fn tessh_check_and_guides() {
        if !on_path("tessh") {
            return;
        }
        let dir = std::env::temp_dir().join(format!("turbine-tessh-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("bad.tsh");
        std::fs::write(&f, "def f x\n    print (1 +\n").unwrap();
        let cmd = check_command("tessh", &f, &dir).unwrap();
        assert!(cmd.starts_with("tessh --check"));
        let r = spawn(&cmd, &dir);
        let mut p = ProblemParser::new(&dir);
        let mut probs = Vec::new();
        while let Ok(e) = r.rx.recv() {
            match e {
                RunEvent::Line(l) => probs.extend(p.line(&l)),
                RunEvent::Done(..) => break,
            }
        }
        assert!(!probs.is_empty(), "tessh --check found nothing");
        assert_eq!(probs[0].path, std::fs::canonicalize(&f).unwrap());
        let md = crate::guides::guide_markdown("tessh", false).unwrap();
        assert!(md.starts_with("# tessh guide"), "{}", &md[..80.min(md.len())]);
        let adv = crate::guides::guide_markdown("tessh-advanced", false).unwrap();
        assert!(adv.contains("Advanced tessh Scripting Guide"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
