//! Data files as a tree, as grove shows them: JSON, YAML, TOML and XML
//! parsed into one shape of node, drawn with ▸/▾ to fold, keys and values
//! in their colours, and a search that keeps the matches and what holds
//! them (opened up to them). Enter on a value (or a double-click) goes to
//! its line in the source when the format says where it is.

use std::collections::HashSet;
use turbine_text::unicode::str_width;
use turbine_ui::event::{Button, Key, KeyEvent, MouseEvent, MouseKind};
use turbine_ui::theme::{Style, Theme};
use turbine_ui::{Canvas, Point, Rect};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Object,
    Array,
    Str,
    Num,
    Bool,
    Null,
    Date,
    /// XML: an element (its attributes and children inside), its text.
    Element,
    Attr,
    Text,
}

#[derive(Clone, Debug)]
pub struct Node {
    pub key: Option<String>,
    pub value: Option<String>,
    pub kind: Kind,
    /// Where it starts in the source (0-based), when the format tells.
    pub line: Option<usize>,
    /// A value's bytes in the source (JSON's literals, XML's attribute
    /// values and text), so it can be edited in place.
    pub span: Option<(usize, usize)>,
    pub children: Vec<Node>,
}

impl Node {
    fn leaf(key: Option<String>, kind: Kind, value: String, line: Option<usize>) -> Node {
        Node { key, value: Some(value), kind, line, span: None, children: Vec::new() }
    }
    fn branch(key: Option<String>, kind: Kind, line: Option<usize>, children: Vec<Node>) -> Node {
        Node { key, value: None, kind, line, span: None, children }
    }
    fn at(mut self, span: (usize, usize)) -> Node {
        self.span = Some(span);
        self
    }
    pub fn is_branch(&self) -> bool {
        !self.children.is_empty() || matches!(self.kind, Kind::Object | Kind::Array | Kind::Element)
    }
}

/// The formats there's a tree for, by language name.
pub fn format_of(lang: &str) -> Option<&'static str> {
    Some(match lang {
        "json" | "jsonc" | "json5" => "json",
        "yaml" => "yaml",
        "toml" => "toml",
        "xml" | "svg" | "html" => "xml",
        _ => return None,
    })
}

pub fn parse(format: &str, text: &str) -> Result<Node, String> {
    match format {
        "json" => json::parse(text),
        "toml" => {
            let t: toml::Table = text.parse().map_err(|e: toml::de::Error| e.to_string())?;
            Ok(from_toml(None, &toml::Value::Table(t)))
        }
        "yaml" => {
            let docs = yaml_rust2::YamlLoader::load_from_str(text).map_err(|e| e.to_string())?;
            Ok(match docs.len() {
                0 => Node::branch(None, Kind::Object, None, Vec::new()),
                1 => from_yaml(None, &docs[0]),
                _ => Node::branch(None, Kind::Array, None, docs.iter().enumerate().map(|(i, d)| from_yaml(Some(format!("document {}", i + 1)), d)).collect()),
            })
        }
        "xml" => {
            let opts = roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
            let doc = roxmltree::Document::parse_with_options(text, opts).map_err(|e| e.to_string())?;
            Ok(from_xml(&doc, doc.root_element()))
        }
        _ => Err(format!("no tree for {format}")),
    }
}

fn from_toml(key: Option<String>, v: &toml::Value) -> Node {
    use toml::Value as V;
    match v {
        V::String(s) => Node::leaf(key, Kind::Str, s.clone(), None),
        V::Integer(n) => Node::leaf(key, Kind::Num, n.to_string(), None),
        V::Float(f) => Node::leaf(key, Kind::Num, f.to_string(), None),
        V::Boolean(b) => Node::leaf(key, Kind::Bool, b.to_string(), None),
        V::Datetime(d) => Node::leaf(key, Kind::Date, d.to_string(), None),
        V::Array(a) => Node::branch(key, Kind::Array, None, a.iter().enumerate().map(|(i, x)| from_toml(Some(i.to_string()), x)).collect()),
        V::Table(t) => Node::branch(key, Kind::Object, None, t.iter().map(|(k, x)| from_toml(Some(k.clone()), x)).collect()),
    }
}

