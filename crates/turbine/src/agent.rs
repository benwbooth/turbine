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
    /// Hold everything after it until the active window shows this text.
    WaitText(String, Duration),
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
    /// Waiting for text to appear (the workbench clears it).
    pub waiting: Option<(String, Instant)>,
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
            waiting: None,
            agent_name: "Agent".into(),
        }
    }

    pub fn instant(&self) -> bool {
        self.cps <= 0.0
    }

    pub fn busy(&self) -> bool {
        !self.queue.is_empty() || self.glide.is_some() || self.waiting.is_some()
    }

    pub fn cancel(&mut self) {
        // Keep replies so callers aren't left waiting.
        let replies: Vec<Step> = self.queue.drain(..).filter(|s| matches!(s, Step::Reply(..))).collect();
        self.queue.extend(replies);
        self.glide = None;
        self.paused = false;
        self.held = None;
        self.waiting = None;
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
        if self.waiting.is_some() {
            return (out, Some(now + Duration::from_millis(16)));
        }
        loop {
            if let Some(g) = self.glide {
                let t = if g.dur.is_zero() { 1.0 } else { (now - g.start).as_secs_f64() / g.dur.as_secs_f64() };
                let t = t.min(1.0);
                // Ease in-out. Like a person, the pointer leaves the menu bar
                // row before it moves sideways, and lines up before going up
                // to it: sliding along the bar would open the menus it passes.
                let ease = |t: f64| t * t * (3.0 - 2.0 * t);
                let (tx, ty) = if g.from.y == 0 && g.to.y > 0 {
                    (((t - 0.15) / 0.85).clamp(0.0, 1.0), (t / 0.3).min(1.0))
                } else if g.to.y == 0 && g.from.y > 0 {
                    ((t / 0.7).min(1.0), ((t - 0.6) / 0.4).clamp(0.0, 1.0))
                } else {
                    (t, t)
                };
                let p = Point::new(
                    g.from.x + ((g.to.x - g.from.x) as f64 * ease(tx)).round() as i32,
                    g.from.y + ((g.to.y - g.from.y) as f64 * ease(ty)).round() as i32,
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
                Step::WaitText(t, d) => {
                    self.waiting = Some((t, now + d));
                    return (out, Some(now + Duration::from_millis(16)));
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every point the pointer passes on a glide from `from` to `to`.
    fn path(from: Point, to: Point) -> Vec<Point> {
        let mut d = Driver::new();
        d.cps = 60.0;
        d.pointer = Some(from);
        d.queue.push_back(Step::MoveTo(to));
        let t0 = Instant::now();
        let mut pts = vec![from];
        for ms in (0..1000).step_by(4) {
            let (out, _) = d.tick(t0 + Duration::from_millis(ms));
            pts.extend(out.into_iter().filter_map(|o| match o {
                Output::Pointer(p, _) => Some(p),
                _ => None,
            }));
        }
        pts
    }

    #[test]
    fn glides_leave_the_menu_bar_before_moving_sideways() {
        // From a menu title down-left to one of its items, and back up.
        let down = path(Point::new(70, 0), Point::new(62, 6));
        assert_eq!(down.last(), Some(&Point::new(62, 6)));
        assert!(down.iter().all(|p| p.y > 0 || p.x == 70), "slid along the bar: {down:?}");
        let up = path(Point::new(62, 6), Point::new(20, 0));
        assert_eq!(up.last(), Some(&Point::new(20, 0)));
        assert!(up.iter().all(|p| p.y > 0 || p.x == 20), "slid along the bar: {up:?}");
    }
}
