//! What git knows of each line, as VS Code shows it: marks in the gutter
//! where the text differs from the last commit (added, changed, deleted
//! here; hovering one shows what was there), and after the cursor's line
//! who last changed it, when, and why (the commit's summary), in grey.
//!
//! The committed text and its blame are read in the background once per
//! file (again after a save, and now and then); the buffer is compared
//! with the committed text as it changes.

use super::*;
use std::sync::mpsc::channel;

#[derive(Clone, Debug)]
pub(super) struct Blame {
    pub author: String,
    pub time: i64,
    pub summary: String,
}

/// A run of changed lines: where in the buffer, how many, what it was.
#[derive(Clone, Debug)]
pub(super) struct Hunk {
    pub start: usize,
    pub len: usize,
    pub old: Vec<String>,
}

#[derive(Default)]
pub(super) struct GitFile {
    /// The committed text (None: not in git).
    head: Option<String>,
    /// Each committed line's blame.
    blame: Vec<Option<Blame>>,
    /// The buffer's version compared last, and when.
    compared: Option<u64>,
    /// Each buffer line's committed line (None: new or changed).
    map: Vec<Option<usize>>,
    pub hunks: Vec<Hunk>,
}

#[derive(Default)]
pub(super) struct GitLines {
    pub files: HashMap<PathBuf, GitFile>,
    asked: HashMap<PathBuf, Instant>,
    rx: Option<Receiver<(PathBuf, Option<String>, Vec<Option<Blame>>)>>,
    tx: Option<Sender<(PathBuf, Option<String>, Vec<Option<Blame>>)>>,
}

/// `git show HEAD:file` and `git blame --porcelain HEAD -- file`.
fn read_git(path: &Path) -> (Option<String>, Vec<Option<Blame>>) {
    let Some(dir) = path.parent() else { return (None, Vec::new()) };
    let run = |args: &[&str]| std::process::Command::new("git").arg("-C").arg(dir).args(args).stdin(std::process::Stdio::null()).output().ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).into_owned());
    let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else { return (None, Vec::new()) };
    let Some(head) = run(&["show", &format!("HEAD:./{name}")]) else { return (None, Vec::new()) };
    let mut blame: Vec<Option<Blame>> = vec![None; head.lines().count()];
    if let Some(out) = run(&["blame", "--porcelain", "HEAD", "--", &name]) {
        let mut commits: HashMap<String, Blame> = HashMap::new();
        let mut cur: Option<(String, usize)> = None;
        for line in out.lines() {
            if let Some(text) = line.strip_prefix('\t') {
                let _ = text;
                if let Some((h, n)) = &cur {
                    if let (Some(b), Some(slot)) = (commits.get(h), blame.get_mut(n.saturating_sub(1))) {
                        *slot = Some(b.clone());
                    }
                }
                continue;
            }
            let mut parts = line.split(' ');
            let first = parts.next().unwrap_or("");
            if first.len() == 40 && first.chars().all(|c| c.is_ascii_hexdigit()) {
                let fin = parts.nth(1).and_then(|n| n.parse::<usize>().ok()).unwrap_or(0);
                commits.entry(first.to_string()).or_insert(Blame { author: String::new(), time: 0, summary: String::new() });
                cur = Some((first.to_string(), fin));
                continue;
            }
            if let Some((h, _)) = &cur {
                let b = commits.get_mut(h).unwrap();
                if let Some(v) = line.strip_prefix("author ") {
                    b.author = v.to_string();
                } else if let Some(v) = line.strip_prefix("author-time ") {
                    b.time = v.parse().unwrap_or(0);
                } else if let Some(v) = line.strip_prefix("summary ") {
                    b.summary = v.to_string();
                }
            }
        }
    }
    (Some(head), blame)
}