fn from_yaml(key: Option<String>, v: &yaml_rust2::Yaml) -> Node {
    use yaml_rust2::Yaml as Y;
    let scalar = |y: &Y| match y {
        Y::String(s) | Y::Real(s) => s.clone(),
        Y::Integer(n) => n.to_string(),
        Y::Boolean(b) => b.to_string(),
        Y::Null => "null".into(),
        _ => String::new(),
    };
    match v {
        Y::String(s) => Node::leaf(key, Kind::Str, s.clone(), None),
        Y::Real(s) => Node::leaf(key, Kind::Num, s.clone(), None),
        Y::Integer(n) => Node::leaf(key, Kind::Num, n.to_string(), None),
        Y::Boolean(b) => Node::leaf(key, Kind::Bool, b.to_string(), None),
        Y::Null | Y::BadValue => Node::leaf(key, Kind::Null, "null".into(), None),
        Y::Alias(n) => Node::leaf(key, Kind::Null, format!("*alias {n}"), None),
        Y::Array(a) => Node::branch(key, Kind::Array, None, a.iter().enumerate().map(|(i, x)| from_yaml(Some(i.to_string()), x)).collect()),
        Y::Hash(h) => Node::branch(key, Kind::Object, None, h.iter().map(|(k, x)| from_yaml(Some(scalar(k)), x)).collect()),
    }
}

fn from_xml(doc: &roxmltree::Document, n: roxmltree::Node) -> Node {
    let line = Some(doc.text_pos_at(n.range().start).row.saturating_sub(1) as usize);
    let mut kids: Vec<Node> = n.attributes().map(|a| Node::leaf(Some(format!("@{}", a.name())), Kind::Attr, a.value().to_string(), line).at((a.range_value().start, a.range_value().end))).collect();
    for c in n.children() {
        if c.is_element() {
            kids.push(from_xml(doc, c));
        } else if c.is_text() {
            let t = c.text().unwrap_or("").trim();
            if !t.is_empty() {
                let l = Some(doc.text_pos_at(c.range().start).row.saturating_sub(1) as usize);
                let r = c.range();
                // (The text without the whitespace around it.)
                let raw = &doc.input_text()[r.clone()];
                let lead = raw.len() - raw.trim_start().len();
                let trail = raw.len() - raw.trim_end().len();
                kids.push(Node::leaf(None, Kind::Text, t.to_string(), l).at((r.start + lead, r.end - trail)));
            }
        }
    }
    // (An element with only text: the text as its value, as one row.)
    if kids.len() == 1 && kids[0].kind == Kind::Text {
        let t = kids.pop().unwrap();
        return Node { key: Some(n.tag_name().name().to_string()), value: t.value, kind: Kind::Element, line, span: t.span, children: Vec::new() };
    }
    Node::branch(Some(n.tag_name().name().to_string()), Kind::Element, line, kids)
}

/// JSON (with // and /* */ comments and trailing commas, as JSONC has),
/// keeping each value's line.
mod json {
    use super::{Kind, Node};

