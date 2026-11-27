//! Turbine's background services. Each runs in other threads or processes
//! and talks to the UI thread only through channels, polled on ticks.

pub mod ai;
pub mod build;
pub mod dap;
pub mod env;
pub mod fake;
#[allow(unused_variables, unused_mut, unused_assignments)] // (as in Tessera)
pub mod guides;
pub mod lsp;
pub mod rpc;
