//! Ctrl+K: Cursor's inline edit. A prompt box at the code (the selection,
//! else the cursor's line); Enter asks the chat model to rewrite it; the
//! answer replaces the code as one undo step, highlighted, and Enter keeps
//! it, Esc puts the code back, Ctrl+K asks again.

use super::shell::Focus;
use super::*;

pub(super) enum Phase {
    /// Typing the instruction.
    Prompt,
    /// Waiting for the model: (request, the answer so far).
    Waiting(u64, String),
    /// The new code is in: keep it or put the old back.
    Review,
}

pub(super) struct InlineEdit {
    pub win: u32,
    /// The code being rewritten (char range; after the answer, the new code's).
    pub range: (usize, usize),
    pub prompt: LineInput,
    pub phase: Phase,
    /// The code before (to try again from).
    pub old: String,
}

/// The model's answer without the Markdown fence it may come in.
pub(super) fn unfence(answer: &str) -> String {
    let t = answer.trim_matches('\n');
    let lines: Vec<&str> = t.lines().collect();
    if lines.first().is_some_and(|l| l.trim_start().starts_with("```")) {
        let end = lines.iter().rposition(|l| l.trim_start().starts_with("```")).filter(|&e| e > 0).unwrap_or(lines.len());
        return lines[1..end].join("\n");
    }
    t.to_string()
}

impl Workbench {
    /// Ctrl+K: start an inline edit on the selection or the cursor's line.
    pub(super) fn inline_start(&mut self) {
        let Some(i) = self.editor_index() else { return };
        if self.windows[i].special.is_some() {
            return;
        }
        self.start_ai();
        let w = &mut self.windows[i];
        let ed = &w.ed;
        let range = match ed.selection() {
            Some((a, b)) => {
                // (Whole lines, as Cursor does.)
                let (la, lb) = (ed.doc.rope.char_to_line(a), ed.doc.rope.char_to_line(b.saturating_sub(1).max(a)));
                (ed.doc.rope.line_to_char(la), line_end(&ed.doc.rope, lb))
            }
            None => {
                let l = ed.doc.rope.char_to_line(ed.cursor);
                (ed.doc.rope.line_to_char(l), line_end(&ed.doc.rope, l))
            }
        };
        let old = ed.doc.rope.slice(range.0..range.1).to_string();
        w.ed.found = vec![range];
        w.ed.found_current = None;
        self.inline = Some(InlineEdit { win: w.win.id, range, prompt: LineInput::default(), phase: Phase::Prompt, old });
        self.shell.focus = Focus::Inline;
    }

    fn inline_send(&mut self) {
        let Some(model) = self.chat_model() else {
            self.say("No chat model yet: waiting for the AI hub's list");
            return;
        };
        let Some(ie) = &self.inline else { return };
        let Some(w) = self.windows.iter().find(|w| w.win.id == ie.win) else { return };
        let rope = &w.ed.doc.rope;
        let (a, b) = ie.range;
        let la = rope.char_to_line(a);
        let lb = rope.char_to_line(b.min(rope.len_chars()));
        let before = rope.slice(rope.line_to_char(la.saturating_sub(30))..a).to_string();
        let after = rope.slice(b..line_end(rope, (lb + 30).min(rope.len_lines().saturating_sub(1)))).to_string();
        let lang = w.lang.clone().unwrap_or_else(|| "text".into());
        let msgs = vec![
            ChatMessage {
                role: "system".into(),
                content: "You rewrite code as asked. Reply with only the new code for the part to rewrite: no explanation, no Markdown fence. Keep its indentation.".into(),
            },
            ChatMessage {
                role: "user".into(),
                content: format!(
                    "Language: {lang}\nInstruction: {}\n\nCode before it (don't repeat):\n{before}\n\nThe part to rewrite:\n{}\n\nCode after it (don't repeat):\n{after}",
                    ie.prompt.text, ie.old
                ),
            },
        ];
        let Some(ai) = self.ai.as_mut() else { return };
        let id = ai.chat(&model, msgs);
        if let Some(ie) = &mut self.inline {
            ie.phase = Phase::Waiting(id, String::new());
        }
    }

    /// A streamed piece for the inline edit. True when it was one.
    pub(super) fn inline_chunk(&mut self, id: u64, text: &str) -> bool {
        match &mut self.inline {
            Some(InlineEdit { phase: Phase::Waiting(i, answer), .. }) if *i == id => {
                answer.push_str(text);
                true
            }
            _ => false,
        }
    }

    /// The answer is in: put it in place, highlighted, for review.
    pub(super) fn inline_done(&mut self, id: u64) -> bool {
        let Some(ie) = &mut self.inline else { return false };
        let Phase::Waiting(i, answer) = &ie.phase else { return false };
        if *i != id {
            return false;
        }
        let mut new = unfence(answer);
        // (Keep the line ending the old code had.)
        if ie.old.ends_with('\n') && !new.ends_with('\n') {
            new.push('\n');
        }
        let (a, b) = ie.range;
        let win = ie.win;
        if let Some(w) = self.windows.iter_mut().find(|w| w.win.id == win) {
            let ed = &mut w.ed;
            ed.doc.break_undo_group();
            ed.doc.replace(a, b, &new, ed.cursor);
            ed.doc.break_undo_group();
            let end = a + new.chars().count();
            ed.cursor = a;
            ed.anchor = None;
            ed.found = vec![(a, end)];
            ed.found_current = Some(0);
            ie.range = (a, end);
            ie.phase = Phase::Review;
        } else {
            self.inline = None;
        }
        true
    }