    struct P<'a> {
        b: &'a [u8],
        i: usize,
        line: usize,
    }

    pub fn parse(text: &str) -> Result<Node, String> {
        let mut p = P { b: text.as_bytes(), i: 0, line: 0 };
        p.ws();
        let v = p.value(None)?;
        p.ws();
        if p.i < p.b.len() {
            return Err(format!("line {}: more after the value", p.line + 1));
        }
        Ok(v)
    }

    impl P<'_> {
        fn err(&self, what: &str) -> String {
            format!("line {}: {what}", self.line + 1)
        }
        fn ws(&mut self) {
            while self.i < self.b.len() {
                match self.b[self.i] {
                    b'\n' => {
                        self.line += 1;
                        self.i += 1;
                    }
                    b' ' | b'\t' | b'\r' => self.i += 1,
                    b'/' if self.b.get(self.i + 1) == Some(&b'/') => {
                        while self.i < self.b.len() && self.b[self.i] != b'\n' {
                            self.i += 1;
                        }
                    }
                    b'/' if self.b.get(self.i + 1) == Some(&b'*') => {
                        self.i += 2;
                        while self.i + 1 < self.b.len() && !(self.b[self.i] == b'*' && self.b[self.i + 1] == b'/') {
                            if self.b[self.i] == b'\n' {
                                self.line += 1;
                            }
                            self.i += 1;
                        }
                        self.i += 2;
                    }
                    _ => break,
                }
            }
        }
        fn value(&mut self, key: Option<String>) -> Result<Node, String> {
            let line = Some(self.line);
            match self.b.get(self.i) {
                Some(b'{') => {
                    self.i += 1;
                    let mut kids = Vec::new();
                    loop {
                        self.ws();
                        if self.b.get(self.i) == Some(&b'}') {
                            self.i += 1;
                            break;
                        }
                        if self.b.get(self.i) != Some(&b'"') {
                            return Err(self.err("a key (in quotes) expected"));
                        }
                        let k = self.string()?;
                        self.ws();
                        if self.b.get(self.i) != Some(&b':') {
                            return Err(self.err(": expected"));
                        }
                        self.i += 1;
                        self.ws();
                        kids.push(self.value(Some(k))?);
                        self.ws();
                        match self.b.get(self.i) {
                            Some(b',') => self.i += 1,
                            Some(b'}') => {}
                            _ => return Err(self.err(", or } expected")),
                        }
                    }
                    Ok(Node::branch(key, Kind::Object, line, kids))
                }
                Some(b'[') => {
                    self.i += 1;
                    let mut kids = Vec::new();
                    loop {
                        self.ws();
                        if self.b.get(self.i) == Some(&b']') {
                            self.i += 1;
                            break;
                        }
                        let n = kids.len();
                        kids.push(self.value(Some(n.to_string()))?);
                        self.ws();
                        match self.b.get(self.i) {
                            Some(b',') => self.i += 1,
                            Some(b']') => {}
                            _ => return Err(self.err(", or ] expected")),
                        }
                    }
                    Ok(Node::branch(key, Kind::Array, line, kids))
                }
                Some(b'"') => {
                    let start = self.i;
                    let s = self.string()?;
                    Ok(Node::leaf(key, Kind::Str, s, line).at((start, self.i)))
                }
                Some(_) => {
                    let start = self.i;
                    while self.i < self.b.len() && !matches!(self.b[self.i], b',' | b'}' | b']' | b' ' | b'\t' | b'\r' | b'\n' | b'/') {
                        self.i += 1;
                    }
                    let w = std::str::from_utf8(&self.b[start..self.i]).unwrap_or("");
                    let kind = match w {
                        "true" | "false" => Kind::Bool,
                        "null" => Kind::Null,
                        _ if w.parse::<f64>().is_ok() => Kind::Num,
                        _ => return Err(self.err(&format!("unexpected {w:?}"))),
                    };
                    Ok(Node::leaf(key, kind, w.to_string(), line).at((start, self.i)))
                }
                None => Err(self.err("a value expected")),
            }
        }
        fn string(&mut self) -> Result<String, String> {
            self.i += 1;
            let mut out = Vec::new();
            while let Some(&c) = self.b.get(self.i) {
                match c {
                    b'"' => {
                        self.i += 1;
                        return Ok(String::from_utf8_lossy(&out).into_owned());
                    }
                    b'\\' => {
                        let e = self.b.get(self.i + 1).copied().unwrap_or(b'\\');
                        self.i += 2;
                        match e {
                            b'n' => out.push(b'\n'),
                            b't' => out.push(b'\t'),
                            b'r' => out.push(b'\r'),
                            b'b' | b'f' => {}
                            b'u' => {
                                let hex = std::str::from_utf8(self.b.get(self.i..self.i + 4).unwrap_or(b"0000")).unwrap_or("0000");
                                self.i += 4;
                                let mut cp = u32::from_str_radix(hex, 16).unwrap_or(0xfffd);
                                // (A surrogate pair.)
                                if (0xd800..0xdc00).contains(&cp) && self.b.get(self.i..self.i + 2) == Some(b"\\u") {
                                    let lo = std::str::from_utf8(self.b.get(self.i + 2..self.i + 6).unwrap_or(b"0000")).unwrap_or("0000");
                                    if let Ok(lo) = u32::from_str_radix(lo, 16) {
                                        cp = 0x10000 + ((cp - 0xd800) << 10) + (lo.wrapping_sub(0xdc00) & 0x3ff);
                                        self.i += 6;
                                    }
                                }
                                let mut buf = [0u8; 4];
                                out.extend_from_slice(char::from_u32(cp).unwrap_or('\u{fffd}').encode_utf8(&mut buf).as_bytes());
                            }
                            other => out.push(other),
                        }
                    }
                    b'\n' => return Err(self.err("a string ends at the line's end")),
                    _ => {
                        out.push(c);
                        self.i += 1;
                    }
                }
            }
            Err(self.err("a string never ends"))
        }
    }
}

/// What a key did.
#[derive(Debug, PartialEq, Eq)]
pub enum Out {
    Ignored,
    Used,
    /// Go to this line of the source.
    Line(usize),
}

