//! Compile, Make and tasks, as Turbo C's Compile menu and Cursor's Run
//! Build Task: the output goes to the Output window, the compiler's
//! messages to Problems (and as marks into editors no language server
//! marks). Files of languages with no language server are checked when
//! opened and saved. Run Without Debugging runs a file in a terminal.

use super::*;
use turbine_services::build::{self, Problem, ProblemParser, RunEvent, Running, Task};

pub(super) struct Job {
    running: Running,
    parser: ProblemParser,
    label: String,
    /// A check of one file (quiet: no Output window): its problems replace
    /// only that file's.
    file: Option<PathBuf>,
    problems: Vec<Problem>,
}

#[derive(Default)]
pub(super) struct BuildState {
    task: Option<Job>,
    checks: Vec<Job>,
    /// Problems the tools reported, by file (canonical).
    pub problems: BTreeMap<PathBuf, Vec<Problem>>,
    /// Each editor's dirty flag last tick (a save is dirty → clean).
    seen: HashMap<u32, bool>,
    /// What each editor's marks were last set from.
    marked: HashMap<u32, usize>,
    generation: usize,
}

impl Workbench {
    fn project_root(&self) -> PathBuf {
        self.windows
            .iter()
            .rev()
            .find(|w| w.term.is_none() && w.special.is_none() && w.ed.doc.path.is_some())
            .and_then(|w| w.ed.doc.path.as_ref())
            .map(|p| lsp::project_root(p))
            .unwrap_or_else(|| self.shell.explorer.root.clone())
    }

    /// Start a task, its output in the Output window.
    pub(super) fn run_task(&mut self, t: &Task) {
        if let Some(j) = self.build.task.take() {
            j.running.kill();
        }
        let i = self.special_window("output");
        {
            let ed = &mut self.windows[i].ed;
            ed.doc = Document::from_str(&format!("> {}  ({})\n", t.command, t.cwd.display()));
            ed.doc.dirty = false;
        }
        self.layout_dock();
        let id = self.windows[i].win.id;
        self.shell.dock.show(crate::dock::Tab::Pane(id));
        self.say(format!("Running {}...", t.label));
        self.build.task = Some(Job { running: build::spawn(&t.command, &t.cwd), parser: ProblemParser::new(&t.cwd), label: t.label.clone(), file: None, problems: Vec::new() });
    }

    pub(super) fn build_command(&mut self, cmd: &str) {
        let root = self.project_root();
        let tasks = build::tasks(&root);
        match cmd {
            "build.make" | "terminal.build" => match build::default_build(&tasks) {
                Some(t) => {
                    let t = t.clone();
                    self.save_all_quietly();
                    self.run_task(&t);
                }
                None => self.say("No build task here: Terminal > Run Task, or add .turbine/tasks.json"),
            },
            "build.all" => match tasks.iter().find(|t| t.group == "build-all").or_else(|| build::default_build(&tasks)) {
                Some(t) => {
                    let t = t.clone();
                    self.save_all_quietly();
                    self.run_task(&t);
                }
                None => self.say("No build task here"),
            },
            "build.compile" => {
                let Some(w) = self.windows.iter().rev().find(|w| w.term.is_none() && w.special.is_none()) else { return };
                let (Some(path), Some(lang)) = (w.ed.doc.path.clone(), w.lang.clone()) else {
                    self.say("Save the file first to compile it");
                    return;
                };
                let root = lsp::project_root(&path);
                match build::check_command(&lang, &path, &root) {
                    Some(c) => {
                        self.save_all_quietly();
                        let t = Task { label: format!("Compile {}", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()), command: c, cwd: root, group: "build".into(), default: false };
                        self.run_task(&t);
                    }
                    None => self.say(format!("Nothing installed checks {lang} files")),
                }
            }
            "build.stop" => match self.build.task.take() {
                Some(j) => {
                    j.running.kill();
                    self.say(format!("Stopped {}", j.label));
                }
                None => self.say("Nothing is building"),
            },
            "terminal.task" => {
                if tasks.is_empty() {
                    self.say("No tasks found: add .turbine/tasks.json (or .vscode/tasks.json)");
                    return;
                }
                let items: Vec<String> = tasks.iter().map(|t| if t.group.is_empty() { t.label.clone() } else { format!("{}  [{}]", t.label, t.group) }).collect();
                let d = Dialog::new(&format!("task:{}", root.display()), "Run Task")
                    .body(ui::list("task", items, 0).lines(12).chars(50))
                    .buttons(vec![ui::button("ok", "~R~un", true), ui::button("cancel", "Cancel", false)]);
                self.dialog = Some(d);
            }
            "run.without_debug" => {
                let Some(w) = self.windows.iter().rev().find(|w| w.term.is_none() && w.special.is_none()) else { return };
                let (Some(path), Some(lang)) = (w.ed.doc.path.clone(), w.lang.clone()) else {
                    self.say("Save the file first to run it");
                    return;
                };
                let root = lsp::project_root(&path);
                let run = tasks.iter().find(|t| t.group == "run").map(|t| t.command.clone()).or_else(|| build::run_command(&lang, &path, &root));
                match run {
                    Some(c) => {
                        self.save_all_quietly();
                        self.new_terminal(Vec::new());
                        if let Some(t) = self.windows.last_mut().and_then(|w| w.term.as_mut()) {
                            t.paste(&format!("cd {} && {c}\n", build::sh_quote(&root.display().to_string())));
                        }
                        self.layout_dock();
                        if let Some(id) = self.windows.last().map(|w| w.win.id) {
                            self.shell.dock.show(crate::dock::Tab::Pane(id));
                        }
                    }
                    None => self.say(format!("Turbine doesn't know how to run {lang} files")),
                }
            }
            _ => {}
        }
    }