    pub(super) fn inline_error(&mut self, id: u64, message: &str) -> bool {
        if matches!(&self.inline, Some(InlineEdit { phase: Phase::Waiting(i, _), .. }) if *i == id) {
            self.say(format!("Inline edit: the model didn't answer: {message}"));
            self.inline_close(false);
            return true;
        }
        false
    }

    /// Done: keep the new code, or put the old back (one undo step).
    pub(super) fn inline_close(&mut self, keep: bool) {
        let Some(ie) = self.inline.take() else { return };
        if let Some(w) = self.windows.iter_mut().find(|w| w.win.id == ie.win) {
            if !keep && matches!(ie.phase, Phase::Review) {
                put_back(&mut w.ed, &ie);
            }
            w.ed.found.clear();
            w.ed.found_current = None;
        }
        self.shell.focus = Focus::Editors;
    }

    pub(super) fn inline_key(&mut self, k: &KeyEvent) {
        let Some(ie) = &mut self.inline else { return };
        match (&ie.phase, k.key) {
            (_, Key::Esc) => self.inline_close(false),
            (Phase::Prompt, Key::Enter) if !ie.prompt.text.trim().is_empty() => self.inline_send(),
            (Phase::Prompt, _) => {
                ie.prompt.key(k);
            }
            (Phase::Review, Key::Enter) => self.inline_close(true),
            (Phase::Review, _) if k.is_ctrl('k') => {
                // Again: the old code back, the prompt to edit.
                if let Some(w) = self.windows.iter_mut().find(|w| w.win.id == ie.win) {
                    put_back(&mut w.ed, ie);
                    let start = ie.range.0;
                    ie.range = (start, start + ie.old.chars().count());
                    w.ed.found = vec![ie.range];
                    w.ed.found_current = None;
                }
                ie.phase = Phase::Prompt;
            }
            _ => {}
        }
    }

    /// The prompt box: under the code being edited (above it near the bottom).
    pub(super) fn inline_rect(&self) -> Option<Rect> {
        let ie = self.inline.as_ref()?;
        let w = self.windows.iter().find(|w| w.win.id == ie.win)?;
        let inner = w.win.inner();
        let line = w.ed.doc.rope.char_to_line(ie.range.0.min(w.ed.doc.rope.len_chars()));
        let y = inner.y + line as i32 - w.ed.scroll_y as i32;
        let h = 3;
        let y = if y - h >= inner.y { y - h } else { (y + 1).min(inner.bottom() - h) };
        let wdt = 64.min(inner.w - 2);
        Some(Rect::new(inner.x + 1, y.max(inner.y), wdt, h))
    }

    pub(super) fn draw_inline(&mut self, c: &mut Canvas, theme: &Theme) {
        let Some(r) = self.inline_rect() else { return };
        let Some(ie) = &self.inline else { return };
        let s = &theme.scheme;
        c.fill(r, " ", s.dialog);
        turbine_ui::frame::draw_frame(c, r, theme.style.menu_frame, s.dialog_frame);
        let icon = if theme.style.icons { "\u{ec10} " } else { "" };
        turbine_ui::frame::draw_title_in(c, r, &format!("{icon}Edit with AI"), s.dialog_frame, 2);
        if theme.style.shadows {
            c.shadow(r, s.shadow);
        }
        let row = Rect::new(r.x + 1, r.y + 1, r.w - 2, 1);
        let focused = self.shell.focus == Focus::Inline;
        match &ie.phase {
            Phase::Prompt => ie.prompt.draw(c, row, theme, focused, "What should change? (Enter)"),
            Phase::Waiting(_, answer) => {
                let dots = ".".repeat(1 + answer.len() / 40 % 3);
                c.sub(row).print(row.x + 1, row.y, &format!("Writing{dots}  (Esc cancels)"), s.dialog);
            }
            Phase::Review => {
                c.sub(row).print(row.x + 1, row.y, "Enter keeps it · Esc puts it back · Ctrl+K again", s.dialog);
            }
        }
    }

    pub(super) fn inline_nodes(&self, root: Node) -> Node {
        let (Some(r), Some(ie)) = (self.inline_rect(), &self.inline) else { return root };
        let state = match ie.phase {
            Phase::Prompt => "prompt",
            Phase::Waiting(..) => "waiting",
            Phase::Review => "review",
        };
        root.child(
            Node::new("inline_edit", "dialog", "Edit with AI", r)
                .focused(true)
                .value(state)
                .child(Node::new("inline_edit.prompt", "input", "Instruction", Rect::new(r.x + 1, r.y + 1, r.w - 2, 1)).value(ie.prompt.text.clone())),
        )
    }
}

/// The old code back in place of the new.
fn put_back(ed: &mut turbine_ui::editor::EditorView, ie: &InlineEdit) {
    let (a, b) = ie.range;
    let b = b.min(ed.doc.rope.len_chars());
    ed.doc.break_undo_group();
    ed.doc.replace(a, b, &ie.old, ed.cursor);
    ed.doc.break_undo_group();
    ed.cursor = a;
    ed.anchor = None;
}

/// The char just past line `l`'s end (its newline included).
fn line_end(rope: &turbine_text::ropey::Rope, l: usize) -> usize {
    if l + 1 < rope.len_lines() { rope.line_to_char(l + 1) } else { rope.len_chars() }
}

#[cfg(test)]
mod tests {
    use super::unfence;

    #[test]
    fn answers_lose_their_fence() {
        assert_eq!(unfence("```rust\nfn a() {}\n```\n"), "fn a() {}");
        assert_eq!(unfence("fn a() {}"), "fn a() {}");
    }
}
