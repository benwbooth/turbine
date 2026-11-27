//! The Source Control view's buttons and menus, as VS Code's: a Commit
//! button (Sync Changes or Publish Branch when there's nothing to commit)
//! with its drop-down (Commit & Push, Commit & Sync, Amend, Undo Last
//! Commit), the branch with its ahead/behind counts, Refresh / Pull / Push
//! and a ⋯ menu (Fetch, Stash, branches, Stage All, History…), and each
//! file's right-click menu (Open, Open Changes, Stage, Discard, Add to
//! .gitignore, Reveal). git runs off the UI thread (Scm::run_with).

use super::*;

/// A toolbar button: its place, action, label and tooltip.
pub(super) struct ScmButton {
    pub rect: Rect,
    pub action: &'static str,
    pub label: String,
    pub tip: &'static str,
}

/// Rows above the file list: the message box, the Commit button, the branch line.
pub(super) const SCM_LIST_TOP: i32 = 3;

impl Workbench {
    /// The folder git runs in for the Source Control view.
    pub(super) fn scm_root(&self) -> PathBuf {
        self.shell.scm.root().unwrap_or_else(|| self.shell.explorer.root.clone())
    }

    /// The big button's job: commit when there are changes, else sync
    /// (ahead or behind) or publish (no upstream).
    fn scm_main(&self) -> (&'static str, String, &'static str) {
        let Some(s) = self.shell.scm.active() else { return ("commit", "✓ Commit".into(), "Commit") };
        if !s.has_changes() && !s.branch.is_empty() {
            if s.upstream.is_none() && !s.branch.starts_with("HEAD") && !s.branch.starts_with("No commits") {
                return ("publish", "☁ Publish Branch".into(), "Push this branch to origin and follow it");
            }
            if s.ahead > 0 || s.behind > 0 {
                return ("sync", format!("⇅ Sync Changes {}↓ {}↑", s.behind, s.ahead), "Pull, then push");
            }
        }
        ("commit", "✓ Commit".into(), "Commit the staged changes (all of them when none are staged)")
    }

    /// The Source Control toolbar's buttons in the view's inside.
    pub(super) fn scm_buttons(&self, inner: Rect) -> Vec<ScmButton> {
        let sc = &self.shell.scm;
        let blank = crate::views::Repo::default();
        let s = sc.active().unwrap_or(&blank);
        let mut v = Vec::new();
        if inner.w < 8 || inner.h < 3 {
            return v;
        }
        // Row 1: the main button, its drop-down at the right.
        let (action, label, tip) = self.scm_main();
        let y = inner.y + 1;
        let main = Rect::new(inner.x + 1, y, (inner.w - 5).max(1), 1);
        v.push(ScmButton { rect: main, action, label, tip });
        v.push(ScmButton { rect: Rect::new(main.right(), y, 3, 1), action: "commit_menu", label: "▾".into(), tip: "More ways to commit" });
        // Row 2: the branch, then Refresh, Pull, Push and ⋯ at the right.
        let y = inner.y + 2;
        let icons: [(&str, &str, &str); 4] = [("refresh", "↻", "Refresh"), ("pull", "↓", "Pull"), ("push", "↑", "Push"), ("more", "⋯", "More Actions…")];
        let icons_x = inner.right() - 1 - icons.len() as i32 * 3;
        let mut branch = if s.branch.is_empty() { "⎇ …".to_string() } else { format!("⎇ {}", s.branch) };
        // (Several projects open: which one this is; the menu switches.)
        if sc.repos.len() > 1 {
            branch = format!("▣ {}  {branch}", s.name());
        }
        if s.ahead > 0 || s.behind > 0 {
            branch.push_str(&format!(" {}↓ {}↑", s.behind, s.ahead));
        }
        let bw = (str_width(&branch) as i32 + 2).min((icons_x - inner.x - 1).max(0));
        if bw > 2 {
            v.push(ScmButton { rect: Rect::new(inner.x + 1, y, bw, 1), action: "branch", label: branch, tip: "Switch Branch… (commits to pull ↓ and to push ↑)" });
        }
        for (k, (a, l, t)) in icons.iter().enumerate() {
            v.push(ScmButton { rect: Rect::new(icons_x + k as i32 * 3, y, 3, 1), action: a, label: (*l).into(), tip: t });
        }
        v
    }

