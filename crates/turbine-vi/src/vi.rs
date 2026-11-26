//! vi for the editor (Tessera's qml/ViKeys.qml and qml/ExLine.qml): normal,
//! visual (v, V, Ctrl+V) and replace modes; counts; operators (d c y > < gu
//! gU g~ g?) over motions and text objects (iw aw iW aW is as ip ap, quotes,
//! brackets, tags); surround.vim (ys, yss, ds, cs, S in visual); registers,
//! marks, dot repeat, and undo by whole changes, as vim's. And the ":" / "/"
//! command line under the editor (`CmdLine`), whose commands are ex.rs's.
//!
//! The interface:
//!
//! ```text
//! Vi::new() -> Vi
//! vi.key(&KeyEvent, &mut Text, &Indent, &mut Ex) -> ViOut
//!     One key. ViOut::None: not vi's (insert mode's keys, or Ctrl+keys vi
//!     doesn't have): the editor's own keys take it (ComposeBox: insert mode
//!     keeps the Emacs keys). Handled: done. HistoryStep(d): j/k past the
//!     first/last line, step through the history. Message: show it on the
//!     command line (CmdLine::show). OpenCmdLine(kind, initial): open it
//!     (CmdLine::open). (Submit / Clear only come from the command line.)
//! vi.set_normal(on, &Text)       to normal mode or insert mode (`normal` is
//!                                read-only: set it through this)
//! vi.reset()                     a new text (after a run): nothing to undo
//! vi.mode_name()                 "NORMAL", "INSERT", "REPLACE", "VISUAL", "V-LINE", "V-BLOCK"
//! vi.pending                     the keys of a command not finished yet
//! vi.visual                      None, Some('v'), Some('V'), Some('\x16') (a block)
//! vi.block_ranges(&Text)         what to draw as vi's: in V-BLOCK each line's
//!                                part (byte ranges; an empty one: a thin bar), in
//!                                normal mode the block cursor (the char under it,
//!                                empty at an empty line's end); else nothing (v and
//!                                V select with the text's own selection)
//! Vi::help()                     the keys table
//! vi.clip_set / vi.clip_get      "+ and "*: the system clipboard (else kept here)
//!
//! CmdLine::new(); cmd.open(kind, initial); cmd.show(text, error);
//! cmd.apply(&ViOut) -> bool      opens / shows what vi.key asked for
//! cmd.key(&KeyEvent, &mut Text, &Indent, &mut Vi, &mut Ex) -> ViOut
//!     While open (cmd.is_open()): every key is the line's (Handled), Enter
//!     runs it: Submit (:w :x :wq), Clear (:q: clear the text; vi is back in
//!     insert mode), else Handled.
//! cmd.kind ('\0' closed, ':' '/' '?'), cmd.text, cmd.cursor (byte offset),
//! cmd.choices (name, help) above it while a command's name is typed, cmd.pick,
//! cmd.message (what a command said, until the next key in the editor:
//! cmd.dismiss(); a one-line one also after 6 s: cmd.expire(now)).
//! ```
//!
//! Offsets are byte offsets into `Text::text`; columns (j/k's, a block's,
//! marks') count chars.

use crate::ex::{Completion, Ex};
use crate::text::{Indent, Text};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// What the editor must do after a key.
#[derive(Clone, Debug, PartialEq)]
pub enum ViOut {
    /// Not vi's: the editor's own keys take it.
    None,
    Handled,
    /// j or k past the first or last line: the history, as Up and Down.
    HistoryStep(i32),
    /// :w, :x, :wq: run the command.
    Submit,
    /// :q: clear the text.
    Clear,
    /// Show this on the command line (true: a complaint).
    Message(String, bool),
    /// Open the command line: ':' '/' '?', with this typed.
    OpenCmdLine(char, String),
    /// What an ex command asks of the host (write, quit, edit, windows,
    /// options), for hosts that own files and windows.
    Actions(Vec<crate::ex::ExAction>),
}

pub const BLOCK: char = '\x16';
const WANT_END: usize = usize::MAX; // ($: j and k keep to the end)

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Char,
    Line,
    Block,
}

#[derive(Clone, Debug, PartialEq)]
struct Motion {
    m: String,
    ch: String,
}

/// A parsed command: [count]["x][count] command.
#[derive(Clone, Debug, Default)]
struct Cmd {
    reg: char, // ('\0': none)
    count: usize,
    count2: usize,
    name: String,
    op: String,
    line: bool,
    motion: Option<Motion>,
    obj: String,
    ch: String,
    target: String,
}

enum Parse<T> {
    Need,
    Bad,
    Got(T),
}
use Parse::*;

macro_rules! tryp {
    ($e:expr) => {
        match $e {
            Got(x) => x,
            Need => return Need,
            Bad => return Bad,
        }
    };
}

/// Where a motion goes.
#[derive(Clone, Debug, Default)]
struct Target {
    pos: usize,
    linewise: bool,
    inclusive: bool,
    jump: bool,
    col: Option<usize>,
    msg: String,
    from: Option<usize>,
    edge: i32,
}
impl Target {
    fn at(pos: usize) -> Target {
        Target { pos, ..Default::default() }
    }
}

/// A range: [a, b), whole lines or not.
#[derive(Clone, Copy, Debug)]
struct Rg {
    a: usize,
    b: usize,
    linewise: bool,
}
fn rg(a: usize, b: usize) -> Rg {
    Rg { a, b, linewise: false }
}

/// A block selection: lines l1..l2, columns c1..c2 (dollar: to each line's end).
#[derive(Clone, Copy, Debug)]
struct Blk {
    l1: usize,
    l2: usize,
    c1: usize,
    c2: usize,
    dollar: bool,
}

enum VRange {
    R(Rg),
    B(Blk),
}

#[derive(Clone, Debug)]
struct InsBlock {
    l1: usize,
    l2: usize,
    col: usize,
    append: bool,
    dollar: bool,
}

/// The insert being typed.
#[derive(Clone, Debug)]
struct Ins {
    start: usize,
    len: usize,
    count: usize,
    cmd: Option<Cmd>,
    prefix: String,
    block: Option<InsBlock>,
    typed: Option<String>,
}

#[derive(Clone, Debug)]
struct LastChange {
    cmd: Cmd,
    text: String,
    replace: bool,
}

#[derive(Clone, Debug)]
struct Diff {
    a: usize,
    del: String,
    ins: String,
}

pub struct Vi {
    pub normal: bool,
    /// The keys of a command not finished yet.
    pub pending: String,
    /// None, 'v', 'V', or '\x16' (Ctrl+V: a block).
    pub visual: Option<char>,
    /// R.
    pub replacing: bool,
    /// (j or k past the first or last line: the input editor goes through its history.)
    pub history_edges: bool,
    /// "+ and "*: the system clipboard (None: kept here).
    pub clip_set: Option<Box<dyn FnMut(&str)>>,
    pub clip_get: Option<Box<dyn FnMut() -> Option<String>>>,
    clip_local: String,
    clip_last: String, // (what we put there, so a block or lines paste back as one)
    v_anchor: usize,
    v_head: usize,
    key_buf: Vec<String>,
    want_col: Option<usize>, // j and k keep to this column (WANT_END: the end)
    last_find: Option<(char, char)>,
    mark_cols: HashMap<char, usize>,
    last_jump: Option<usize>,
    last_visual: Option<(char, usize, usize)>,
    last_insert: Option<usize>,
    reg_kind: HashMap<char, Kind>,
    last_change: Option<LastChange>,
    replaying: bool,
    ins: Option<Ins>,
    sel_snap: Option<(usize, usize, usize)>,
    // Undo: each command, and each insert with the command that began it, is one step.
    undo_stack: Vec<Diff>,
    redo_stack: Vec<Diff>,
    last_known: Option<String>,
    change_base: Option<String>,
    out: Option<ViOut>,
}

impl Default for Vi {
    fn default() -> Self {
        Self::new()
    }
}

// ---- text ----------------------------------------------------------------
fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c as u32 >= 0xC0
}
fn is_blank(c: char) -> bool {
    c == ' ' || c == '\t'
}
fn ws(c: char) -> bool {
    c.is_whitespace() || c == '\u{feff}'
}
fn at(t: &Text, p: usize) -> Option<char> {
    t.char_at(p)
}
fn byte(t: &Text, p: usize) -> Option<u8> {
    t.text.as_bytes().get(p).copied()
}
/// Byte index of char n in s (at most its end).
fn cidx(s: &str, n: usize) -> usize {
    s.char_indices().nth(n).map_or(s.len(), |(i, _)| i)
}
fn clen(s: &str) -> usize {
    s.chars().count()
}
fn lines_range(t: &Text, l1: usize, l2: usize) -> Rg {
    let a = t.pos_of_line(l1);
    let e = t.line_end(t.pos_of_line(l2));
    Rg { a, b: if e < t.len() { e + 1 } else { e }, linewise: true }
}
fn indent_of(t: &Text, p: usize) -> String {
    t.text[t.line_start(p)..t.first_non_blank(p)].to_string()
}
/// The last char's position in [a, b) (a if empty).
fn last_in(t: &Text, a: usize, b: usize) -> usize {
    if b > a {
        t.prev(b).max(a)
    } else {
        a
    }
}
/// Replace [a, b) with s, the cursor moving with the text around it (as a
/// text field's does).
fn rep(t: &mut Text, a: usize, b: usize, s: &str) {
    let mut c = t.cursor;
    if c > a {
        c = if c >= b { c - (b - a) } else { a };
    }
    if c >= a && !s.is_empty() {
        c += s.len();
    }
    t.replace(a, b, s);
    t.cursor = c.min(t.len());
}
fn ins_at(t: &mut Text, p: usize, s: &str) {
    rep(t, p, p, s);
}
// Lines l1..l2 (whole, without their last newline) become `lines`.
fn replace_lines(t: &mut Text, l1: usize, l2: usize, lines: &[String]) {
    let a = t.pos_of_line(l1);
    let b = t.line_end(t.pos_of_line(l2));
    rep(t, a, b, &lines.join("\n"));
}
fn set_cursor(t: &mut Text, p: usize) {
    let p = t.clamp_pos(p);
    t.move_to(p, false);
}
fn split_lines(t: &Text) -> Vec<String> {
    t.text.split('\n').map(String::from).collect()
}

// ---- keys ----------------------------------------------------------------
fn token(e: &KeyEvent) -> String {
    let m = e.modifiers;
    let ctrl = m.contains(KeyModifiers::CONTROL) && !m.intersects(KeyModifiers::ALT | KeyModifiers::META | KeyModifiers::SUPER);
    if e.code == KeyCode::Esc || (ctrl && e.code == KeyCode::Char('[')) {
        return "<esc>".into();
    }
    if ctrl {
        return match e.code {
            KeyCode::Char(c) if c.is_ascii_alphabetic() => format!("<C-{}>", c.to_ascii_lowercase()),
            _ => String::new(),
        };
    }
    if m.intersects(KeyModifiers::ALT | KeyModifiers::META | KeyModifiers::SUPER) {
        return String::new();
    }
    match e.code {
        KeyCode::Enter => "<cr>".into(),
        KeyCode::Backspace => "<bs>".into(),
        KeyCode::Delete => "<del>".into(),
        KeyCode::Left => "h".into(),
        KeyCode::Right => "l".into(),
        KeyCode::Up => "k".into(),
        KeyCode::Down => "j".into(),
        KeyCode::Home => "0".into(),
        KeyCode::End => "$".into(),
        KeyCode::Char(c) if c >= ' ' => c.to_string(),
        _ => String::new(),
    }
}

// ---- parsing: [count]["x][count] command ---------------------------------------
fn tk(k: &[String], i: usize) -> Option<&str> {
    k.get(i).map(String::as_str)
}
fn one(s: Option<&str>) -> bool {
    s.is_some_and(|s| s.chars().count() == 1)
}
fn one_in(s: Option<&str>, set: &str) -> bool {
    one(s) && set.contains(s.unwrap())
}
fn is_digit(s: &str) -> bool {
    s.len() == 1 && s.as_bytes()[0].is_ascii_digit()
}
const SIMPLE_MOTIONS: &str = "hjklwbeWBE0^$G|;,%{}()+-_ nN";

