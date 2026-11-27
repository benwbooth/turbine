//! Git, beyond the Source Control view: a file's changes, the working
//! tree's and the staged diff (as delta shows them), the history and a
//! commit, branches, pull / push / fetch / stash (their output in the
//! Output window), and who changed the cursor's line. git runs off the UI
//! thread.

use super::preview::PageKind;
use super::*;
use std::sync::mpsc::{Sender, channel};

pub(super) enum GitAnswer {
    Branches(PathBuf, Result<String, String>),
    Blame(Result<String, String>),
}

pub(super) struct GitState {
    tx: Sender<GitAnswer>,
    rx: Receiver<GitAnswer>,
}

impl Default for GitState {
    fn default() -> Self {
        let (tx, rx) = channel();
        GitState { tx, rx }
    }
}

/// The repository a path is in (the folder holding .git).
pub(super) fn repo_root(p: &Path) -> Option<PathBuf> {
    let mut d = if p.is_dir() { Some(p) } else { p.parent() };
    while let Some(x) = d {
        if x.join(".git").exists() {
            return Some(x.to_path_buf());
        }
        d = x.parent();
    }
    None
}

impl Workbench {
    /// The front file and its repository: (root, path from the root).
    fn git_here(&self) -> Option<(PathBuf, Option<String>)> {
        let file = self.windows.iter().rev().find(|w| w.term.is_none() && w.special.is_none()).and_then(|w| w.ed.doc.path.clone());
        let root = file.as_deref().and_then(repo_root).or_else(|| repo_root(&self.shell.explorer.root))?;
        let rel = file.and_then(|f| Self::canon(&f).strip_prefix(Self::canon(&root)).ok().map(|r| r.display().to_string()));
        Some((root, rel))
    }

