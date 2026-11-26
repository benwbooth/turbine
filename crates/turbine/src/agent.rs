//! The agent driver: turns high-level actions ("open File/Save As",
//! "type this") into paced, visible input events.
//!
//! An agent never teleports. The pointer glides to its target, menus open
//! item by item, and text is typed at a readable speed, so a person can
//! follow along. Real input from the person pauses the agent.

use serde_json::Value;
use std::collections::VecDeque;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};
use turbine_ui::event::{Button, KeyEvent};
use turbine_ui::Point;

#[derive(Debug)]
pub enum Step {
    /// Glide the pointer to a point.
    MoveTo(Point),
    /// Glide to a widget found in the automation tree when this step runs
    /// (menus and dialogs only exist once earlier steps have opened them).
    MoveToTarget(String),
    Press(Button),
    Release(Button),
    Key(KeyEvent),
    /// Type one character, waiting the typing interval first.
    Char(char),
    Paste(String),
    Pause(Duration),
    /// Run a command directly (only for things with no UI path).
    Command(String),
    /// Send a reply once everything before it is done.
    Reply(Sender<Value>, Value),
}

pub struct Driver {
    pub queue: VecDeque<Step>,
    /// Where the agent's pointer is (None until it first moves).
    pub pointer: Option<Point>,
    glide: Option<Glide>,
    next_at: Instant,
    /// Characters per second; 0 = instant.
    pub cps: f64,
    /// Pause between pointer actions.
    pub beat: Duration,
    pub paused: bool,
    /// Mouse button the agent is holding down (pointer moves become drags).
    pub held: Option<Button>,
    /// Name shown in the status bar while driving.
    pub agent_name: String,
}

#[derive(Debug, Clone, Copy)]
struct Glide {
    from: Point,
    to: Point,
    start: Instant,
    dur: Duration,
}

/// What the workbench should do for the driver this tick.
pub enum Output {
    /// Pointer moved; the button held at that moment, if any.
    Pointer(Point, Option<Button>),
    Press(Point, Button),
    Release(Point, Button),
    Key(KeyEvent),
    Paste(String),
    Command(String),
    Resolve(String),
    Reply(Sender<Value>, Value),
}

impl Driver {
    pub fn new() -> Self {
        Driver {
            queue: VecDeque::new(),
            pointer: None,
            glide: None,
            next_at: Instant::now(),
            cps: 25.0,
            beat: Duration::from_millis(90),
            paused: false,
            held: None,
            agent_name: "Agent".into(),
        }
    }

    pub fn instant(&self) -> bool {
        self.cps <= 0.0
    }

    pub fn busy(&self) -> bool {
        !self.queue.is_empty() || self.glide.is_some()
    }

    pub fn cancel(&mut self) {
        // Keep replies so callers aren't left waiting.
        let replies: Vec<Step> = self.queue.drain(..).filter(|s| matches!(s, Step::Reply(..))).collect();
        self.queue.extend(replies);
        self.glide = None;
        self.paused = false;
        self.held = None;
    }

    /// Replace the step at the front (used when a target resolves).
    pub fn push_front(&mut self, s: Step) {
        self.queue.push_front(s);
    }

    /// Advance. Returns outputs to apply now, and when to tick next.
    pub fn tick(&mut self, now: Instant) -> (Vec<Output>, Option<Instant>) {
        let mut out = Vec::new();
        if self.paused {
            return (out, None);
        }
        loop {
            if let Some(g) = self.glide {
                let t = if g.dur.is_zero() { 1.0 } else { (now - g.start).as_secs_f64() / g.dur.as_secs_f64() };
                let t = t.min(1.0);
                // ease in-out
                let e = t * t * (3.0 - 2.0 * t);
                let p = Point::new(
                    g.from.x + ((g.to.x - g.from.x) as f64 * e).round() as i32,
                    g.from.y + ((g.to.y - g.from.y) as f64 * e).round() as i32,
                );
                if Some(p) != self.pointer {
                    self.pointer = Some(p);
                    out.push(Output::Pointer(p, self.held));
                }
                if t < 1.0 {
                    return (out, Some(now + Duration::from_millis(16)));
                }
                self.glide = None;
                self.next_at = now + self.beat_dur();
            }
            if now < self.next_at && !self.instant() {
                return (out, Some(self.next_at));
            }
            let Some(step) = self.queue.pop_front() else { return (out, None) };
            match step {
                Step::MoveTo(to) => {
                    let from = self.pointer.unwrap_or(Point::new(to.x, 0));
                    let dist = ((to.x - from.x).abs() + (to.y - from.y).abs() * 2) as u64;
                    let dur = if self.instant() { Duration::ZERO } else { Duration::from_millis((dist * 9).clamp(120, 600)) };
                    self.glide = Some(Glide { from, to, start: now, dur });
                }
                Step::MoveToTarget(t) => {
                    out.push(Output::Resolve(t));
                    // The workbench pushes a MoveTo (or reports failure).
                    return (out, Some(now));
                }
                Step::Press(b) => {
                    let p = self.pointer.unwrap_or_default();
                    out.push(Output::Press(p, b));
                    self.held = Some(b);
                    self.next_at = now + self.beat_dur() / 2;
                }
                Step::Release(b) => {
                    let p = self.pointer.unwrap_or_default();
                    out.push(Output::Release(p, b));
                    self.held = None;
                    self.next_at = now + self.beat_dur();
                    return (out, Some(self.next_at));
                }
                Step::Key(k) => {
                    out.push(Output::Key(k));
                    self.next_at = now + self.beat_dur();
                    return (out, Some(self.next_at));
                }
                Step::Char(c) => {
                    out.push(Output::Key(KeyEvent::plain(turbine_ui::Key::Char(c))));
                    let gap = if self.instant() { Duration::ZERO } else { self.char_gap(c) };
                    self.next_at = now + gap;
                    if !self.instant() {
                        return (out, Some(self.next_at));
                    }
                }
                Step::Paste(s) => out.push(Output::Paste(s)),
                Step::Pause(d) => {
                    if !self.instant() {
                        self.next_at = now + d;
                    }
                }
                Step::Command(c) => {
                    out.push(Output::Command(c));
                    self.next_at = now + self.beat_dur();
                    return (out, Some(self.next_at));
                }
                Step::Reply(tx, v) => out.push(Output::Reply(tx, v)),
            }
        }
    }

    fn beat_dur(&self) -> Duration {
        if self.instant() { Duration::ZERO } else { self.beat }
    }

    /// Typing rhythm: a little slower after punctuation and line ends.
    fn char_gap(&self, c: char) -> Duration {
        let base = 1.0 / self.cps;
        let f = match c {
            '\n' => 3.0,
            '.' | ',' | ';' | ':' | '{' | '}' | '(' | ')' => 1.6,
            ' ' => 1.2,
            _ => 1.0,
        };
        Duration::from_secs_f64(base * f)
    }
}