    /// The Run Task dialog's choice.
    pub(super) fn task_chosen(&mut self, d: &Dialog, root: &str) {
        let tasks = build::tasks(Path::new(root));
        if let Some(t) = d.selected_index("task").and_then(|i| tasks.get(i)) {
            let t = t.clone();
            self.save_all_quietly();
            self.run_task(&t);
        }
    }

    /// Save every changed file that has a name (before building).
    fn save_all_quietly(&mut self) {
        for w in &mut self.windows {
            if w.term.is_none() && w.special.is_none() && w.ed.doc.dirty && w.ed.doc.path.is_some() {
                let _ = w.ed.doc.save();
            }
        }
    }

    fn check_file(&mut self, path: &Path, lang: &str) {
        let root = lsp::project_root(path);
        let Some(cmd) = build::check_command(lang, path, &root) else { return };
        let canon = Self::canon(path);
        self.build.checks.retain(|j| {
            let same = j.file.as_ref() == Some(&canon);
            if same {
                j.running.kill();
            }
            !same
        });
        self.build.checks.push(Job { running: build::spawn(&cmd, &root), parser: ProblemParser::new(&root), label: cmd, file: Some(canon), problems: Vec::new() });
    }

    /// Each tick: output and problems from what's running; checks of files
    /// no language server looks at, when opened or saved.
    pub(super) fn build_tick(&mut self) {
        let mut want = Vec::new();
        for w in &self.windows {
            if w.term.is_some() || w.special.is_some() {
                continue;
            }
            let (Some(p), Some(lang)) = (&w.ed.doc.path, &w.lang) else { continue };
            let dirty = w.ed.doc.dirty;
            let before = self.build.seen.insert(w.win.id, dirty);
            let no_server = w.lsp_key.is_none() && (self.lsp_missing.contains(lang) || lsp::server_names(lang).is_empty());
            if no_server && !dirty && before != Some(false) {
                want.push((p.clone(), lang.clone()));
            }
        }
        for (p, l) in want {
            self.check_file(&p, &l);
        }
        let mut changed = false;
        // The task.
        let mut out = String::new();
        let mut finished = None;
        if let Some(j) = &mut self.build.task {
            for e in j.running.rx.try_iter() {
                match e {
                    RunEvent::Line(l) => {
                        if let Some(p) = j.parser.line(&l) {
                            j.problems.push(p);
                        }
                        out.push_str(&l);
                        out.push('\n');
                    }
                    RunEvent::Done(code, why) => {
                        finished = Some((code, why));
                        break;
                    }
                }
            }
        }
        if !out.is_empty() {
            self.append_output(&out);
        }
        if let Some((code, why)) = finished {
            let j = self.build.task.take().unwrap();
            let (errors, warnings) = (j.problems.iter().filter(|p| p.severity == 1).count(), j.problems.iter().filter(|p| p.severity == 2).count());
            // A build's problems replace the files it reported on, and any
            // it reported on before.
            self.build.problems.retain(|_, v| v.iter().all(|p| p.severity > 2));
            for p in j.problems {
                self.build.problems.entry(p.path.clone()).or_default().push(p);
            }
            changed = true;
            let how = match code {
                Some(0) => "succeeded".to_string(),
                Some(c) => format!("failed (exit {c})"),
                None if !why.is_empty() => format!("didn't start: {why}"),
                None => "stopped".to_string(),
            };
            let tail = if errors + warnings > 0 { format!(": {errors} errors, {warnings} warnings") } else { String::new() };
            self.append_output(&format!("\n{} {how}{tail}\n", j.label));
            self.say(format!("{} {how}{tail}", j.label));
            if errors > 0 {
                self.show_tab(crate::dock::Tab::Problems);
                self.shell.focus = shell::Focus::Editors;
            }
        }
        // Quiet checks.
        let mut done = Vec::new();
        for (i, j) in self.build.checks.iter_mut().enumerate() {
            for e in j.running.rx.try_iter() {
                match e {
                    RunEvent::Line(l) => {
                        if let Some(p) = j.parser.line(&l) {
                            j.problems.push(p);
                        }
                    }
                    RunEvent::Done(..) => {
                        done.push(i);
                        break;
                    }
                }
            }
        }
        for i in done.into_iter().rev() {
            let j = self.build.checks.remove(i);
            let f = j.file.clone().unwrap_or_default();
            let mine: Vec<Problem> = j.problems.into_iter().filter(|p| p.path == f).collect();
            if mine.is_empty() {
                self.build.problems.remove(&f);
            } else {
                self.build.problems.insert(f, mine);
            }
            changed = true;
        }
        if changed {
            self.build.generation += 1;
        }
        // Marks in editors no language server marks.
        let generation = self.build.generation;
        for w in &mut self.windows {
            if w.term.is_some() || w.special.is_some() || w.lsp_key.is_some() {
                continue;
            }
            let Some(p) = &w.ed.doc.path else { continue };
            if self.build.marked.get(&w.win.id) == Some(&generation) {
                continue;
            }
            self.build.marked.insert(w.win.id, generation);
            let probs = self.build.problems.get(&Self::canon(p));
            w.ed.diagnostics = probs
                .into_iter()
                .flatten()
                .map(|p| {
                    let line = p.line.min(w.ed.doc.len_lines().saturating_sub(1));
                    let text = w.ed.doc.line(line);
                    let start = text.char_indices().nth(p.col).map(|(b, _)| b).unwrap_or(text.len().min(text.len() - text.trim_start().len()));
                    let end = text[start..].find(|c: char| !(c.is_alphanumeric() || c == '_')).map(|e| start + e.max(1)).unwrap_or(text.len()).max(start);
                    DiagMark { line, start, end, severity: p.severity, message: p.message.clone() }
                })
                .collect();
        }
    }

    /// Problems in files that aren't open (for the Problems view).
    pub(super) fn closed_file_problems(&self) -> Vec<(PathBuf, Vec<Problem>)> {
        self.build
            .problems
            .iter()
            .filter(|(p, _)| !self.windows.iter().any(|w| w.term.is_none() && w.ed.doc.path.as_ref().is_some_and(|q| Self::canon(q) == **p)))
            .map(|(p, v)| (p.clone(), v.clone()))
            .collect()
    }

    pub(super) fn build_busy(&self) -> bool {
        self.build.task.is_some() || !self.build.checks.is_empty()
    }
}
