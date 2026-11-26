//! vi's ":" commands for the editor (Tessera's qml/ExCommands.qml): the Ex
//! commands VSCodeVim has that mean something for one text, plus :g and :v.
//! `run(line, …)` does one and returns what to show under it ("" for
//! nothing; `error` says whether it's a complaint). What belongs to the
//! editor (writing, quitting, reading a file, an option) goes out as an
//! `ExAction`; `ExOut::to_vi` gives ComposeBox's meanings for them (:w / :x
//! / :wq run the command, :q clears it, :w {file} and :e are refused).
//!
//! Ranges: N, ., $, %, 'x (a mark; '< '> the selection), /pat/ and ?pat?,
//! each with +N/-N, joined by , or ;. Patterns are vim's (magic: \( \) \|
//! \+ \? \= \{n,m} \< \>; \v very magic; \c and \C), smart case by default.
//!
//! Commands: :N (go to line), :s[ubstitute] (flags g c-less i I n &, and :&
//! :s alone to repeat), :g[lobal] / :v[global] / :g!, :d[elete] [x], :y[ank],
//! :pu[t][!], :m[ove], :t / :co[py], :j[oin][!], :> :<, :norm[al][!],
//! :sor[t][!] (i n u x r /pat/), :ret[ab][!] [n], :le[ft] :ri[ght] :ce[nter],
//! :ma[rk] / :k, :delm[arks][!], :marks, :reg[isters] / :di[splay],
//! :p[rint] :nu[mber] :# :l[ist] :=, :f[ile], :u[ndo] :red[o],
//! :noh[lsearch], :se[t] (wrap, ignorecase, smartcase, shiftwidth, tabstop,
//! expandtab, number, hlsearch: some go to the owner), :ec[ho],
//! :w[rite] / :up[date] / :sav[eas] / :w {file}, :wq, :x[it], :exi[t],
//! :q[uit][!], :qa[ll], :wa[ll], :xa[ll], :wqa[ll], :e[dit][!] {file},
//! :r[ead] {file}, :pw[d].
//!
//! The unnamed register is the editor's kill ring (`Text::kill_ring`), so
//! Emacs's Ctrl+K / Ctrl+Y and vi's yanks share it, as in ComposeBox.
//!
//! Patterns: Tessera turns vim's into JavaScript's RegExp; there's no regex
//! crate here, so `Re` below is a small backtracking engine for that
//! JavaScript subset (groups, alternation, classes, quantifiers greedy and
//! lazy, \b, backreferences). Its \w and \b take any letter as a word
//! letter, not only ASCII (JavaScript's), so * finds "café".

use crate::text::{Indent, Text};
use crate::vi::{Vi, ViOut};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashMap;

// ---- patterns: JavaScript's RegExp, the subset Tessera's vim patterns become ----

#[derive(Debug, Clone)]
enum Node {
    Lit(char),
    Any,
    Set(bool, Vec<SetItem>),
    Bol,
    Eol,
    WordB(bool),
    Group(Option<usize>, Vec<Vec<Node>>),
    Back(usize),
    Rep(Box<Node>, usize, usize, bool),
}

#[derive(Debug, Clone)]
enum SetItem {
    Ch(char),
    Range(char, char),
    Class(char),
}

enum Esc {
    Ch(char),
    Class(char),
    WordB(bool),
    Back(usize),
}

/// A compiled pattern.
pub struct Re {
    top: Vec<Node>,
    groups: usize,
    icase: bool,
}

/// A match's groups (0: the whole match), as byte ranges.
pub type Caps = Vec<Option<(usize, usize)>>;

fn js_space(c: char) -> bool {
    c.is_whitespace() || c == '\u{feff}'
}
fn js_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}
fn class_has(k: char, c: char) -> bool {
    match k {
        'd' => c.is_ascii_digit(),
        'D' => !c.is_ascii_digit(),
        'w' => js_word(c),
        'W' => !js_word(c),
        's' => js_space(c),
        'S' => !js_space(c),
        _ => false,
    }
}
fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}
fn upper(c: char) -> char {
    c.to_uppercase().next().unwrap_or(c)
}

struct Parser {
    s: Vec<char>,
    i: usize,
    groups: usize,
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.s.get(self.i).copied()
    }
    fn alt(&mut self) -> Result<Vec<Vec<Node>>, String> {
        let mut alts = vec![self.seq()?];
        while self.peek() == Some('|') {
            self.i += 1;
            alts.push(self.seq()?);
        }
        Ok(alts)
    }
    fn seq(&mut self) -> Result<Vec<Node>, String> {
        let mut out = vec![];
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' {
                break;
            }
            let a = self.atom()?;
            out.push(self.quant(a)?);
        }
        Ok(out)
    }
    /// {n}, {n,}, {n,m} at `at`: (min, max, index after); else None (a literal {).
    fn brace(&self, at: usize) -> Option<(usize, usize, usize)> {
        let s = &self.s;
        if s.get(at) != Some(&'{') {
            return None;
        }
        let mut i = at + 1;
        let num = |i: &mut usize| {
            let st = *i;
            let mut n: usize = 0;
            while let Some(d) = s.get(*i).and_then(|c| c.to_digit(10)) {
                n = n.saturating_mul(10).saturating_add(d as usize);
                *i += 1;
            }
            (*i > st).then_some(n)
        };
        let a = num(&mut i)?;
        let mut b = a;
        if s.get(i) == Some(&',') {
            i += 1;
            b = num(&mut i).unwrap_or(usize::MAX);
        }
        if s.get(i) != Some(&'}') {
            return None;
        }
        Some((a, b, i + 1))
    }
    fn quant(&mut self, a: Node) -> Result<Node, String> {
        let (min, max) = match self.peek() {
            Some('*') => (0, usize::MAX),
            Some('+') => (1, usize::MAX),
            Some('?') => (0, 1),
            Some('{') => match self.brace(self.i) {
                Some((x, y, j)) => {
                    self.i = j - 1;
                    (x, y)
                }
                None => return Ok(a),
            },
            _ => return Ok(a),
        };
        self.i += 1;
        if max < min {
            return Err("numbers out of order in {} quantifier".into());
        }
        let greedy = if self.peek() == Some('?') {
            self.i += 1;
            false
        } else {
            true
        };
        if matches!(self.peek(), Some('*' | '+' | '?')) || self.brace(self.i).is_some() {
            return Err("Nothing to repeat".into());
        }
        Ok(Node::Rep(Box::new(a), min, max, greedy))
    }
    fn esc(&mut self) -> Result<Esc, String> {
        let Some(d) = self.peek() else { return Err("\\ at end of pattern".into()) };
        self.i += 1;
        let hex = |p: &mut Parser, n: usize| -> Option<char> {
            let s: String = p.s.get(p.i..p.i + n)?.iter().collect();
            let v = u32::from_str_radix(&s, 16).ok()?;
            p.i += n;
            char::from_u32(v)
        };
        Ok(match d {
            'd' | 'D' | 'w' | 'W' | 's' | 'S' => Esc::Class(d),
            'b' => Esc::WordB(true),
            'B' => Esc::WordB(false),
            'n' => Esc::Ch('\n'),
            't' => Esc::Ch('\t'),
            'r' => Esc::Ch('\r'),
            'f' => Esc::Ch('\x0c'),
            'v' => Esc::Ch('\x0b'),
            '0' => Esc::Ch('\0'),
            '1'..='9' => Esc::Back(d as usize - '0' as usize),
            'x' => hex(self, 2).map(Esc::Ch).unwrap_or(Esc::Ch('x')),
            'u' => hex(self, 4).map(Esc::Ch).unwrap_or(Esc::Ch('u')),
            _ => Esc::Ch(d),
        })
    }
    fn set_item(&mut self, c: char) -> Result<SetItem, String> {
        if c != '\\' {
            return Ok(SetItem::Ch(c));
        }
        Ok(match self.esc()? {
            Esc::Ch(x) => SetItem::Ch(x),
            Esc::Class(k) => SetItem::Class(k),
            Esc::WordB(true) => SetItem::Ch('\x08'),
            Esc::WordB(false) => SetItem::Ch('B'),
            Esc::Back(n) => SetItem::Ch(char::from_u32(n as u32).unwrap_or('\0')),
        })
    }
    fn set(&mut self) -> Result<Node, String> {
        let neg = self.peek() == Some('^');
        if neg {
            self.i += 1;
        }
        let mut items = vec![];
        loop {
            let Some(c) = self.peek() else { return Err("Unterminated character class".into()) };
            self.i += 1;
            if c == ']' {
                break;
            }
            let lo = self.set_item(c)?;
            if self.peek() == Some('-') && self.s.get(self.i + 1).is_some_and(|&x| x != ']') {
                self.i += 1;
                let c2 = self.s[self.i];
                self.i += 1;
                let hi = self.set_item(c2)?;
                match (&lo, &hi) {
                    (SetItem::Ch(a), SetItem::Ch(b)) => {
                        if b < a {
                            return Err("Range out of order in character class".into());
                        }
                        items.push(SetItem::Range(*a, *b));
                    }
                    _ => {
                        items.push(lo);
                        items.push(SetItem::Ch('-'));
                        items.push(hi);
                    }
                }
                continue;
            }
            items.push(lo);
        }
        Ok(Node::Set(neg, items))
    }
    fn atom(&mut self) -> Result<Node, String> {
        let c = self.s[self.i];
        self.i += 1;
        Ok(match c {
            '(' => {
                let cap = if self.peek() == Some('?') && self.s.get(self.i + 1) == Some(&':') {
                    self.i += 2;
                    None
                } else {
                    self.groups += 1;
                    Some(self.groups)
                };
                let alts = self.alt()?;
                if self.peek() != Some(')') {
                    return Err("Unterminated group".into());
                }
                self.i += 1;
                Node::Group(cap, alts)
            }
            '.' => Node::Any,
            '^' => Node::Bol,
            '$' => Node::Eol,
            '[' => self.set()?,
            '\\' => match self.esc()? {
                Esc::Ch(x) => Node::Lit(x),
                Esc::Class(k) => Node::Set(false, vec![SetItem::Class(k)]),
                Esc::WordB(b) => Node::WordB(b),
                Esc::Back(n) => Node::Back(n),
            },
            '*' | '+' | '?' => return Err("Nothing to repeat".into()),
            '{' if self.brace(self.i - 1).is_some() => return Err("Nothing to repeat".into()),
            c => Node::Lit(c),
        })
    }
}

struct M<'s> {
    s: &'s str,
    icase: bool,
    caps: Caps,
}