    /// Draw the toolbar (the main button as the scheme's buttons).
    pub(super) fn draw_scm_buttons(&self, c: &mut Canvas, inner: Rect, theme: &Theme) {
        let s = &theme.scheme;
        let plain = Style::new(s.panel.fg, s.panel.bg);
        // (The scheme's button, as the dialogs' are; another when it would
        // vanish into the panel.)
        let button = if s.button.bg != s.panel.bg { s.button } else { s.button_focused };
        let busy = self.shell.scm.status == "Working…";
        let mut c = c.sub(inner);
        for b in self.scm_buttons(inner) {
            match b.action {
                "commit" | "sync" | "publish" => {
                    c.fill(b.rect, " ", button);
                    let lw = str_width(&b.label) as i32;
                    c.print(b.rect.x + ((b.rect.w - lw) / 2).max(0), b.rect.y, &b.label, button);
                }
                "commit_menu" => {
                    c.fill(b.rect, " ", button);
                    c.print(b.rect.x, b.rect.y, "│▾ ", button);
                }
                "branch" => {
                    c.print(b.rect.x, b.rect.y, &b.label, Style::new(s.panel_frame.fg, s.panel.bg));
                }
                _ => {
                    let st = if busy && b.action == "refresh" { Style::new(vga::YELLOW, s.panel.bg) } else { plain };
                    c.print(b.rect.x + 1, b.rect.y, &b.label, st);
                }
            }
        }
    }

    /// A click on the toolbar: true when it hit a button.
    pub(super) fn scm_click(&mut self, at: Point, inner: Rect) -> bool {
        let Some(b) = self.scm_buttons(inner).into_iter().find(|b| b.rect.contains(at)) else { return false };
        let below = Point::new(b.rect.x, b.rect.y + 1);
        match b.action {
            "commit_menu" => self.scm_menu_open("commit", Point::new(b.rect.right() - 1, b.rect.y + 1)),
            "more" => self.scm_menu_open("more", Point::new(b.rect.right() - 1, b.rect.y + 1)),
            "branch" => self.scm_menu_open("branch", below),
            a => self.scm_action(a),
        }
        true
    }

    /// Open one of the view's menus: "commit" (the drop-down), "more"
    /// (⋯), "branch", or "file" (the selected file's).
    pub(super) fn scm_menu_open(&mut self, which: &str, at: Point) {
        let it = |l: &str, d: &str, a: &str| ((l.to_string(), d.to_string()), format!("scm:{a}"));
        let items: Vec<((String, String), String)> = match which {
            "commit" => vec![
                it("Commit", "Ctrl+Enter", "commit"),
                it("Commit & Push", "", "commit_push"),
                it("Commit & Sync", "pull, then push", "commit_sync"),
                it("Commit (Amend)", "into the last commit", "amend"),
                it("Undo Last Commit", "its changes kept", "undo_last"),
            ],
            "more" => vec![
                it("Pull", "", "pull"),
                it("Pull (Rebase)", "", "pull_rebase"),
                it("Push", "", "push"),
                it("Sync", "pull, then push", "sync"),
                it("Fetch", "all remotes", "fetch"),
                it("Stage All Changes", "", "stage_all"),
                it("Unstage All Changes", "", "unstage_all"),
                it("Discard All Changes…", "tracked files", "discard_all"),
                it("Stash", "with untracked files", "stash"),
                it("Pop Latest Stash", "", "pop"),
                it("Switch Branch…", "", "checkout"),
                it("Create Branch…", "", "new_branch"),
                it("View History", "", "history"),
                it("Show All Changes", "as a diff", "diff_all"),
            ],
            "branch" => {
                let s = &self.shell.scm;
                let cur = s.active_index();
                // (Several projects: each repository, to go to its changes.)
                let mut v: Vec<((String, String), String)> = if s.repos.len() < 2 {
                    Vec::new()
                } else {
                    s.repos
                        .iter()
                        .enumerate()
                        .map(|(k, r)| {
                            let mark = if k == cur { "◉" } else { "○" };
                            let detail = match (r.branch.is_empty(), r.changed_count()) {
                                (true, _) => r.error.clone().unwrap_or_default(),
                                (false, 0) => format!("⎇ {}", r.branch),
                                (false, n) => format!("⎇ {}  • {n}", r.branch),
                            };
                            ((format!("{mark} {}", r.name()), detail), format!("scm:repo:{k}"))
                        })
                        .collect()
                };
                v.extend([
                it("Switch Branch…", "", "checkout"),
                it("Create Branch…", "", "new_branch"),
                it("Publish Branch", "push to origin", "publish"),
                it("Fetch", "", "fetch"),
                ]);
                v
            }
            "file" => {
                let i = self.shell.scm.list.selected;
                let Some((staged, _)) = self.shell.scm.entry(i) else { return };
                let untracked = self.shell.scm.codes.get(i) == Some(&'U');
                let conflict = self.shell.scm.codes.get(i) == Some(&'C');
                let mut v = vec![it("Open File", "o", "f_open"), it("Open Changes", "Enter", "f_diff")];
                v.push(if conflict {
                    it("Mark Resolved", "stage it", "f_stage")
                } else if staged {
                    it("Unstage Changes", "Space", "f_stage")
                } else {
                    it("Stage Changes", "Space", "f_stage")
                });
                if !staged {
                    v.push(it(if untracked { "Delete File…" } else { "Discard Changes…" }, if untracked { "to the trash" } else { "" }, "f_discard"));
                }
                v.push(it("Add to .gitignore", "", "f_ignore"));
                v.push(it("File History", "", "f_history"));
                v.push(it("Reveal in Explorer", "", "f_reveal"));
                v.push(it("Copy Path", "", "f_copy"));
                v
            }
            _ => return,
        };
        let (rows, acts): (Vec<(String, String)>, Vec<String>) = items.into_iter().unzip();
        let mut popup = ListPopup::new(rows);
        popup.max_rows = 16;
        popup.place(at, self.screen());
        self.status_menu = Some((popup, at, acts));
    }