/// The tree on screen: what's open, the search, the selection.
pub struct StructView {
    pub root: Result<Node, String>,
    open: HashSet<Vec<usize>>,
    /// Shown rows: each node's path (child indexes from the root).
    rows: Vec<Vec<usize>>,
    pub selected: usize,
    pub scroll: usize,
    pub query: String,
    pub in_search: bool,
    /// Rows matching the search.
    pub matches: usize,
    pub area: Rect,
    last_click: Option<(usize, std::time::Instant)>,
    /// The scroll bar's thumb held: where in it.
    bar_grab: Option<usize>,
    /// Everything opened by Expand All (the toolbar's toggle shows Collapse).
    pub all_open: bool,
}

impl StructView {
    pub fn new(root: Result<Node, String>) -> StructView {
        let mut v = StructView { root, open: HashSet::new(), rows: Vec::new(), selected: 0, scroll: 0, query: String::new(), in_search: false, matches: 0, area: Rect::default(), last_click: None, bar_grab: None, all_open: false };
        // The top two levels open to start with.
        v.open.insert(Vec::new());
        if let Ok(r) = &v.root {
            for i in 0..r.children.len() {
                v.open.insert(vec![i]);
            }
        }
        v.rebuild();
        v
    }

    /// New contents (the file changed): what was open stays open.
    pub fn replace(&mut self, root: Result<Node, String>) {
        let sel = self.rows.get(self.selected).cloned();
        self.root = root;
        self.rebuild();
        if let Some(p) = sel.and_then(|s| self.rows.iter().position(|r| *r == s)) {
            self.selected = p;
        }
    }

    pub fn node(&self, path: &[usize]) -> Option<&Node> {
        let mut n = self.root.as_ref().ok()?;
        for &i in path {
            n = n.children.get(i)?;
        }
        Some(n)
    }

    fn hit(&self, n: &Node) -> bool {
        let q = self.query.to_lowercase();
        !q.is_empty() && (n.key.as_deref().is_some_and(|k| k.to_lowercase().contains(&q)) || n.value.as_deref().is_some_and(|v| v.to_lowercase().contains(&q)))
    }

    fn any_hit(&self, n: &Node) -> bool {
        self.hit(n) || n.children.iter().any(|c| self.any_hit(c))
    }

    /// The rows: the open nodes' children; while searching, the matches and
    /// the nodes holding them (opened).
    pub fn rebuild(&mut self) {
        let mut rows = Vec::new();
        let mut matches = 0;
        if let Ok(root) = &self.root {
            let searching = !self.query.is_empty();
            let mut stack: Vec<(Vec<usize>, &Node)> = root.children.iter().enumerate().rev().map(|(i, c)| (vec![i], c)).collect();
            while let Some((path, n)) = stack.pop() {
                if searching && !self.any_hit(n) {
                    continue;
                }
                if searching && self.hit(n) {
                    matches += 1;
                }
                let open = if searching { n.children.iter().any(|c| self.any_hit(c)) } else { self.open.contains(&path) };
                if open {
                    for (i, c) in n.children.iter().enumerate().rev() {
                        let mut p = path.clone();
                        p.push(i);
                        stack.push((p, c));
                    }
                }
                rows.push(path);
                if rows.len() > 200_000 {
                    break;
                }
            }
        }
        self.rows = rows;
        self.matches = matches;
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
    }

    /// The selected row's node.
    pub fn selected_node(&self) -> Option<&Node> {
        self.node(self.rows.get(self.selected)?)
    }

    /// The selected row's path, as jq writes it: .a.b[2]
    pub fn selected_path(&self) -> String {
        let Some(p) = self.rows.get(self.selected) else { return String::new() };
        let mut out = String::new();
        let mut n = match &self.root {
            Ok(r) => r,
            Err(_) => return out,
        };
        for &i in p {
            let parent_kind = n.kind;
            let Some(c) = n.children.get(i) else { break };
            match (&c.key, parent_kind) {
                (_, Kind::Array) => out.push_str(&format!("[{i}]")),
                (Some(k), _) if k.chars().all(|ch| ch.is_alphanumeric() || ch == '_') && !k.is_empty() => out.push_str(&format!(".{k}")),
                (Some(k), _) => out.push_str(&format!(".{k:?}")),
                (None, _) => out.push_str(&format!("[{i}]")),
            }
            n = c;
        }
        if out.is_empty() { ".".into() } else { out }
    }

