//! The smaller commands: Open Recent, Back and Forward (where you were),
//! Show Clipboard, a calculator (Turbo Pascal's had one), New Window,
//! Split Terminal, and the Options menu's Editor and Directories pages.

use super::*;

#[derive(Default)]
pub(super) struct Extras {
    /// Files opened, newest first.
    pub recent: Vec<PathBuf>,
    /// Places jumped from (Back) and back from (Forward): (file, line).
    back: Vec<(PathBuf, usize)>,
    forward: Vec<(PathBuf, usize)>,
}

impl Workbench {
    pub(super) fn note_recent(&mut self, p: &Path) {
        let p = Self::canon(p);
        self.extras.recent.retain(|x| *x != p);
        self.extras.recent.insert(0, p);
        self.extras.recent.truncate(30);
    }

    /// Before a jump: where the cursor is now, for Back.
    pub(super) fn nav_push(&mut self) {
        let Some(w) = self.windows.iter().rev().find(|w| w.term.is_none() && w.special.is_none() && w.dock.is_none()) else { return };
        let Some(p) = w.ed.doc.path.clone() else { return };
        let here = (p, w.ed.cursor_line_col().0);
        if self.extras.back.last() != Some(&here) {
            self.extras.back.push(here);
            if self.extras.back.len() > 100 {
                self.extras.back.remove(0);
            }
        }
        self.extras.forward.clear();
    }

    fn nav_go(&mut self, back: bool) {
        let from = if back { self.extras.back.pop() } else { self.extras.forward.pop() };
        let Some((p, line)) = from else {
            self.say(if back { "Nowhere to go back to" } else { "Nowhere to go forward to" });
            return;
        };
        if let Some(w) = self.windows.iter().rev().find(|w| w.term.is_none() && w.special.is_none() && w.dock.is_none()) {
            if let Some(cur) = w.ed.doc.path.clone() {
                let here = (cur, w.ed.cursor_line_col().0);
                if back { self.extras.forward.push(here) } else { self.extras.back.push(here) }
            }
        }
        let fwd = std::mem::take(&mut self.extras.forward);
        self.goto_file_quiet(&p, line);
        self.extras.forward = fwd;
    }

    /// Go to a place without noting it for Back.
    fn goto_file_quiet(&mut self, p: &Path, line: usize) {
        let back = std::mem::take(&mut self.extras.back);
        self.goto_file(p, Some(line), 0);
        self.extras.back = back;
    }

    pub(super) fn extras_command(&mut self, cmd: &str) -> bool {
        match cmd {
            "file.recent" => {
                if self.extras.recent.is_empty() {
                    self.say("No files opened yet");
                    return true;
                }
                let items: Vec<String> = self.extras.recent.iter().map(|p| p.display().to_string()).collect();
                let d = Dialog::new("recent", "Open Recent")
                    .body(ui::list("file", items, 0).activates().lines(14).chars(60))
                    .buttons(vec![ui::button("ok", "~O~pen", true), ui::button("cancel", "Cancel", false)]);
                self.dialog = Some(d);
            }
            "go.back" => self.nav_go(true),
            "go.forward" => self.nav_go(false),
            "edit.show_clipboard" => {
                let text = if self.clipboard.is_empty() { "(The clipboard is empty.)".to_string() } else { self.clipboard.clone() };
                self.show_plain("Clipboard", &text);
            }
            "tools.calculator" => {
                let d = Dialog::new("calculator", "Calculator")
                    .body(ui::col(vec![
                        ui::input("expr", "~E~xpression:", "").chars(40),
                        ui::input("result", "~R~esult:", "").chars(40),
                        ui::label("+ - * / % ^ ( )  0x1F 0b101  sqrt sin cos tan ln log abs  pi e").chars(40),
                    ]))
                    .buttons(vec![ui::button("eval", "~E~valuate", true), ui::button("copy", "~C~opy", false), ui::button("cancel", "Close", false)]);
                self.dialog = Some(d);
            }
            "file.new_window" => {
                let me = std::env::current_exe();
                match me.and_then(|exe| std::process::Command::new(exe).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn()) {
                    Ok(_) => self.say("Opened a new window"),
                    Err(e) => self.say(format!("Can't open a new window: {e}")),
                }
            }
            "terminal.split" => self.run_command("terminal.new"),
            "options.editor" => self.open_settings_at("Editor"),
            "options.dirs" => self.open_settings_at("Files"),
            _ => return false,
        }
        true
    }

    /// The extras' dialogs. True when it was one.
    pub(super) fn extras_dialog(&mut self, d: Dialog, b: &str) -> bool {
        match d.kind.as_str() {
            "recent" => {
                if let Some(p) = d.selected_index("file").and_then(|i| self.extras.recent.get(i).cloned()) {
                    self.goto_file(&p, None, 0);
                }
            }
            "calculator" => {
                let mut d = d;
                let expr = d.value("expr").unwrap_or_default();
                let shown = match calc(&expr) {
                    Ok(v) => show_number(v),
                    Err(e) => format!("Error: {e}"),
                };
                if b == "copy" {
                    let v = shown.split("  ").next().unwrap_or("").to_string();
                    self.copy(v);
                }
                d.set_value("result", &shown);
                self.dialog = Some(d);
            }
            _ => return false,
        }
        true
    }
}

