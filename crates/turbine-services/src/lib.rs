//! Turbine's background services. Each runs in other threads or processes
//! and talks to the UI thread only through channels, polled on ticks.

pub mod fake;
pub mod lsp;
pub mod rpc;