    /// Select the row under a point (for a right-click), true when one is.
    pub fn select_at(&mut self, p: Point) -> bool {
        let a = self.area;
        if !a.contains(p) || p.y == a.y {
            return false;
        }
        let i = self.scroll + (p.y - a.y - 1) as usize;
        if i < self.rows.len() {
            self.selected = i;
            true
        } else {
            false
        }
    }

    pub fn rows(&self) -> usize {
        self.rows.len()
    }

    fn toggle(&mut self, i: usize, open: Option<bool>) {
        let Some(p) = self.rows.get(i).cloned() else { return };
        if !self.node(&p).is_some_and(Node::is_branch) {
            return;
        }
        let now = self.open.contains(&p);
        let want = open.unwrap_or(!now);
        if want {
            self.open.insert(p);
        } else {
            self.open.remove(&p);
        }
        self.rebuild();
    }

    /// Open everything under the selected row (or all, from the top).
    /// Everything open (Expand All), or only the top (Collapse All).
    pub fn expand_all(&mut self, open: bool) {
        self.all_open = open;
        if open {
            self.open_all(None);
        } else {
            self.open.retain(|p| p.is_empty());
            self.rebuild();
            self.selected = 0;
            self.scroll = 0;
        }
    }

    fn open_all(&mut self, from: Option<Vec<usize>>) {
        fn walk(n: &Node, p: Vec<usize>, open: &mut HashSet<Vec<usize>>) {
            if n.is_branch() {
                open.insert(p.clone());
            }
            for (i, c) in n.children.iter().enumerate() {
                let mut q = p.clone();
                q.push(i);
                walk(c, q, open);
            }
        }
        let start = from.unwrap_or_default();
        if let Some(n) = self.node(&start).cloned() {
            walk(&n, start, &mut self.open);
        }
        self.rebuild();
    }

    fn reveal(&mut self) {
        let h = (self.area.h - 1).max(1) as usize;
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + h {
            self.scroll = self.selected + 1 - h;
        }
    }

    pub fn key(&mut self, k: &KeyEvent) -> Out {
        let n = self.rows.len();
        let page = (self.area.h - 2).max(1) as usize;
        if self.in_search {
            match k.key {
                Key::Esc => {
                    self.in_search = false;
                    if !self.query.is_empty() {
                        self.query.clear();
                        self.rebuild();
                    }
                }
                Key::Enter | Key::Down | Key::Tab => self.in_search = false,
                Key::Backspace => {
                    self.query.pop();
                    self.rebuild();
                    self.selected = 0;
                }
                Key::Char(c) if !k.mods.ctrl && !k.mods.alt => {
                    self.query.push(c);
                    self.rebuild();
                    // (To the first match.)
                    self.selected = (0..self.rows.len()).find(|&i| self.node(&self.rows[i]).is_some_and(|x| self.hit(x))).unwrap_or(0);
                    self.reveal();
                }
                _ => return Out::Ignored,
            }
            return Out::Used;
        }
        match k.key {
            Key::Char('f') if k.mods.ctrl => self.in_search = true,
            Key::Char('/') if !k.mods.ctrl => self.in_search = true,
            Key::Esc if !self.query.is_empty() => {
                self.query.clear();
                self.rebuild();
            }
            Key::Up => self.selected = self.selected.saturating_sub(1),
            Key::Down => self.selected = (self.selected + 1).min(n.saturating_sub(1)),
            Key::PageUp => self.selected = self.selected.saturating_sub(page),
            Key::PageDown => self.selected = (self.selected + page).min(n.saturating_sub(1)),
            Key::Home => self.selected = 0,
            Key::End => self.selected = n.saturating_sub(1),
            Key::Right => {
                let p = self.rows.get(self.selected).cloned().unwrap_or_default();
                if self.node(&p).is_some_and(Node::is_branch) && !self.open.contains(&p) && self.query.is_empty() {
                    self.toggle(self.selected, Some(true));
                } else if self.rows.get(self.selected + 1).is_some_and(|q| q.len() > p.len()) {
                    self.selected += 1;
                }
            }
            Key::Left => {
                let p = self.rows.get(self.selected).cloned().unwrap_or_default();
                if self.open.contains(&p) && self.query.is_empty() && self.node(&p).is_some_and(Node::is_branch) {
                    self.toggle(self.selected, Some(false));
                } else if p.len() > 1 {
                    let parent = &p[..p.len() - 1];
                    if let Some(i) = self.rows.iter().position(|r| r == parent) {
                        self.selected = i;
                    }
                }
            }
            Key::Char(' ') => self.toggle(self.selected, None),
            Key::Char('*') => {
                let p = self.rows.get(self.selected).cloned();
                self.open_all(p);
            }
            Key::Char('-') => {
                self.all_open = false;
                self.open.retain(|p| p.is_empty());
                self.rebuild();
                self.selected = 0;
            }
            Key::Enter => {
                let p = self.rows.get(self.selected).cloned().unwrap_or_default();
                match self.node(&p) {
                    Some(x) if x.is_branch() && !k.mods.ctrl => self.toggle(self.selected, None),
                    Some(x) => return x.line.map_or(Out::Used, Out::Line),
                    None => {}
                }
            }
            _ => return Out::Ignored,
        }
        self.reveal();
        Out::Used
    }

