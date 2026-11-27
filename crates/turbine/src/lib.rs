//! Turbine's IDE: the workbench (menus, windows, editors, terminals,
//! dialogs), the agent driver and tools, and the automation endpoints.
//! Front ends (the terminal one in `main.rs`, the Qt app in turbine-app)
//! run a [`workbench::Workbench`] as a `turbine_ui::App`.

pub mod agent;
pub mod dock;
pub mod file_dialog;
pub mod mcp;
pub mod palette;
pub mod server;
pub mod tools;
pub mod views;
pub mod vim;
pub mod workbench;
