//! The File Open / Save As dialog.
//!
//! ```text
//! ┌──────────────────────────── Open ────────────────────────────┐
//! │ File name:                                                    │
//! │ [main.rs                                        ]   < Open >  │
//! │ /home/ben/src/turbine/crates                       < Cancel > │
//! │ ┌Places──────┐┌Files──────────────────────────────┐           │
//! │ │Home        ││ Name                 Size  Modified│  Encoding │
//! │ │Documents   ││ ../                               │  [UTF-8 ↓]│
//! │ │Projects    ││ src/                  DIR  …       │           │
//! │ │This project││ Cargo.toml          1 KB  …        │           │
//! │ └────────────┘└────────────────────────────────────┘          │
//! │ Files of type: [All files (*) ↓]   [ ] Hidden files           │
//! └───────────────────────────────────────────────────────────────┘
//! ```
//!
//! Resizable like every dialog (the lists stretch). Typing a name filters
//! the files (Open); Enter or a double-click goes into a folder or picks a
//! file; Backspace in an empty name goes up; Places jump to common folders
//! and recent ones. Save As also chooses the encoding and line endings.

use std::path::{Path, PathBuf};
use std::time::SystemTime;
use turbine_text::encoding::{LineEnding, TextEncoding};
use turbine_ui::dialog::{self as ui, Dialog};