    pub fn mouse(&mut self, m: &MouseEvent) -> Out {
        let a = self.area;
        // The scroll bar: a press scrolls there at once and drags on.
        let body_h = (a.h - 1).max(0) as usize;
        let track = (body_h as i32 - 2).max(0) as usize;
        let bar_x = a.right() - 1;
        let set_from = |v: &mut StructView, y: i32, grab: usize| {
            let (_, len) = turbine_ui::frame::thumb(track, 0, body_h, v.rows.len());
            let room = track.saturating_sub(len);
            let span = v.rows.len().saturating_sub(body_h);
            if room > 0 && span > 0 {
                let at = (y - (a.y + 2) - grab as i32).clamp(0, room as i32) as usize;
                v.scroll = (at * span + room / 2) / room;
            }
        };
        if let Some(grab) = self.bar_grab {
            match m.kind {
                MouseKind::Drag(Button::Left) => set_from(self, m.pos.y, grab),
                _ => self.bar_grab = None,
            }
            return Out::Used;
        }
        if self.rows.len() > body_h && m.pos.x == bar_x && m.pos.y > a.y && m.pos.y < a.bottom() && matches!(m.kind, MouseKind::Down(Button::Left)) {
            if m.pos.y == a.y + 1 {
                self.scroll = self.scroll.saturating_sub(1);
            } else if m.pos.y == a.bottom() - 1 {
                self.scroll = (self.scroll + 1).min(self.rows.len().saturating_sub(body_h));
            } else {
                let (t_at, t_len) = turbine_ui::frame::thumb(track, self.scroll, body_h, self.rows.len());
                let off = (m.pos.y - (a.y + 2)).max(0) as usize;
                let grab = if off >= t_at && off < t_at + t_len { off - t_at } else { t_len / 2 };
                set_from(self, m.pos.y, grab);
                self.bar_grab = Some(grab);
            }
            return Out::Used;
        }
        if !a.contains(m.pos) {
            return Out::Ignored;
        }
        match m.kind {
            MouseKind::ScrollUp => self.scroll = self.scroll.saturating_sub(3),
            MouseKind::ScrollDown => self.scroll = (self.scroll + 3).min(self.rows.len().saturating_sub(1)),
            MouseKind::Down(Button::Left) => {
                // The first row is the search field.
                if m.pos.y == a.y {
                    self.in_search = true;
                    return Out::Used;
                }
                self.in_search = false;
                let i = self.scroll + (m.pos.y - a.y - 1) as usize;
                if i >= self.rows.len() {
                    return Out::Used;
                }
                let depth = self.rows[i].len() as i32 - 1;
                let arrow_x = a.x + 1 + depth * 2;
                let double = self.last_click.is_some_and(|(j, t)| j == i && t.elapsed() < std::time::Duration::from_millis(400));
                self.last_click = Some((i, std::time::Instant::now()));
                self.selected = i;
                if m.pos.x == arrow_x || (double && self.node(&self.rows[i]).is_some_and(Node::is_branch)) {
                    self.toggle(i, None);
                } else if double {
                    if let Some(l) = self.node(&self.rows[i]).and_then(|x| x.line) {
                        return Out::Line(l);
                    }
                }
            }
            _ => return Out::Ignored,
        }
        Out::Used
    }

