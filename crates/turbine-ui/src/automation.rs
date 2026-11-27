//! The automation tree: what agents (and accessibility tools) see.
//!
//! Every visible widget reports a node. Agents find targets by id or by
//! role + name, then act through ordinary input events, so whatever an
//! agent does, a person watching sees happen.

use crate::geom::{Point, Rect};
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct Node {
    pub id: String,
    pub role: String,
    pub name: String,
    pub rect: Rect,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub focused: bool,
    #[serde(skip_serializing_if = "is_true")]
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// What hovering shows (a cut name in full, a path, an icon's meaning).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tip: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Node>,
}

fn is_true(b: &bool) -> bool {
    *b
}

impl Node {
    pub fn new(id: impl Into<String>, role: &str, name: impl Into<String>, rect: Rect) -> Self {
        Node {
            id: id.into(),
            role: role.into(),
            name: name.into(),
            rect,
            focused: false,
            enabled: true,
            value: None,
            tip: None,
            children: Vec::new(),
        }
    }
    pub fn focused(mut self, f: bool) -> Self {
        self.focused = f;
        self
    }
    pub fn enabled(mut self, e: bool) -> Self {
        self.enabled = e;
        self
    }
    pub fn value(mut self, v: impl Into<String>) -> Self {
        self.value = Some(v.into());
        self
    }
    pub fn tip(mut self, t: impl Into<String>) -> Self {
        self.tip = Some(t.into());
        self
    }
    pub fn child(mut self, n: Node) -> Self {
        self.children.push(n);
        self
    }

    /// Depth-first search.
    pub fn find(&self, pred: &dyn Fn(&Node) -> bool) -> Option<&Node> {
        if pred(self) {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.find(pred))
    }

    /// Find by id, or by "role:name" (name compared case-insensitively,
    /// ignoring `~` hotkey markers and trailing "...").
    pub fn find_target(&self, target: &str) -> Option<&Node> {
        if let Some(n) = self.find(&|n| n.id == target) {
            return Some(n);
        }
        let norm = |s: &str| s.replace('~', "").trim_end_matches("...").trim().to_lowercase();
        let (role, name) = target.split_once(':').unwrap_or(("", target));
        let name = norm(name);
        self.find(&|n| (role.is_empty() || n.role == role) && norm(&n.name) == name)
    }

    /// Where to click to activate this node.
    pub fn click_point(&self) -> Point {
        let r = self.rect;
        Point::new(r.x + (r.w / 2).min(2), r.y + r.h / 2)
    }
}