impl<'s> M<'s> {
    fn at(&self, i: usize) -> Option<char> {
        self.s[i..].chars().next()
    }
    fn before(&self, i: usize) -> Option<char> {
        self.s[..i].chars().next_back()
    }
    fn eq(&self, a: char, b: char) -> bool {
        a == b || (self.icase && fold(a) == fold(b))
    }
    fn in_set(&self, items: &[SetItem], x: char) -> bool {
        let has = |x: char| {
            items.iter().any(|it| match *it {
                SetItem::Ch(c) => c == x,
                SetItem::Range(a, b) => a <= x && x <= b,
                SetItem::Class(k) => class_has(k, x),
            })
        };
        has(x) || (self.icase && (has(fold(x)) || has(upper(x))))
    }
    fn seq(&mut self, ns: &[Node], i: usize, k: &mut dyn FnMut(&mut M<'s>, usize) -> bool) -> bool {
        match ns.split_first() {
            None => k(self, i),
            Some((n, rest)) => self.node(n, i, &mut |m: &mut M<'s>, j| m.seq(rest, j, k)),
        }
    }
    fn node(&mut self, n: &Node, i: usize, k: &mut dyn FnMut(&mut M<'s>, usize) -> bool) -> bool {
        match n {
            Node::Lit(c) => match self.at(i) {
                Some(x) if self.eq(x, *c) => k(self, i + x.len_utf8()),
                _ => false,
            },
            Node::Any => match self.at(i) {
                Some(x) if !matches!(x, '\n' | '\r' | '\u{2028}' | '\u{2029}') => k(self, i + x.len_utf8()),
                _ => false,
            },
            Node::Set(neg, items) => match self.at(i) {
                Some(x) if self.in_set(items, x) != *neg => k(self, i + x.len_utf8()),
                _ => false,
            },
            Node::Bol => i == 0 && k(self, i),
            Node::Eol => i == self.s.len() && k(self, i),
            Node::WordB(b) => {
                let w = self.before(i).is_some_and(js_word) != self.at(i).is_some_and(js_word);
                w == *b && k(self, i)
            }
            Node::Group(cap, alts) => {
                for alt in alts {
                    let ok = match *cap {
                        None => self.seq(alt, i, k),
                        Some(g) => self.seq(alt, i, &mut |m: &mut M<'s>, j| {
                            let old = m.caps[g];
                            m.caps[g] = Some((i, j));
                            if k(m, j) {
                                true
                            } else {
                                m.caps[g] = old;
                                false
                            }
                        }),
                    };
                    if ok {
                        return true;
                    }
                }
                false
            }
            Node::Back(g) => {
                let s = self.s;
                let want = self.caps.get(*g).copied().flatten().map(|(a, b)| &s[a..b]).unwrap_or("");
                let mut j = i;
                for c in want.chars() {
                    match self.at(j) {
                        Some(x) if self.eq(x, c) => j += x.len_utf8(),
                        _ => return false,
                    }
                }
                k(self, j)
            }
            Node::Rep(n, min, max, greedy) => self.rep(n, *min, *max, *greedy, 0, i, k),
        }
    }
    #[allow(clippy::too_many_arguments)]
    fn rep(&mut self, n: &Node, min: usize, max: usize, greedy: bool, count: usize, i: usize, k: &mut dyn FnMut(&mut M<'s>, usize) -> bool) -> bool {
        // (An empty pass once the least is met ends the repeating: no looping on nothing.)
        if greedy {
            if count < max && self.node(n, i, &mut |m: &mut M<'s>, j| (j != i || count < min) && m.rep(n, min, max, greedy, count + 1, j, k)) {
                return true;
            }
            count >= min && k(self, i)
        } else {
            if count >= min && k(self, i) {
                return true;
            }
            count < max && self.node(n, i, &mut |m: &mut M<'s>, j| (j != i || count < min) && m.rep(n, min, max, greedy, count + 1, j, k))
        }
    }
}

impl Re {
    /// A JavaScript pattern (`icase`: its "i" flag).
    pub fn new(js: &str, icase: bool) -> Result<Re, String> {
        let mut p = Parser { s: js.chars().collect(), i: 0, groups: 0 };
        let err = |e: String| format!("SyntaxError: Invalid regular expression: /{js}/: {e}");
        let alts = p.alt().map_err(err)?;
        if p.i < p.s.len() {
            return Err(err("Unmatched ')'".into()));
        }
        Ok(Re { top: vec![Node::Group(Some(0), alts)], groups: p.groups, icase })
    }
    /// The first match at or after `from`.
    pub fn exec_at(&self, s: &str, from: usize) -> Option<Caps> {
        let mut i = from;
        loop {
            let mut m = M { s, icase: self.icase, caps: vec![None; self.groups + 1] };
            if m.seq(&self.top, i, &mut |_, _| true) {
                return Some(m.caps);
            }
            if i >= s.len() {
                return None;
            }
            i += s[i..].chars().next().map_or(1, char::len_utf8);
        }
    }
    pub fn test(&self, s: &str) -> bool {
        self.exec_at(s, 0).is_some()
    }
    /// Each match from the start (as JavaScript's "g": an empty one steps a char on).
    pub fn all(&self, s: &str, limit: usize) -> Vec<Caps> {
        let mut out = vec![];
        let mut i = 0;
        while i <= s.len() && out.len() < limit {
            let Some(c) = self.exec_at(s, i) else { break };
            let (a, b) = c[0].unwrap();
            out.push(c);
            i = if b == a {
                if a >= s.len() {
                    break;
                }
                a + s[a..].chars().next().map_or(1, char::len_utf8)
            } else {
                b
            };
        }
        out
    }
    /// String.replace: the first match, or every one; `f` makes each one's text
    /// from its groups (an unmatched one: None). Returns the text and how many.
    pub fn replace(&self, s: &str, every: bool, mut f: impl FnMut(&[Option<&str>]) -> String) -> (String, usize) {
        let ms = if every { self.all(s, usize::MAX) } else { self.exec_at(s, 0).into_iter().collect() };
        let mut out = String::new();
        let mut last = 0;
        for c in &ms {
            let (a, b) = c[0].unwrap();
            out.push_str(&s[last..a]);
            let g: Vec<Option<&str>> = c.iter().map(|x| x.map(|(a, b)| &s[a..b])).collect();
            out.push_str(&f(&g));
            last = b;
        }
        out.push_str(&s[last..]);
        (out, ms.len())
    }
}

// ---- what a command asks of the editor ----------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub enum ExAction {
    /// "" is the text itself (in the editor: run it).
    Write(String, bool),
    Quit(bool),
    /// :wa — write every changed buffer.
    WriteAll(bool),
    /// :qa — quit every window.
    QuitAll(bool),
    /// Window and buffer commands for a host with several windows:
    /// "split", "vsplit", "new", "vnew", "enew", "only", "bnext", "bprev".
    Window(String),
    /// "" with force: reload.
    EditFile(String, bool),
    SetOption(String, String),
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct ExOut {
    /// What to show under the line ("" for nothing).
    pub text: String,
    pub error: bool,
    pub actions: Vec<ExAction>,
}

impl ExOut {
    /// What the input editor does with it (ComposeBox's ExCommands handlers):
    /// :w runs the command, :q clears it, :w {file} and :e are refused. (:wq
    /// writes then quits: here it runs the command, as its help says.)
    pub fn to_vi(&self) -> ViOut {
        for a in &self.actions {
            if let ExAction::Write(p, _) = a {
                return if p.is_empty() { ViOut::Submit } else { ViOut::Message("Not here: :w {file} (this runs the command)".into(), true) };
            }
        }
        for a in &self.actions {
            match a {
                ExAction::Quit(_) => return ViOut::Clear,
                ExAction::EditFile(..) => return ViOut::Message("Not here: :e (this is the command being written)".into(), true),
                _ => {}
            }
        }
        if !self.text.is_empty() {
            return ViOut::Message(self.text.clone(), self.error);
        }
        ViOut::Handled
    }
}

/// A completion for the command line: the command in full, its shortest
/// form, what it does, and the range typed before it.
#[derive(Clone, Debug, PartialEq)]
pub struct Completion {
    pub name: String,
    pub short: String,
    pub help: String,
    pub head: String,
}

// [name as typed in full, the shortest it can be, what it does]
const COMMAND_LIST: &[(&str, &str, &str)] = &[
    ("substitute", "s", "s/pat/rep/[g i I n] — replace in the range (default: this line)"),
    ("global", "g", "g/pat/cmd — run cmd on each line matching (default cmd: print)"),
    ("vglobal", "v", "v/pat/cmd — run cmd on each line NOT matching"),
    ("delete", "d", "delete the range's lines [into register x] [count]"),
    ("yank", "y", "copy the range's lines [into register x]"),
    ("put", "pu", "put the register's lines after the line (put! before)"),
    ("move", "m", "move the range below line {address} (0: to the top)"),
    ("copy", "co", "copy the range below line {address} (same as :t)"),
    ("t", "t", "copy the range below line {address}"),
    ("join", "j", "join the range's lines (join! without spaces)"),
    (">", ">", "indent the range (>> twice)"),
    ("<", "<", "unindent the range"),
    ("normal", "norm", "normal {keys} — run vi keys on each line of the range"),
    ("sort", "sor", "sort the range (all lines): ! reverse, i any case, n numbers, u unique, /pat/"),
    ("retab", "ret", "tabs to spaces [tabstop]"),
    ("left", "le", "align the range left [indent]"),
    ("right", "ri", "align the range right [width]"),
    ("center", "ce", "center the range [width]"),
    ("mark", "ma", "mark {a-z} — set a mark on the line (use as 'a)"),
    ("k", "k", "k{a-z} — set a mark on the line"),
    ("delmarks", "delm", "delete marks (delm! all)"),
    ("marks", "marks", "list the marks"),
    ("registers", "reg", "list the registers"),
    ("display", "di", "list the registers"),
    ("print", "p", "show the range's lines"),
    ("number", "nu", "show the range's lines with numbers"),
    ("#", "#", "show the range's lines with numbers"),
    ("list", "l", "show the range's lines with tabs and ends"),
    ("=", "=", "the last line's number (or the range's)"),
    ("file", "f", "the file's name, lines and where you are"),
    ("undo", "u", "undo"),
    ("redo", "red", "redo"),
    ("nohlsearch", "noh", "clear the search highlight"),
    ("set", "se", "set ic scs sw=N ts=N et wrap nu (no…, …!, …?)"),
    ("echo", "ec", "show the text"),
    ("write", "w", "save (w {file}: save a copy there; w!: even if unchanged)"),
    ("update", "up", "save if changed"),
    ("saveas", "sav", "save as {file}, and edit that"),
    ("wq", "wq", "save and close the editor"),
    ("xit", "x", "save if changed and close the editor"),
    ("exit", "exi", "save if changed and close the editor"),
    ("wall", "wa", "save"),
    ("wqall", "wqa", "save and close"),
    ("xall", "xa", "save and close"),
    ("quit", "q", "close the editor (q! drops the changes)"),
    ("qall", "qa", "close the editor"),
    ("close", "clo", "close the editor"),
    ("edit", "e", "e! drops the changes and reads the file again; e {file} opens one"),
    ("read", "r", "r {file} — put a file's lines after the line"),
    ("pwd", "pw", "the file's folder"),
];

// ---- Ex ------------------------------------------------------------------------

struct Ln {
    s: String,
    id: u64,
}

#[derive(Clone)]
struct Sub {
    pat: String,
    rep: String,
    flags: String,
}

struct Rg {
    a: i64,
    b: i64,
    given: bool,
    rest: String,
}

/// What a command works with: the text, its indentation, and the vi keys
/// (for :normal).
struct Cx<'a> {
    t: &'a mut Text,
    ind: &'a Indent,
    vi: Option<&'a mut Vi>,
}

type R<T> = Result<T, String>;

pub struct Ex {
    /// (:set sw / et's indentation; None: the editor's.)
    pub indent_unit: Option<String>,
    pub ignore_case: bool,
    pub smart_case: bool,
    pub tabstop: usize,
    /// Named registers (a-z, and "0" the last yank); the unnamed one is the
    /// text's kill ring.
    pub registers: HashMap<char, String>,
    /// a-z: a line number.
    pub marks: HashMap<char, usize>,
    pub history: Vec<String>,
    pub last_search: String,
    last_sub: Option<Sub>,
    pub file_name: String,
    pub folder: String,
    /// (How to read a file, for :r: its text or why not.)
    pub read_file: Option<Box<dyn Fn(&str) -> Result<String, String>>>,
    /// An owner's own meanings: name -> its help, or None to leave it out.
    pub help_for: HashMap<&'static str, Option<&'static str>>,
    pub error: bool,
    search_back: bool,
    // The text as lines with ids, so :g and :m keep track of lines as they move.
    ln: Vec<Ln>,
    // (A text ending in a newline has no empty line after it, as in vim: the
    // newline is the last line's end, put back when stored.)
    ends_with_newline: bool,
    next_id: u64,
    actions: Vec<ExAction>,
}

impl Default for Ex {
    fn default() -> Self {
        Self::new()
    }
}

fn lines(n: usize) -> String {
    format!("{n}{}", if n == 1 { " line" } else { " lines" })
}

/// The text between delimiters from `i` (escaped ones kept): (text, index after).
fn delimited(s: &[char], mut i: usize, d: char) -> (String, usize) {
    let mut out = String::new();
    while i < s.len() {
        if s[i] == '\\' && s.get(i + 1) == Some(&d) {
            if d == '/' || d == '?' {
                out.push('\\');
            }
            out.push(d);
            i += 2;
            continue;
        }
        if s[i] == '\\' && i + 1 < s.len() {
            out.push(s[i]);
            out.push(s[i + 1]);
            i += 2;
            continue;
        }
        if s[i] == d {
            return (out, i + 1);
        }
        out.push(s[i]);
        i += 1;
    }
    (out, i)
}

/// vim's replacement as text for a match (& \0-\9 \n \r \t \u \U \l \L \E ~).
fn replacement(rep: &str, m: &[Option<&str>]) -> String {
    let mut out = String::new();
    let (mut mode, mut once) = (' ', ' ');
    let put = |s: &str, out: &mut String, mode: char, once: &mut char| {
        for ch in s.chars() {
            if *once == 'u' {
                out.extend(ch.to_uppercase());
                *once = ' ';
            } else if *once == 'l' {
                out.extend(ch.to_lowercase());
                *once = ' ';
            } else if mode == 'U' {
                out.extend(ch.to_uppercase());
            } else if mode == 'L' {
                out.extend(ch.to_lowercase());
            } else {
                out.push(ch);
            }
        }
    };
    let r: Vec<char> = rep.chars().collect();
    let mut i = 0;
    while i < r.len() {
        let c = r[i];
        if c == '&' {
            put(m[0].unwrap_or(""), &mut out, mode, &mut once);
        } else if c == '\\' && i + 1 < r.len() {
            i += 1;
            let d = r[i];
            match d {
                '0'..='9' => put(m.get(d as usize - '0' as usize).copied().flatten().unwrap_or(""), &mut out, mode, &mut once),
                'n' | 'r' => out.push('\n'),
                't' => out.push('\t'),
                'u' | 'l' => once = d,
                'U' | 'L' => mode = d,
                'E' | 'e' => mode = ' ',
                _ => put(&d.to_string(), &mut out, mode, &mut once),
            }
        } else {
            put(&c.to_string(), &mut out, mode, &mut once);
        }
        i += 1;
    }
    out
}

/// (Commands after :g joined by |, a \| kept as a |.)
fn split_bar(c: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut it = c.chars().peekable();
    while let Some(ch) = it.next() {
        if ch == '\\' && it.peek() == Some(&'|') {
            cur.push('|');
            it.next();
        } else if ch == '|' {
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(ch);
        }
    }
    out.push(cur);
    out
}

fn digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}
fn num(s: &str) -> i64 {
    s.parse::<i64>().unwrap_or(i64::MAX)
}
/// "^( {1,n}|\t)" gone from s.
fn outdent(s: &str, n: usize) -> String {
    if let Some(r) = s.strip_prefix('\t') {
        if !s.starts_with(' ') {
            return r.to_string();
        }
    }
    let sp = s.len() - s.trim_start_matches(' ').len();
    if sp > 0 {
        return s[sp.min(n.max(1))..].to_string();
    }
    s.to_string()
}
/// Byte index of char `n` in s (at most its end).
fn cidx(s: &str, n: usize) -> usize {
    s.char_indices().nth(n).map_or(s.len(), |(i, _)| i)
}
fn js_trim(s: &str) -> &str {
    s.trim_matches(js_space)
}

impl Ex {
    pub fn new() -> Ex {
        // (The input editor's meanings, ComposeBox's: :w and :x run the
        // command, :q clears it; files are no business of it.)
        let rc = Some("run the command");
        let ce = Some("clear the editor");
        let help_for: HashMap<&'static str, Option<&'static str>> = [
            ("write", rc),
            ("update", rc),
            ("wq", rc),
            ("xit", rc),
            ("exit", rc),
            ("wall", rc),
            ("wqall", rc),
            ("xall", rc),
            ("quit", ce),
            ("qall", ce),
            ("close", ce),
            ("saveas", None),
            ("edit", None),
            ("read", None),
            ("file", None),
            ("pwd", None),
        ]
        .into_iter()
        .collect();
        Ex {
            indent_unit: None,
            ignore_case: true,
            smart_case: true,
            tabstop: 8,
            registers: HashMap::new(),
            marks: HashMap::new(),
            history: vec![],
            last_search: String::new(),
            last_sub: None,
            file_name: String::new(),
            folder: String::new(),
            read_file: None,
            help_for,
            error: false,
            search_back: false,
            ln: vec![],
            ends_with_newline: false,
            next_id: 0,
            actions: vec![],
        }
    }

    fn unit(&self, ind: &Indent) -> String {
        self.indent_unit.clone().unwrap_or_else(|| ind.unit())
    }
    fn mk(&mut self, s: String) -> Ln {
        self.next_id += 1;
        Ln { s, id: self.next_id }
    }

    // ---- lines -------------------------------------------------------------
    pub fn load(&mut self, t: &Text) {
        let mut s = t.text.as_str();
        self.ends_with_newline = s.ends_with('\n');
        if self.ends_with_newline {
            s = &s[..s.len() - 1];
        }
        let parts: Vec<String> = s.split('\n').map(String::from).collect();
        self.ln = parts.into_iter().map(|s| self.mk(s)).collect();
    }
    fn store(&mut self, t: &mut Text, cursor_line: Option<i64>, col: Option<usize>) {
        let mut s = self.ln.iter().map(|o| o.s.as_str()).collect::<Vec<_>>().join("\n");
        if self.ends_with_newline {
            s.push('\n');
        }
        if s != t.text {
            // (Only what changed, between the same start and end: one step to undo.)
            let o = t.text.as_bytes();
            let n = s.as_bytes();
            let mut a = 0;
            while a < o.len() && a < n.len() && o[a] == n[a] {
                a += 1;
            }
            while !t.text.is_char_boundary(a) || !s.is_char_boundary(a) {
                a -= 1;
            }
            let mut b = 0;
            while b < o.len() - a && b < n.len() - a && o[o.len() - 1 - b] == n[n.len() - 1 - b] {
                b += 1;
            }
            while !t.text.is_char_boundary(o.len() - b) || !s.is_char_boundary(n.len() - b) {
                b -= 1;
            }
            let put = s[a..n.len() - b].to_string();
            t.replace(a, o.len() - b, &put);
        }
        if let Some(cl) = cursor_line {
            let n = cl.clamp(0, self.ln.len() as i64 - 1).max(0) as usize;
            let p: usize = self.ln[..n].iter().map(|l| l.s.len() + 1).sum();
            let line = self.ln.get(n).map(|l| l.s.as_str()).unwrap_or("");
            let c = match col {
                Some(c) => cidx(line, c),
                None => line.len() - line.trim_start_matches([' ', '\t']).len(),
            };
            t.move_to((p + c).min(t.len()), false);
        }
    }
    fn cursor_line(t: &Text) -> i64 {
        t.line_of(t.cursor) as i64
    }

    // ---- patterns ----------------------------------------------------------
    /// vim's regex (magic) as a JavaScript one.
    pub fn regex(&self, pat: &str, flags: &str) -> R<Re> {
        let mut pat = pat.to_string();
        let mut ic = self.ignore_case;
        if pat.contains("\\c") {
            ic = true;
            pat = pat.replace("\\c", "");
        } else if pat.contains("\\C") {
            ic = false;
            pat = pat.replace("\\C", "");
        } else if ic && self.smart_case {
            let mut plain = String::new();
            let mut it = pat.chars();
            while let Some(c) = it.next() {
                if c == '\\' {
                    it.next();
                } else {
                    plain.push(c);
                }
            }
            if plain.chars().any(|c| c.is_ascii_uppercase()) {
                ic = false;
            }
        }
        if flags.contains('i') {
            ic = true;
        }
        if flags.contains('I') {
            ic = false;
        }
        let mut js = String::new();
        if let Some(v) = pat.strip_prefix("\\v") {
            js = v.replace('<', "\\b").replace('>', "\\b").replace('=', "?");
        } else {
            let p: Vec<char> = pat.chars().collect();
            let mut i = 0;
            while i < p.len() {
                let c = p[i];
                if c == '\\' && i + 1 < p.len() {
                    i += 1;
                    let d = p[i];
                    if "(){}|+?".contains(d) {
                        js.push(d);
                    } else if d == '=' {
                        js.push('?');
                    } else if d == '<' || d == '>' {
                        js.push_str("\\b");
                    } else if d == 'n' {
                        js.push_str("\\n");
                    } else if d == 't' {
                        js.push_str("\\t");
                    } else {
                        js.push('\\'); // \s \d \w \. \/ ...
                        js.push(d);
                    }
                } else if "(){}|+?".contains(c) {
                    js.push('\\');
                    js.push(c);
                } else if c == '~' {
                    js.push_str("\\~");
                } else {
                    js.push(c);
                }
                i += 1;
            }
        }
        Re::new(&js, ic)
    }

    // ---- ranges ------------------------------------------------------------
    /// One address from `i`: (line (0-based) or None, index after).
    fn address(&mut self, t: &Text, s: &[char], mut i: usize, cur: i64) -> R<(Option<i64>, usize)> {
        let mut line = None;
        match s.get(i).copied() {
            Some('.') => {
                line = Some(cur);
                i += 1;
            }
            Some('$') => {
                line = Some(self.ln.len() as i64 - 1);
                i += 1;
            }
            Some(c) if c.is_ascii_digit() => {
                let mut n: i64 = 0;
                while let Some(d) = s.get(i).and_then(|c| c.to_digit(10)) {
                    n = n.saturating_mul(10).saturating_add(d as i64);
                    i += 1;
                }
                line = Some(n - 1);
            }
            Some('\'') if i + 1 < s.len() => {
                let k = s[i + 1];
                i += 2;
                let l = self.mark_line(t, k);
                if l < 0 {
                    return Err("E20: Mark not set".into());
                }
                line = Some(l);
            }
            Some(c) if c == '/' || c == '?' => {
                let (p, j) = delimited(s, i + 1, c);
                i = j;
                let re = self.regex(if !p.is_empty() { &p } else { &self.last_search }, "")?;
                if !p.is_empty() {
                    self.last_search = p.clone();
                }
                let len = self.ln.len() as i64;
                let mut found = -1;
                for k in 1..=len {
                    let n = if c == '/' { (cur + k).rem_euclid(len) } else { (cur - k + len * 2).rem_euclid(len) };
                    if re.test(&self.ln[n as usize].s) {
                        found = n;
                        break;
                    }
                }
                if found < 0 {
                    return Err(format!("E486: Pattern not found: {p}"));
                }
                line = Some(found);
            }
            _ => {}
        }
        // (+N, -N, a bare + or -; a number after an address adds to it.)
        while let Some(&c) = s.get(i).filter(|c| **c == '+' || **c == '-') {
            let sign = if c == '+' { 1 } else { -1 };
            i += 1;
            let st = i;
            while s.get(i).is_some_and(|c| c.is_ascii_digit()) {
                i += 1;
            }
            let n: String = s[st..i].iter().collect();
            line = Some(line.unwrap_or(cur) + sign * if n.is_empty() { 1 } else { num(&n) });
        }
        Ok((line, i))
    }
    pub fn mark_line(&self, t: &Text, k: char) -> i64 {
        if k == '<' || k == '>' {
            let Some((s, e)) = t.selection() else { return -1 };
            return t.line_of(if k == '<' { s } else { t.prev(e).max(s) }) as i64;
        }
        match self.marks.get(&k) {
            Some(&m) if m < self.ln.len() => m as i64,
            _ => -1,
        }
    }
    /// The range at the start of `s`.
    fn range(&mut self, t: &Text, s: &str, cur: i64) -> R<Rg> {
        let s = s.trim_start_matches(|c: char| js_space(c) || c == ':');
        let last = self.ln.len() as i64 - 1;
        if let Some(r) = s.strip_prefix('%') {
            return Ok(Rg { a: 0, b: last, given: true, rest: r.into() });
        }
        if let Some(r) = s.strip_prefix('*') {
            let (a, b) = (self.mark_line(t, '<'), self.mark_line(t, '>'));
            if a < 0 {
                return Err("E20: Mark not set".into());
            }
            return Ok(Rg { a, b, given: true, rest: r.into() });
        }
        let cs: Vec<char> = s.chars().collect();
        let (a0, mut i) = self.address(t, &cs, 0, cur)?;
        let mut cur = cur;
        let mut a = a0;
        let mut b = a0;
        while matches!(cs.get(i), Some(',' | ';')) {
            if cs[i] == ';' {
                if let Some(x) = a {
                    cur = x;
                }
            }
            let r = self.address(t, &cs, i + 1, cur)?;
            if a.is_none() {
                a = Some(cur);
            }
            b = Some(r.0.unwrap_or(cur));
            i = r.1;
        }
        let given = a.is_some();
        let (mut a, mut b) = match a {
            None => (cur, cur),
            Some(a) => (a, b.unwrap_or(a)),
        };
        if b < a {
            std::mem::swap(&mut a, &mut b);
        }
        Ok(Rg { a, b, given, rest: cs[i..].iter().collect() })
    }
    fn check(&self, a: i64, b: i64) -> R<()> {
        if a < 0 || b >= self.ln.len() as i64 {
            return Err("E16: Invalid range".into());
        }
        Ok(())
    }

    // ---- "/" and "?": the next match from the cursor, wrapping ---------------
    pub fn search(&mut self, t: &mut Text, pat: &str, back: bool) -> String {
        let pat = if pat.is_empty() { self.last_search.clone() } else { pat.to_string() };
        if pat.is_empty() {
            return "E35: No previous regular expression".into();
        }
        self.last_search = pat;
        self.search_back = back;
        self.search_next(t, false)
    }
    pub fn search_next(&mut self, t: &mut Text, reverse: bool) -> String {
        if self.last_search.is_empty() {
            return "E35: No previous regular expression".into();
        }
        let Ok(re) = self.regex(&self.last_search, "") else { return "E383: Invalid search string".into() };
        let p = t.cursor;
        let all: Vec<usize> = re.all(&t.text, 200000).iter().map(|c| c[0].unwrap().0).collect();
        if all.is_empty() {
            return format!("E486: Pattern not found: {}", self.last_search);
        }
        let back = self.search_back != reverse;
        let found = if back { all.iter().rposition(|&x| x < p) } else { all.iter().position(|&x| x > p) };
        let (i, wrapped) = match found {
            Some(i) => (i, false),
            None => (if back { all.len() - 1 } else { 0 }, true),
        };
        t.move_to(all[i], false);
        if wrapped {
            if back { "search hit TOP, continuing at BOTTOM" } else { "search hit BOTTOM, continuing at TOP" }.into()
        } else {
            String::new()
        }
    }

    // ---- messages -----------------------------------------------------------
    fn show(&self, a: i64, b: i64, nums: bool, list: bool) -> String {
        let mut out = vec![];
        let mut i = a;
        while i <= b && out.len() < 200 {
            let l = &self.ln[i as usize].s;
            let s = if list { l.replace('\t', "^I") + "$" } else { l.clone() };
            out.push(if nums { format!("{:>3} {s}", i + 1) } else { s });
            i += 1;
        }
        out.join("\n")
    }

    // ---- running one ---------------------------------------------------------
    /// One command line (without the vi keys: :normal isn't there).
    pub fn run(&mut self, line: &str, t: &mut Text, ind: &Indent) -> ExOut {
        self.run_vi(line, t, ind, None)
    }
    /// One command line, with the vi keys for :normal.
    pub fn run_vi(&mut self, line: &str, t: &mut Text, ind: &Indent, vi: Option<&mut Vi>) -> ExOut {
        self.error = false;
        self.actions.clear();
        let line = line.trim_start_matches(js_space).trim_start_matches(':');
        if js_trim(line).is_empty() {
            return ExOut::default();
        }
        if self.history.last().map(String::as_str) != Some(line) {
            self.history.push(line.to_string());
            if self.history.len() > 100 {
                self.history.remove(0);
            }
        }
        self.load(t);
        let cur = Self::cursor_line(t);
        let mut cx = Cx { t, ind, vi };
        let r = self.command(&mut cx, line, cur, false);
        let actions = std::mem::take(&mut self.actions);
        match r {
            Ok(s) => ExOut { text: s, error: false, actions },
            Err(e) => {
                self.error = true;
                ExOut { text: e, error: true, actions }
            }
        }
    }

    /// A command line on the lines; in_g: inside :g (no cursor moves, no history).
    fn command(&mut self, cx: &mut Cx, line: &str, cur: i64, in_g: bool) -> R<String> {
        let r = self.range(cx.t, line, cur)?;
        let rest = r.rest.trim_start_matches(js_space).to_string();
        let n_lines = self.ln.len() as i64;
        // (A range alone: go to its last line.)
        if rest.is_empty() {
            if !r.given {
                return Ok(String::new());
            }
            let n = r.b.clamp(0, n_lines - 1);
            if !in_g {
                self.store(cx.t, Some(n), None);
            }
            return Ok(String::new());
        }
        // The name, then a !, then its arguments.
        let nlen = {
            let first = rest.chars().next().unwrap();
            if first.is_ascii_alphabetic() {
                rest.find(|c: char| !c.is_ascii_alphabetic()).unwrap_or(rest.len())
            } else if "&<>=#!~".contains(first) {
                rest.find(|c: char| !"&<>=#!~".contains(c)).unwrap_or(rest.len())
            } else {
                0
            }
        };
        let mut name = rest[..nlen].to_string();
        let mut after = rest[nlen..].to_string();
        // (:s and :g take their delimiter right after the name: :s/a/b/, :g#x#d.)
        let sflags = |n: &str| n.len() > 1 && n.starts_with('s') && n[1..].chars().all(|c| "gciIn&".contains(c));
        let is_sub = matches!(name.as_str(), "s" | "su" | "sub" | "subs" | "subst" | "substi" | "substit" | "substitu" | "substitut" | "substitute");
        if is_sub || (sflags(&name) && after.chars().next().is_some_and(|c| !js_word(c) && !js_space(c))) {
            if sflags(&name) {
                after = format!("//~/{}{}", &name[1..], after);
                name = "s".into();
            }
            let _ = name;
            return self.substitute(cx, &r, after.trim_start_matches(js_space), in_g);
        }
        if name == "&" || name == "&&" {
            let a = format!("&{}{}", if name == "&&" { "&" } else { "" }, js_trim(&after));
            return self.substitute(cx, &r, &a, in_g);
        }
        if name == "~" {
            let a = format!("//~/{}", js_trim(&after));
            return self.substitute(cx, &r, &a, in_g);
        }
        if matches!(name.as_str(), "g" | "gl" | "glo" | "glob" | "globa" | "global" | "v" | "vg" | "vgl" | "vglo" | "vglob" | "vgloba" | "vglobal") {
            let mut invert = name.starts_with('v');
            if let Some(x) = after.strip_prefix('!') {
                invert = true;
                after = x.to_string();
            }
            return self.global(cx, &r, after.trim_start_matches(js_space), invert, in_g);
        }
        let bang = after.starts_with('!');
        if bang {
            after = after[1..].to_string();
        }
        let arg = js_trim(&after).to_string();
        let is = |abbrev: &str, full: &str| name.len() >= abbrev.len() && full.starts_with(name.as_str()) && name.starts_with(abbrev);
        let cl = r.b;

        if (name.starts_with('>') && name.chars().all(|c| c == '>')) || (name.starts_with('<') && name.chars().all(|c| c == '<')) {
            self.check(r.a, r.b)?;
            let dir = name.starts_with('>');
            let times = name.len();
            let (mut a, mut b) = (r.a, r.b);
            if digits(&arg) {
                a = r.b;
                b = (n_lines - 1).min(r.b + num(&arg) - 1);
            }
            let unit = self.unit(cx.ind);
            for i in a..=b {
                let l = &mut self.ln[i as usize].s;
                for _ in 0..times {
                    *l = if dir {
                        if l.is_empty() {
                            String::new()
                        } else {
                            unit.clone() + l
                        }
                    } else {
                        outdent(l, unit.chars().count())
                    };
                }
            }
            if !in_g {
                self.store(cx.t, Some(b), None);
            }
            return Ok(String::new());
        }
        if name == "=" {
            return Ok(((if r.given { r.b } else { n_lines - 1 }) + 1).to_string());
        }
        if name == "#" || is("nu", "number") {
            self.check(r.a, r.b)?;
            return Ok(self.show(r.a, r.b, true, false));
        }
        if is("p", "print") || is("P", "Print") {
            self.check(r.a, r.b)?;
            return Ok(self.show(r.a, r.b, false, false));
        }
        if is("l", "list") {
            self.check(r.a, r.b)?;
            return Ok(self.show(r.a, r.b, false, true));
        }
        // (:d and :y's argument: [x] [count].)
        let reg_count = |arg: &str| -> Option<(char, Option<i64>)> {
            let mut it = arg.chars().peekable();
            let mut reg = '\0';
            if let Some(&c) = it.peek() {
                if c.is_ascii_alphabetic() || c == '"' {
                    reg = c;
                    it.next();
                }
            }
            let rest: String = it.collect();
            let rest = rest.trim_start_matches(js_space);
            if rest.is_empty() {
                Some((reg, None))
            } else if digits(rest) {
                Some((reg, Some(num(rest))))
            } else {
                None
            }
        };
        if is("d", "delete") {
            self.check(r.a, r.b)?;
            let (mut a, mut b, mut reg) = (r.a, r.b, '\0');
            if let Some((rg, cnt)) = reg_count(&arg) {
                reg = rg;
                if let Some(c) = cnt {
                    a = r.b;
                    b = (n_lines - 1).min(r.b + c - 1);
                }
            }
            let cut = self.ln[a as usize..=b as usize].iter().map(|o| o.s.as_str()).collect::<Vec<_>>().join("\n") + "\n";
            self.put_register(cx.t, reg, &cut);
            self.ln.drain(a as usize..=b as usize);
            if self.ln.is_empty() {
                let l = self.mk(String::new());
                self.ln.push(l);
            }
            if !in_g {
                let at = a.min(self.ln.len() as i64 - 1);
                self.store(cx.t, Some(at), None);
            }
            return Ok(if b - a + 1 > 2 && !in_g { lines((b - a + 1) as usize) + " fewer" } else { String::new() });
        }
        if is("y", "yank") {
            self.check(r.a, r.b)?;
            let (mut a, mut b, mut reg) = (r.a, r.b, '\0');
            if let Some((rg, cnt)) = reg_count(&arg) {
                reg = rg;
                if let Some(c) = cnt {
                    a = r.b;
                    b = (n_lines - 1).min(r.b + c - 1);
                }
            }
            let text = self.ln[a as usize..=b as usize].iter().map(|o| o.s.as_str()).collect::<Vec<_>>().join("\n") + "\n";
            self.put_register(cx.t, reg, &text);
            return Ok(if b - a + 1 > 2 { lines((b - a + 1) as usize) + " yanked" } else { String::new() });
        }
        if is("pu", "put") {
            let reg = if arg.chars().count() == 1 && arg.chars().all(|c| c.is_ascii_alphabetic() || c == '"') { arg.chars().next() } else { None };
            let text = match reg {
                Some(r) if r != '"' => self.registers.get(&r.to_ascii_lowercase()).cloned().unwrap_or_default(),
                _ => cx.t.kill_ring.clone(),
            };
            let body = text.strip_suffix('\n').unwrap_or(&text).to_string();
            let add: Vec<Ln> = body.split('\n').map(String::from).collect::<Vec<_>>().into_iter().map(|s| self.mk(s)).collect();
            let at = (if bang { r.b } else { r.b + 1 }).clamp(0, self.ln.len() as i64) as usize;
            let n = add.len();
            self.ln.splice(at..at, add);
            if !in_g {
                self.store(cx.t, Some((at + n) as i64 - 1), None);
            }
            return Ok(String::new());
        }
        if is("m", "move") || is("t", "t") || is("co", "copy") {
            self.check(r.a, r.b)?;
            let copy = (name != "m" && !"move".starts_with(name.as_str())) || name == "t";
            let dest = if arg == "0" { -1 } else { self.range(cx.t, &arg, cur)?.b };
            if dest >= self.ln.len() as i64 {
                return Err("E16: Invalid range".into());
            }
            let dest = dest.max(-1);
            let (a, b) = (r.a as usize, r.b as usize);
            if copy {
                let texts: Vec<String> = self.ln[a..=b].iter().map(|o| o.s.clone()).collect();
                let add: Vec<Ln> = texts.into_iter().map(|s| self.mk(s)).collect();
                let n = add.len();
                let at = (dest + 1) as usize;
                self.ln.splice(at..at, add);
                if !in_g {
                    self.store(cx.t, Some(dest + n as i64), None);
                }
            } else {
                if dest >= r.a && dest < r.b {
                    return Err("E134: Cannot move a range of lines into itself".into());
                }
                let block: Vec<Ln> = self.ln.drain(a..=b).collect();
                let n = block.len() as i64;
                let at = if dest >= r.b { dest - n + 1 } else { dest + 1 };
                let at = at.clamp(0, self.ln.len() as i64) as usize;
                self.ln.splice(at..at, block);
                if !in_g {
                    self.store(cx.t, Some(at as i64 + n - 1), None);
                }
            }
            return Ok(String::new());
        }
        if is("j", "join") {
            self.check(r.a, r.b)?;
            let a = r.a;
            let mut b = if r.given && r.a != r.b { r.b } else { r.b + 1 };
            if digits(&arg) {
                b = r.b + num(&arg) - 1;
            }
            b = b.min(n_lines - 1);
            if b <= a {
                return Ok(String::new());
            }
            let mut s = self.ln[a as usize].s.clone();
            for i in a + 1..=b {
                let l = &self.ln[i as usize].s;
                let next = if bang { l.as_str() } else { l.trim_start_matches(js_space) };
                if bang || s.is_empty() || next.is_empty() || s.ends_with(js_space) {
                    s += next;
                } else {
                    s += if next.starts_with(')') { "" } else { " " };
                    s += next;
                }
            }
            let len = s.chars().count();
            self.ln[a as usize].s = s;
            self.ln.drain(a as usize + 1..=b as usize);
            if !in_g {
                self.store(cx.t, Some(a), Some(len));
            }
            return Ok(String::new());
        }
        if is("norm", "normal") {
            self.check(r.a, r.b)?;
            if cx.vi.is_none() {
                return Err("Not here: :normal".into());
            }
            if arg.is_empty() {
                return Err("E471: Argument required".into());
            }
            let keys = after.strip_prefix(' ').unwrap_or(&after).to_string();
            self.normal(cx, r.a as usize, r.b as usize, &keys);
            return Ok(String::new());
        }
        if is("sor", "sort") {
            let (a, b) = if r.given { (r.a, r.b) } else { (0, n_lines - 1) };
            self.check(a, b)?;
            let mut opts = arg.clone();
            let mut pat: Option<Re> = None;
            // (/pat/: the first / with a closing one.)
            if let Some(st) = opts.find('/') {
                let cs: Vec<char> = opts[st + 1..].chars().collect();
                let mut i = 0;
                let mut close = None;
                while i < cs.len() {
                    if cs[i] == '\\' && i + 1 < cs.len() {
                        i += 2;
                        continue;
                    }
                    if cs[i] == '/' {
                        close = Some(i);
                        break;
                    }
                    i += 1;
                }
                if let Some(c) = close {
                    let inner: String = cs[..c].iter().collect();
                    let whole = format!("/{inner}/");
                    pat = Some(self.regex(if inner.is_empty() { &self.last_search } else { &inner }, "")?);
                    opts = opts.replacen(&whole, "", 1);
                }
            }
            let ic = opts.contains('i');
            let numeric = opts.chars().any(|c| "nfxob".contains(c));
            let uniq = opts.contains('u');
            let use_match = opts.contains('r');
            let hex = opts.contains('x');
            let key = |s: &str| -> String {
                let mut k = s.to_string();
                if let Some(p) = &pat {
                    k = match p.exec_at(s, 0) {
                        Some(c) => {
                            let (x, y) = c[0].unwrap();
                            if use_match { s[x..y].to_string() } else { s[y..].to_string() }
                        }
                        None => {
                            if use_match {
                                String::new()
                            } else {
                                s.to_string()
                            }
                        }
                    };
                }
                if ic {
                    k.to_lowercase()
                } else {
                    k
                }
            };
            let num_of = |s: &str| -> f64 {
                let b = s.as_bytes();
                for i in 0..b.len() {
                    let mut j = i;
                    let neg = b[j] == b'-';
                    if neg {
                        j += 1;
                    }
                    if hex {
                        let mut k = j;
                        if b.get(k) == Some(&b'0') && matches!(b.get(k + 1), Some(b'x' | b'X')) && b.get(k + 2).is_some_and(|c| c.is_ascii_hexdigit()) {
                            k += 2;
                        }
                        let st = k;
                        while b.get(k).is_some_and(|c| c.is_ascii_hexdigit()) {
                            k += 1;
                        }
                        if k > st {
                            let v = i128::from_str_radix(&s[st..k], 16).map(|v| v as f64).unwrap_or(f64::INFINITY);
                            return if neg { -v } else { v };
                        }
                    } else {
                        let st = j;
                        let mut k = j;
                        while b.get(k).is_some_and(|c| c.is_ascii_digit()) {
                            k += 1;
                        }
                        if k > st {
                            if b.get(k) == Some(&b'.') && b.get(k + 1).is_some_and(|c| c.is_ascii_digit()) {
                                k += 1;
                                while b.get(k).is_some_and(|c| c.is_ascii_digit()) {
                                    k += 1;
                                }
                            }
                            return s[i..k].parse::<f64>().unwrap_or(0.0);
                        }
                    }
                }
                f64::NEG_INFINITY
            };
            let mut part: Vec<Ln> = self.ln.drain(a as usize..=b as usize).collect();
            part.sort_by(|x, y| {
                let (kx, ky) = (key(&x.s), key(&y.s));
                if numeric {
                    (num_of(&kx) - num_of(&ky)).partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal)
                } else {
                    kx.cmp(&ky)
                }
            });
            if bang {
                part.reverse();
            }
            if uniq {
                let mut out: Vec<Ln> = vec![];
                for o in part {
                    if out.last().is_none_or(|l| key(&l.s) != key(&o.s)) {
                        out.push(o);
                    }
                }
                part = out;
            }
            let at = a as usize;
            self.ln.splice(at..at, part);
            if !in_g {
                self.store(cx.t, Some(a), None);
            }
            return Ok(String::new());
        }
        if is("ret", "retab") {
            let (a, b) = if r.given { (r.a, r.b) } else { (0, n_lines - 1) };
            self.check(a, b)?;
            let ts = if digits(&arg) { num(&arg).max(1) as usize } else { self.tabstop };
            for i in a..=b {
                let mut out = String::new();
                let mut col = 0;
                for c in self.ln[i as usize].s.chars() {
                    if c == '\t' {
                        let n = ts - (col % ts);
                        out.push_str(&" ".repeat(n));
                        col += n;
                    } else {
                        out.push(c);
                        col += 1;
                    }
                }
                self.ln[i as usize].s = out;
            }
            if digits(&arg) {
                self.tabstop = ts;
            }
            if !in_g {
                self.store(cx.t, Some(cl), None);
            }
            return Ok(String::new());
        }
        if is("le", "left") || is("ri", "right") || is("ce", "center") {
            self.check(r.a, r.b)?;
            let which = name.chars().next().unwrap();
            let w = if digits(&arg) { num(&arg) } else if which == 'l' { 0 } else { 80 };
            for i in r.a..=r.b {
                let s = js_trim(&self.ln[i as usize].s).to_string();
                let len = s.chars().count() as i64;
                let pad = match which {
                    'l' => w,
                    'r' => (w - len).max(0),
                    _ => ((w - len) / 2).max(0),
                } as usize;
                self.ln[i as usize].s = " ".repeat(pad) + &s;
            }
            if !in_g {
                self.store(cx.t, Some(r.b), None);
            }
            return Ok(String::new());
        }
        if is("ma", "mark") || name == "k" || (name.len() == 2 && name.starts_with('k') && name.as_bytes()[1].is_ascii_lowercase()) {
            let k = if name.len() == 2 && name.starts_with('k') { name[1..].to_string() } else { arg.clone() };
            let mut ks = k.chars();
            let (Some(c), None) = (ks.next(), ks.next()) else { return Err("E191: Argument must be a letter".into()) };
            if !(c.is_ascii_alphabetic() || c == '\'' || c == '`') {
                return Err("E191: Argument must be a letter".into());
            }
            self.marks.insert(c, r.b.max(0) as usize);
            return Ok(String::new());
        }
        if is("delm", "delmarks") {
            if bang {
                self.marks.clear();
                return Ok(String::new());
            }
            for part in arg.split(js_space).filter(|p| !p.is_empty()) {
                let b = part.as_bytes();
                if b.len() == 3 && b[1] == b'-' && b[0].is_ascii_lowercase() && b[2].is_ascii_lowercase() {
                    for c in b[0]..=b[2] {
                        self.marks.remove(&(c as char));
                    }
                } else {
                    for c in part.chars() {
                        self.marks.remove(&c);
                    }
                }
            }
            return Ok(String::new());
        }
        if name == "marks" {
            let mut out = vec!["mark line  col text".to_string()];
            let mut ks: Vec<char> = self.marks.keys().copied().collect();
            ks.sort();
            for k in ks {
                let n = self.mark_line(cx.t, k);
                if n >= 0 {
                    out.push(format!(" {k}  {:>5}    0 {}", n + 1, js_trim(&self.ln[n as usize].s)));
                }
            }
            return Ok(out.join("\n"));
        }
        if is("reg", "registers") || is("di", "display") {
            let mut out = vec!["Type Name Content".to_string()];
            let fmt = |s: &str| s.replace('\n', "^J").chars().take(70).collect::<String>();
            if !cx.t.kill_ring.is_empty() {
                out.push(format!("  l  \"\"   {}", fmt(&cx.t.kill_ring)));
            }
            let mut ks: Vec<char> = self.registers.keys().copied().collect();
            ks.sort();
            for k in ks {
                if arg.is_empty() || arg.contains(k) {
                    out.push(format!("  l  \"{k}   {}", fmt(&self.registers[&k])));
                }
            }
            return Ok(out.join("\n"));
        }
        if is("f", "file") {
            let n = Self::cursor_line(cx.t);
            let len = self.ln.len().max(1) as f64;
            let pct = ((n + 1) as f64 * 100.0 / len + 0.5).floor() as i64;
            let name = if self.file_name.is_empty() { "[No Name]" } else { &self.file_name };
            let ls = if cx.t.text.is_empty() { String::new() } else { format!(" {}", lines(self.ln.len())) };
            return Ok(format!("\"{name}\"{ls} --{pct}%--"));
        }
        if is("pw", "pwd") {
            return Ok(self.folder.clone());
        }
        if is("ec", "echo") {
            let cs: Vec<char> = arg.chars().collect();
            if cs.len() >= 2 && (cs[0] == '"' || cs[0] == '\'') && cs[cs.len() - 1] == cs[0] {
                return Ok(cs[1..cs.len() - 1].iter().collect());
            }
            return Ok(arg);
        }
        if is("u", "undo") {
            cx.t.undo();
            return Ok(String::new());
        }
        if is("red", "redo") {
            cx.t.redo();
            return Ok(String::new());
        }
        if is("noh", "nohlsearch") {
            self.actions.push(ExAction::SetOption("hlsearch-off".into(), "true".into()));
            return Ok(String::new());
        }
        if is("se", "set") {
            return self.set_options(&arg, cx.ind);
        }
        if is("sav", "saveas") {
            if arg.is_empty() {
                return Err("E471: Argument required".into());
            }
            let p = self.path(&arg);
            self.actions.push(ExAction::Write(p, bang));
            return Ok(String::new());
        }
        if is("w", "write") || is("up", "update") {
            if arg.starts_with('!') {
                return Err("Not here: :w !{cmd}".into());
            }
            let p = if arg.is_empty() { String::new() } else { self.path(&arg) };
            self.actions.push(ExAction::Write(p, bang));
            return Ok(String::new());
        }
        if name == "wqa" || is("wqa", "wqall") || is("xa", "xall") {
            self.actions.push(ExAction::WriteAll(bang));
            self.actions.push(ExAction::QuitAll(bang));
            return Ok(String::new());
        }
        if name == "wq" || is("x", "xit") || is("exi", "exit") {
            let p = if arg.is_empty() { String::new() } else { self.path(&arg) };
            self.actions.push(ExAction::Write(p, bang));
            self.actions.push(ExAction::Quit(bang));
            return Ok(String::new());
        }
        if is("wa", "wall") {
            self.actions.push(ExAction::WriteAll(bang));
            return Ok(String::new());
        }
        if is("qa", "qall") || is("quita", "quitall") {
            self.actions.push(ExAction::QuitAll(bang));
            return Ok(String::new());
        }
        if is("q", "quit") || is("clo", "close") {
            self.actions.push(ExAction::Quit(bang));
            return Ok(String::new());
        }
        // Windows and buffers (Turbine has several of each).
        let window = if is("sp", "split") || is("new", "new") {
            Some(if name.starts_with('n') { "new" } else { "split" })
        } else if is("vs", "vsplit") || is("vne", "vnew") {
            Some(if name.starts_with("vn") { "vnew" } else { "vsplit" })
        } else if is("on", "only") {
            Some("only")
        } else if is("bn", "bnext") || is("tabn", "tabnext") {
            Some("bnext")
        } else if is("bp", "bprevious") || is("bN", "bNext") || is("tabp", "tabprevious") {
            Some("bprev")
        } else if is("tabe", "tabedit") || is("tabnew", "tabnew") {
            Some("new")
        } else {
            None
        };
        if let Some(w) = window {
            if (w == "split" || w == "vsplit") && !arg.is_empty() {
                let p = self.path(&arg);
                self.actions.push(ExAction::Window(w.into()));
                self.actions.push(ExAction::EditFile(p, bang));
            } else {
                self.actions.push(ExAction::Window(w.into()));
            }
            return Ok(String::new());
        }
        if is("e", "edit") || name == "ex" {
            let p = if arg.is_empty() { String::new() } else { self.path(&arg) };
            self.actions.push(ExAction::EditFile(p, bang));
            return Ok(String::new());
        }
        if is("ene", "enew") {
            self.actions.push(ExAction::Window("enew".into()));
            return Ok(String::new());
        }
        if is("r", "read") {
            if arg.starts_with('!') {
                return Err("Not here: :r !{cmd}".into());
            }
            let Some(rf) = &self.read_file else { return Err("Not here: :read".into()) };
            let p = if arg.is_empty() { String::new() } else { self.path(&arg) };
            let text = rf(&p)?.replace("\r\n", "\n");
            let body = text.strip_suffix('\n').unwrap_or(&text).to_string();
            let add: Vec<Ln> = body.split('\n').map(String::from).collect::<Vec<_>>().into_iter().map(|s| self.mk(s)).collect();
            let at = (if r.given { r.b + 1 } else { cur + 1 }).clamp(0, self.ln.len() as i64) as usize;
            self.ln.splice(at..at, add);
            if !in_g {
                self.store(cx.t, Some(at as i64), None);
            }
            return Ok(String::new());
        }
        if name == "!" || (bang && name.is_empty()) {
            return Err("Not here: :!{cmd} (run it in the terminal)".into());
        }
        if matches!(
            name.as_str(),
            "sp" | "split" | "vs" | "vsplit" | "new" | "vne" | "vnew" | "tabe" | "tabedit" | "tabnew" | "tabc" | "tabclose" | "tabn" | "tabnext" | "tabp" | "tabprevious" | "b" | "buffer" | "bn" | "bnext" | "bp" | "bprevious" | "bd" | "bdelete" | "on" | "only" | "te" | "terminal" | "sh" | "shell"
        ) {
            return Err(format!("Not here: :{name} (a file card shows one file)"));
        }
        Err(format!("E492: Not an editor command: {}", js_trim(line)))
    }

    fn path(&self, p: &str) -> String {
        let mut p = p.to_string();
        if p == "~" || p.starts_with("~/") {
            // (~: the home folder the file's folder is in.)
            let f = &self.folder;
            let home = ["/home/", "/Users/"]
                .iter()
                .find_map(|h| f.strip_prefix(h).map(|r| format!("{h}{}", r.split('/').next().unwrap_or(""))))
                .or_else(|| (f == "/root" || f.starts_with("/root/")).then(|| "/root".to_string()))
                .unwrap_or_else(|| f.clone());
            p = home + &p[1..];
        }
        if p.starts_with('/') || self.folder.is_empty() {
            p
        } else {
            format!("{}/{p}", self.folder.trim_end_matches('/'))
        }
    }

    /// Into a register: "_" nowhere, A-Z appends to a-z; every one also the unnamed.
    pub fn put_register(&mut self, t: &mut Text, reg: char, text: &str) {
        if reg == '_' {
            return;
        }
        if reg != '\0' && reg != '"' {
            let k = reg.to_ascii_lowercase();
            let e = self.registers.entry(k).or_default();
            if reg != k {
                e.push_str(text); // (A-Z appends.)
            } else {
                *e = text.to_string();
            }
        }
        t.kill_ring = text.to_string();
    }

    /// :s/pat/rep/flags [count], the range's lines.
    fn substitute(&mut self, cx: &mut Cx, r: &Rg, args: &str, in_g: bool) -> R<String> {
        let (pat, rep, mut flags);
        let mut count = String::new();
        let only_flags = args.chars().all(|c| "gciIn&".contains(c) || c.is_ascii_digit() || js_space(c));
        if let Some(rest) = args.strip_prefix('&') {
            let Some(ls) = &self.last_sub else { return Err("E35: No previous regular expression".into()) };
            pat = ls.pat.clone();
            rep = ls.rep.clone();
            flags = (if rest.starts_with('&') { ls.flags.clone() } else { String::new() }) + args.trim_start_matches('&');
        } else if args.is_empty() || only_flags {
            let Some(ls) = &self.last_sub else { return Err("E35: No previous regular expression".into()) };
            pat = ls.pat.clone();
            rep = ls.rep.clone();
            flags = args.to_string();
        } else {
            let cs: Vec<char> = args.chars().collect();
            let d = cs[0];
            let (p, i) = delimited(&cs, 1, d);
            let (rp, j) = delimited(&cs, i, d);
            let last_rep = self.last_sub.as_ref().map(|s| s.rep.clone());
            pat = if !p.is_empty() {
                p
            } else if !self.last_search.is_empty() {
                self.last_search.clone()
            } else {
                self.last_sub.as_ref().map(|s| s.pat.clone()).unwrap_or_default()
            };
            rep = if rp == "~" && last_rep.is_some() {
                last_rep.unwrap()
            } else {
                // (~ not after a backslash: the last replacement.)
                let rc: Vec<char> = rp.chars().collect();
                let mut o = String::new();
                for (k, &c) in rc.iter().enumerate() {
                    if c == '~' && (k == 0 || rc[k - 1] != '\\') {
                        o.push_str(last_rep.as_deref().unwrap_or(""));
                    } else {
                        o.push(c);
                    }
                }
                o
            };
            flags = cs[j.min(cs.len())..].iter().collect();
        }
        if pat.is_empty() {
            return Err("E35: No previous regular expression".into());
        }
        {
            // ^\s*([&gciIn]*)\s*(\d*)\s*$
            let f = flags.trim_start_matches(js_space);
            let fl: String = f.chars().take_while(|c| "&gciIn".contains(*c)).collect();
            let rest = f[fl.len()..].trim_start_matches(js_space);
            let dg: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if rest[dg.len()..].trim_start_matches(js_space).is_empty() {
                flags = fl;
                count = dg;
            }
        }
        self.last_sub = Some(Sub { pat: pat.clone(), rep: rep.clone(), flags: flags.replace('&', "") });
        self.last_search = pat.clone();
        let (mut a, mut b) = (r.a, r.b);
        if !count.is_empty() {
            a = r.b;
            b = (self.ln.len() as i64 - 1).min(r.b + num(&count) - 1);
        }
        self.check(a, b)?;
        let re = self.regex(&pat, &flags)?;
        let all = flags.contains('g');
        let count_only = flags.contains('n');
        let (mut n, mut lines_hit, mut last) = (0, 0, a);
        // (A pattern with \n works on the joined lines.)
        if pat.contains("\\n") {
            let block = self.ln[a as usize..=b as usize].iter().map(|o| o.s.as_str()).collect::<Vec<_>>().join("\n");
            let (out, hits) = re.replace(&block, all, |m| replacement(&rep, m));
            n = hits;
            lines_hit = if hits > 0 { 1 } else { 0 };
            if !count_only && hits > 0 {
                let add: Vec<Ln> = out.split('\n').map(String::from).collect::<Vec<_>>().into_iter().map(|s| self.mk(s)).collect();
                self.ln.splice(a as usize..=b as usize, add);
            }
        } else {
            let mut i = a;
            while i <= b {
                let s = self.ln[i as usize].s.clone();
                let (out, hits) = re.replace(&s, all, |m| replacement(&rep, m));
                if hits > 0 {
                    n += hits;
                    lines_hit += 1;
                    last = i;
                    if !count_only {
                        let parts: Vec<String> = out.split('\n').map(String::from).collect();
                        self.ln[i as usize].s = parts[0].clone();
                        if parts.len() > 1 {
                            let more = parts.len() - 1;
                            let add: Vec<Ln> = parts.into_iter().skip(1).map(|x| self.mk(x)).collect();
                            let at = i as usize + 1;
                            self.ln.splice(at..at, add);
                            b += more as i64;
                            i += more as i64;
                        }
                    }
                }
                i += 1;
            }
        }
        if n == 0 {
            if in_g {
                return Ok(String::new());
            }
            return Err(format!("E486: Pattern not found: {pat}"));
        }
        if count_only {
            return Ok(format!("{n}{} on {}", if n == 1 { " match" } else { " matches" }, lines(lines_hit)));
        }
        if !in_g {
            self.store(cx.t, Some(last), None);
        }
        Ok(if lines_hit > 1 && !in_g { format!("{n} substitution{} on {}", if n == 1 { "" } else { "s" }, lines(lines_hit)) } else { String::new() })
    }

    /// :g/pat/cmd (each matching line, cmd :p if none), :v and :g! the others.
    fn global(&mut self, cx: &mut Cx, r: &Rg, args: &str, invert: bool, in_g: bool) -> R<String> {
        if in_g {
            return Err("E147: Cannot do :global recursive".into());
        }
        let cs: Vec<char> = args.chars().collect();
        let Some(&d) = cs.first() else { return Err("E476: Invalid command".into()) };
        let (p, i) = delimited(&cs, 1, d);
        let pat = if p.is_empty() { self.last_search.clone() } else { p };
        if !pat.is_empty() {
            self.last_search = pat.clone();
        }
        let rest: String = cs[i.min(cs.len())..].iter().collect();
        let cmd = if js_trim(&rest).is_empty() { "p".to_string() } else { js_trim(&rest).to_string() };
        let re = self.regex(&pat, "")?;
        let (a, b) = if r.given { (r.a, r.b) } else { (0, self.ln.len() as i64 - 1) };
        self.check(a, b)?;
        let hit: Vec<u64> = self.ln[a as usize..=b as usize].iter().filter(|o| re.test(&o.s) != invert).map(|o| o.id).collect();
        if hit.is_empty() {
            return Err(format!("E486: Pattern not found: {pat}"));
        }
        let mut shown = vec![];
        let mut last_line: i64 = -1;
        let find = |ln: &[Ln], id: u64| ln.iter().position(|o| o.id == id);
        for id in hit {
            let Some(n) = find(&self.ln, id) else { continue }; // (deleted by a command on an earlier one)
            // (Commands run one after another: :g/x/s/a/b/|s/c/d/)
            for one in split_bar(&cmd) {
                let out = self.command(cx, &one, n as i64, true)?;
                if !out.is_empty() {
                    shown.push(out);
                }
            }
            last_line = match find(&self.ln, id) {
                Some(k) => k as i64,
                None => (n as i64).min(self.ln.len() as i64 - 1),
            };
        }
        self.store(cx.t, Some(last_line.max(0)), None);
        Ok(shown.join("\n"))
    }

    // ---- completion: the commands, with what each does ------------------------
    /// The ones the command line's word could be, best first.
    pub fn completions(&self, line: &str) -> Vec<Completion> {
        // (Only while typing the command's name, after any range: past it,
        // its arguments, there's nothing to offer.)
        let cs: Vec<char> = line.chars().collect();
        let mut i = 0;
        loop {
            match cs.get(i) {
                Some(&c) if js_space(c) || c.is_ascii_digit() || ".$%,;+-:".contains(c) => i += 1,
                Some('\'') if cs.get(i + 1).is_some_and(|c| c.is_ascii_alphabetic() || *c == '<' || *c == '>') => i += 2,
                _ => break,
            }
        }
        let head: String = cs[..i].iter().collect();
        let word: String = cs[i..].iter().collect();
        if !(word.chars().all(|c| c.is_ascii_alphabetic()) || word.chars().all(|c| "<>=#&~".contains(c))) {
            return vec![];
        }
        let w = word.to_lowercase();
        let mut out = vec![];
        for &(name, short, help) in COMMAND_LIST {
            let help = match self.help_for.get(name) {
                Some(None) => continue,
                Some(Some(h)) => *h,
                None => help,
            };
            if name.starts_with(&w) || (!w.is_empty() && short.starts_with(&w)) {
                out.push(Completion { name: name.into(), short: short.into(), help: help.into(), head: head.clone() });
            }
        }
        // (Exact abbreviations first: s is substitute, d delete.)
        out.sort_by_key(|c| (c.short != w, !c.name.starts_with(&w)));
        out
    }

    /// :normal {keys} on each line of the range, the keys in vi's normal mode.
    fn normal(&mut self, cx: &mut Cx, a: usize, b: usize, keys: &str) {
        self.store(cx.t, None, None);
        // (The lines, found again by their text as the keys change them.)
        let objs: Vec<String> = self.ln[a..=b].iter().map(|o| o.s.clone()).collect();
        for o in objs {
            self.load(cx.t);
            let Some(n) = self.ln.iter().position(|x| x.s == o) else { continue };
            self.store(cx.t, Some(n as i64), Some(0));
            if let Some(vi) = cx.vi.as_deref_mut() {
                vi.set_normal(true, cx.t);
                vi.clear_pending();
            }
            for c in keys.chars() {
                let ev = if c == '\x1b' { KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE) } else { KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE) };
                let done = match cx.vi.as_deref_mut() {
                    Some(vi) => vi.key(&ev, cx.t, cx.ind, self) != ViOut::None,
                    None => false,
                };
                if !done && c != '\x1b' {
                    let p = cx.t.cursor;
                    cx.t.replace(p, p, &c.to_string());
                }
            }
            if let Some(vi) = cx.vi.as_deref_mut() {
                vi.set_normal(true, cx.t);
                vi.clear_pending();
            }
        }
        self.load(cx.t);
    }

    /// :set name, noname, name!, name?, name=value (and :set alone: what's set).
    fn set_options(&mut self, arg: &str, ind: &Indent) -> R<String> {
        let unit = self.unit(ind);
        if arg.is_empty() || arg == "all" {
            return Ok(format!(
                "ignorecase={} smartcase={} shiftwidth={} tabstop={} expandtab={}",
                self.ignore_case,
                self.smart_case,
                unit.chars().count(),
                self.tabstop,
                unit != "\t"
            ));
        }
        let mut out = vec![];
        for part in arg.split(js_space).filter(|p| !p.is_empty()) {
            // ^(no|inv)?([a-z]+)(!|\?|[=:](.*))?$
            let (pre, body) = if let Some(b) = part.strip_prefix("no").filter(|b| b.starts_with(|c: char| c.is_ascii_lowercase())) {
                ("no", b)
            } else if let Some(b) = part.strip_prefix("inv").filter(|b| b.starts_with(|c: char| c.is_ascii_lowercase())) {
                ("inv", b)
            } else {
                ("", part)
            };
            let nl = body.find(|c: char| !c.is_ascii_lowercase()).unwrap_or(body.len());
            let (raw, tail) = body.split_at(nl);
            if raw.is_empty() || !(tail.is_empty() || tail == "!" || tail == "?" || tail.starts_with('=') || tail.starts_with(':')) {
                return Err(format!("E518: Unknown option: {part}"));
            }
            let neg = pre == "no";
            let inv = pre == "inv" || tail == "!";
            let ask = tail == "?";
            let name = match raw {
                "ic" => "ignorecase",
                "scs" => "smartcase",
                "sw" => "shiftwidth",
                "ts" => "tabstop",
                "et" => "expandtab",
                "sts" => "softtabstop",
                "nu" => "number",
                "hls" => "hlsearch",
                "is" => "incsearch",
                n => n,
            };
            let val = if tail.starts_with('=') || tail.starts_with(':') { Some(&tail[1..]) } else { None };
            let flag = |cur: bool| if inv { !cur } else { !neg };
            let cur_unit = self.unit(ind);
            match name {
                "ignorecase" => {
                    if ask {
                        out.push(format!("{}{name}", if self.ignore_case { "" } else { "no" }));
                    } else {
                        self.ignore_case = flag(self.ignore_case);
                    }
                }
                "smartcase" => {
                    if ask {
                        out.push(format!("{}{name}", if self.smart_case { "" } else { "no" }));
                    } else {
                        self.smart_case = flag(self.smart_case);
                    }
                }
                "shiftwidth" | "softtabstop" => match val {
                    Some(v) if !ask => {
                        let n = v.parse::<f64>().unwrap_or(0.0);
                        if n.is_nan() || n <= 0.0 {
                            return Err("E487: Argument must be positive".into());
                        }
                        self.indent_unit = Some(if cur_unit == "\t" { "\t".into() } else { " ".repeat(n as usize) });
                        self.actions.push(ExAction::SetOption("shiftwidth".into(), (n as usize).to_string()));
                    }
                    _ => out.push(format!("{name}={}", cur_unit.chars().count())),
                },
                "tabstop" => match val {
                    Some(v) if !ask => {
                        let n = v.parse::<f64>().unwrap_or(0.0);
                        if n.is_nan() || n <= 0.0 {
                            return Err("E487: Argument must be positive".into());
                        }
                        self.tabstop = n as usize;
                    }
                    _ => out.push(format!("tabstop={}", self.tabstop)),
                },
                "expandtab" => {
                    if ask {
                        out.push(format!("{}{name}", if cur_unit != "\t" { "" } else { "no" }));
                    } else {
                        let on = flag(cur_unit != "\t");
                        self.indent_unit = Some(if on { " ".repeat((if cur_unit == "\t" { self.tabstop } else { cur_unit.len() }).max(1)) } else { "\t".into() });
                        self.actions.push(ExAction::SetOption("expandtab".into(), on.to_string()));
                    }
                }
                "wrap" | "number" | "hlsearch" | "incsearch" | "list" | "readonly" => {
                    // (The owner's: it says whether it has it.)
                    if ask {
                        out.push(name.to_string());
                    } else {
                        let v = if inv { "toggle".to_string() } else { (!neg).to_string() };
                        self.actions.push(ExAction::SetOption(name.into(), v));
                    }
                }
                _ => return Err(format!("E518: Unknown option: {raw}")),
            }
        }
        Ok(out.join("  "))
    }
}

