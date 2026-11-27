//! Turbine's built-in terminal.
//!
//! Ghostty's terminal engine (libghostty-vt, as in Tessera) parses what the
//! program writes; its cell grid is drawn into Turbine's own, so the
//! terminal works the same in TUI mode (a terminal inside a terminal), the
//! desktop app and the web.

pub mod terminal;
pub mod view;

pub use terminal::{MouseAct, MouseOutcome, Spawn, Terminal};
pub use view::TerminalView;