    pub fn draw(&mut self, c: &mut Canvas, r: Rect, theme: &Theme, focused: bool) {
        self.area = r;
        let s = &theme.scheme;
        let base = s.editor;
        c.fill(r, " ", base);
        let syn = |k: &str| {
            let i = turbine_ui::theme::SYNTAX_KINDS.iter().position(|x| *x == k).unwrap_or(0);
            Style { fg: s.syntax.get(i).copied().flatten().unwrap_or(base.fg), ..base }
        };
        // The search field.
        let field = Rect::new(r.x, r.y, r.w, 1);
        let fs = if self.in_search && focused { s.input } else { Style { fg: turbine_ui::theme::vga::CYAN, ..base } };
        c.fill(field, " ", fs);
        let count = if self.query.is_empty() { String::new() } else { format!("  {} found", self.matches) };
        let label = format!(" Search: {}{}", self.query, if self.in_search && focused { "_" } else { "" });
        c.print(r.x, r.y, &label, fs);
        c.print(r.x + str_width(&label) as i32, r.y, &count, fs);
        if !self.in_search && self.query.is_empty() {
            let hint = "/ to search   Space opens   * opens all   Enter: its line ";
            if (str_width(&label) + str_width(hint) + 4) as i32 <= r.w {
                c.print(r.right() - str_width(hint) as i32, r.y, hint, fs);
            }
        }
        let body = Rect::new(r.x, r.y + 1, r.w, r.h - 1);
        match &self.root {
            Err(e) => {
                c.print(body.x + 1, body.y, &format!("Can't read it as a tree: {e}"), syn("escape"));
                return;
            }
            Ok(_) => {}
        }
        let h = body.h.max(0) as usize;
        self.scroll = self.scroll.min(self.rows.len().saturating_sub(1));
        let q = self.query.to_lowercase();
        for (k, path) in self.rows.iter().enumerate().skip(self.scroll).take(h) {
            let y = body.y + (k - self.scroll) as i32;
            let Some(n) = self.node(path) else { continue };
            let sel = k == self.selected;
            let line_style = if sel && !self.in_search { if focused { s.editor_selection } else { s.panel_selected } } else { base };
            c.fill(Rect::new(body.x, y, body.w, 1), " ", line_style);
            let mut lc = c.sub(Rect::new(body.x, y, body.w, 1));
            let mut x = body.x + 1 + (path.len() as i32 - 1) * 2;
            let pick = |st: Style| if sel && !self.in_search { line_style } else { st };
            if n.is_branch() {
                let open = !self.query.is_empty() || self.open.contains(path);
                lc.print(x, y, if open { "▾" } else { "▸" }, pick(syn("punctuation")));
            }
            x += 2;
            let mut spans: Vec<(String, Style)> = Vec::new();
            if let Some(key) = &n.key {
                let st = match n.kind {
                    Kind::Element => syn("tag"),
                    Kind::Attr => syn("attribute"),
                    _ if key.parse::<usize>().is_ok() => syn("number"),
                    _ => syn("property"),
                };
                spans.push((key.clone(), st));
            }
            match (&n.value, n.is_branch()) {
                (Some(v), _) => {
                    if n.key.is_some() {
                        spans.push((if n.kind == Kind::Element { " " } else { ": " }.into(), syn("punctuation")));
                    }
                    let st = match n.kind {
                        Kind::Str | Kind::Attr | Kind::Text | Kind::Element => syn("string"),
                        Kind::Num => syn("number"),
                        Kind::Bool | Kind::Null => syn("constant"),
                        Kind::Date => syn("constant"),
                        _ => base,
                    };
                    let v = v.replace('\n', "⏎");
                    spans.push((if n.kind == Kind::Str { format!("\"{v}\"") } else { v }, st));
                }
                (None, true) => {
                    let what = match n.kind {
                        Kind::Array => format!(" [{}]", n.children.len()),
                        Kind::Element => format!(" <{}>", n.children.len()),
                        _ => format!(" {{{}}}", n.children.len()),
                    };
                    spans.push((what, syn("comment")));
                    // (A closed one's first value, to tell rows apart: name = "x".)
                    let open = !self.query.is_empty() || self.open.contains(path);
                    if !open {
                        if let Some(c) = n.children.iter().find(|c| c.value.is_some() && c.key.is_some()) {
                            let v = c.value.clone().unwrap_or_default().replace('\n', "⏎");
                            let v: String = v.chars().take(40).collect();
                            spans.push((format!("  {} = ", c.key.clone().unwrap_or_default()), syn("comment")));
                            spans.push((if c.kind == Kind::Str { format!("\"{v}\"") } else { v }, syn("string")));
                        }
                    }
                }
                _ => {}
            }
            for (t, st) in &spans {
                let st = pick(*st);
                // (The searched-for text marked where it shows.)
                if !q.is_empty() {
                    let lower = t.to_lowercase();
                    if let Some(at) = lower.find(&q).filter(|_| lower.len() == t.len()) {
                        lc.print(x, y, &t[..at], st);
                        let hx = x + str_width(&t[..at]) as i32;
                        lc.print(hx, y, &t[at..at + q.len()], s.find_match);
                        lc.print(hx + str_width(&t[at..at + q.len()]) as i32, y, &t[at + q.len()..], st);
                        x += str_width(t) as i32;
                        continue;
                    }
                }
                lc.print(x, y, t, st);
                x += str_width(t) as i32;
            }
        }
        // (A scroll bar when the rows don't all fit.)
        if self.rows.len() > h && h >= 3 {
            turbine_ui::frame::draw_vscroll(c, r.right() - 1, body.y, body.h, self.scroll, h, self.rows.len(), s.scrollbar, theme.style.scroll);
        }
        if focused && self.in_search {
            c.set_cursor(Point::new((r.x + 9 + str_width(&self.query) as i32).min(r.right() - 1), r.y), turbine_ui::CursorShape::Bar);
        }
    }

