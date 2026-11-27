//! Turbine's widget toolkit.
//!
//! Everything draws into a [`Surface`] of character cells. Front ends
//! (terminal, desktop app, web) only turn surfaces into pixels or escape
//! sequences and turn their input into [`Event`]s.

pub mod automation;
pub mod dialog;
pub mod editor;
pub mod event;
pub mod frame;
pub mod geom;
pub mod menu;
pub mod statusbar;
pub mod surface;
pub mod theme;
pub mod window;

pub use event::{Event, Key, KeyEvent, Mods, MouseEvent};
pub use geom::{Point, Rect};
pub use surface::{Canvas, Cell, CursorShape, Surface};
pub use theme::Theme;

use std::time::Instant;

/// What a front end runs.
pub trait App {
    fn event(&mut self, ev: Event);
    /// Advance timers and animations. Returns when it next wants a tick.
    fn tick(&mut self, now: Instant) -> Option<Instant>;
    fn render(&mut self, surface: &mut Surface);
    fn should_quit(&self) -> bool;
    /// Text the app copied, for the system clipboard (taken once).
    fn take_clipboard(&mut self) -> Option<String> {
        None
    }
}
