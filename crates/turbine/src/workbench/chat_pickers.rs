//! The AI chat's model and thinking pickers, on the line above its question
//! field (as VS Code's chat footer): `◆ Inkling 2 Mini ▾` and `Think: Medium ▾`.
//! A click opens a list; the choices are settings (chat_model, thinking).
//! The models are the hub's (hearth's, its "default" and "fast" first) or,
//! standalone, the ones Turbine found; cloud ones are marked ☁.

use super::*;

/// The thinking levels: (setting, shown).
pub(super) const THINKING: [(&str, &str); 5] = [("", "Default"), ("off", "Off"), ("low", "Low"), ("medium", "Medium"), ("high", "High")];

impl Workbench {
    /// The thinking level's name for the picker.
    fn thinking_label(&self) -> &'static str {
        let cur = self.chat_thinking.as_deref().unwrap_or("");
        THINKING.iter().find(|(k, _)| *k == cur).map_or("Default", |(_, l)| l)
    }

    /// The pickers' places on the chat's divider line (part rect `r`):
    /// (place, which, label).
    pub(super) fn chat_pickers(&self, r: Rect) -> Vec<(Rect, &'static str, String)> {
        let y = r.bottom() - 3;
        let model = match self.chat_model() {
            Some(m) => format!("{}{}", if m.cloud { "☁ " } else { "◆ " }, if m.label.is_empty() { m.name } else { m.label }),
            None if self.ai.is_none() => "◆ AI off".into(),
            None => "◆ No models".into(),
        };
        let think = format!("Think: {}", self.thinking_label());
        let tw = str_width(&think) as i32 + 2;
        // (The model's name gives way first: the thinking level stays whole.)
        let room = (r.w - 4 - tw - 1).max(6);
        let mut m = format!("{model} ▾");
        if str_width(&m) as i32 > room {
            let mut t: String = model.chars().collect();
            while !t.is_empty() && str_width(&t) as i32 + 3 > room {
                t.pop();
            }
            m = format!("{t}… ▾");
        }
        let mw = str_width(&m) as i32;
        let mx = r.x + 2;
        vec![(Rect::new(mx, y, mw, 1), "model", m), (Rect::new(mx + mw + 1, y, tw, 1), "thinking", format!("{think} ▾"))]
    }

    pub(super) fn draw_chat_pickers(&self, c: &mut Canvas, r: Rect, theme: &Theme, focused: bool) {
        let s = &theme.scheme;
        let fs = if focused { s.frame_active } else { s.frame };
        for (pr, _, label) in self.chat_pickers(r) {
            c.print(pr.x, pr.y, " ", fs);
            c.print(pr.x + 1, pr.y, &label, Style { fg: s.menu_hotkey.fg, ..fs });
            c.print(pr.x + 1 + str_width(&label) as i32, pr.y, " ", fs);
        }
    }

    /// A click on a picker: its list, under it. True when it hit one.
    pub(super) fn chat_picker_click(&mut self, r: Rect, at: Point) -> bool {
        let Some((pr, which, _)) = self.chat_pickers(r).into_iter().find(|(pr, _, _)| pr.contains(at)) else { return false };
        if self.ai.is_none() {
            self.start_ai();
        }
        let (items, acts, sel): (Vec<(String, String)>, Vec<String>, usize) = match which {
            "model" => {
                let cur = self.chat_model().map(|m| m.name);
                let models: Vec<ai::Model> = self.ai.as_ref().map(|a| a.models.iter().filter(|m| m.chat).cloned().collect()).unwrap_or_default();
                if models.is_empty() {
                    self.say("No models yet: the hub's list (or Ollama's) is on its way");
                    return true;
                }
                let sel = models.iter().position(|m| Some(&m.name) == cur.as_ref()).unwrap_or(0);
                let items = models
                    .iter()
                    .map(|m| {
                        let label = if m.label.is_empty() { m.name.clone() } else { m.label.clone() };
                        (format!("{}{label}", if m.cloud { "☁ " } else { "" }), if m.cloud { "cloud".into() } else { String::new() })
                    })
                    .collect();
                (items, models.iter().map(|m| format!("chatmodel:{}", m.name)).collect(), sel)
            }
            _ => {
                let cur = self.chat_thinking.as_deref().unwrap_or("");
                let notes = ["the model's own", "answer at once", "a little", "", "think it through"];
                let items = THINKING.iter().zip(notes).map(|((_, l), n)| (l.to_string(), n.to_string())).collect();
                (items, THINKING.iter().map(|(k, _)| format!("thinking:{k}")).collect(), THINKING.iter().position(|(k, _)| *k == cur).unwrap_or(0))
            }
        };
        let mut popup = ListPopup::new(items);
        popup.max_rows = 18;
        popup.sel = sel;
        // (Above the picker: the chat's field is under it.)
        let p = Point::new(pr.x, pr.y);
        popup.place(p, self.screen());
        self.status_menu = Some((popup, p, acts));
        true
    }

    /// A picker's choice: true when it was one.
    pub(super) fn chat_picker_choice(&mut self, a: &str) -> bool {
        if let Some(name) = a.strip_prefix("chatmodel:") {
            self.setting_set("chat_model", name);
            self.save_look();
            true
        } else if let Some(l) = a.strip_prefix("thinking:") {
            self.setting_set("thinking", l);
            self.save_look();
            true
        } else {
            false
        }
    }
}