#[cfg(test)]
mod tests {
    use crate::text::IndentMode;
    use super::*;

    fn ind() -> Indent {
        Indent { mode: IndentMode::Spaces, size: 4 }
    }
    fn text(s: &str, cursor: usize) -> Text {
        let mut t = Text::new();
        t.set(s);
        t.cursor = cursor;
        t
    }
    fn run(ex: &mut Ex, t: &mut Text, cmd: &str) -> ExOut {
        ex.run(cmd, t, &ind())
    }

    #[test]
    fn regex_engine() {
        let re = Re::new("a(b|c)+d", false).unwrap();
        assert!(re.test("xxabcbd"));
        assert!(!re.test("xxad"));
        let re = Re::new("(\\w+) \\1", false).unwrap();
        assert!(re.test("hello hello"));
        assert!(!re.test("hello world"));
        let re = Re::new("[a-c]{2,3}?x", true).unwrap();
        assert_eq!(re.exec_at("zzABx", 0).unwrap()[0], Some((2, 5)));
        assert!(Re::new("(", false).is_err());
        assert!(Re::new("*a", false).is_err());
        let re = Re::new("\\bcafé\\b", false).unwrap();
        assert!(re.test("un café noir"));
        let (s, n) = Re::new("x*", false).unwrap().replace("abc", true, |_| "-".into());
        assert_eq!((s.as_str(), n), ("-a-b-c-", 4));
    }