fn parse_count(k: &[String], mut i: usize) -> (usize, usize) {
    let mut c = String::new();
    while i < k.len() && is_digit(&k[i]) && (k[i] != "0" || !c.is_empty()) {
        c.push_str(&k[i]);
        i += 1;
    }
    (c.parse().unwrap_or(0), i)
}
fn parse_motion(k: &[String], j: usize) -> Parse<(Motion, usize)> {
    let Some(c) = tk(k, j) else { return Need };
    let mo = |m: &str, ch: &str| Motion { m: m.into(), ch: ch.into() };
    if c == "<cr>" || c == "<bs>" {
        return Got((mo(c, ""), j + 1));
    }
    if !one(Some(c)) {
        return Bad;
    }
    if SIMPLE_MOTIONS.contains(c) {
        return Got((mo(c, ""), j + 1));
    }
    if "fFtT`'".contains(c) {
        if j + 1 >= k.len() {
            return Need;
        }
        if !one(tk(k, j + 1)) {
            return Bad;
        }
        return Got((mo(c, &k[j + 1]), j + 2));
    }
    if c == "g" {
        if j + 1 >= k.len() {
            return Need;
        }
        if ["g", "e", "E", "_"].contains(&k[j + 1].as_str()) {
            return Got((mo(&format!("g{}", k[j + 1]), ""), j + 2));
        }
        return Bad;
    }
    Bad
}
fn parse_object(k: &[String], j: usize) -> Parse<(String, usize)> {
    let c = tk(k, j);
    if c != Some("i") && c != Some("a") {
        return Bad;
    }
    if j + 1 >= k.len() {
        return Need;
    }
    if !one_in(tk(k, j + 1), "wWsp\"'`()b[]{}B<>t") {
        return Bad;
    }
    Got((format!("{}{}", k[j], k[j + 1]), j + 2))
}
// A surround target: one character, or a tag (< or t, its name, then > or Enter).
fn parse_target(k: &[String], j: usize) -> Parse<(String, usize)> {
    if j >= k.len() {
        return Need;
    }
    let c = k[j].as_str();
    if c == "<" || c == "t" {
        let mut s = String::new();
        for (i, x) in k.iter().enumerate().skip(j + 1) {
            if x == ">" || x == "<cr>" {
                return Got((format!("<{}>", s.trim()), i + 1));
            }
            if x == "<bs>" {
                s.pop();
                continue;
            }
            if !one(Some(x)) {
                return Bad;
            }
            s.push_str(x);
        }
        return Need;
    }
    if !one(Some(c)) {
        return Bad;
    }
    Got((c.to_string(), j + 1))
}
fn parse_prefix(k: &[String]) -> Parse<(char, usize, usize)> {
    let mut i = 0;
    let mut reg = '\0';
    if k[0] == "\"" {
        if k.len() < 2 {
            return Need;
        }
        let mut cs = k[1].chars();
        match (cs.next(), cs.next()) {
            (Some(c), None) if c.is_ascii_alphanumeric() || "\"_+*-".contains(c) => reg = c,
            _ => return Bad,
        }
        i = 2;
    }
    let (count, j) = parse_count(k, i);
    if j >= k.len() {
        return Need;
    }
    Got((reg, count, j))
}
fn parse(k: &[String]) -> Parse<Cmd> {
    let (reg, count, i) = tryp!(parse_prefix(k));
    let mut r = Cmd { reg, count, ..Default::default() };
    let c = tk(k, i);
    let n1 = tk(k, i + 1);
    // ds{c}, cs{old}{new}
    if (c == Some("d") || c == Some("c")) && n1 == Some("s") {
        if i + 2 >= k.len() {
            return Need;
        }
        if !one(tk(k, i + 2)) {
            return Bad;
        }
        r.name = format!("{}s", c.unwrap());
        r.ch = k[i + 2].clone();
        if c == Some("d") {
            return Got(r);
        }
        let (tg, _) = tryp!(parse_target(k, i + 3));
        r.target = tg;
        return Got(r);
    }
    let mut op = String::new();
    let mut j = i + 1;
    if one_in(c, "dcy<>") {
        op = c.unwrap().to_string();
    }
    if c == Some("g") {
        let Some(n) = n1 else { return Need };
        if one_in(Some(n), "uU~?") {
            op = format!("g{n}");
            j = i + 2;
        }
    }
    if op == "y" && n1 == Some("s") {
        op = "ys".into();
        j = i + 2;
    }
    if !op.is_empty() {
        let (c2, jj) = parse_count(k, j);
        r.count2 = c2;
        j = jj;
        if j >= k.len() {
            return Need;
        }
        let last = if op == "ys" { "s".to_string() } else { op.chars().last().unwrap().to_string() };
        let mut rest = None;
        if k[j] == last {
            r.line = true;
            rest = Some(j + 1);
        } else if op.len() == 2 && op.starts_with('g') && op != "ys" && k[j] == "g" {
            if j + 1 >= k.len() {
                return Need;
            }
            if k[j + 1] == last {
                r.line = true;
                rest = Some(j + 2);
            }
        }
        let rest = match rest {
            Some(x) => x,
            None => match parse_motion(k, j) {
                Need => return Need,
                Got((m, nx)) => {
                    r.motion = Some(m);
                    nx
                }
                Bad => {
                    let (o, nx) = tryp!(parse_object(k, j));
                    r.obj = o;
                    nx
                }
            },
        };
        r.op = op;
        if r.op == "ys" {
            let (tg, _) = tryp!(parse_target(k, rest));
            r.target = tg;
        }
        return Got(r);
    }
    match parse_motion(k, i) {
        Need => return Need,
        Got((m, _)) => {
            r.motion = Some(m);
            return Got(r);
        }
        Bad => {}
    }
    let c = c.unwrap_or("");
    if c == "r" || c == "m" {
        let Some(n) = n1 else { return Need };
        if !one(Some(n)) && !(c == "r" && n == "<cr>") {
            return Bad;
        }
        r.name = c.into();
        r.ch = n.into();
        return Got(r);
    }
    if c == "g" {
        if let Some(n) = n1.filter(|n| ["J", "v", "i", "I", "p", "P", "&"].contains(n)) {
            r.name = format!("g{n}");
            return Got(r);
        }
        return Bad;
    }
    if [
        "x", "X", "s", "S", "C", "D", "Y", "p", "P", "J", "~", "i", "a", "I", "A", "o", "O", "R", "u", "<C-r>", ".", "v", "V", "<C-v>", "<C-q>", "*", "#", ":", "/", "?", "&", "<C-a>", "<C-x>", "<del>",
    ]
    .contains(&c)
    {
        r.name = c.into();
        return Got(r);
    }
    Bad
}
fn parse_visual(k: &[String]) -> Parse<Cmd> {
    let (reg, count, i) = tryp!(parse_prefix(k));
    let c = tk(k, i).unwrap_or("");
    let n1 = tk(k, i + 1);
    let mut r = Cmd { reg, count, ..Default::default() };
    match parse_motion(k, i) {
        Need => return Need,
        Got((m, _)) => {
            r.motion = Some(m);
            return Got(r);
        }
        Bad => {}
    }
    match parse_object(k, i) {
        Need => return Need,
        Got((o, _)) => {
            r.obj = o;
            return Got(r);
        }
        Bad => {}
    }
    if c == "r" {
        let Some(n) = n1 else { return Need };
        if !one(Some(n)) {
            return Bad;
        }
        r.name = "r".into();
        r.ch = n.into();
        return Got(r);
    }
    if c == "S" {
        let (tg, _) = tryp!(parse_target(k, i + 1));
        r.name = "S".into();
        r.target = tg;
        return Got(r);
    }
    if c == "g" {
        let Some(n) = n1 else { return Need };
        if ["u", "U", "~", "?", "J", "v"].contains(&n) {
            r.name = format!("g{n}");
            return Got(r);
        }
        return Bad;
    }
    if [
        "d", "x", "X", "D", "c", "s", "C", "R", "y", "Y", ">", "<", "~", "u", "U", "J", "p", "P", "I", "A", "o", "O", "v", "V", "<C-v>", "<C-q>", ":", "<C-a>", "<C-x>", "<del>",
    ]
    .contains(&c)
    {
        r.name = c.into();
        return Got(r);
    }
    Bad
}

fn is_change(r: &Cmd) -> bool {
    if !r.op.is_empty() {
        return r.op != "y";
    }
    [
        "x", "X", "s", "S", "C", "D", "p", "P", "J", "~", "r", "R", "i", "a", "I", "A", "o", "O", "gI", "gi", "<C-a>", "<C-x>", "gJ", "gp", "gP", "ds", "cs", "&", "g&", "<del>",
    ]
    .contains(&r.name.as_str())
}

// ---- motions' helpers ------------------------------------------------------
fn cls(c: Option<char>, big: bool) -> i32 {
    match c {
        None => -1,
        Some(' ' | '\t' | '\n') => 0,
        Some(_) if big => 1,
        Some(c) if is_word(c) => 2,
        _ => 1,
    }
}
fn word_fwd(t: &Text, mut p: usize, big: bool) -> usize {
    let n = t.len();
    if p >= n {
        return n;
    }
    let c0 = cls(at(t, p), big);
    if c0 != 0 {
        while p < n && cls(at(t, p), big) == c0 {
            p = t.next(p);
        }
    }
    while p < n {
        let c = at(t, p).unwrap();
        if c == '\n' {
            p = t.next(p);
            if p < n && at(t, p) == Some('\n') {
                return p;
            }
            continue;
        }
        if is_blank(c) {
            p = t.next(p);
            continue;
        }
        break;
    }
    p
}
fn word_end_fwd(t: &Text, p: usize, big: bool) -> usize {
    let n = t.len();
    let mut p = t.next(p);
    while p < n && cls(at(t, p), big) == 0 {
        p = t.next(p);
    }
    if p >= n {
        return t.prev(n);
    }
    let c = cls(at(t, p), big);
    while t.next(p) < n && cls(at(t, t.next(p)), big) == c {
        p = t.next(p);
    }
    p
}
fn word_back(t: &Text, p: usize, big: bool) -> usize {
    if p == 0 {
        return 0;
    }
    let mut p = t.prev(p);
    while p > 0 && cls(at(t, p), big) == 0 {
        if at(t, p) == Some('\n') && t.char_before(p) == Some('\n') {
            return p;
        }
        p = t.prev(p);
    }
    if cls(at(t, p), big) == 0 {
        return p;
    }
    let c = cls(at(t, p), big);
    while p > 0 && cls(t.char_before(p), big) == c {
        p = t.prev(p);
    }
    p
}
fn word_end_back(t: &Text, mut p: usize, big: bool) -> usize {
    let c = cls(at(t, p), big);
    if c > 0 {
        while p > 0 && cls(t.char_before(p), big) == c {
            p = t.prev(p);
        }
    }
    if p == 0 {
        return 0;
    }
    p = t.prev(p);
    while p > 0 && cls(at(t, p), big) == 0 {
        if at(t, p) == Some('\n') && t.char_before(p) == Some('\n') {
            return p;
        }
        p = t.prev(p);
    }
    p
}
fn find_char(t: &Text, c: char, ch: char, n: usize, p: usize, again: bool) -> Option<Target> {
    let a = t.line_start(p);
    let e = t.line_end(p);
    let mut q = p;
    if c == 'f' || c == 't' {
        let mut from = t.next(p);
        if c == 't' && again {
            from = t.next(from);
        }
        for k in 0..n {
            let s = if k == 0 { from } else { t.next(q) };
            q = s + t.text.get(s..)?.find(ch)?;
            if q >= e {
                return None;
            }
        }
        return Some(Target { pos: if c == 't' { t.prev(q) } else { q }, inclusive: true, ..Default::default() });
    }
    // (One back from p, two for T again: none before the line's start.)
    let mut from = p;
    for _ in 0..(if c == 'T' && again { 2 } else { 1 }) {
        if from <= a {
            return None;
        }
        from = t.prev(from);
    }
    for k in 0..n {
        let s = if k == 0 {
            from
        } else {
            if q <= a {
                return None;
            }
            t.prev(q)
        };
        q = t.text[..t.next(s)].rfind(ch)?;
        if q < a {
            return None;
        }
    }
    Some(Target::at(if c == 'T' { t.next(q) } else { q }))
}
fn pair_of(c: u8) -> u8 {
    match c {
        b'(' => b')',
        b')' => b'(',
        b'[' => b']',
        b']' => b'[',
        b'{' => b'}',
        b'}' => b'{',
        _ => b'>',
    }
}
fn match_bracket(t: &Text, p: usize) -> Option<usize> {
    let b = t.text.as_bytes();
    let e = t.line_end(p);
    let mut i = p;
    while i < e && !b"()[]{}".contains(&b[i]) {
        i += 1;
    }
    if i >= e {
        return None;
    }
    let c = b[i];
    let o = pair_of(c);
    let mut depth = 0;
    if b"([{".contains(&c) {
        for (j, &x) in b.iter().enumerate().skip(i) {
            if x == c {
                depth += 1;
            } else if x == o {
                depth -= 1;
                if depth == 0 {
                    return Some(j);
                }
            }
        }
    } else {
        for j in (0..=i).rev() {
            if b[j] == c {
                depth += 1;
            } else if b[j] == o {
                depth -= 1;
                if depth == 0 {
                    return Some(j);
                }
            }
        }
    }
    None
}
fn para_fwd(t: &Text, p: usize) -> usize {
    let n = t.len();
    let mut a = t.line_start(p);
    while a < n && byte(t, a) == Some(b'\n') {
        a += 1;
    }
    while a < n && byte(t, a) != Some(b'\n') {
        let e = t.line_end(a);
        if e >= n {
            return n;
        }
        a = e + 1;
    }
    a
}
fn para_back(t: &Text, p: usize) -> usize {
    let mut a = t.line_start(p);
    while a > 0 && byte(t, a) == Some(b'\n') {
        a = t.line_start(a - 1);
    }
    while a > 0 && byte(t, a) != Some(b'\n') {
        a = t.line_start(a - 1);
    }
    a
}
fn sentence_starts(t: &Text) -> Vec<usize> {
    let n = t.len();
    let mut out = vec![];
    let mut k = 0;
    while k < n && at(t, k).is_some_and(ws) {
        k = t.next(k);
    }
    out.push(k);
    for (i, c) in t.text.char_indices() {
        if c == '\n' && byte(t, i + 1) == Some(b'\n') {
            out.push(i + 1);
            let mut j = i + 1;
            while j < n && at(t, j).is_some_and(ws) {
                j = t.next(j);
            }
            if j < n {
                out.push(j);
            }
        } else if ".!?".contains(c) {
            let mut j = i + 1;
            while j < n && at(t, j).is_some_and(|c| ")]\"'".contains(c)) {
                j += 1;
            }
            if j < n && at(t, j).is_some_and(ws) {
                while j < n && at(t, j).is_some_and(ws) && !(byte(t, j) == Some(b'\n') && byte(t, j + 1) == Some(b'\n')) {
                    j = t.next(j);
                }
                if j < n && byte(t, j) != Some(b'\n') {
                    out.push(j);
                }
            }
        }
    }
    out.retain(|&x| x <= n);
    out.sort();
    out.dedup();
    out
}