    pub(super) fn git_command(&mut self, cmd: &str) {
        let Some((root, rel)) = self.git_here() else {
            self.say("Not in a git repository");
            return;
        };
        let diff = PageKind::Diff { side_by_side: false };
        let task = |label: &str, command: &str| turbine_services::build::Task { label: label.into(), command: command.into(), cwd: root.clone(), group: String::new(), default: false };
        match cmd {
            "git.diff_file" => match rel {
                Some(r) => {
                    let name = Path::new(&r).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    self.show_git_page(&format!("Changes: {name}"), diff, &root, vec!["diff".into(), "HEAD".into(), "--".into(), r]);
                }
                None => self.say("Open a file in the repository first"),
            },
            "git.diff_all" => self.show_git_page("Changes", diff, &root, vec!["diff".into(), "HEAD".into()]),
            "git.diff_staged" => self.show_git_page("Staged Changes", diff, &root, vec!["diff".into(), "--cached".into()]),
            "git.log" => self.show_git_page("History", PageKind::Log, &root, vec!["log".into(), "-500".into(), "--format=%h\u{1f}%s\u{1f}%an\u{1f}%ar".into()]),
            "git.log_file" => match rel {
                Some(r) => self.show_git_page("File History", PageKind::Log, &root, vec!["log".into(), "-500".into(), "--format=%h\u{1f}%s\u{1f}%an\u{1f}%ar".into(), "--".into(), r]),
                None => self.say("Open a file in the repository first"),
            },
            "git.pull" => self.run_task(&task("git pull", "git pull")),
            "git.push" => self.run_task(&task("git push", "git push")),
            "git.fetch" => self.run_task(&task("git fetch", "git fetch --all --prune")),
            "git.stash" => self.run_task(&task("git stash", "git stash push")),
            "git.stash_pop" => self.run_task(&task("git stash pop", "git stash pop")),
            "git.checkout" => {
                let tx = self.git.tx.clone();
                let r = root.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(GitAnswer::Branches(r.clone(), crate::views::git(&r, &["branch", "--all", "--format=%(HEAD) %(refname:short)"])));
                });
            }
            "git.blame" => {
                let (Some(r), Some(w)) = (rel, self.windows.iter().rev().find(|w| w.term.is_none() && w.special.is_none())) else { return };
                let line = w.ed.cursor_line_col().0 + 1;
                let tx = self.git.tx.clone();
                std::thread::spawn(move || {
                    let out = crate::views::git(&root, &["blame", "--porcelain", "-L", &format!("{line},{line}"), "--", &r]);
                    let _ = tx.send(GitAnswer::Blame(out));
                });
            }
            _ => {}
        }
    }

    pub(super) fn git_tick(&mut self) {
        let answers: Vec<GitAnswer> = self.git.rx.try_iter().collect();
        for a in answers {
            match a {
                GitAnswer::Branches(root, Ok(out)) => {
                    let mut current = 0;
                    let names: Vec<String> = out
                        .lines()
                        .enumerate()
                        .map(|(i, l)| {
                            if l.starts_with('*') {
                                current = i;
                            }
                            l[1..].trim().to_string()
                        })
                        .filter(|n| !n.is_empty() && !n.ends_with("/HEAD"))
                        .collect();
                    let d = Dialog::new(&format!("checkout:{}", root.display()), "Switch Branch")
                        .body(ui::list("branch", names, current).activates().lines(14).chars(40))
                        .buttons(vec![ui::button("ok", "~S~witch", true), ui::button("cancel", "Cancel", false)]);
                    self.dialog = Some(d);
                }
                GitAnswer::Branches(_, Err(e)) => self.say(format!("git: {e}")),
                GitAnswer::Blame(Ok(out)) => {
                    let field = |k: &str| out.lines().find_map(|l| l.strip_prefix(k)).unwrap_or("").trim().to_string();
                    let hash: String = out.split_whitespace().next().unwrap_or("").chars().take(8).collect();
                    let when = field("author-time ").parse::<i64>().ok().map(|t| {
                        let days = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(t) - t) / 86400;
                        if days == 0 { "today".to_string() } else { format!("{days} days ago") }
                    });
                    if hash.chars().all(|c| c == '0') {
                        self.say("Not committed yet");
                    } else {
                        self.say(format!("{hash} {}, {}: {}", field("author "), when.unwrap_or_default(), field("summary ")));
                    }
                }
                GitAnswer::Blame(Err(e)) => self.say(format!("git blame: {e}")),
            }
        }
    }

    /// Switch Branch's choice.
    pub(super) fn checkout_chosen(&mut self, d: &Dialog, root: &str) {
        let Some(b) = d.value("branch") else { return };
        // (A remote branch: its local one, made to follow it.)
        let local = b.split_once('/').filter(|(r, _)| *r == "origin").map(|(_, n)| n.to_string());
        let cmd = match local {
            Some(n) => format!("git switch {} || git switch -c {} --track {}", build_quote(&n), build_quote(&n), build_quote(&b)),
            None => format!("git switch {}", build_quote(&b)),
        };
        let t = turbine_services::build::Task { label: format!("git switch {b}"), command: cmd, cwd: PathBuf::from(root), group: String::new(), default: false };
        self.run_task(&t);
    }

    /// The Source Control view's Enter: the file's diff (its staged one
    /// when it's staged).
    pub(super) fn scm_diff(&mut self, staged: bool, rel: &Path) {
        let Some((root, _)) = self.git_here() else { return };
        let r = rel.display().to_string();
        let name = rel.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let root2 = root.clone();
        self.show_page_from(&format!("Changes: {name}"), PageKind::Diff { side_by_side: false }, &root, move || {
            // (Untracked: the whole file is new; diff --no-index says so.)
            let tracked = crate::views::git(&root2, &["ls-files", "--error-unmatch", "--", &r]).is_ok();
            let args: Vec<&str> = match (tracked, staged) {
                (false, _) => vec!["diff", "--no-index", "--", "/dev/null", &r],
                (true, true) => vec!["diff", "--cached", "--", &r],
                (true, false) => vec!["diff", "--", &r],
            };
            let out = std::process::Command::new("git").arg("-C").arg(&root2).args(&args).output().map_err(|e| e.to_string())?;
            // (diff --no-index exits 1 when there are differences.)
            if out.status.success() || out.status.code() == Some(1) {
                Ok(String::from_utf8_lossy(&out.stdout).into_owned())
            } else {
                Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
            }
        });
    }
}

fn build_quote(s: &str) -> String {
    turbine_services::build::sh_quote(s)
}