/// A number as the calculator shows it: decimal, and hex and binary for
/// whole numbers.
fn show_number(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        let n = v as i64;
        if n >= 0 { format!("{n}  0x{n:X}  0b{n:b}") } else { format!("{n}") }
    } else {
        let s = format!("{v:.12}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// Evaluate an arithmetic expression.
pub(super) fn calc(s: &str) -> Result<f64, String> {
    struct P<'a> {
        s: &'a [u8],
        i: usize,
    }
    impl P<'_> {
        fn ws(&mut self) {
            while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
                self.i += 1;
            }
        }
        fn eat(&mut self, c: u8) -> bool {
            self.ws();
            if self.s.get(self.i) == Some(&c) {
                self.i += 1;
                true
            } else {
                false
            }
        }
        fn expr(&mut self) -> Result<f64, String> {
            let mut v = self.term()?;
            loop {
                if self.eat(b'+') {
                    v += self.term()?;
                } else if self.eat(b'-') {
                    v -= self.term()?;
                } else {
                    return Ok(v);
                }
            }
        }
        fn term(&mut self) -> Result<f64, String> {
            let mut v = self.power()?;
            loop {
                if self.eat(b'*') {
                    v *= self.power()?;
                } else if self.eat(b'/') {
                    let d = self.power()?;
                    if d == 0.0 {
                        return Err("division by zero".into());
                    }
                    v /= d;
                } else if self.eat(b'%') {
                    v %= self.power()?;
                } else {
                    return Ok(v);
                }
            }
        }
        fn power(&mut self) -> Result<f64, String> {
            let b = self.unary()?;
            if self.eat(b'^') {
                return Ok(b.powf(self.power()?));
            }
            Ok(b)
        }
        fn unary(&mut self) -> Result<f64, String> {
            if self.eat(b'-') {
                return Ok(-self.unary()?);
            }
            if self.eat(b'+') {
                return self.unary();
            }
            self.atom()
        }
        fn atom(&mut self) -> Result<f64, String> {
            self.ws();
            if self.eat(b'(') {
                let v = self.expr()?;
                if !self.eat(b')') {
                    return Err("missing )".into());
                }
                return Ok(v);
            }
            let start = self.i;
            if self.s[self.i..].starts_with(b"0x") || self.s[self.i..].starts_with(b"0b") {
                let radix = if self.s[self.i + 1] == b'x' { 16 } else { 2 };
                self.i += 2;
                let a = self.i;
                while self.i < self.s.len() && (self.s[self.i] as char).is_digit(radix) {
                    self.i += 1;
                }
                let t = std::str::from_utf8(&self.s[a..self.i]).unwrap();
                return i64::from_str_radix(t, radix).map(|n| n as f64).map_err(|_| "bad number".into());
            }
            while self.i < self.s.len() && (self.s[self.i].is_ascii_digit() || self.s[self.i] == b'.' || ((self.s[self.i] == b'e' || self.s[self.i] == b'E') && self.i > start && self.s[start].is_ascii_digit())) {
                self.i += 1;
            }
            if self.i > start {
                let t = std::str::from_utf8(&self.s[start..self.i]).unwrap();
                return t.parse().map_err(|_| format!("bad number {t}"));
            }
            while self.i < self.s.len() && self.s[self.i].is_ascii_alphabetic() {
                self.i += 1;
            }
            let name = std::str::from_utf8(&self.s[start..self.i]).unwrap().to_ascii_lowercase();
            match name.as_str() {
                "pi" => return Ok(std::f64::consts::PI),
                "e" => return Ok(std::f64::consts::E),
                "" => return Err("expected a number".into()),
                _ => {}
            }
            let arg = self.atom()?;
            Ok(match name.as_str() {
                "sqrt" => arg.sqrt(),
                "sin" => arg.sin(),
                "cos" => arg.cos(),
                "tan" => arg.tan(),
                "ln" => arg.ln(),
                "log" => arg.log10(),
                "abs" => arg.abs(),
                "exp" => arg.exp(),
                "floor" => arg.floor(),
                "ceil" => arg.ceil(),
                "round" => arg.round(),
                _ => return Err(format!("no function {name}")),
            })
        }
    }
    let mut p = P { s: s.as_bytes(), i: 0 };
    let v = p.expr()?;
    p.ws();
    if p.i < p.s.len() {
        return Err(format!("unexpected {}", &s[p.i..]));
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::calc;

    #[test]
    fn calculator() {
        assert_eq!(calc("1 + 2 * 3"), Ok(7.0));
        assert_eq!(calc("(1 + 2) * 3"), Ok(9.0));
        assert_eq!(calc("2 ^ 3 ^ 2"), Ok(512.0));
        assert_eq!(calc("0xFF + 0b11"), Ok(258.0));
        assert_eq!(calc("-sqrt(16) + abs(-2)"), Ok(-2.0));
        assert!(calc("1 / 0").is_err() && calc("2 +").is_err() && calc("foo(1)").is_err());
        assert_eq!(super::show_number(255.0), "255  0xFF  0b11111111");
    }
}