    /// The Source Control view's actions (its buttons', menus' and keys').
    pub(super) fn scm_action(&mut self, a: &str) {
        let root = self.scm_root();
        let s = &mut self.shell.scm;
        let sv = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<String>>();
        match a {
            "commit" | "amend" | "commit_push" | "commit_sync" => {
                let extra: &[&str] = if a == "amend" { &["--amend"] } else { &[] };
                match s.commit_args(extra) {
                    Ok(args) => {
                        let mut cmds = vec![args];
                        if a == "commit_sync" {
                            cmds.push(sv(&["pull"]));
                        }
                        if a != "commit" && a != "amend" {
                            cmds.push(s.push_args());
                        }
                        s.run_all(&root, cmds);
                    }
                    Err(e) => self.say(e),
                }
            }
            "undo_last" => s.run(&root, sv(&["reset", "--soft", "HEAD~1"])),
            "refresh" => s.refresh_all(),
            "pull" => s.run(&root, sv(&["pull"])),
            "pull_rebase" => s.run(&root, sv(&["pull", "--rebase"])),
            "push" | "publish" => {
                let p = s.push_args();
                s.run(&root, p);
            }
            "sync" => {
                let p = s.push_args();
                s.run_all(&root, vec![sv(&["pull"]), p]);
            }
            "fetch" => s.run(&root, sv(&["fetch", "--all", "--prune"])),
            "stage_all" => s.run(&root, sv(&["add", "-A"])),
            "unstage_all" => s.run(&root, sv(&["reset", "-q"])),
            "stash" => s.run(&root, sv(&["stash", "push", "-u"])),
            "pop" => s.run(&root, sv(&["stash", "pop"])),
            "discard_all" => {
                let n = s.active().map_or(0, |r| r.changes.iter().filter(|(c, _)| *c != 'U').count());
                if n == 0 {
                    self.say("No changes to discard (untracked files are left alone)");
                    return;
                }
                self.scm_confirm("discard_all", "Discard All Changes", &format!("Discard the changes to {n} file{}? This can't be undone.", if n == 1 { "" } else { "s" }), "~D~iscard");
            }
            "checkout" => self.git_branches(root),
            "new_branch" => {
                let d = Dialog::new("scm_branch", "Create Branch")
                    .body(ui::input("name", "Branch ~n~ame:", "").chars(40))
                    .buttons(vec![ui::button("ok", "~C~reate", true), ui::button("cancel", "Cancel", false)]);
                self.dialog = Some(d);
            }
            "history" => self.show_git_page("History", super::preview::PageKind::Log, &root, vec!["log".into(), "-500".into(), "--format=%h\u{1f}%s\u{1f}%an\u{1f}%ar".into()]),
            "diff_all" => self.show_git_page("Changes", super::preview::PageKind::Diff { side_by_side: false }, &root, vec!["diff".into(), "HEAD".into()]),
            f if f.starts_with("f_") => self.scm_file_action(f),
            r if r.starts_with("repo:") => {
                let k: usize = r[5..].parse().unwrap_or(usize::MAX);
                self.shell.scm.select_repo(k);
            }
            _ => {}
        }
    }