    #[test]
    fn substitute() {
        let mut ex = Ex::new();
        let mut t = text("a a a\nb a", 0);
        assert_eq!(run(&mut ex, &mut t, "s/a/b/g").text, "");
        assert_eq!(t.text, "b b b\nb a");
        let mut t = text("one two\ntwo two\nthree", 0);
        let o = run(&mut ex, &mut t, "%s/two/2/g");
        assert_eq!(t.text, "one 2\n2 2\nthree");
        assert_eq!(o.text, "3 substitutions on 2 lines");
        // (Groups, case changes, smart case.)
        let mut t = text("foo bar", 0);
        run(&mut ex, &mut t, "s/\\(foo\\) \\(bar\\)/\\u\\2 \\U\\1/");
        assert_eq!(t.text, "Bar FOO");
        let mut t = text("Foo foo", 0);
        run(&mut ex, &mut t, "s/Foo/x/g");
        assert_eq!(t.text, "x foo");
        // (:& again; not found is an error.)
        let mut t = text("Foo\nFoo", 0);
        run(&mut ex, &mut t, "%&");
        assert_eq!(t.text, "x\nx");
        let o = run(&mut ex, &mut t, "s/zzz/y/");
        assert!(o.error && o.text.starts_with("E486"));
    }

    #[test]
    fn ranges() {
        let mut ex = Ex::new();
        let mut t = text("1\n2\n3\n4\n5", 0);
        run(&mut ex, &mut t, "2,4d");
        assert_eq!(t.text, "1\n5");
        assert_eq!(t.kill_ring, "2\n3\n4\n");
        let mut t = text("a\nb\nc\nd", 0);
        run(&mut ex, &mut t, ".,+1s/$/!/");
        assert_eq!(t.text, "a!\nb!\nc\nd");
        run(&mut ex, &mut t, "/c/,$s/^/-/");
        assert_eq!(t.text, "a!\nb!\n-c\n-d");
        let o = run(&mut ex, &mut t, "9d");
        assert_eq!(o.text, "E16: Invalid range");
        run(&mut ex, &mut t, "1m$");
        assert_eq!(t.text, "b!\n-c\n-d\na!");
        run(&mut ex, &mut t, "$t0");
        assert_eq!(t.text, "a!\nb!\n-c\n-d\na!");
        assert_eq!(run(&mut ex, &mut t, "=").text, "5");
        run(&mut ex, &mut t, "3");
        assert_eq!(t.line_of(t.cursor), 2);
    }

