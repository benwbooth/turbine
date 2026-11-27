//! A project's own environment, as its tools should see it: direnv's (and
//! so nix-direnv's: `use flake`, `use nix`) when the project has an
//! `.envrc`, read in the background once per project and again when the
//! `.envrc` changes; and tools a project needs but nobody installed, run
//! from nixpkgs with `nix shell` instead.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

#[derive(Clone, Debug)]
enum State {
    /// direnv is reading it (nix-direnv's first time can take minutes).
    Loading,
    /// Done: the variables to set (None: no .envrc, or direnv refused).
    Ready(Option<HashMap<String, String>>, Option<String>),
}

/// The project's environment: Ready(changes to the process's own), or
/// Loading (ask again in a moment).
#[derive(Clone, Debug)]
pub enum Env {
    Loading,
    Ready(HashMap<String, String>),
}

fn cache() -> &'static Mutex<HashMap<PathBuf, (State, Option<SystemTime>)>> {
    static C: OnceLock<Mutex<HashMap<PathBuf, (State, Option<SystemTime>)>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// The .envrc that governs a folder (it or the nearest above it).
pub fn envrc_for(root: &Path) -> Option<PathBuf> {
    let mut d = Some(root);
    while let Some(x) = d {
        let f = x.join(".envrc");
        if f.is_file() {
            return Some(f);
        }
        d = x.parent();
    }
    None
}

/// The environment for a project's tools. Never waits: the first ask
/// starts direnv in the background and says Loading.
pub fn project_env(root: &Path) -> Env {
    let Some(envrc) = envrc_for(root) else { return Env::Ready(HashMap::new()) };
    if !crate::rpc::on_path("direnv") {
        return Env::Ready(HashMap::new());
    }
    let stamp = std::fs::metadata(&envrc).and_then(|m| m.modified()).ok();
    let mut c = cache().lock().unwrap_or_else(|e| e.into_inner());
    match c.get(root) {
        Some((State::Ready(v, _), s)) if *s == stamp => return Env::Ready(v.clone().unwrap_or_default()),
        Some((State::Loading, _)) => return Env::Loading,
        _ => {}
    }
    c.insert(root.to_path_buf(), (State::Loading, stamp));
    drop(c);
    let root = root.to_path_buf();
    std::thread::spawn(move || {
        let (vars, err) = direnv_export(&root);
        cache().lock().unwrap_or_else(|e| e.into_inner()).insert(root, (State::Ready(vars, err), stamp));
    });
    Env::Loading
}

/// What direnv said when it couldn't give the environment (a blocked
/// .envrc wants `direnv allow`), once it's done.
pub fn project_env_error(root: &Path) -> Option<String> {
    match cache().lock().unwrap_or_else(|e| e.into_inner()).get(root) {
        Some((State::Ready(_, e), _)) => e.clone(),
        _ => None,
    }
}

fn direnv_export(root: &Path) -> (Option<HashMap<String, String>>, Option<String>) {
    let out = std::process::Command::new("direnv").arg("export").arg("json").current_dir(root).stdin(std::process::Stdio::null()).output();
    let Ok(o) = out else { return (None, Some("direnv didn't run".into())) };
    let err = String::from_utf8_lossy(&o.stderr);
    if err.contains("is blocked") {
        return (None, Some(format!("direnv: {} is blocked; run `direnv allow` there to use its environment", envrc_for(root).map(|p| p.display().to_string()).unwrap_or_default())));
    }
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap_or(serde_json::Value::Null);
    let mut vars = HashMap::new();
    if let Some(m) = v.as_object() {
        for (k, val) in m {
            // (null: direnv unsets it; its own bookkeeping isn't for tools.)
            if k.starts_with("DIRENV_") {
                continue;
            }
            vars.insert(k.clone(), val.as_str().unwrap_or("").to_string());
        }
    }
    (Some(vars), None)
}

/// A program found on the project's PATH (direnv's, else the process's).
pub fn find_program(name: &str, env: &HashMap<String, String>) -> Option<PathBuf> {
    if Path::new(name).is_absolute() {
        return Path::new(name).is_file().then(|| PathBuf::from(name));
    }
    let path = env.get("PATH").cloned().or_else(|| std::env::var("PATH").ok())?;
    std::env::split_paths(&path).map(|d| d.join(name)).find(|p| p.is_file())
}

/// Whether nix can run things (`nix shell nixpkgs#…`).
pub fn has_nix() -> bool {
    crate::rpc::on_path("nix")
}

/// A command to run a nixpkgs package's program without installing it.
pub fn nix_shell(pkg: &str, argv: &[String]) -> Vec<String> {
    let mut v = vec!["nix".to_string(), "--extra-experimental-features".into(), "nix-command flakes".into(), "shell".into(), format!("nixpkgs#{pkg}"), "-c".into()];
    v.extend(argv.iter().cloned());
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_envrc_is_ready_and_empty() {
        let d = std::env::temp_dir().join(format!("turbine-env-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        // (A temp folder has no .envrc above it, normally.)
        if envrc_for(&d).is_none() {
            assert!(matches!(project_env(&d), Env::Ready(m) if m.is_empty()));
        }
        let mut env = HashMap::new();
        env.insert("PATH".to_string(), "/nonexistent".to_string());
        assert!(find_program("sh", &env).is_none());
        assert_eq!(nix_shell("clang-tools", &["clangd".into()])[4], "nixpkgs#clang-tools");
        std::fs::remove_dir_all(&d).unwrap();
    }
}
