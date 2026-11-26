//! Vim for Turbine.
//!
//! `vi`, `ex` and `text` come from tessh's editor (Tessera,
//! `tessh/src/editor/{vi,ex,text}.rs`, as of tessera 731ca5d), kept close to
//! the original so fixes can flow both ways. Turbine's changes there: ex
//! commands for files and windows reach the host as `ExAction`s
//! (`ViOut::Actions`). `session` is Turbine's: a host-facing wrapper with
//! insert-mode keys, macros and `Ctrl-W`.

pub mod ex;
pub mod session;
pub mod text;
pub mod vi;

pub use ex::ExAction;
pub use session::{Out, Session};