    #[test]
    fn global_and_more() {
        let mut ex = Ex::new();
        let mut t = text("keep 1\ndrop\nkeep 2\ndrop", 0);
        run(&mut ex, &mut t, "g/drop/d");
        assert_eq!(t.text, "keep 1\nkeep 2");
        run(&mut ex, &mut t, "v/1/s/keep/k/");
        assert_eq!(t.text, "keep 1\nk 2");
        let mut t = text("c\na\nb\na", 0);
        run(&mut ex, &mut t, "sort u");
        assert_eq!(t.text, "a\nb\nc");
        let mut t = text("a\n  b\nc", 0);
        run(&mut ex, &mut t, "%j");
        assert_eq!(t.text, "a b c");
        let mut t = text("x\ny", 0);
        run(&mut ex, &mut t, "%>");
        assert_eq!(t.text, "    x\n    y");
        assert_eq!(run(&mut ex, &mut t, "%p").text, "    x\n    y");
        assert_eq!(run(&mut ex, &mut t, "frob").text, "E492: Not an editor command: frob");
    }

    #[test]
    fn owner_commands() {
        let mut ex = Ex::new();
        let mut t = text("ls", 0);
        assert_eq!(run(&mut ex, &mut t, "w").to_vi(), ViOut::Submit);
        assert_eq!(run(&mut ex, &mut t, "wq").to_vi(), ViOut::Submit);
        assert_eq!(run(&mut ex, &mut t, "q!").to_vi(), ViOut::Clear);
        assert!(matches!(run(&mut ex, &mut t, "w foo").to_vi(), ViOut::Message(_, true)));
        assert!(matches!(run(&mut ex, &mut t, "e foo").to_vi(), ViOut::Message(_, true)));
        let c = ex.completions("2,3s");
        assert_eq!(c[0].name, "substitute");
        assert_eq!(c[0].head, "2,3");
        assert!(ex.completions("e").iter().all(|c| c.name != "edit"));
        assert_eq!(ex.completions("w")[0].help, "run the command");
        assert!(ex.completions("s/a").is_empty());
        assert_eq!(run(&mut ex, &mut t, "set ts=4 ic?").text, "ignorecase");
        assert_eq!(ex.tabstop, 4);
    }

    #[test]
    fn search() {
        let mut ex = Ex::new();
        let mut t = text("foo bar foo\nbar", 0);
        assert_eq!(ex.search(&mut t, "bar", false), "");
        assert_eq!(t.cursor, 4);
        ex.search_next(&mut t, false);
        assert_eq!(t.cursor, 12);
        assert_eq!(ex.search_next(&mut t, false), "search hit BOTTOM, continuing at TOP");
        assert_eq!(t.cursor, 4);
    }
}