pub struct Entry {
    pub name: String,
    pub dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

pub const TYPES: &[(&str, &[&str])] = &[
    ("All files (*)", &[]),
    ("Source code", &[
        "rs", "py", "c", "h", "cc", "cpp", "hpp", "js", "ts", "tsx", "go", "java", "rb", "lua", "sh", "zig", "nix", "bas", "pas",
        "cs", "kt", "swift", "php", "ml", "hs", "ex", "qml",
    ]),
    ("Text (*.txt *.md *.rst)", &["txt", "md", "rst", "text", "log"]),
    ("Config (*.toml *.json *.yaml *.ini)", &["toml", "json", "yaml", "yml", "ini", "cfg", "conf"]),
    ("BASIC (*.bas *.bi *.bm)", &["bas", "bi", "bm"]),
    ("Rust (*.rs)", &["rs"]),
    ("Python (*.py)", &["py", "pyi"]),
];

/// The encodings to choose from: (shown, label, BOM). The Unicode ones
/// and the two common DOS pages first, then every other.
pub fn encodings() -> Vec<(String, String, bool)> {
    let mut v: Vec<(String, String, bool)> = [
        ("UTF-8", "utf-8", false),
        ("UTF-8 with BOM", "utf-8", true),
        ("UTF-16 LE", "utf-16le", true),
        ("UTF-16 BE", "utf-16be", true),
        ("CP437 (DOS US)", "cp437", false),
        ("CP850 (DOS Western)", "cp850", false),
    ]
    .iter()
    .map(|(a, b, c)| (a.to_string(), b.to_string(), *c))
    .collect();
    for (label, name) in turbine_text::encoding::all_encodings() {
        if !v.iter().any(|(_, l, _)| *l == label) {
            v.push((name, label, false));
        }
    }
    v
}

pub const LINE_ENDINGS: &[(&str, LineEnding)] =
    &[("LF (Unix)", LineEnding::Lf), ("CRLF (DOS)", LineEnding::CrLf), ("CR (classic Mac)", LineEnding::Cr)];

/// The file dialog's state beside its `Dialog` (kind "files").
pub struct FileDialog {
    pub save: bool,
    /// Open Folder: only folders listed; OK picks the folder shown.
    pub folder: bool,
    /// Add Folder to Workspace (a folder picked to go beside the project).
    pub add_to_workspace: bool,
    pub dir: PathBuf,
    pub entries: Vec<Entry>,
    /// Indexes into `entries` of the rows shown (the first may be "..").
    pub shown: Vec<Option<usize>>,
    pub places: Vec<(String, PathBuf)>,
    /// The files list's width the rows were laid out for.
    width: i32,
}

fn human_size(n: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < 4 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{n} B") } else if v < 10.0 { format!("{v:.1} {}", U[i]) } else { format!("{v:.0} {}", U[i]) }
}

/// "2026-11-26 17:05" in UTC (no time zone data needed).
fn date(t: SystemTime) -> String {
    let secs = t.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", rem / 3600, rem % 3600 / 60)
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

impl FileDialog {
    /// The dialog and its state. `name`: the file name to start with (Save
    /// As); `encoding`/`line_ending`: the document's.
    pub fn new(
        save: bool,
        dir: PathBuf,
        name: &str,
        project: Option<PathBuf>,
        recent: &[PathBuf],
        encoding: Option<(TextEncoding, bool)>,
        line_ending: LineEnding,
    ) -> (FileDialog, Dialog) {
        let mut places: Vec<(String, PathBuf)> = Vec::new();
        if let Some(h) = home() {
            places.push(("Home".into(), h.clone()));
            for (label, sub) in [("Documents", "Documents"), ("Downloads", "Downloads"), ("Desktop", "Desktop"), ("Projects", "src")] {
                if h.join(sub).is_dir() {
                    places.push((label.into(), h.join(sub)));
                }
            }
        }
        if let Some(p) = project.filter(|p| p.is_dir()) {
            places.push(("This project".into(), p));
        }
        places.push(("/ (Root)".into(), PathBuf::from("/")));
        for r in recent.iter().take(6) {
            if r.is_dir() && !places.iter().any(|(_, p)| p == r) {
                let name = r.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| r.display().to_string());
                places.push((format!("↻ {name}"), r.clone()));
            }
        }

        let title = if save { "Save As" } else { "Open" };
        let places_list = ui::list("places", places.iter().map(|(l, _)| l.clone()).collect(), 0).grow_x(false).lines(12);
        let files_list = ui::list("files", Vec::new(), 0).activates().header("").chars(62).lines(12);
        let mut rows = vec![
            ui::input("fname", "File ~n~ame:", name).chars(40),
            ui::label(&dir.display().to_string()).id("path").chars(40),
            ui::row(vec![ui::group("~P~laces", places_list), ui::group("~F~iles", files_list)]).grow_y(true),
            ui::row(vec![
                ui::combo("type", "Files of ~t~ype:", TYPES.iter().map(|(l, _)| l.to_string()).collect(), 0),
                ui::check("hidden", "~H~idden files", false),
            ]),
        ];
        if save {
            let enc = encoding
                .and_then(|(e, bom)| {
                    let is = |label: &str| TextEncoding::for_label(label) == Some(e);
                    let all = encodings();
                    all.iter().position(|(_, l, b)| is(l) && *b == bom).or_else(|| all.iter().position(|(_, l, _)| is(l)))
                })
                .unwrap_or(0);
            let le = LINE_ENDINGS.iter().position(|(_, l)| *l == line_ending).unwrap_or(0);
            rows.push(ui::row(vec![
                ui::combo("encoding", "~E~ncoding:", encodings().into_iter().map(|(l, _, _)| l).collect(), enc),
                ui::combo("eol", "~L~ine endings:", LINE_ENDINGS.iter().map(|(l, _)| l.to_string()).collect(), le),
            ]));
        }
        let mut d = Dialog::new("filedlg", title)
            .body(ui::col(rows))
            .buttons(vec![ui::button("ok", if save { "~S~ave" } else { "~O~pen" }, true), ui::button("cancel", "Cancel", false)]);
        d.focus_id("fname");
        let mut f = FileDialog { save, folder: false, add_to_workspace: false, dir, entries: Vec::new(), shown: Vec::new(), places, width: 62 };
        f.read_dir();
        f.refresh(&mut d);
        (f, d)
    }

    fn read_dir(&mut self) {
        let mut entries: Vec<Entry> = std::fs::read_dir(&self.dir)
            .map(|rd| {
                rd.flatten()
                    .map(|e| {
                        let md = e.metadata().ok();
                        // (A link to a folder counts as a folder.)
                        let dir = e.path().is_dir();
                        Entry {
                            name: e.file_name().to_string_lossy().into_owned(),
                            dir,
                            size: md.as_ref().map(|m| m.len()).unwrap_or(0),
                            modified: md.and_then(|m| m.modified().ok()),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        entries.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        self.entries = entries;
    }

    /// Rebuild the files list from the folder, filter, hidden files and (to
    /// open) what's typed.
    pub fn refresh(&mut self, d: &mut Dialog) {
        let hidden = d.checked("hidden");
        let types = TYPES.get(d.selected_index("type").unwrap_or(0)).map(|(_, e)| *e).unwrap_or(&[]);
        let typed = d.value("fname").unwrap_or_default();
        let filter = if self.save || typed.contains('/') { String::new() } else { typed.to_lowercase() };
        let mut shown: Vec<Option<usize>> = Vec::new();
        if self.dir.parent().is_some() {
            shown.push(None);
        }
        for (i, e) in self.entries.iter().enumerate() {
            if !hidden && e.name.starts_with('.') {
                continue;
            }
            if self.folder && !e.dir {
                continue;
            }
            if !e.dir && !types.is_empty() {
                let ext = Path::new(&e.name).extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
                if !types.contains(&ext.as_str()) {
                    continue;
                }
            }
            if !filter.is_empty() && !e.name.to_lowercase().contains(&filter) {
                continue;
            }
            shown.push(Some(i));
        }
        // Columns that fit: name, size, modified (dropped when narrow).
        let text_w = (self.width - 2).max(8) as usize;
        let show_date = text_w >= 14 + 1 + 9 + 2 + 16;
        let show_size = text_w >= 14 + 1 + 9;
        let name_w = text_w - if show_date { 1 + 9 + 2 + 16 } else if show_size { 1 + 9 } else { 0 };
        let rows: Vec<String> = shown
            .iter()
            .map(|s| match s {
                None => "../".to_string(),
                Some(i) => {
                    let e = &self.entries[*i];
                    let mut name = if e.dir { format!("{}/", e.name) } else { e.name.clone() };
                    if turbine_text::unicode::str_width(&name) > name_w {
                        while turbine_text::unicode::str_width(&name) > name_w - 1 {
                            name.pop();
                        }
                        name.push('…');
                    }
                    let pad = name_w.saturating_sub(turbine_text::unicode::str_width(&name));
                    let size = if e.dir { "<DIR>".to_string() } else { human_size(e.size) };
                    let pad = " ".repeat(pad);
                    match (show_size, show_date) {
                        (_, true) => format!("{name}{pad} {size:>9}  {}", e.modified.map(date).unwrap_or_default()),
                        (true, false) => format!("{name}{pad} {size:>9}"),
                        _ => name,
                    }
                }
            })
            .collect();
        // (With a name typed, its first match is selected; else the first file.)
        let sel = if filter.is_empty() { 0 } else { shown.iter().position(|s| s.is_some()).unwrap_or(0) };
        d.set_items("files", rows, sel);
        if let Some(turbine_ui::dialog::Placed { control: turbine_ui::dialog::Control::List { header, .. }, .. }) = d.controls.iter_mut().find(|p| p.control.id() == Some("files")) {
            *header = Some(match (show_size, show_date) {
                (_, true) => format!("{:<name_w$} {:>9}  Modified", "Name", "Size"),
                (true, false) => format!("{:<name_w$} {:>9}", "Name", "Size"),
                _ => "Name".to_string(),
            });
        }
        d.set_label("path", &self.dir.display().to_string());
        self.shown = shown;
    }

    fn list_width(&self, d: &Dialog) -> i32 {
        d.controls.iter().position(|p| p.control.id() == Some("files")).map(|i| d.content_rect(i).w).unwrap_or(36)
    }

    /// After a layout: lay the rows out again if the list changed width.
    pub fn fit(&mut self, d: &mut Dialog) {
        let w = self.list_width(d);
        if w != self.width {
            self.width = w;
            let sel = d.selected_index("files").unwrap_or(0);
            self.refresh(d);
            if let Some(turbine_ui::dialog::Placed { control: turbine_ui::dialog::Control::List { selected, .. }, .. }) =
                d.controls.iter_mut().find(|p| p.control.id() == Some("files"))
            {
                *selected = sel.min(self.shown.len().saturating_sub(1));
            }
        }
    }

    pub fn go(&mut self, dir: PathBuf, d: &mut Dialog) {
        self.dir = std::fs::canonicalize(&dir).unwrap_or(dir);
        self.read_dir();
        if !self.save {
            d.set_value("fname", "");
        }
        self.refresh(d);
    }

    pub fn up(&mut self, d: &mut Dialog) {
        if let Some(p) = self.dir.parent().map(Path::to_path_buf) {
            self.go(p, d);
        }
    }

    /// The selected row: None for "..", Some((path, is_dir)) otherwise.
    pub fn selected(&self, d: &Dialog) -> Option<Option<(PathBuf, bool)>> {
        let i = d.selected_index("files")?;
        let s = self.shown.get(i)?;
        Some(s.map(|j| (self.dir.join(&self.entries[j].name), self.entries[j].dir)))
    }

    /// What OK should do: a folder to enter, or a file chosen.
    /// Open Folder: the same dialog listing only folders.
    pub fn folder(dir: PathBuf, project: Option<PathBuf>, recent: &[PathBuf]) -> (FileDialog, Dialog) {
        let (mut f, mut d) = FileDialog::new(false, dir, "", project, recent, None, LineEnding::Lf);
        f.folder = true;
        d.title = "Open Folder".into();
        d.set_caption("ok", "~S~elect Folder");
        d.set_caption("fname", "~F~older:");
        f.refresh(&mut d);
        (f, d)
    }

    pub fn target(&self, d: &Dialog) -> Option<PathBuf> {
        let typed = d.value("fname").unwrap_or_default();
        let typed = typed.trim();
        if self.folder {
            // The folder shown, or one typed.
            return Some(if typed.is_empty() {
                self.dir.clone()
            } else if let Some(rest) = typed.strip_prefix("~/") {
                home().map(|h| h.join(rest)).unwrap_or_else(|| PathBuf::from(typed))
            } else if Path::new(typed).is_absolute() {
                PathBuf::from(typed)
            } else {
                self.dir.join(typed)
            });
        }
        if typed.is_empty() {
            return match self.selected(d)? {
                None => self.dir.parent().map(Path::to_path_buf),
                Some((p, _)) => Some(p),
            };
        }
        let p = if let Some(rest) = typed.strip_prefix("~/") {
            home().map(|h| h.join(rest)).unwrap_or_else(|| PathBuf::from(typed))
        } else if Path::new(typed).is_absolute() {
            PathBuf::from(typed)
        } else {
            self.dir.join(typed)
        };
        // (To open with a name typed that only filters: the match selected.)
        if !self.save && !p.exists() {
            if let Some(Some((sel, _))) = self.selected(d) {
                return Some(sel);
            }
        }
        Some(p)
    }

    /// The encoding and line endings chosen (Save As).
    pub fn format(&self, d: &Dialog) -> Option<(TextEncoding, bool, LineEnding)> {
        let (_, label, bom) = encodings().get(d.selected_index("encoding")?)?.clone();
        let enc = TextEncoding::for_label(&label).unwrap_or(TextEncoding::Utf8);
        let (_, le) = LINE_ENDINGS.get(d.selected_index("eol")?)?;
        Some((enc, bom, *le))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use turbine_text::encoding::CodePage;

    #[test]
    fn lists_filters_and_targets() {
        let dir = std::env::temp_dir().join(format!("turbine-fd-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("main.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.join("notes.txt"), "hi").unwrap();
        std::fs::write(dir.join(".hidden"), "").unwrap();
        let (mut f, mut d) = FileDialog::new(false, dir.clone(), "", None, &[], None, LineEnding::Lf);
        d.layout(turbine_ui::Rect::new(0, 0, 120, 40), &turbine_ui::Theme::default());
        f.fit(&mut d);
        let names = |d: &Dialog| d.automation_node().find_target("dialog.files").unwrap().children.iter().map(|c| c.name.clone()).collect::<Vec<_>>();
        let n = names(&d);
        assert!(n[0] == "../" && n[1].starts_with("sub/") && n.iter().any(|x| x.starts_with("main.rs")), "{n:?}");
        assert!(!n.iter().any(|x| x.starts_with(".hidden")));
        // Typing filters (Open); OK opens the match.
        d.set_value("fname", "mai");
        f.refresh(&mut d);
        assert_eq!(names(&d).len(), 2, "{:?}", names(&d));
        assert_eq!(f.target(&d), Some(dir.join("main.rs")));
        // Hidden files on.
        d.set_value("fname", "");
        for p in &mut d.controls {
            if let turbine_ui::dialog::Control::Check { checked, .. } = &mut p.control {
                *checked = true;
            }
        }
        f.refresh(&mut d);
        assert!(names(&d).iter().any(|x| x.starts_with(".hidden")));
        // Into a folder and back up.
        f.go(dir.join("sub"), &mut d);
        assert!(f.dir.ends_with("sub"));
        f.up(&mut d);
        assert_eq!(f.dir, std::fs::canonicalize(&dir).unwrap());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn save_as_chooses_encoding_and_line_endings() {
        let (f, mut d) = FileDialog::new(true, std::env::temp_dir(), "x.bas", None, &[], Some((TextEncoding::Utf8, false)), LineEnding::Lf);
        assert_eq!(f.format(&d), Some((TextEncoding::Utf8, false, LineEnding::Lf)));
        for p in &mut d.controls {
            if let turbine_ui::dialog::Control::Combo { id, selected, .. } = &mut p.control {
                *selected = if id == "encoding" { 4 } else { 1 };
            }
        }
        assert_eq!(f.format(&d), Some((TextEncoding::Dos(CodePage::Cp437), false, LineEnding::CrLf)));
        assert_eq!(d.value("fname").as_deref(), Some("x.bas"));
    }
}