    /// An action on the selected file.
    fn scm_file_action(&mut self, a: &str) {
        let root = self.scm_root();
        let i = self.shell.scm.list.selected;
        let Some((staged, rel)) = self.shell.scm.entry(i) else { return };
        let code = self.shell.scm.codes.get(i).copied().unwrap_or(' ');
        let path = rel.display().to_string();
        let full = root.join(&rel);
        match a {
            "f_open" => self.goto_file(&full, None, 0),
            "f_diff" => self.scm_diff(staged, &rel),
            "f_stage" => self.shell.scm.toggle_stage(i),
            "f_discard" if code == 'U' => {
                self.scm_confirm(&format!("trash\u{1f}{}", full.display()), "Delete File", &format!("Move {path} to the trash?"), "~D~elete");
            }
            "f_discard" => {
                self.scm_confirm(&format!("discard\u{1f}{path}"), "Discard Changes", &format!("Discard the changes to {path}? This can't be undone."), "~D~iscard");
            }
            "f_ignore" => {
                let line = format!("/{path}");
                self.shell.scm.run_with(&root, move |r| add_to_gitignore(r, &line).map(|()| format!("Added {line} to .gitignore")));
            }
            "f_history" => {
                self.show_git_page("File History", super::preview::PageKind::Log, &root, vec!["log".into(), "-500".into(), "--format=%h\u{1f}%s\u{1f}%an\u{1f}%ar".into(), "--".into(), path]);
            }
            "f_reveal" => {
                let p = std::fs::canonicalize(&full).unwrap_or(full);
                self.show_tab(crate::dock::Tab::Explorer);
                self.shell.explorer.reveal(&p);
            }
            "f_copy" => {
                self.set_clipboard(full.display().to_string());
                self.say(format!("Copied {}", full.display()));
            }
            _ => {}
        }
    }

    fn scm_confirm(&mut self, what: &str, title: &str, text: &str, ok: &str) {
        let d = Dialog::new(&format!("scm_confirm:{what}"), title)
            .body(ui::label(text))
            .buttons(vec![ui::button("ok", ok, false), ui::button("cancel", "Cancel", true)]);
        self.dialog = Some(d);
    }

    /// The view's dialogs' answers: true when it was one of them.
    pub(super) fn scm_dialog_done(&mut self, kind: &str, d: &Dialog) -> bool {
        let root = self.scm_root();
        let sv = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<String>>();
        if kind == "scm_branch" {
            let name = d.value("name").unwrap_or_default().trim().to_string();
            if !name.is_empty() {
                self.shell.scm.run(&root, vec!["switch".into(), "-c".into(), name]);
            }
            return true;
        }
        let Some(what) = kind.strip_prefix("scm_confirm:") else { return false };
        if what == "discard_all" {
            self.shell.scm.run(&root, sv(&["restore", "--worktree", "--", "."]));
        } else if let Some(p) = what.strip_prefix("discard\u{1f}") {
            self.shell.scm.run(&root, vec!["restore".into(), "--worktree".into(), "--".into(), p.to_string()]);
        } else if let Some(p) = what.strip_prefix("trash\u{1f}") {
            let p = PathBuf::from(p);
            self.shell.scm.run_with(&root, move |_| super::explorer_ops::trash(&p).map(|()| format!("Moved {} to the trash", p.display())));
        }
        true
    }

    /// Tooltips for the toolbar (automation nodes).
    pub(super) fn scm_nodes(&self, inner: Rect, mut pn: Node) -> Node {
        for b in self.scm_buttons(inner) {
            let name = match b.action {
                "commit_menu" => "Commit Options".to_string(),
                "branch" | "commit" | "sync" | "publish" => b.label.clone(),
                _ => b.tip.to_string(),
            };
            pn = pn.child(Node::new(format!("scm.{}", b.action), "button", name, b.rect).tip(b.tip));
        }
        pn
    }
}

/// Add a line to the repository's .gitignore (made when it's missing),
/// unless it's there already.
pub(super) fn add_to_gitignore(root: &Path, line: &str) -> Result<(), String> {
    let p = root.join(".gitignore");
    let mut text = std::fs::read_to_string(&p).unwrap_or_default();
    if text.lines().any(|l| l.trim() == line) {
        return Ok(());
    }
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(line);
    text.push('\n');
    std::fs::write(&p, text).map_err(|e| e.to_string())
}