/// "3 days ago".
fn ago(t: i64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let s = (now - t).max(0);
    let (n, unit) = match s {
        0..60 => return "just now".into(),
        60..3600 => (s / 60, "minute"),
        3600..86_400 => (s / 3600, "hour"),
        86_400..2_592_000 => (s / 86_400, "day"),
        2_592_000..31_536_000 => (s / 2_592_000, "month"),
        _ => (s / 31_536_000, "year"),
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

/// Compare the committed text with the buffer: each buffer line's mark (1
/// added, 2 changed, 3 lines deleted just after it), its committed line,
/// and the changed runs.
pub(super) fn compare(head: &str, now: &str) -> (Vec<u8>, Vec<Option<usize>>, Vec<Hunk>) {
    use similar::{DiffOp, TextDiff};
    let n_lines = now.split('\n').count();
    let mut marks = vec![0u8; n_lines];
    let mut map = vec![None; n_lines];
    let mut hunks = Vec::new();
    let old: Vec<&str> = head.split('\n').collect();
    let diff = TextDiff::from_lines(head, now);
    for op in diff.ops() {
        match *op {
            DiffOp::Equal { old_index, new_index, len } => {
                for k in 0..len {
                    if let Some(m) = map.get_mut(new_index + k) {
                        *m = Some(old_index + k);
                    }
                }
            }
            DiffOp::Insert { new_index, new_len, .. } => {
                for k in 0..new_len {
                    if let Some(m) = marks.get_mut(new_index + k) {
                        *m = 1;
                    }
                }
                hunks.push(Hunk { start: new_index, len: new_len, old: Vec::new() });
            }
            DiffOp::Delete { old_index, old_len, new_index } => {
                let at = new_index.saturating_sub(1).min(n_lines.saturating_sub(1));
                if marks[at] == 0 {
                    marks[at] = 3;
                }
                hunks.push(Hunk { start: at, len: 1, old: old[old_index..(old_index + old_len).min(old.len())].iter().map(|s| s.to_string()).collect() });
            }
            DiffOp::Replace { old_index, old_len, new_index, new_len } => {
                for k in 0..new_len {
                    if let Some(m) = marks.get_mut(new_index + k) {
                        *m = 2;
                    }
                }
                hunks.push(Hunk { start: new_index, len: new_len, old: old[old_index..(old_index + old_len).min(old.len())].iter().map(|s| s.to_string()).collect() });
            }
        }
    }
    (marks, map, hunks)
}

impl Workbench {
    /// Each tick: read files' git in the background; compare the front
    /// editor with its committed text; its cursor line's blame.
    pub(super) fn gitlines_tick(&mut self) {
        let front = self.editor_index();
        let gl = &mut self.gitlines;
        if gl.tx.is_none() {
            let (tx, rx) = channel();
            gl.tx = Some(tx);
            gl.rx = Some(rx);
        }
        while let Some(Ok((p, head, blame))) = gl.rx.as_ref().map(|r| r.try_recv()) {
            let f = gl.files.entry(p).or_default();
            f.head = head;
            f.blame = blame;
            f.compared = None;
        }
        let Some(i) = front else { return };
        let w = &mut self.windows[i];
        let Some(path) = w.ed.doc.path.clone() else { return };
        // (Asked again after a save, and every half minute: commits come.)
        let saved_now = w.was_dirty && !w.ed.doc.dirty;
        let stale = gl.asked.get(&path).is_none_or(|t| t.elapsed() > Duration::from_secs(30)) || saved_now;
        if stale {
            gl.asked.insert(path.clone(), Instant::now());
            let tx = gl.tx.clone().unwrap();
            let p = path.clone();
            std::thread::spawn(move || {
                let (head, blame) = read_git(&p);
                let _ = tx.send((p, head, blame));
            });
        }
        let Some(f) = gl.files.get_mut(&path) else { return };
        let Some(head) = &f.head else {
            w.ed.git_marks.clear();
            w.ed.blame = None;
            return;
        };
        let v = w.ed.doc.version;
        if f.compared != Some(v) {
            let (marks, map, hunks) = compare(head, &w.ed.doc.rope.to_string());
            w.ed.git_marks = marks;
            f.map = map;
            f.hunks = hunks;
            f.compared = Some(v);
        }
        // The cursor line's blame, after the line in grey.
        let line = w.ed.cursor_line_col().0;
        w.ed.blame = Some((line, match f.map.get(line).copied().flatten() {
            Some(old) => match f.blame.get(old).cloned().flatten() {
                Some(b) => format!("{}, {} • {}", b.author, ago(b.time), b.summary),
                None => String::new(),
            },
            None => "You, not committed yet".into(),
        }));
    }

    /// The tooltip for a change mark in the gutter: what the lines were.
    pub(super) fn git_mark_tip(&self, p: Point) -> Option<(Rect, String)> {
        let w = self.windows.last().filter(|w| w.term.is_none() && w.dock.is_none())?;
        let a = w.ed.area();
        if p.y < a.y || p.y >= a.bottom() || p.x != a.x || w.ed.gutter_width() == 0 {
            return None;
        }
        let ch = w.ed.char_at(Point::new(a.x + w.ed.gutter_width(), p.y))?;
        let line = w.ed.doc.rope.char_to_line(ch);
        let mark = *w.ed.git_marks.get(line)?;
        if mark == 0 {
            return None;
        }
        let f = self.gitlines.files.get(w.ed.doc.path.as_ref()?)?;
        let h = f.hunks.iter().find(|h| line >= h.start && line < h.start + h.len.max(1))?;
        let mut out = vec![match mark {
            1 => format!("Added: {} line{}", h.len, if h.len == 1 { "" } else { "s" }),
            2 => "Changed: it was".into(),
            _ => format!("Deleted after this line: {} line{}", h.old.len(), if h.old.len() == 1 { "" } else { "s" }),
        }];
        for l in h.old.iter().take(20) {
            out.push(format!("- {l}"));
        }
        if mark != 3 {
            for k in h.start..(h.start + h.len).min(w.ed.doc.len_lines()).min(h.start + 20) {
                out.push(format!("+ {}", w.ed.doc.line(k)));
            }
        }
        Some((Rect::new(p.x, p.y, 1, 1), out.join("\n")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_added_changed_deleted() {
        let head = "a\nb\nc\nd\n";
        let now = "a\nB\nc\nnew\n";
        let (marks, map, hunks) = compare(head, now);
        assert_eq!(&marks[..4], &[0, 2, 0, 2]);
        assert_eq!(map[0], Some(0));
        assert_eq!(map[2], Some(2));
        assert!(hunks.iter().any(|h| h.old == vec!["b".to_string()]));
        let (marks, _, _) = compare("a\nb\nc\n", "a\nc\n");
        assert_eq!(marks[0], 3, "deleted after line 0");
        let (marks, _, _) = compare("a\n", "a\nx\n");
        assert_eq!(marks[1], 1);
        assert_eq!(ago(0).ends_with("years ago"), true);
    }
}