// ---- text objects ----------------------------------------------------------
fn word_object(t: &Text, p: usize, big: bool, around: bool, n: usize) -> Option<Rg> {
    let ls = t.line_start(p);
    let le = t.line_end(p);
    if le == ls {
        return None;
    }
    let p = if p >= le { t.prev(le) } else { p };
    let kind = |i: usize| {
        let c = at(t, i).unwrap_or(' ');
        if is_blank(c) {
            0
        } else if big || !is_word(c) {
            1
        } else {
            2
        }
    };
    let (mut a, mut b) = (p, p);
    let c = kind(p);
    while a > ls && kind(t.prev(a)) == c {
        a = t.prev(a);
    }
    while b < le && kind(b) == c {
        b = t.next(b);
    }
    let run = |b: &mut usize| {
        let c2 = kind(*b);
        while *b < le && kind(*b) == c2 {
            *b = t.next(*b);
        }
    };
    if !around {
        let mut k = 1;
        while k < n && b < le {
            run(&mut b);
            k += 1;
        }
        return Some(rg(a, b));
    }
    if c == 0 {
        if b < le {
            run(&mut b);
        }
    } else {
        let mut b2 = b;
        while b2 < le && kind(b2) == 0 {
            b2 = t.next(b2);
        }
        if b2 > b {
            b = b2;
        } else {
            let mut a2 = a;
            while a2 > ls && kind(t.prev(a2)) == 0 {
                a2 = t.prev(a2);
            }
            if a2 > ls {
                a = a2;
            }
        }
    }
    let mut k = 1;
    while k < n && b < le {
        run(&mut b);
        while b < le && kind(b) == 0 {
            b = t.next(b);
        }
        k += 1;
    }
    Some(rg(a, b))
}
fn sentence_object(t: &Text, p: usize, around: bool) -> Option<Rg> {
    let s = sentence_starts(t);
    let a = s.iter().copied().filter(|&v| v <= p).last().unwrap_or(0);
    let mut b = s.iter().copied().find(|&v| v > p).unwrap_or(t.len());
    if !around {
        while b > a && t.char_before(b).is_some_and(ws) {
            b = t.prev(b);
        }
    }
    Some(rg(a, b))
}
fn para_object(t: &Text, p: usize, around: bool, n: usize) -> Option<Rg> {
    let lines = split_lines(t);
    let nl = lines.len();
    let blank = |l: usize| lines[l].trim_matches(ws).is_empty();
    let l0 = t.line_of(p);
    let (mut s, mut e) = (l0, l0);
    let b = blank(l0);
    while s > 0 && blank(s - 1) == b {
        s -= 1;
    }
    while e + 1 < nl && blank(e + 1) == b {
        e += 1;
    }
    let mut k = 1;
    while k < n && e + 1 < nl {
        let b2 = blank(e + 1);
        e += 1;
        while e + 1 < nl && blank(e + 1) == b2 {
            e += 1;
        }
        k += 1;
    }
    if around {
        if !b {
            let mut e2 = e;
            while e2 + 1 < nl && blank(e2 + 1) {
                e2 += 1;
            }
            if e2 > e {
                e = e2;
            } else {
                while s > 0 && blank(s - 1) {
                    s -= 1;
                }
            }
        } else {
            while e + 1 < nl && !blank(e + 1) {
                e += 1;
            }
        }
    }
    Some(lines_range(t, s, e))
}
fn quote_object(t: &Text, q: char, p: usize, around: bool) -> Option<Rg> {
    let ls = t.line_start(p);
    let le = t.line_end(p);
    let mut qs = vec![];
    let mut prev = None;
    for (i, c) in t.text[ls..le].char_indices() {
        if c == q && prev != Some('\\') {
            qs.push(ls + i);
        }
        prev = Some(c);
    }
    let mut oc = None;
    let mut k = 0;
    while k + 1 < qs.len() {
        if qs[k] <= p && p <= qs[k + 1] {
            oc = Some((qs[k], qs[k + 1]));
            break;
        }
        k += 2;
    }
    if oc.is_none() {
        for k in 0..qs.len().saturating_sub(1) {
            if qs[k] > p {
                oc = Some((qs[k], qs[k + 1]));
                break;
            }
        }
    }
    let (o, c) = oc?;
    let ql = q.len_utf8();
    if !around {
        return Some(rg(o + ql, c));
    }
    let (mut a, mut b) = (o, c + ql);
    if b < le && at(t, b).is_some_and(is_blank) {
        while b < le && at(t, b).is_some_and(is_blank) {
            b += 1;
        }
    } else {
        let mut a2 = a;
        while a2 > ls && t.char_before(a2).is_some_and(is_blank) {
            a2 -= 1;
        }
        if a2 > ls {
            a = a2;
        }
    }
    Some(rg(a, b))
}
fn enclosing_open(t: &Text, open: u8, close: u8, from: Option<usize>) -> Option<usize> {
    let b = t.text.as_bytes();
    let from = from?;
    if b.get(from) == Some(&open) {
        return Some(from);
    }
    let mut depth = 0;
    for i in (0..from.min(b.len())).rev() {
        if b[i] == close {
            depth += 1;
        } else if b[i] == open {
            if depth == 0 {
                return Some(i);
            }
            depth -= 1;
        }
    }
    None
}
fn close_of(t: &Text, open: u8, close: u8, o: usize) -> Option<usize> {
    let mut depth = 0;
    for (i, &x) in t.text.as_bytes().iter().enumerate().skip(o) {
        if x == open {
            depth += 1;
        } else if x == close {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}
// The pair's (o, c) around p (n levels out); not in one: the next on the line.
fn bracket_pair(t: &Text, open: u8, close: u8, p: usize, n: usize) -> Option<(usize, usize)> {
    let mut o = None;
    let mut from = Some(p);
    for _ in 0..n {
        let Some(x) = enclosing_open(t, open, close, from) else { break };
        o = Some(x);
        from = x.checked_sub(1);
        if close_of(t, open, close, x).is_none() {
            o = None;
            break;
        }
    }
    let o = match o {
        Some(o) => o,
        None => {
            let i = p + t.text[p..].find(open as char)?;
            if i >= t.line_end(p) {
                return None;
            }
            i
        }
    };
    Some((o, close_of(t, open, close, o)?))
}
fn bracket_object(t: &Text, open: u8, close: u8, p: usize, around: bool, n: usize) -> Option<Rg> {
    let (o, c) = bracket_pair(t, open, close, p, n)?;
    if around {
        return Some(rg(o, c + 1));
    }
    let (a, b) = (o + 1, c);
    // (Over lines, "{" ending its line and "}" starting its own: the lines between.)
    let lsb = t.line_start(b);
    if byte(t, a) == Some(b'\n') && t.text[lsb..b].bytes().all(|x| x == b' ' || x == b'\t') && lsb > a + 1 {
        return Some(Rg { a: a + 1, b: lsb, linewise: true });
    }
    Some(rg(a, b))
}
struct TagPair {
    os: usize,
    oe: usize,
    cs: usize,
    ce: usize,
}
/// A tag at i (<(\/?)([A-Za-z][\w:.-]*)(?:\s[^<>]*?)?(\/?)>): (closing, name, self-closing, end).
fn tag_at(s: &str, i: usize) -> Option<(bool, &str, bool, usize)> {
    let b = s.as_bytes();
    let mut j = i + 1;
    let closing = b.get(j) == Some(&b'/');
    if closing {
        j += 1;
    }
    if !b.get(j).is_some_and(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let ns = j;
    while b.get(j).is_some_and(|c| c.is_ascii_alphanumeric() || b"_:.-".contains(c)) {
        j += 1;
    }
    let name = &s[ns..j];
    let end = |j: usize| -> Option<(bool, usize)> {
        if s[j..].starts_with("/>") {
            Some((true, j + 2))
        } else if s[j..].starts_with('>') {
            Some((false, j + 1))
        } else {
            None
        }
    };
    // (Attributes: a space, then the least up to "/>" or ">".)
    if let Some(c) = s[j..].chars().next().filter(|c| ws(*c)) {
        let mut k = j + c.len_utf8();
        while k < s.len() {
            if let Some((sc, e)) = end(k) {
                return Some((closing, name, sc, e));
            }
            let c = s[k..].chars().next().unwrap();
            if c == '<' || c == '>' {
                break;
            }
            k += c.len_utf8();
        }
    }
    let (sc, e) = end(j)?;
    Some((closing, name, sc, e))
}
fn tag_pairs(t: &Text) -> Vec<TagPair> {
    let s = &t.text;
    let mut stack: Vec<(&str, usize, usize)> = vec![];
    let mut pairs = vec![];
    let mut i = 0;
    while let Some(k) = s[i..].find('<') {
        let at = i + k;
        match tag_at(s, at) {
            Some((closing, name, sc, e)) => {
                i = e;
                if sc {
                    continue;
                }
                if !closing {
                    stack.push((name, at, e));
                } else if let Some(k) = stack.iter().rposition(|x| x.0 == name) {
                    let o = stack[k];
                    stack.truncate(k);
                    pairs.push(TagPair { os: o.1, oe: o.2, cs: at, ce: e });
                }
            }
            None => i = at + 1,
        }
    }
    pairs
}
fn tag_object(t: &Text, p: usize, around: bool, n: usize) -> Option<Rg> {
    let mut ps: Vec<TagPair> = tag_pairs(t).into_iter().filter(|x| x.os <= p && p < x.ce).collect();
    ps.sort_by_key(|x| x.ce - x.os);
    let pr = ps.get(n.checked_sub(1)?)?;
    Some(if around { rg(pr.os, pr.ce) } else { rg(pr.oe, pr.cs) })
}
fn object_range(t: &Text, o: &str, p: usize, n: usize) -> Option<Rg> {
    let mut cs = o.chars();
    let around = cs.next() == Some('a');
    let k = cs.next()?;
    match k {
        'w' | 'W' => word_object(t, p, k == 'W', around, n),
        's' => sentence_object(t, p, around),
        'p' => para_object(t, p, around, n),
        '"' | '\'' | '`' => quote_object(t, k, p, around),
        '(' | ')' | 'b' => bracket_object(t, b'(', b')', p, around, n),
        '[' | ']' => bracket_object(t, b'[', b']', p, around, n),
        '{' | '}' | 'B' => bracket_object(t, b'{', b'}', p, around, n),
        '<' | '>' => bracket_object(t, b'<', b'>', p, around, n),
        't' => tag_object(t, p, around, n),
        _ => None,
    }
}

// ---- case ------------------------------------------------------------------
fn toggle_case(s: &str) -> String {
    let mut o = String::new();
    for c in s.chars() {
        let up: String = c.to_uppercase().collect();
        if up == c.to_string() {
            o.extend(c.to_lowercase());
        } else {
            o.push_str(&up);
        }
    }
    o
}
fn case_op(op: &str, s: &str) -> String {
    match op {
        "gu" | "u" => s.to_lowercase(),
        "gU" | "U" => s.to_uppercase(),
        "g?" => s
            .chars()
            .map(|c| match c {
                'a'..='z' => (((c as u8 - b'a') + 13) % 26 + b'a') as char,
                'A'..='Z' => (((c as u8 - b'A') + 13) % 26 + b'A') as char,
                _ => c,
            })
            .collect(),
        _ => toggle_case(s),
    }
}

// ---- surround.vim ----------------------------------------------------------
// The opening and closing text for a target: "(" "[" "{" with spaces
// inside, ")" "]" "}" b B r a ">" without, a tag, or the character twice.
fn surround_pair(target: &str) -> (String, String) {
    if target.len() > 1 && target.starts_with('<') {
        let inner = &target[1..target.len() - 1];
        let name = inner.split(ws).next().unwrap_or("");
        return (target.to_string(), format!("</{name}>"));
    }
    let (o, c) = match target {
        "(" => ("( ", " )"),
        ")" | "b" => ("(", ")"),
        "[" => ("[ ", " ]"),
        "]" | "r" => ("[", "]"),
        "{" => ("{ ", " }"),
        "}" | "B" => ("{", "}"),
        ">" | "a" => ("<", ">"),
        _ => (target, target),
    };
    (o.to_string(), c.to_string())
}

/// The keys, for a help table: [group, [(keys, what)]].
const HELP: &[(&str, &[(&str, &str)])] = &[
    (
        "vi: moving (a count first repeats: 3w, 5j)",
        &[
            ("h j k l  w b e ge  W B E gE", "Letter, line; word start, back, end (W: words with punctuation)"),
            ("0 ^ $ g_ |  gg G", "Line start, first letter, end, last letter, column; first, last line (5G: line 5)"),
            ("f F t T {c}  ; ,", "To a letter on the line (t: just before); again, back"),
            ("% { } ( )", "The matching bracket; paragraph; sentence"),
            ("* # / ? n N", "Search the word, a pattern; next, previous"),
            ("m{a-z}  `{a-z} '{a-z}  ``", "Set a mark; go to it, its line; back where you jumped from"),
        ],
    ),
    (
        "vi: changing",
        &[
            ("d c y > < gu gU g~ {motion}", "Delete, change, copy, indent, lower, upper, swap case: dw, c$, y2j; doubled for lines: dd cc yy >> guu"),
            ("iw aw  iW aW  is as  ip ap", "Text objects, after an operator or v: word, WORD, sentence, paragraph (i: inside, a: with spaces)"),
            ("i\" i' i`  i( ib  i[  i{ iB  i<  it", "Inside quotes, brackets, a tag (a\u{2026}: with them): ci\", da(, yit"),
            ("ys{motion}{c}  yss{c}", "Surround with c: ysiw) \u{2192} (word); ( [ { add spaces inside; t or < a tag"),
            ("ds{c}  cs{old}{new}", "Delete or change the surrounding pair: ds\", cs'\", cst<b>"),
            ("x X s S C D Y J gJ", "Letter; before; change letter, line; change, delete to the end; copy line; join lines"),
            ("r{c} R ~ Ctrl+A Ctrl+X", "Replace a letter; type over; swap case; add to, subtract from the number"),
            ("i a I A o O gi", "Insert: before, after, line start, end, new line below, above, where you last did"),
            ("p P  \"{a-z} \"+", "Paste after, before; into or from a register: \"ayiw, \"ap; \"+ (or \"*) the system clipboard: \"+yy, \"+p"),
            ("u  Ctrl+R  .", "Undo, redo, do the last change again"),
            ("v V Ctrl+V  gv  o", "Select letters, lines, a block; the last selection; its other end"),
            ("S{c} (selecting)  I A (block)", "Surround the selection; type on every line of the block"),
            (": & g&", "Commands (:s/a/b/g, :g, :normal\u{2026}); the last :s again, on every line"),
        ],
    ),
];

impl Vi {
    pub fn new() -> Vi {
        Vi {
            normal: false,
            pending: String::new(),
            visual: None,
            replacing: false,
            history_edges: true,
            clip_set: None,
            clip_get: None,
            clip_local: String::new(),
            clip_last: String::new(),
            v_anchor: 0,
            v_head: 0,
            key_buf: vec![],
            want_col: None,
            last_find: None,
            mark_cols: HashMap::new(),
            last_jump: None,
            last_visual: None,
            last_insert: None,
            reg_kind: HashMap::new(),
            last_change: None,
            replaying: false,
            ins: None,
            sel_snap: None,
            undo_stack: vec![],
            redo_stack: vec![],
            last_known: None,
            change_base: None,
            out: None,
        }
    }

    pub fn mode_name(&self) -> &'static str {
        if !self.normal {
            return if self.replacing { "REPLACE" } else { "INSERT" };
        }
        match self.visual {
            Some('v') => "VISUAL",
            Some('V') => "V-LINE",
            Some(BLOCK) => "V-BLOCK",
            _ => "NORMAL",
        }
    }

    pub fn help() -> Vec<(&'static str, Vec<(&'static str, &'static str)>)> {
        HELP.iter().map(|(g, rows)| (*g, rows.to_vec())).collect()
    }

    /// (A new text, not an edit of the old: nothing to undo.)
    pub fn reset(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.change_base = None;
        self.key_buf.clear();
        self.pending.clear();
        self.visual = None;
        self.ins = None;
        self.replacing = false;
        self.last_known = None;
    }

    /// Normal mode (true) or insert mode, as setting ViKeys.viNormal.
    pub fn set_normal(&mut self, on: bool, t: &Text) {
        let changed = self.normal != on;
        self.normal = on;
        if !changed {
            return;
        }
        if on {
            self.end_change(t);
            self.sync_external(t);
        } else if self.visual.is_some() {
            self.visual = None;
        }
    }
    /// The unfinished command dropped (as setting viPending to "").
    pub fn clear_pending(&mut self) {
        self.pending.clear();
        self.key_buf.clear();
    }

    fn message(&mut self, s: &str, error: bool) {
        self.out = Some(ViOut::Message(s.to_string(), error));
    }
    fn msg_e(&mut self, s: &str) {
        if !s.is_empty() {
            self.message(s, s.starts_with('E'));
        }
    }

    fn clamp_cursor(&self, t: &mut Text) {
        if self.visual.is_some() || !self.normal || t.selection().is_some() {
            return;
        }
        let p = t.cursor;
        if p > 0 && p == t.line_end(p) && p > t.line_start(p) {
            t.cursor = t.prev(p);
        }
    }

    // ---- keys ----------------------------------------------------------------
    /// One key; see the module's comment.
    pub fn key(&mut self, e: &KeyEvent, t: &mut Text, ind: &Indent, ex: &mut Ex) -> ViOut {
        if e.kind == KeyEventKind::Release {
            return ViOut::None;
        }
        self.out = None;
        let used = self.handle(e, t, ind, ex);
        match self.out.take() {
            Some(o) => o,
            None if used => ViOut::Handled,
            None => ViOut::None,
        }
    }

    fn handle(&mut self, e: &KeyEvent, t: &mut Text, ind: &Indent, ex: &mut Ex) -> bool {
        if !self.normal {
            return self.insert_key(e, t);
        }
        self.sync_selection(t);
        let tk = token(e);
        if tk.is_empty() {
            return matches!(e.code, KeyCode::Char(_)) && !e.modifiers.contains(KeyModifiers::CONTROL);
        }
        if tk.starts_with("<C-") && self.key_buf.is_empty() && !["<C-r>", "<C-a>", "<C-x>", "<C-v>", "<C-q>"].contains(&tk.as_str()) {
            return false;
        }
        if tk == "<esc>" {
            let had = !self.key_buf.is_empty();
            self.clear_pending();
            if !had && self.visual.is_some() {
                self.end_visual(t);
            }
            return true;
        }
        self.key_buf.push(tk);
        let r = if self.visual.is_some() { parse_visual(&self.key_buf) } else { parse(&self.key_buf) };
        match r {
            Need => self.pending = self.key_buf.concat(),
            Bad => self.clear_pending(),
            Got(r) => {
                self.clear_pending();
                self.run(r, t, ind, ex);
            }
        }
        true
    }

    // Insert mode: Esc (or Ctrl+[) back to normal, R's typing over; the rest is the editor's.
    fn insert_key(&mut self, e: &KeyEvent, t: &mut Text) -> bool {
        let tk = token(e);
        if tk == "<esc>" {
            self.leave_insert(t);
            return true;
        }
        if self.replacing {
            if let KeyCode::Char(c) = e.code {
                if c >= ' ' && !e.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
                    self.overwrite(t, &c.to_string());
                    if let Some(ins) = &mut self.ins {
                        ins.typed.get_or_insert_with(String::new).push(c);
                    }
                    return true;
                }
            }
            if e.code == KeyCode::Backspace {
                let p = t.cursor;
                let ls = t.line_start(p);
                set_cursor(t, if p > ls { t.prev(p) } else { ls });
                return true;
            }
        }
        false
    }
    fn overwrite(&mut self, t: &mut Text, s: &str) {
        for ch in s.chars() {
            let p = t.cursor;
            if p < t.line_end(p) && ch != '\n' {
                let q = t.next(p);
                rep(t, p, q, "");
            }
            let cs = ch.to_string();
            rep(t, p, p, &cs);
            t.cursor = p + cs.len();
        }
    }
    fn enter_insert(&mut self, t: &Text, cmd: Option<Cmd>, count: usize, prefix: String, block: Option<InsBlock>, typed: Option<String>) {
        self.ins = Some(Ins { start: t.cursor, len: t.len(), count: count.max(1), cmd, prefix, block, typed });
        self.set_normal(false, t);
    }
    fn leave_insert(&mut self, t: &mut Text) {
        let mut p = t.cursor;
        if let Some(ins) = self.ins.clone() {
            let typed = match &ins.typed {
                Some(s) => s.clone(),
                None => {
                    if p >= ins.start && t.len() - ins.len == p - ins.start && t.text.is_char_boundary(ins.start) {
                        t.text[ins.start..p].to_string()
                    } else {
                        String::new()
                    }
                }
            };
            if !typed.is_empty() && ins.count > 1 && !self.replacing {
                let more = format!("{}{}", ins.prefix, typed).repeat(ins.count - 1);
                ins_at(t, p, &more);
                p += more.len();
                t.cursor = p;
            }
            if let Some(b) = &ins.block {
                if !typed.is_empty() && !typed.contains('\n') {
                    let mut lines = split_lines(t);
                    let mut l = b.l1 + 1;
                    while l <= b.l2 && l < lines.len() {
                        let mut line = lines[l].clone();
                        let col = if b.append && b.dollar { clen(&line) } else { b.col };
                        if clen(&line) < col {
                            if !b.append {
                                l += 1;
                                continue;
                            }
                            line += &" ".repeat(col - clen(&line));
                        }
                        let i = cidx(&line, col);
                        lines[l] = format!("{}{}{}", &line[..i], typed, &line[i..]);
                        l += 1;
                    }
                    if b.l2 > b.l1 {
                        let keep = t.pos_at_col(t.pos_of_line(b.l1), b.col);
                        let last = b.l2.min(lines.len() - 1);
                        replace_lines(t, b.l1 + 1, last, &lines[b.l1 + 1..=last]);
                        p = t.next(keep);
                        t.cursor = p;
                    }
                }
            }
            if let Some(cmd) = ins.cmd {
                if !self.replaying {
                    self.last_change = Some(LastChange { cmd, text: typed, replace: self.replacing });
                }
            }
        }
        self.last_insert = Some(p);
        self.ins = None;
        self.replacing = false;
        let ls = t.line_start(p);
        set_cursor(t, if p > ls { t.prev(p) } else { p });
        self.set_normal(true, t);
    }

    // ---- running -----------------------------------------------------------------
    fn run(&mut self, r: Cmd, t: &mut Text, ind: &Indent, ex: &mut Ex) {
        if r.name == "u" || r.name == "<C-r>" {
            self.sync_external(t);
            if r.name == "u" {
                self.undo(t, r.count.max(1));
            } else {
                self.redo(t, r.count.max(1));
            }
            self.clamp_cursor(t);
            return;
        }
        self.sync_external(t);
        self.begin_change(t);
        if self.visual.is_some() {
            self.run_visual(r, t, ind, ex);
        } else {
            self.run_normal(r, t, ind, ex);
        }
        if self.normal {
            self.end_change(t);
        }
        self.clamp_cursor(t);
        self.update_visual(t);
    }

    fn run_normal(&mut self, r: Cmd, t: &mut Text, ind: &Indent, ex: &mut Ex) {
        let p = t.cursor;
        let n = r.count.max(1);
        if r.motion.is_none() {
            self.want_col = None;
        }
        if (!r.name.is_empty() || !r.op.is_empty()) && !self.replaying && is_change(&r) && r.name != "." {
            self.last_change = Some(LastChange { cmd: r.clone(), text: String::new(), replace: false });
        }
        if !r.op.is_empty() {
            self.operate(&r, t, ind, ex);
            return;
        }
        if let Some(mo) = &r.motion {
            let Some(m) = self.motion_target(mo, p, r.count, false, t, ex) else { return };
            self.move_to(m, mo, t);
            return;
        }
        let ls = t.line_start(p);
        let le = t.line_end(p);
        let l0 = t.line_of(p);
        let lc = t.line_count();
        let last_l = (lc - 1).min(l0 + n - 1);
        match r.name.as_str() {
            "x" | "<del>" => {
                if p < le {
                    self.delete_range(r.reg, rg(p, t.fwd(p, n, le)), t, ex)
                }
            }
            "X" => {
                if p > ls {
                    self.delete_range(r.reg, rg(t.back(p, n, ls), p), t, ex)
                }
            }
            "s" => self.change(r.reg, rg(p, t.fwd(p, n, le)), Some(r.clone()), t, ex),
            "S" => self.change(r.reg, lines_range(t, l0, last_l), Some(r.clone()), t, ex),
            "C" => {
                let b = t.line_end(t.pos_of_line(last_l));
                self.change(r.reg, rg(p, b), Some(r.clone()), t, ex)
            }
            "D" => {
                let b = t.line_end(t.pos_of_line(last_l));
                self.delete_range(r.reg, rg(p, b), t, ex)
            }
            "Y" => self.yank(r.reg, lines_range(t, l0, last_l), t, ex),
            "p" => self.put(r.reg, true, n, false, t, ex),
            "P" => self.put(r.reg, false, n, false, t, ex),
            "gp" => self.put(r.reg, true, n, true, t, ex),
            "gP" => self.put(r.reg, false, n, true, t, ex),
            "J" => join(t, l0, n.max(2), true),
            "gJ" => join(t, l0, n.max(2), false),
            "~" => {
                let b = t.fwd(p, n, le);
                if b > p {
                    let s = toggle_case(&t.text[p..b]);
                    rep(t, p, b, &s);
                    set_cursor(t, p + s.len());
                }
            }
            "r" => {
                if clen(&t.text[p..le]) < n {
                    return;
                }
                let b = t.fwd(p, n, le);
                if r.ch == "<cr>" {
                    let ind_s = indent_of(t, p);
                    rep(t, p, b, &format!("\n{ind_s}"));
                    set_cursor(t, p + 1 + ind_s.len());
                    return;
                }
                let s = r.ch.repeat(n);
                rep(t, p, b, &s);
                set_cursor(t, p + s.len() - r.ch.len());
            }
            "R" => {
                self.replacing = true;
                self.enter_insert(t, Some(r.clone()), n, String::new(), None, Some(String::new()));
            }
            "i" => self.enter_insert(t, Some(r.clone()), n, String::new(), None, None),
            "a" => {
                set_cursor(t, t.fwd(p, 1, le));
                self.enter_insert(t, Some(r.clone()), n, String::new(), None, None)
            }
            "I" => {
                set_cursor(t, t.first_non_blank(p));
                self.enter_insert(t, Some(r.clone()), n, String::new(), None, None)
            }
            "gI" => {
                set_cursor(t, ls);
                self.enter_insert(t, Some(r.clone()), n, String::new(), None, None)
            }
            "A" => {
                set_cursor(t, le);
                self.enter_insert(t, Some(r.clone()), n, String::new(), None, None)
            }
            "gi" => {
                let q = self.last_insert.map_or(p, |x| t.clamp_pos(x));
                set_cursor(t, q);
                self.enter_insert(t, Some(r.clone()), n, String::new(), None, None)
            }
            "o" => {
                let ind_s = indent_of(t, p);
                ins_at(t, le, &format!("\n{ind_s}"));
                set_cursor(t, le + 1 + ind_s.len());
                self.enter_insert(t, Some(r.clone()), n, format!("\n{ind_s}"), None, None);
            }
            "O" => {
                let ind_s = indent_of(t, p);
                ins_at(t, ls, &format!("{ind_s}\n"));
                set_cursor(t, ls + ind_s.len());
                self.enter_insert(t, Some(r.clone()), n, format!("\n{ind_s}"), None, None);
            }
            "." => self.repeat_change(&r, t, ind, ex),
            "v" | "V" | "<C-v>" | "<C-q>" => self.start_visual(
                match r.name.as_str() {
                    "v" => 'v',
                    "V" => 'V',
                    _ => BLOCK,
                },
                t,
            ),
            "gv" => {
                let Some((mode, a, h)) = self.last_visual else { return };
                self.visual = Some(mode);
                self.v_anchor = t.clamp_pos(a);
                self.v_head = t.clamp_pos(h);
            }
            "*" | "#" => {
                let mut s = p;
                while s < le && !at(t, s).is_some_and(is_word) {
                    s = t.next(s);
                }
                if s >= le {
                    return;
                }
                let (mut a, mut e) = (s, s);
                while a > ls && t.char_before(a).is_some_and(is_word) {
                    a = t.prev(a);
                }
                while e < le && at(t, e).is_some_and(is_word) {
                    e = t.next(e);
                }
                let w = t.text[a..e].to_string();
                self.last_jump = Some(p);
                let mut msg = ex.search(t, &format!("\\<{w}\\>"), r.name == "#");
                let mut k = 1;
                while k < n && !msg.starts_with('E') {
                    msg = ex.search_next(t, false);
                    k += 1;
                }
                self.msg_e(&msg);
            }
            ":" => {
                let init = if r.count > 1 {
                    format!(".,.+{}", r.count - 1)
                } else if r.count == 1 {
                    ".".into()
                } else {
                    String::new()
                };
                self.out = Some(ViOut::OpenCmdLine(':', init));
            }
            "/" | "?" => self.out = Some(ViOut::OpenCmdLine(r.name.chars().next().unwrap(), String::new())),
            "&" | "g&" => {
                let o = ex.run(if r.name == "&" { "s" } else { "%s//~/&" }, t, ind);
                if !o.text.is_empty() {
                    self.message(&o.text, o.error);
                }
            }
            "m" => {
                let Some(c) = r.ch.chars().next().filter(|c| c.is_ascii_alphabetic()) else { return };
                ex.load(t);
                ex.marks.insert(c, l0);
                self.mark_cols.insert(c, t.col_of(p));
            }
            "ds" => self.delete_surround(r.ch.chars().next().unwrap_or(' '), n, t),
            "cs" => self.change_surround(r.ch.chars().next().unwrap_or(' '), &r.target, n, t),
            "<C-a>" => increment(t, p, n as i64),
            "<C-x>" => increment(t, p, -(n as i64)),
            _ => {}
        }
    }

    fn move_to(&mut self, m: Target, mo: &Motion, t: &mut Text) {
        if m.edge != 0 {
            if self.history_edges {
                self.out = Some(ViOut::HistoryStep(m.edge));
            }
            return;
        }
        if m.jump {
            self.last_jump = Some(t.cursor);
        }
        set_cursor(t, m.pos);
        self.want_col = m.col.or(if mo.m == "$" { Some(WANT_END) } else { None });
        self.msg_e(&m.msg);
    }

    // ---- motions: where one goes from p ---------------------------------------
    fn motion_target(&mut self, mo: &Motion, p: usize, count: usize, in_op: bool, t: &mut Text, ex: &mut Ex) -> Option<Target> {
        let n = count.max(1);
        let l0 = t.line_of(p);
        let lc = t.line_count();
        let ls = t.line_start(p);
        let le = t.line_end(p);
        let ch = mo.ch.chars().next().unwrap_or('\0');
        match mo.m.as_str() {
            "h" | "<bs>" => (p > ls).then(|| Target::at(t.back(p, n, ls))),
            "l" | " " => {
                let max = if in_op { le } else { ls.max(if le > ls { t.prev(le) } else { le }) };
                (p < max).then(|| Target::at(t.fwd(p, n, max)))
            }
            "j" | "k" => {
                let dest = if mo.m == "j" { l0 as i64 + n as i64 } else { l0 as i64 - n as i64 };
                if dest < 0 || dest >= lc as i64 {
                    if self.history_edges && !in_op && self.visual.is_none() {
                        return Some(Target { edge: if mo.m == "j" { 1 } else { -1 }, ..Default::default() });
                    }
                    if (mo.m == "j" && l0 == lc - 1) || (mo.m == "k" && l0 == 0) {
                        return None;
                    }
                }
                let d = dest.clamp(0, lc as i64 - 1) as usize;
                let col = self.want_col.unwrap_or_else(|| t.col_of(p));
                let a = t.pos_of_line(d);
                Some(Target { pos: t.pos_at_col(a, col), linewise: true, col: Some(col), ..Default::default() })
            }
            "+" | "<cr>" | "-" | "_" => {
                let dest = match mo.m.as_str() {
                    "-" => l0 as i64 - n as i64,
                    "_" => l0 as i64 + n as i64 - 1,
                    _ => l0 as i64 + n as i64,
                };
                if dest < 0 || dest >= lc as i64 {
                    return None;
                }
                Some(Target { pos: t.first_non_blank(t.pos_of_line(dest as usize)), linewise: true, ..Default::default() })
            }
            "G" | "gg" => {
                let dest = if count > 0 {
                    lc.min(count) - 1
                } else if mo.m == "G" {
                    lc - 1
                } else {
                    0
                };
                Some(Target { pos: t.first_non_blank(t.pos_of_line(dest)), linewise: true, jump: true, ..Default::default() })
            }
            "0" => Some(Target::at(ls)),
            "^" => Some(Target::at(t.first_non_blank(p))),
            "$" => Some(Target { pos: t.line_end(t.pos_of_line((lc - 1).min(l0 + n - 1))), col: Some(WANT_END), ..Default::default() }),
            "g_" => {
                let a = t.pos_of_line((lc - 1).min(l0 + n - 1));
                let mut e = t.line_end(a);
                while e > a && t.char_before(e).is_some_and(is_blank) {
                    e = t.prev(e);
                }
                Some(Target { pos: if e > a { t.prev(e) } else { a }, inclusive: true, ..Default::default() })
            }
            "|" => Some(Target::at(t.fwd(ls, n - 1, le))),
            "w" | "W" | "b" | "B" => {
                let big = mo.m == "W" || mo.m == "B";
                let mut q = p;
                for _ in 0..n {
                    q = if mo.m.eq_ignore_ascii_case("w") { word_fwd(t, q, big) } else { word_back(t, q, big) };
                }
                (q != p).then(|| Target::at(q))
            }
            "e" | "E" => {
                let mut q = p;
                for _ in 0..n {
                    q = word_end_fwd(t, q, mo.m == "E");
                }
                Some(Target { pos: q, inclusive: true, ..Default::default() })
            }
            "ge" | "gE" => {
                let mut q = p;
                for _ in 0..n {
                    q = word_end_back(t, q, mo.m == "gE");
                }
                Some(Target { pos: q, inclusive: true, ..Default::default() })
            }
            "f" | "F" | "t" | "T" => {
                let c = mo.m.chars().next().unwrap();
                let f = find_char(t, c, ch, n, p, false);
                self.last_find = Some((c, ch));
                f
            }
            ";" | "," => {
                let (mut c, fch) = self.last_find?;
                if mo.m == "," {
                    c = match c {
                        'f' => 'F',
                        'F' => 'f',
                        't' => 'T',
                        _ => 't',
                    };
                }
                find_char(t, c, fch, n, p, true)
            }
            "%" => {
                if count > 0 {
                    let dest = lc.min((count * lc).div_ceil(100)).max(1) - 1;
                    return Some(Target { pos: t.first_non_blank(t.pos_of_line(dest)), linewise: true, jump: true, ..Default::default() });
                }
                let q = match_bracket(t, p)?;
                // (From before the bracket, an operator covers from the cursor.)
                Some(Target { pos: q, inclusive: true, jump: true, from: Some(p.min(q)), ..Default::default() })
            }
            "}" | "{" => {
                let mut q = p;
                for _ in 0..n {
                    q = if mo.m == "}" { para_fwd(t, q) } else { para_back(t, q) };
                }
                Some(Target { pos: q, jump: true, ..Default::default() })
            }
            ")" | "(" => {
                let s = sentence_starts(t);
                let mut q = p;
                for _ in 0..n {
                    q = if mo.m == ")" { s.iter().copied().find(|&v| v > q).unwrap_or(t.len()) } else { s.iter().copied().filter(|&v| v < q).last().unwrap_or(0) };
                }
                Some(Target::at(q))
            }
            "`" | "'" => {
                let mut pos = None;
                if ch == '`' || ch == '\'' {
                    pos = self.last_jump;
                } else if ch == '<' || ch == '>' {
                    let (_, a, h) = self.last_visual?;
                    pos = Some(if ch == '<' { a.min(h) } else { a.max(h) });
                } else {
                    ex.load(t);
                    let l = ex.mark_line(t, ch);
                    if l >= 0 {
                        let a = t.pos_of_line(l as usize);
                        pos = Some(t.pos_at_col(a, self.mark_cols.get(&ch).copied().unwrap_or(0)));
                    }
                }
                let Some(pos) = pos else {
                    self.message("E20: Mark not set", true);
                    return None;
                };
                let pos = t.clamp_pos(pos);
                Some(if mo.m == "'" {
                    Target { pos: t.first_non_blank(pos), linewise: true, jump: true, ..Default::default() }
                } else {
                    Target { pos, jump: true, ..Default::default() }
                })
            }
            "n" | "N" => {
                let keep = (t.cursor, t.anchor);
                t.cursor = p;
                let mut msg = String::new();
                for _ in 0..n {
                    msg = ex.search_next(t, mo.m == "N");
                    if msg.starts_with('E') {
                        break;
                    }
                }
                let q = t.cursor;
                t.cursor = keep.0;
                t.anchor = keep.1;
                if msg.starts_with('E') {
                    self.message(&msg, true);
                    return None;
                }
                Some(Target { pos: q, jump: true, msg, ..Default::default() })
            }
            _ => None,
        }
    }

    // ---- operators ---------------------------------------------------------------
    fn operate(&mut self, r: &Cmd, t: &mut Text, ind: &Indent, ex: &mut Ex) {
        let p = t.cursor;
        let counted = r.count > 0 || r.count2 > 0;
        let n = r.count.max(1) * r.count2.max(1);
        let range;
        if r.line {
            let l0 = t.line_of(p);
            let l2 = (t.line_count() - 1).min(l0 + n - 1);
            if r.op == "ys" {
                let a = t.first_non_blank(p);
                let mut b = t.line_end(t.pos_of_line(l2));
                while b > a && t.char_before(b).is_some_and(is_blank) {
                    b = t.prev(b);
                }
                range = rg(a, b);
            } else {
                range = lines_range(t, l0, l2);
            }
        } else if !r.obj.is_empty() {
            let Some(x) = object_range(t, &r.obj, p, n) else { return };
            range = x;
        } else {
            let mo = r.motion.clone().unwrap();
            if r.op == "c" && (mo.m == "w" || mo.m == "W") && at(t, p).is_some() && cls(at(t, p), false) != 0 {
                // (cw: to the word's end, as ce, the word under the cursor counting as the first.)
                let big = mo.m == "W";
                let mut q = p;
                for k in 0..n {
                    if k == 0 && cls(at(t, t.next(q)), big) != cls(at(t, q), big) {
                        continue;
                    }
                    q = word_end_fwd(t, q, big);
                }
                range = rg(p, t.next(q).min(t.len()));
            } else {
                let Some(m) = self.motion_target(&mo, p, if counted { n } else { 0 }, true, t, ex) else { return };
                if m.edge != 0 {
                    return;
                }
                let mut a = p.min(m.pos);
                let mut b = p.max(m.pos);
                if let Some(f) = m.from {
                    a = a.min(f);
                }
                if m.linewise {
                    range = lines_range(t, t.line_of(a), t.line_of(b));
                } else {
                    if m.inclusive {
                        b = t.next(b).min(t.len());
                    } else if m.pos > p && t.line_of(m.pos) > t.line_of(p) {
                        // (An exclusive motion onto a later line stops at the line before's end.)
                        if mo.m == "w" || mo.m == "W" {
                            let e = t.line_start(m.pos) - 1;
                            if e > a {
                                b = e;
                            }
                        } else if m.pos == t.line_start(m.pos) {
                            b = m.pos - 1;
                        }
                    }
                    range = rg(a, b);
                }
            }
        }
        self.apply_op(&r.op, range, r, t, ind, ex);
    }
    fn apply_op(&mut self, op: &str, x: Rg, r: &Cmd, t: &mut Text, ind: &Indent, ex: &mut Ex) {
        let p = t.cursor;
        match op {
            "y" => {
                self.yank(r.reg, x, t, ex);
                if !x.linewise {
                    set_cursor(t, x.a);
                } else if x.a < t.line_start(p) {
                    let c = t.col_of(p);
                    set_cursor(t, t.pos_at_col(x.a, c));
                }
            }
            "d" => self.delete_range(r.reg, x, t, ex),
            "c" => self.change(r.reg, x, Some(r.clone()), t, ex),
            ">" | "<" => {
                let (l1, l2) = (t.line_of(x.a), t.line_of(last_in(t, x.a, x.b)));
                shift_lines(t, ind, l1, l2, if op == ">" { 1 } else { -1 }, 1)
            }
            "gu" | "gU" | "g~" | "g?" => {
                let s = case_op(op, &t.text[x.a..x.b]);
                rep(t, x.a, x.b, &s);
                set_cursor(t, if x.linewise { x.a.max(p.min(t.line_end(x.a))) } else { x.a });
            }
            "ys" => self.surround_range(x, &r.target, t, ind),
            _ => {}
        }
    }

    // ---- registers ---------------------------------------------------------------
    // "+ and "* are the system clipboard; the rest live in Ex (the unnamed one
    // is the text's kill ring).
    fn is_clip(reg: char) -> bool {
        reg == '+' || reg == '*'
    }
    fn clip_put(&mut self, s: &str) {
        match &mut self.clip_set {
            Some(f) => f(s),
            None => self.clip_local = s.to_string(),
        }
        self.clip_last = s.to_string();
    }
    fn clip_take(&mut self) -> String {
        if let Some(f) = &mut self.clip_get {
            if let Some(s) = f() {
                return s;
            }
        }
        self.clip_local.clone()
    }
    fn reg_name(reg: char) -> char {
        if reg == '\0' || reg == '"' {
            '"'
        } else if Self::is_clip(reg) {
            '+'
        } else {
            reg.to_ascii_lowercase()
        }
    }
    fn put_reg(&mut self, reg: char, text: &str, kind: Kind, yank: bool, t: &mut Text, ex: &mut Ex) {
        if reg == '_' {
            return;
        }
        if Self::is_clip(reg) {
            self.clip_put(text);
        }
        ex.put_register(t, if reg == '"' || Self::is_clip(reg) { '\0' } else { reg }, text);
        self.reg_kind.insert('"', kind);
        if reg != '\0' {
            self.reg_kind.insert(Self::reg_name(reg), kind);
        }
        if yank {
            ex.registers.insert('0', text.to_string());
            self.reg_kind.insert('0', kind);
        }
    }
    fn get_reg(&mut self, reg: char, t: &Text, ex: &Ex) -> (String, Kind) {
        let name = Self::reg_name(reg);
        let guess = |s: &str| if s.ends_with('\n') { Kind::Line } else { Kind::Char };
        if name == '+' {
            let s = self.clip_take();
            let k = match self.reg_kind.get(&'+') {
                Some(&k) if s == self.clip_last => k,
                _ => guess(&s),
            };
            return (s, k);
        }
        let text = if name == '"' { t.kill_ring.clone() } else { ex.registers.get(&name).cloned().unwrap_or_default() };
        let k = self.reg_kind.get(&name).copied().unwrap_or_else(|| guess(&text));
        (text, k)
    }
    fn range_text(t: &Text, x: Rg) -> String {
        let mut s = t.text[x.a..x.b].to_string();
        if x.linewise && !s.ends_with('\n') {
            s.push('\n');
        }
        s
    }
    fn kind_of(x: Rg) -> Kind {
        if x.linewise {
            Kind::Line
        } else {
            Kind::Char
        }
    }
    fn yank(&mut self, reg: char, x: Rg, t: &mut Text, ex: &mut Ex) {
        let s = Self::range_text(t, x);
        self.put_reg(reg, &s, Self::kind_of(x), true, t, ex);
    }
    fn delete_range(&mut self, reg: char, x: Rg, t: &mut Text, ex: &mut Ex) {
        let s = Self::range_text(t, x);
        self.put_reg(reg, &s, Self::kind_of(x), false, t, ex);
        let mut a = x.a;
        if x.linewise && x.b == t.len() && byte(t, x.b.wrapping_sub(1)) != Some(b'\n') && a > 0 {
            a -= 1;
        }
        rep(t, a, x.b, "");
        let q = if x.linewise { t.first_non_blank(if a == x.a { a } else { t.line_start(a) }.min(t.len())) } else { a };
        set_cursor(t, q);
    }
    fn change(&mut self, reg: char, x: Rg, cmd: Option<Cmd>, t: &mut Text, ex: &mut Ex) {
        let s = Self::range_text(t, x);
        self.put_reg(reg, &s, Self::kind_of(x), false, t, ex);
        if x.linewise {
            let ind_s = indent_of(t, x.a);
            let nl = if x.b > x.a && byte(t, x.b - 1) == Some(b'\n') { "\n" } else { "" };
            rep(t, x.a, x.b, &format!("{ind_s}{nl}"));
            set_cursor(t, x.a + ind_s.len());
        } else {
            rep(t, x.a, x.b, "");
            set_cursor(t, x.a);
        }
        self.enter_insert(t, cmd, 1, String::new(), None, None);
    }
    fn put(&mut self, reg: char, after: bool, n: usize, g: bool, t: &mut Text, ex: &mut Ex) {
        let (text, kind) = self.get_reg(reg, t, ex);
        if text.is_empty() {
            return;
        }
        let p = t.cursor;
        match kind {
            Kind::Line => {
                let body = if text.ends_with('\n') { text.clone() } else { text.clone() + "\n" }.repeat(n);
                if after {
                    let e = t.line_end(p);
                    if e >= t.len() {
                        ins_at(t, e, &format!("\n{}", &body[..body.len() - 1]));
                        let q = if g { t.len() } else { t.first_non_blank(e + 1) };
                        set_cursor(t, q);
                    } else {
                        ins_at(t, e + 1, &body);
                        let q = if g { e + 1 + body.len() } else { t.first_non_blank(e + 1) };
                        set_cursor(t, q);
                    }
                } else {
                    let a = t.line_start(p);
                    ins_at(t, a, &body);
                    let q = if g { a + body.len() } else { t.first_non_blank(a) };
                    set_cursor(t, q);
                }
            }
            Kind::Block => self.put_block(&text, after, n, t),
            Kind::Char => {
                let body = text.repeat(n);
                let at = if after && p < t.line_end(p) { t.next(p) } else { p };
                ins_at(t, at, &body);
                let q = if g {
                    at + body.len()
                } else if body.contains('\n') {
                    at
                } else {
                    t.prev(at + body.len())
                };
                set_cursor(t, q);
            }
        }
    }
    fn put_block(&mut self, text: &str, after: bool, n: usize, t: &mut Text) {
        let p = t.cursor;
        let parts: Vec<&str> = text.split('\n').collect();
        let w = parts.iter().map(|s| clen(s)).max().unwrap_or(0);
        let l0 = t.line_of(p);
        let col = t.col_of(if after && p < t.line_end(p) { t.next(p) } else { p });
        let mut lines = split_lines(t);
        let last = l0 + parts.len() - 1;
        while lines.len() <= last {
            lines.push(String::new());
        }
        for (i, piece) in parts.iter().enumerate() {
            let mut line = lines[l0 + i].clone();
            if clen(&line) < col {
                line += &" ".repeat(col - clen(&line));
            }
            let mut s = String::new();
            for k in 0..n {
                s.push_str(piece);
                if k < n - 1 || clen(&line) > col {
                    s.push_str(&" ".repeat(w - clen(piece)));
                }
            }
            let ci = cidx(&line, col);
            lines[l0 + i] = format!("{}{}{}", &line[..ci], s, &line[ci..]);
        }
        let old_last = last.min(t.line_count() - 1);
        let a = t.pos_of_line(l0);
        let b = t.line_end(t.pos_of_line(old_last));
        rep(t, a, b, &lines[l0..=last].join("\n"));
        set_cursor(t, t.pos_at_col(a, col));
    }

    // ---- surround ----------------------------------------------------------------
    fn surround_range(&mut self, x: Rg, target: &str, t: &mut Text, ind: &Indent) {
        let (o, c) = surround_pair(target);
        if x.linewise {
            let l1 = t.line_of(x.a);
            let l2 = t.line_of(last_in(t, x.a, x.b));
            let ind_s = indent_of(t, x.a);
            let unit = ind.unit();
            let mut out = vec![format!("{ind_s}{}", o.trim())];
            for s in &split_lines(t)[l1..=l2] {
                out.push(if s.is_empty() { s.clone() } else { format!("{unit}{s}") });
            }
            out.push(format!("{ind_s}{}", c.trim()));
            replace_lines(t, l1, l2, &out);
            set_cursor(t, t.first_non_blank(t.pos_of_line(l1)));
            return;
        }
        let (mut a, mut b) = (x.a, x.b);
        while b > a && t.char_before(b).is_some_and(ws) {
            b = t.prev(b);
        }
        while a < b && at(t, a).is_some_and(ws) {
            a = t.next(a);
        }
        let s = format!("{o}{}{c}", &t.text[a..b]);
        rep(t, a, b, &s);
        set_cursor(t, a);
    }
    // The pair `ch` names around the cursor: (a, b, ia, ib) (outer and inner).
    fn surround_find(ch: char, p: usize, n: usize, t: &Text) -> Option<(usize, usize, usize, usize)> {
        let open = match ch {
            '(' | ')' | 'b' => Some(b'('),
            '[' | ']' | 'r' => Some(b'['),
            '{' | '}' | 'B' => Some(b'{'),
            '<' | '>' | 'a' => Some(b'<'),
            _ => None,
        };
        if let Some(open) = open {
            let (o, c) = bracket_pair(t, open, pair_of(open), p, n)?;
            let (mut ia, mut ib) = (o + 1, c);
            if "([{".contains(ch) {
                while ia < ib && at(t, ia).is_some_and(is_blank) {
                    ia += 1;
                }
                while ib > ia && t.char_before(ib).is_some_and(is_blank) {
                    ib -= 1;
                }
            }
            return Some((o, c + 1, ia, ib));
        }
        if ch == 't' {
            let x = tag_object(t, p, true, n)?;
            let y = tag_object(t, p, false, n)?;
            return Some((x.a, x.b, y.a, y.b));
        }
        let q = quote_object(t, ch, p, false)?;
        let l = ch.len_utf8();
        Some((q.a - l, q.b + l, q.a, q.b))
    }
    fn delete_surround(&mut self, ch: char, n: usize, t: &mut Text) {
        let Some((a, b, ia, ib)) = Self::surround_find(ch, t.cursor, n, t) else { return };
        let inner = t.text[ia..ib].to_string();
        rep(t, a, b, &inner);
        set_cursor(t, a);
    }
    fn change_surround(&mut self, ch: char, target: &str, n: usize, t: &mut Text) {
        let Some((a, b, ia, ib)) = Self::surround_find(ch, t.cursor, n, t) else { return };
        let (o, c) = surround_pair(target);
        let s = format!("{o}{}{c}", &t.text[ia..ib]);
        rep(t, a, b, &s);
        set_cursor(t, a);
    }

    // ---- dot ------------------------------------------------------------------------
    fn repeat_change(&mut self, r: &Cmd, t: &mut Text, ind: &Indent, ex: &mut Ex) {
        let Some(lc) = self.last_change.clone() else { return };
        let mut c = lc.cmd.clone();
        if r.count > 0 {
            c.count = r.count;
            c.count2 = 0;
        }
        if r.reg != '\0' {
            c.reg = r.reg;
        }
        self.replaying = true;
        self.run_normal(c, t, ind, ex);
        if !self.normal {
            if lc.replace {
                self.overwrite(t, &lc.text);
            } else if !lc.text.is_empty() {
                let p = t.cursor;
                ins_at(t, p, &lc.text);
                t.cursor = p + lc.text.len();
            }
            if let Some(ins) = &mut self.ins {
                ins.typed = Some(lc.text.clone());
            }
            self.leave_insert(t);
        }
        self.replaying = false;
    }

    // ---- visual ---------------------------------------------------------------------
    fn start_visual(&mut self, mode: char, t: &Text) {
        self.visual = Some(mode);
        self.v_anchor = t.cursor;
        self.v_head = t.cursor;
    }
    fn end_visual(&mut self, t: &mut Text) {
        self.last_visual = Some((self.visual.unwrap_or('v'), self.v_anchor, self.v_head));
        let h = t.clamp_pos(self.v_head);
        self.visual = None;
        t.anchor = None;
        set_cursor(t, h);
        self.sel_snap = None;
        self.clamp_cursor(t);
    }
    // (An operator done: the selection is gone, the cursor where the operator left it.)
    fn drop_visual(&mut self) {
        self.last_visual = Some((self.visual.unwrap_or('v'), self.v_anchor, self.v_head));
        self.visual = None;
        self.sel_snap = None;
    }
    fn visual_range(&self, t: &Text) -> VRange {
        let va = t.clamp_pos(self.v_anchor);
        let vh = t.clamp_pos(self.v_head);
        let (a, h) = (va.min(vh), va.max(vh));
        match self.visual {
            Some('v') => VRange::R(rg(a, t.next(h).min(t.len()))),
            Some('V') => VRange::R(lines_range(t, t.line_of(a), t.line_of(h))),
            _ => {
                let (ca, ch) = (t.col_of(va), t.col_of(vh));
                VRange::B(Blk { l1: t.line_of(a), l2: t.line_of(h), c1: ca.min(ch), c2: ca.max(ch), dollar: self.want_col == Some(WANT_END) })
            }
        }
    }
    fn update_visual(&mut self, t: &mut Text) {
        if self.visual.is_none() {
            return;
        }
        self.v_head = t.clamp_pos(self.v_head);
        self.v_anchor = t.clamp_pos(self.v_anchor);
        match self.visual_range(t) {
            VRange::B(_) => {
                t.anchor = None;
                t.cursor = self.v_head;
            }
            VRange::R(x) => {
                if self.v_head >= self.v_anchor {
                    t.anchor = Some(x.a);
                    t.cursor = x.b;
                } else {
                    t.anchor = Some(x.b);
                    t.cursor = x.a;
                }
            }
        }
        self.sel_snap = Some(Self::snap(t));
    }
    fn snap(t: &Text) -> (usize, usize, usize) {
        let (s, e) = t.selection().unwrap_or((t.cursor, t.cursor));
        (s, e, t.cursor)
    }
    // A selection made otherwise (the mouse, Shift+arrows) is a visual one; a click ends it.
    fn sync_selection(&mut self, t: &Text) {
        let (s, e, c) = Self::snap(t);
        if self.visual.is_some() && self.sel_snap == Some((s, e, c)) {
            return;
        }
        if self.visual == Some(BLOCK) && s == e && self.sel_snap.is_some_and(|x| x.2 == c) {
            return;
        }
        if s == e {
            if self.visual.is_some() {
                self.visual = None;
                self.sel_snap = None;
            }
            return;
        }
        self.visual = Some('v');
        if c == e {
            self.v_anchor = s;
            self.v_head = t.prev(e).max(s);
        } else {
            self.v_anchor = t.prev(e).max(s);
            self.v_head = s;
        }
        self.sel_snap = Some((s, e, c));
    }

    fn run_visual(&mut self, r: Cmd, t: &mut Text, ind: &Indent, ex: &mut Ex) {
        let n = r.count.max(1);
        if let Some(mo) = &r.motion {
            let Some(m) = self.motion_target(mo, self.v_head, r.count, false, t, ex) else { return };
            if m.edge != 0 {
                return;
            }
            if m.jump {
                self.last_jump = Some(self.v_head);
            }
            self.v_head = m.pos;
            self.want_col = m.col.or(if mo.m == "$" { Some(WANT_END) } else { None });
            self.msg_e(&m.msg);
            return;
        }
        if !r.obj.is_empty() {
            let mut o = object_range(t, &r.obj, self.v_head, n);
            if let (Some(x), VRange::R(cur)) = (o, self.visual_range(t)) {
                // (The same object selected already: the one around it.)
                if self.v_anchor != self.v_head && x.a == cur.a && x.b == cur.b {
                    o = object_range(t, &r.obj, self.v_head, n + 1);
                }
            }
            let Some(o) = o else { return };
            if o.linewise && self.visual == Some('v') {
                self.visual = Some('V');
            }
            let (lo, hi) = (self.v_anchor.min(self.v_head), self.v_anchor.max(self.v_head));
            if self.v_anchor == self.v_head || (o.a <= lo && o.b > hi) {
                self.v_anchor = o.a;
                self.v_head = last_in(t, o.a, o.b);
            } else if self.v_head > self.v_anchor {
                self.v_head = self.v_head.max(last_in(t, o.a, o.b));
            } else {
                self.v_head = self.v_head.min(o.a);
            }
            return;
        }
        let vr = self.visual_range(t);
        let mode = self.visual;
        match r.name.as_str() {
            "o" => {
                std::mem::swap(&mut self.v_anchor, &mut self.v_head);
                return;
            }
            "O" => {
                if mode != Some(BLOCK) {
                    std::mem::swap(&mut self.v_anchor, &mut self.v_head);
                    return;
                }
                let (ca, ch) = (t.col_of(self.v_anchor), t.col_of(self.v_head));
                self.v_anchor = t.pos_at_col(t.line_start(self.v_anchor), ch);
                self.v_head = t.pos_at_col(t.line_start(self.v_head), ca);
                return;
            }
            "v" | "V" | "<C-v>" | "<C-q>" => {
                let m = match r.name.as_str() {
                    "v" => 'v',
                    "V" => 'V',
                    _ => BLOCK,
                };
                if Some(m) == self.visual {
                    self.end_visual(t);
                } else {
                    self.visual = Some(m);
                }
                return;
            }
            "gv" => {
                let Some(lv) = self.last_visual else { return };
                let cur = (self.visual.unwrap_or('v'), self.v_anchor, self.v_head);
                self.visual = Some(lv.0);
                self.v_anchor = t.clamp_pos(lv.1);
                self.v_head = t.clamp_pos(lv.2);
                self.last_visual = Some(cur);
                return;
            }
            ":" => {
                self.drop_visual();
                // (The selection stays for '<,'>: a block's lines as one.)
                match vr {
                    VRange::B(b) => {
                        let a = t.pos_of_line(b.l1);
                        let e = t.line_end(t.pos_of_line(b.l2));
                        t.select(a, e);
                    }
                    VRange::R(x) => t.select(x.a, x.b),
                }
                self.out = Some(ViOut::OpenCmdLine(':', "'<,'>".into()));
                return;
            }
            _ => {}
        }
        self.drop_visual();
        t.anchor = None;
        let x = match vr {
            VRange::B(b) => {
                self.block_op(&r, b, n, t, ind, ex);
                return;
            }
            VRange::R(x) => x,
        };
        let (l1, l2) = (t.line_of(x.a), t.line_of(last_in(t, x.a, x.b)));
        let lines = lines_range(t, l1, l2);
        match r.name.as_str() {
            "d" | "x" | "<del>" => self.delete_range(r.reg, x, t, ex),
            "X" | "D" => self.delete_range(r.reg, lines, t, ex),
            "c" | "s" => self.change(r.reg, x, None, t, ex),
            "C" | "R" => self.change(r.reg, lines, None, t, ex),
            "y" => {
                self.yank(r.reg, x, t, ex);
                set_cursor(t, x.a);
            }
            "Y" => {
                self.yank(r.reg, lines, t, ex);
                set_cursor(t, x.a);
            }
            ">" | "<" => shift_lines(t, ind, l1, l2, if r.name == ">" { 1 } else { -1 }, n),
            "~" | "u" | "U" | "g~" | "gu" | "gU" | "g?" => {
                let op = if r.name == "~" { "g~" } else { r.name.as_str() };
                let s = case_op(op, &t.text[x.a..x.b]);
                rep(t, x.a, x.b, &s);
                set_cursor(t, x.a);
            }
            "J" | "gJ" => join(t, l1, (l2 - l1 + 1).max(2), r.name == "J"),
            "r" => {
                let s: String = t.text[x.a..x.b].chars().map(|c| if c == '\n' { "\n".to_string() } else { r.ch.clone() }).collect();
                rep(t, x.a, x.b, &s);
                set_cursor(t, x.a);
            }
            "p" | "P" => {
                let (text, kind) = self.get_reg(r.reg, t, ex);
                if r.name == "p" {
                    let s = Self::range_text(t, x);
                    self.put_reg('\0', &s, Self::kind_of(x), false, t, ex);
                }
                let mut body = text;
                if kind == Kind::Line && !x.linewise {
                    body = format!("\n{}\n", body.strip_suffix('\n').unwrap_or(&body));
                } else if kind != Kind::Line && x.linewise {
                    body.push('\n');
                }
                rep(t, x.a, x.b, &body);
                set_cursor(t, x.a);
            }
            "I" => {
                set_cursor(t, if x.linewise { t.first_non_blank(x.a) } else { x.a });
                self.enter_insert(t, None, 1, String::new(), None, None);
            }
            "A" => {
                set_cursor(t, if x.linewise { t.line_end(last_in(t, x.a, x.b)) } else { x.b });
                self.enter_insert(t, None, 1, String::new(), None, None);
            }
            "S" => self.surround_range(x, &r.target, t, ind),
            "<C-a>" | "<C-x>" => {
                for l in l1..=l2 {
                    let a = t.pos_of_line(l);
                    increment(t, if l == l1 { x.a } else { a }, if r.name == "<C-a>" { n as i64 } else { -(n as i64) });
                }
                set_cursor(t, x.a);
            }
            _ => {}
        }
    }
    // A block's operators: each line's part of it.
    fn block_op(&mut self, r: &Cmd, b: Blk, n: usize, t: &mut Text, ind: &Indent, ex: &mut Ex) {
        let lines = split_lines(t);
        // (Each line's part, as byte offsets in the line.)
        let seg = |l: usize| {
            let s = &lines[l];
            let len = clen(s);
            let a = b.c1.min(len);
            let e = if b.dollar { len } else { (b.c2 + 1).min(len) };
            (cidx(s, a), cidx(s, e))
        };
        let parts: Vec<String> = (b.l1..=b.l2).map(|l| {
            let (x, y) = seg(l);
            lines[l][x..y].to_string()
        }).collect();
        let at = t.pos_of_line(b.l1) + cidx(&lines[b.l1], b.c1);
        let each = |t: &mut Text, f: &dyn Fn(&str, usize, usize) -> String| {
            let out: Vec<String> = (b.l1..=b.l2).map(|l| {
                let (x, y) = seg(l);
                f(&lines[l], x, y)
            }).collect();
            replace_lines(t, b.l1, b.l2, &out);
        };
        let joined = parts.join("\n");
        let blk = || Some(InsBlock { l1: b.l1, l2: b.l2, col: b.c1, append: false, dollar: false });
        match r.name.as_str() {
            "y" | "Y" => {
                self.put_reg(r.reg, &joined, Kind::Block, true, t, ex);
                set_cursor(t, at);
            }
            "d" | "x" | "<del>" => {
                self.put_reg(r.reg, &joined, Kind::Block, false, t, ex);
                each(t, &|s, x, y| format!("{}{}", &s[..x], &s[y..]));
                set_cursor(t, at);
            }
            "D" | "X" => {
                self.put_reg(r.reg, &joined, Kind::Block, false, t, ex);
                each(t, &|s, x, _| s[..x].to_string());
                set_cursor(t, at);
            }
            "c" | "s" => {
                self.put_reg(r.reg, &joined, Kind::Block, false, t, ex);
                each(t, &|s, x, y| format!("{}{}", &s[..x], &s[y..]));
                set_cursor(t, at);
                self.enter_insert(t, None, 1, String::new(), blk(), None);
            }
            "C" => {
                each(t, &|s, x, _| s[..x].to_string());
                set_cursor(t, at);
                self.enter_insert(t, None, 1, String::new(), blk(), None);
            }
            "I" => {
                set_cursor(t, at);
                self.enter_insert(t, None, 1, String::new(), blk(), None);
            }
            "A" => {
                let s = &lines[b.l1];
                let col = if b.dollar { clen(s) } else { b.c2 + 1 };
                if clen(s) < col {
                    let padded = format!("{s}{}", " ".repeat(col - clen(s)));
                    replace_lines(t, b.l1, b.l1, &[padded]);
                }
                set_cursor(t, t.pos_at_col(t.pos_of_line(b.l1), col));
                self.enter_insert(t, None, 1, String::new(), Some(InsBlock { l1: b.l1, l2: b.l2, col, append: true, dollar: b.dollar }), None);
            }
            "r" => {
                let ch = r.ch.clone();
                each(t, &|s, x, y| format!("{}{}{}", &s[..x], s[x..y].chars().map(|_| ch.as_str()).collect::<String>(), &s[y..]));
                set_cursor(t, at);
            }
            "~" | "u" | "U" | "g~" | "gu" | "gU" | "g?" => {
                let op = if r.name == "~" { "g~".to_string() } else { r.name.clone() };
                each(t, &|s, x, y| format!("{}{}{}", &s[..x], case_op(&op, &s[x..y]), &s[y..]));
                set_cursor(t, at);
            }
            ">" | "<" => shift_lines(t, ind, b.l1, b.l2, if r.name == ">" { 1 } else { -1 }, n),
            "J" | "gJ" => join(t, b.l1, (b.l2 - b.l1 + 1).max(2), r.name == "J"),
            "S" => {
                let (o, c) = surround_pair(&r.target);
                each(t, &|s, x, y| if y > x { format!("{}{o}{}{c}{}", &s[..x], &s[x..y], &s[y..]) } else { s.to_string() });
                set_cursor(t, at);
            }
            "p" | "P" => {
                let (text, kind) = self.get_reg(r.reg, t, ex);
                each(t, &|s, x, y| format!("{}{}", &s[..x], &s[y..]));
                set_cursor(t, at);
                if kind == Kind::Block {
                    self.put_block(&text, false, 1, t);
                } else {
                    ins_at(t, at, &text);
                }
            }
            _ => {}
        }
    }

    /// What to draw as vi's: a block selection's part of each line, or the
    /// block cursor in normal mode (the char under it).
    pub fn block_ranges(&self, t: &Text) -> Vec<(usize, usize)> {
        if !self.normal {
            return vec![];
        }
        match self.visual {
            None => {
                let c = t.clamp_pos(t.cursor);
                let e = if at(t, c).is_some_and(|x| x != '\n') { t.next(c) } else { c };
                vec![(c, e)]
            }
            Some(BLOCK) => {
                let VRange::B(r) = self.visual_range(t) else { return vec![] };
                let mut out = vec![];
                for l in r.l1..=r.l2 {
                    let ls = t.pos_of_line(l);
                    let len = clen(&t.text[ls..t.line_end(ls)]);
                    let c1 = r.c1.min(len);
                    let c2 = if r.dollar { len } else { (r.c2 + 1).min(len) };
                    if c2 <= c1 && len < r.c1 {
                        continue;
                    }
                    out.push((t.pos_at_col(ls, c1), t.pos_at_col(ls, c2)));
                }
                out
            }
            _ => vec![],
        }
    }

    // ---- undo -----------------------------------------------------------------------
    fn diff_of(o: &str, n: &str) -> Diff {
        let (ob, nb) = (o.as_bytes(), n.as_bytes());
        let mut a = 0;
        while a < ob.len() && a < nb.len() && ob[a] == nb[a] {
            a += 1;
        }
        while !o.is_char_boundary(a) || !n.is_char_boundary(a) {
            a -= 1;
        }
        let mut b = 0;
        while b < ob.len() - a && b < nb.len() - a && ob[ob.len() - 1 - b] == nb[nb.len() - 1 - b] {
            b += 1;
        }
        while !o.is_char_boundary(ob.len() - b) || !n.is_char_boundary(nb.len() - b) {
            b -= 1;
        }
        Diff { a, del: o[a..ob.len() - b].to_string(), ins: n[a..nb.len() - b].to_string() }
    }
    fn push_undo(&mut self, o: &str, n: &str) {
        if o == n {
            return;
        }
        self.undo_stack.push(Self::diff_of(o, n));
        if self.undo_stack.len() > 500 {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }
    fn begin_change(&mut self, t: &Text) {
        if self.change_base.is_none() {
            self.change_base = Some(t.text.clone());
        }
    }
    fn end_change(&mut self, t: &Text) {
        let Some(base) = self.change_base.take() else { return };
        self.push_undo(&base, &t.text);
        self.last_known = Some(t.text.clone());
    }
    // (A change from elsewhere, a : command, the history, typing begun in insert mode: one step.)
    fn sync_external(&mut self, t: &Text) {
        let Some(known) = &self.last_known else {
            self.last_known = Some(t.text.clone());
            return;
        };
        if self.change_base.is_none() && *known != t.text {
            let k = known.clone();
            self.push_undo(&k, &t.text);
            self.last_known = Some(t.text.clone());
        }
    }
    fn undo(&mut self, t: &mut Text, n: usize) {
        if self.visual.is_some() {
            self.end_visual(t);
        }
        let mut done = 0;
        while done < n {
            let Some(u) = self.undo_stack.pop() else { break };
            rep(t, u.a, u.a + u.ins.len(), &u.del);
            set_cursor(t, u.a);
            self.redo_stack.push(u);
            done += 1;
        }
        self.last_known = Some(t.text.clone());
        if done == 0 {
            self.message("Already at oldest change", false);
        }
    }
    fn redo(&mut self, t: &mut Text, n: usize) {
        if self.visual.is_some() {
            self.end_visual(t);
        }
        let mut done = 0;
        while done < n {
            let Some(u) = self.redo_stack.pop() else { break };
            rep(t, u.a, u.a + u.del.len(), &u.ins);
            set_cursor(t, u.a);
            self.undo_stack.push(u);
            done += 1;
        }
        self.last_known = Some(t.text.clone());
        if done == 0 {
            self.message("Already at newest change", false);
        }
    }
}

// ---- other changes ---------------------------------------------------------------
fn shift_lines(t: &mut Text, ind: &Indent, l1: usize, l2: usize, dir: i32, times: usize) {
    let unit = ind.unit();
    let ul = clen(&unit).max(1);
    let lines = split_lines(t);
    let mut out = vec![];
    for s in &lines[l1..=l2.min(lines.len() - 1)] {
        let mut s = s.clone();
        for _ in 0..times {
            if dir > 0 {
                if !s.is_empty() {
                    s = format!("{unit}{s}");
                }
            } else if s.starts_with('\t') {
                s.remove(0);
            } else {
                let sp = s.len() - s.trim_start_matches(' ').len();
                s.drain(..sp.min(ul));
            }
        }
        out.push(s);
    }
    replace_lines(t, l1, l2, &out);
    set_cursor(t, t.first_non_blank(t.pos_of_line(l1)));
}
fn join(t: &mut Text, l0: usize, count: usize, spaces: bool) {
    let lines = split_lines(t);
    let last = (lines.len() - 1).min(l0 + count - 1);
    if last == l0 {
        return;
    }
    let mut s = lines[l0].clone();
    let mut at = 0;
    for nx in &lines[l0 + 1..=last] {
        let mut nx = nx.clone();
        at = s.len();
        if spaces {
            nx = nx.trim_start_matches(ws).to_string();
            if !nx.is_empty() && !s.is_empty() && !s.ends_with([' ', '\t']) && !nx.starts_with(')') {
                nx = format!(" {nx}");
            }
        }
        s += &nx;
    }
    replace_lines(t, l0, last, &[s]);
    set_cursor(t, t.pos_of_line(l0) + at);
}
// The number at or after p on its line (0x hex or decimal), plus delta.
fn increment(t: &mut Text, p: usize, delta: i64) {
    let ls = t.line_start(p);
    let le = t.line_end(p);
    let line = t.text[ls..le].to_string();
    let b = line.as_bytes();
    // /0[xX][0-9a-fA-F]+|-?\d+/g: the first ending past p.
    let mut found = None;
    let mut i = 0;
    while i < b.len() {
        let mut m = None;
        if b[i] == b'0' && matches!(b.get(i + 1), Some(b'x' | b'X')) && b.get(i + 2).is_some_and(|c| c.is_ascii_hexdigit()) {
            let mut j = i + 2;
            while b.get(j).is_some_and(|c| c.is_ascii_hexdigit()) {
                j += 1;
            }
            m = Some(j);
        } else {
            let mut j = i;
            if b[j] == b'-' {
                j += 1;
            }
            let st = j;
            while b.get(j).is_some_and(|c| c.is_ascii_digit()) {
                j += 1;
            }
            if j > st {
                m = Some(j);
            }
        }
        match m {
            Some(j) => {
                if ls + j > p {
                    found = Some((i, j));
                    break;
                }
                i = j;
            }
            None => i += 1,
        }
    }
    let Some((mut idx, end)) = found else { return };
    let mut s = &line[idx..end];
    if s.starts_with('-') && idx > 0 && line[..idx].chars().next_back().is_some_and(is_word) {
        s = &s[1..];
        idx += 1;
    }
    let out = if s.len() > 2 && (s.starts_with("0x") || s.starts_with("0X")) {
        let digits = &s[2..];
        let v = u128::from_str_radix(digits, 16).unwrap_or(u128::MAX) as i128;
        let m = 1i128 << 32;
        let v = ((v + delta as i128) % m + m) % m;
        let mut o = format!("{v:x}");
        if digits.bytes().any(|c| (b'A'..=b'F').contains(&c)) && !digits.bytes().any(|c| (b'a'..=b'f').contains(&c)) {
            o = o.to_uppercase();
        }
        format!("{}{:0>w$}", &s[..2], o, w = digits.len())
    } else {
        let neg = s.starts_with('-');
        let digits = if neg { &s[1..] } else { s };
        let v = s.parse::<i128>().unwrap_or(0) + delta as i128;
        if digits.len() > 1 && digits.starts_with('0') {
            format!("{}{:0>w$}", if v < 0 { "-" } else { "" }, v.abs(), w = digits.len())
        } else {
            v.to_string()
        }
    };
    let a = ls + idx;
    rep(t, a, a + s.len(), &out);
    set_cursor(t, a + out.len() - 1);
}

// ---- the command line (ExLine.qml) ------------------------------------------------
/// vi's command line under the editor: ":" for Ex commands, "/" and "?" to
/// search. While a command's name is typed, the commands it could be, each
/// with what it does, show above it (↑/↓ pick one, Tab completes it); ↑/↓
/// otherwise go through the ones run before; Enter runs, Esc (or Backspace
/// on nothing) closes. What a command says shows here afterwards, until the
/// next key in the editor.
pub struct CmdLine {
    /// ':' '/' '?' while open, '\0' closed.
    pub kind: char,
    pub text: String,
    pub cursor: usize,
    pub message: Option<(String, bool)>,
    /// (name, with its short form if it has one; what it does) above the line.
    pub choices: Vec<(String, String)>,
    pub pick: usize,
    completions: Vec<Completion>,
    history_at: Option<usize>,
    message_at: Option<Instant>,
}

impl Default for CmdLine {
    fn default() -> Self {
        Self::new()
    }
}

impl CmdLine {
    pub fn new() -> CmdLine {
        CmdLine { kind: '\0', text: String::new(), cursor: 0, message: None, choices: vec![], pick: 0, completions: vec![], history_at: None, message_at: None }
    }
    pub fn is_open(&self) -> bool {
        self.kind != '\0'
    }
    /// What it shows: the line ("kind" + text) while open, else the message.
    pub fn visible(&self) -> bool {
        self.is_open() || self.message.is_some()
    }
    pub fn open(&mut self, kind: char, initial: &str, ex: &Ex) {
        self.kind = kind;
        self.message = None;
        self.history_at = None;
        self.set_text(initial.to_string(), ex);
    }
    pub fn close(&mut self) {
        self.kind = '\0';
        self.choices.clear();
        self.completions.clear();
    }
    pub fn show(&mut self, text: &str, error: bool) {
        self.message = Some((text.to_string(), error));
        self.message_at = Some(Instant::now());
    }
    /// (A key in the editor: what a command said goes.)
    pub fn dismiss(&mut self) {
        if !self.is_open() {
            self.message = None;
        }
    }
    /// A one-line message goes after 6 s (true: it went, redraw).
    pub fn expire(&mut self, now: Instant) -> bool {
        match (&self.message, self.message_at) {
            (Some((m, _)), Some(at)) if !m.contains('\n') && now.duration_since(at) >= Duration::from_secs(6) => {
                self.message = None;
                true
            }
            _ => false,
        }
    }
    /// Opens or shows what vi asked for (true: it was one of those).
    pub fn apply(&mut self, out: &ViOut, ex: &Ex) -> bool {
        match out {
            ViOut::Message(m, e) => self.show(m, *e),
            ViOut::OpenCmdLine(k, init) => self.open(*k, init, ex),
            _ => return false,
        }
        true
    }
    fn set_text(&mut self, s: String, ex: &Ex) {
        self.text = s;
        self.cursor = self.text.len();
        self.changed(ex);
    }
    fn changed(&mut self, ex: &Ex) {
        self.pick = 0;
        self.completions = if self.kind == ':' { ex.completions(&self.text) } else { vec![] };
        self.choices = self
            .completions
            .iter()
            .map(|c| (if c.short != c.name { format!("{}  ({})", c.name, c.short) } else { c.name.clone() }, c.help.clone()))
            .collect();
    }
    /// Text pasted into it.
    pub fn insert(&mut self, s: &str, ex: &Ex) {
        let s = s.replace('\n', " ");
        self.text.insert_str(self.cursor, &s);
        self.cursor += s.len();
        self.changed(ex);
    }
    fn complete(&mut self, ex: &Ex) {
        let Some(c) = self.completions.get(self.pick.min(self.completions.len().saturating_sub(1))).cloned() else { return };
        let sp = if ["<", ">", "=", "#"].contains(&c.name.as_str()) { "" } else { " " };
        self.set_text(format!("{}{}{sp}", c.head, c.name), ex);
    }
    fn run(&mut self, t: &mut Text, ind: &Indent, vi: &mut Vi, ex: &mut Ex) -> ViOut {
        let k = self.kind;
        let text = self.text.clone();
        self.close();
        if k == '/' || k == '?' {
            t.anchor = None;
            let r = ex.search(t, &text, k == '?');
            if !r.is_empty() {
                self.show(&r, r.starts_with('E'));
            }
            return ViOut::Handled;
        }
        let exo = ex.run_vi(&text, t, ind, Some(vi));
        // (The selection '<,'> was for is done with: not a visual one again.)
        t.anchor = None;
        if !exo.actions.is_empty() {
            if !exo.text.is_empty() {
                self.show(&exo.text, exo.error);
            }
            return ViOut::Actions(exo.actions);
        }
        let out = exo.to_vi();
        match out {
            ViOut::Message(m, e) => {
                self.show(&m, e);
                ViOut::Handled
            }
            ViOut::Clear => {
                vi.set_normal(false, t);
                ViOut::Clear
            }
            o => o,
        }
    }
    fn word_left(&self) -> usize {
        let s = &self.text[..self.cursor];
        let t = s.trim_end_matches(' ');
        t.rfind(|c: char| c == ' ' || c == '/').map_or(0, |i| i + 1).min(t.len())
    }
    /// One key while it's open.
    pub fn key(&mut self, k: &KeyEvent, t: &mut Text, ind: &Indent, vi: &mut Vi, ex: &mut Ex) -> ViOut {
        if !self.is_open() || k.kind == KeyEventKind::Release {
            return ViOut::None;
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Esc => {
                self.close();
                t.anchor = None;
            }
            KeyCode::Backspace if self.text.is_empty() => self.close(),
            KeyCode::Char('[') if ctrl => self.close(),
            KeyCode::Enter => return self.run(t, ind, vi, ex),
            KeyCode::Tab if !self.completions.is_empty() => self.complete(ex),
            KeyCode::Up | KeyCode::Down if self.completions.len() > 1 && self.history_at.is_none() => {
                let n = self.completions.len();
                self.pick = (self.pick + if k.code == KeyCode::Down { 1 } else { n - 1 }) % n;
            }
            KeyCode::Up | KeyCode::Down => {
                let h = &ex.history;
                if h.is_empty() {
                    return ViOut::Handled;
                }
                let i = self.history_at.unwrap_or(h.len()) as i64 + if k.code == KeyCode::Up { -1 } else { 1 };
                let i = i.clamp(0, h.len() as i64) as usize;
                self.history_at = Some(i);
                let s = h.get(i).cloned().unwrap_or_default();
                let at = self.history_at;
                self.set_text(s, ex);
                self.history_at = at;
            }
            KeyCode::Backspace => {
                let a = if ctrl { self.word_left() } else { self.text[..self.cursor].char_indices().next_back().map_or(0, |(i, _)| i) };
                self.text.replace_range(a..self.cursor, "");
                self.cursor = a;
                self.changed(ex);
            }
            KeyCode::Delete => {
                if let Some(c) = self.text[self.cursor..].chars().next() {
                    self.text.replace_range(self.cursor..self.cursor + c.len_utf8(), "");
                    self.changed(ex);
                }
            }
            KeyCode::Left => {
                self.cursor = if ctrl { self.word_left() } else { self.text[..self.cursor].char_indices().next_back().map_or(0, |(i, _)| i) };
            }
            KeyCode::Right => {
                if ctrl {
                    let s = &self.text[self.cursor..];
                    let skip = s.len() - s.trim_start_matches(' ').len();
                    self.cursor += skip + s[skip..].find([' ', '/']).unwrap_or(s.len() - skip);
                } else if let Some(c) = self.text[self.cursor..].chars().next() {
                    self.cursor += c.len_utf8();
                }
            }
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.text.len(),
            // (The shell's: Ctrl+W a word back, Ctrl+U to the start.)
            KeyCode::Char('w') if ctrl => {
                let a = self.word_left();
                self.text.replace_range(a..self.cursor, "");
                self.cursor = a;
                self.changed(ex);
            }
            KeyCode::Char('u') if ctrl => {
                self.text.replace_range(..self.cursor, "");
                self.cursor = 0;
                self.changed(ex);
            }
            KeyCode::Char('a') if ctrl => self.cursor = 0,
            KeyCode::Char('e') if ctrl => self.cursor = self.text.len(),
            KeyCode::Char(c) if !ctrl && !k.modifiers.contains(KeyModifiers::ALT) => {
                self.text.insert(self.cursor, c);
                self.cursor += c.len_utf8();
                self.changed(ex);
            }
            _ => {}
        }
        ViOut::Handled
    }
}

#[cfg(test)]
mod tests {
    use crate::text::IndentMode;
    use super::*;

    struct Ed {
        vi: Vi,
        t: Text,
        ex: Ex,
        ind: Indent,
        last: ViOut,
    }
    /// The text with "|" for the cursor, in normal mode.
    fn ed(s: &str) -> Ed {
        let mut t = Text::new();
        let p = s.find('|').unwrap();
        t.set(&s.replace('|', ""));
        t.cursor = p;
        let mut vi = Vi::new();
        vi.set_normal(true, &t);
        Ed { vi, t, ex: Ex::new(), ind: Indent { mode: IndentMode::Spaces, size: 4 }, last: ViOut::None }
    }
    impl Ed {
        /// Keys: chars, and <esc> <cr> <C-r> <C-v> <bs>.
        fn k(&mut self, keys: &str) -> &mut Ed {
            let mut it = keys.chars().peekable();
            while let Some(c) = it.next() {
                let ev = if c == '<' && it.peek().is_some_and(|c| c.is_ascii_alphabetic()) && keys.contains('>') {
                    let mut name = String::new();
                    for d in it.by_ref() {
                        if d == '>' {
                            break;
                        }
                        name.push(d);
                    }
                    match name.as_str() {
                        "esc" => KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                        "cr" => KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                        "bs" => KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
                        n if n.starts_with("C-") => KeyEvent::new(KeyCode::Char(n.chars().nth(2).unwrap()), KeyModifiers::CONTROL),
                        n => panic!("key {n}"),
                    }
                } else {
                    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
                };
                self.last = self.vi.key(&ev, &mut self.t, &self.ind, &mut self.ex);
                // (Insert mode: the editor types what vi leaves.)
                if self.last == ViOut::None {
                    if let KeyCode::Char(c) = ev.code {
                        if ev.modifiers.is_empty() {
                            self.t.insert(&c.to_string());
                        }
                    }
                }
            }
            self
        }
        fn is(&self, s: &str) {
            let mut want = s.to_string();
            let p = want.find('|').unwrap();
            want.remove(p);
            assert_eq!(self.t.text, want);
            assert_eq!(self.t.cursor, p, "cursor in {:?}", self.t.text);
        }
    }

    #[test]
    fn delete_word_and_change() {
        ed("|foo bar baz").k("dw").is("|bar baz");
        ed("foo b|ar baz").k("ciwX<esc>").is("foo |X baz");
        ed("one two |three").k("db").is("one |three");
        ed("a(b|, c)d").k("da(").is("a|d");
        ed("x = \"he|llo\" y").k("ci\"bye<esc>").is("x = \"by|e\" y");
        ed("foo |bar").k("cwqux<esc>").is("foo qu|x");
    }

    #[test]
    fn lines_and_registers() {
        ed("1\n|2\n3\n4").k("d2j").is("|1");
        ed("|a\nb").k("yyp").is("a\n|a\nb");
        ed("|a\nb").k("\"xyyj\"xP").is("a\n|a\nb");
        ed("|ab").k("xp").is("b|a");
        ed("a\n|b\nc").k("ddP").is("a\n|b\nc");
    }

    #[test]
    fn counts_and_dot() {
        ed("|a b c d e").k("2dw").is("|c d e");
        ed("|a b c d e").k("dw..").is("|d e");
        ed("|x\ny\nz").k("Ahi<esc>j.").is("xhi\nyh|i\nz");
        ed("|ab").k("3ix<esc>").is("xx|xab");
        ed("|aaa").k("3rb").is("bb|b");
    }

    #[test]
    fn visual() {
        let mut e = ed("|a\nb\nc");
        e.k("Vj>");
        e.is("    |a\n    b\nc");
        assert_eq!(e.vi.visual, None);
        ed("|hello world").k("vey").is("|hello world");
        let mut e = ed("|hello world");
        e.k("ve");
        assert_eq!(e.vi.mode_name(), "VISUAL");
        assert_eq!(e.t.selection(), Some((0, 5)));
        e.k("d").is("| world");
        // (A block: each line's part.)
        let mut e = ed("|abc\ndef\nghi");
        e.k("<C-v>jl");
        assert_eq!(e.vi.block_ranges(&e.t), vec![(0, 2), (4, 6)]);
        e.k("d").is("|c\nf\nghi");
        ed("|ab\ncd").k("<C-v>jIX<esc>").is("|Xab\nXcd");
    }

    #[test]
    fn undo_redo() {
        let mut e = ed("|one two");
        e.k("dwdw").is("|");
        e.k("u").is("|two");
        e.k("u").is("|one two");
        e.k("<C-r>").is("|two");
        // (An insert with its command is one step.)
        let mut e = ed("|x");
        e.k("ofoo<esc>u").is("|x");
        e.k("u");
        assert_eq!(e.last, ViOut::Message("Already at oldest change".into(), false));
    }

    #[test]
    fn history_edges_and_motions() {
        let mut e = ed("a\n|b");
        e.k("j");
        assert_eq!(e.last, ViOut::HistoryStep(1));
        e.k("kk");
        assert_eq!(e.last, ViOut::HistoryStep(-1));
        ed("ab|cdef").k("$").is("abcde|f");
        ed("abc d|ef").k("0").is("|abc def");
        ed("|a(b)c").k("%").is("a(b|)c");
        ed("|abcabc").k("2fc").is("abcab|c");
        ed("|a1 a2 a1").k("*").is("a1 a2 |a1");
        ed("|x 9 y").k("<C-a>").is("x 1|0 y");
        let mut e = ed("|x");
        e.k("3:");
        assert_eq!(e.last, ViOut::OpenCmdLine(':', ".,.+2".into()));
        // (Insert mode: vi leaves the keys to the editor.)
        let mut e = ed("|x");
        e.k("i");
        let ev = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL);
        assert_eq!(e.vi.key(&ev, &mut e.t, &e.ind, &mut e.ex), ViOut::None);
    }

    #[test]
    fn surround_and_text_objects() {
        ed("|word here").k("ysiw)").is("|(word) here");
        ed("'a|b'").k("cs'\"").is("|\"ab\"");
        ed("(a|b)").k("ds(").is("|ab");
        ed("<b>x|y</b>").k("dit").is("<b>|</b>");
        ed("a |b c").k("daw").is("a |c");
    }

    #[test]
    fn command_line() {
        let mut e = ed("|a a\na");
        let mut cmd = CmdLine::new();
        e.k(":");
        assert!(cmd.apply(&e.last, &e.ex));
        for c in "%s/a/b/g".chars() {
            cmd.key(&KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE), &mut e.t, &e.ind, &mut e.vi, &mut e.ex);
        }
        let o = cmd.key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut e.t, &e.ind, &mut e.vi, &mut e.ex);
        assert_eq!(o, ViOut::Handled);
        assert_eq!(e.t.text, "b b\nb");
        assert!(!cmd.is_open());
        // (u takes back the : command as one step.)
        e.k("u");
        assert_eq!(e.t.text, "a a\na");
        // (Completion, :normal, :w.)
        cmd.open(':', "", &e.ex);
        cmd.key(&KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE), &mut e.t, &e.ind, &mut e.vi, &mut e.ex);
        assert!(cmd.choices.iter().any(|c| c.0.starts_with("normal")));
        cmd.open(':', "%norm Ax", &e.ex);
        cmd.key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut e.t, &e.ind, &mut e.vi, &mut e.ex);
        assert_eq!(e.t.text, "a ax\nax");
        assert!(e.vi.normal);
        cmd.open(':', "w", &e.ex);
        let o = cmd.key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut e.t, &e.ind, &mut e.vi, &mut e.ex);
        assert_eq!(o, ViOut::Actions(vec![crate::ex::ExAction::Write(String::new(), false)]));
        // (Visual then : gives '<,'>.)
        let mut e = ed("|x\ny\nz");
        e.k("Vj:");
        assert_eq!(e.last, ViOut::OpenCmdLine(':', "'<,'>".into()));
        cmd.apply(&e.last, &e.ex);
        for c in "s/$/!/".chars() {
            cmd.key(&KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE), &mut e.t, &e.ind, &mut e.vi, &mut e.ex);
        }
        cmd.key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut e.t, &e.ind, &mut e.vi, &mut e.ex);
        assert_eq!(e.t.text, "x!\ny!\nz");
    }
}