    /// The full text of the row under a point (for its tooltip when cut).
    pub fn row_text_at(&self, p: Point) -> Option<String> {
        let a = self.area;
        if !a.contains(p) || p.y == a.y {
            return None;
        }
        let i = self.scroll + (p.y - a.y - 1) as usize;
        let n = self.node(self.rows.get(i)?)?;
        let depth = self.rows[i].len() as i32 - 1;
        let text = match (&n.key, &n.value) {
            (Some(k), Some(v)) => format!("{k}: {v}"),
            (Some(k), None) => k.clone(),
            (None, Some(v)) => v.clone(),
            _ => return None,
        };
        (3 + depth * 2 + str_width(&text) as i32 > a.w).then_some(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(n: &Node) -> Vec<String> {
        n.children.iter().map(|c| c.key.clone().unwrap_or_default()).collect()
    }

    #[test]
    fn parses_every_format_in_order() {
        let j = parse("json", "{\n  // a comment\n  \"b\": 1,\n  \"a\": {\"x\": [true, null, \"s\\u00e9\"]},\n}\n").unwrap();
        assert_eq!(keys(&j), ["b", "a"]);
        assert_eq!(j.children[1].line, Some(3));
        assert_eq!(j.children[1].children[0].children[2].value.as_deref(), Some("sé"));
        let t = parse("toml", "[package]\nname = \"x\"\n[deps]\nz = 1\na = 2\n").unwrap();
        assert_eq!(keys(&t), ["package", "deps"]);
        assert_eq!(keys(&t.children[1]), ["z", "a"]);
        let y = parse("yaml", "b: 1\na:\n  - x\n  - y\n").unwrap();
        assert_eq!(keys(&y), ["b", "a"]);
        assert_eq!(y.children[1].children.len(), 2);
        let x = parse("xml", "<?xml version=\"1.0\"?>\n<root id=\"7\">\n  <item>one</item>\n  <item>two</item>\n</root>\n").unwrap();
        assert_eq!(keys(&x), ["@id", "item", "item"]);
        assert_eq!(x.children[2].value.as_deref(), Some("two"));
        assert_eq!(x.children[2].line, Some(3));
        assert!(parse("json", "{\"a\": }").is_err());
    }

    #[test]
    fn folds_and_searches() {
        let mut v = StructView::new(parse("json", "{\"a\": {\"deep\": {\"needle\": 1}}, \"b\": 2}"));
        v.area = Rect::new(0, 0, 60, 20);
        // Two levels open: a, deep (closed), b.
        assert_eq!(v.rows(), 3);
        v.key(&KeyEvent::plain(Key::Char('*')));
        assert_eq!(v.rows(), 4);
        v.key(&KeyEvent::plain(Key::Char('-')));
        assert_eq!(v.rows(), 2);
        // Searching shows the match and what holds it.
        v.key(&KeyEvent::plain(Key::Char('/')));
        for ch in "needle".chars() {
            v.key(&KeyEvent::plain(Key::Char(ch)));
        }
        assert_eq!((v.rows(), v.matches), (3, 1));
        assert_eq!(v.selected, 2);
        v.key(&KeyEvent::plain(Key::Esc));
        v.key(&KeyEvent::plain(Key::Esc));
        assert_eq!(v.rows(), 2);
    }
}
