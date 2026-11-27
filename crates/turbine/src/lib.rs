//! Turbine's IDE: the workbench (menus, windows, editors, terminals,
//! dialogs), the agent driver and tools, and the automation endpoints.
//! Front ends (the terminal one in `main.rs`, the Qt app in turbine-app)
//! run a [`workbench::Workbench`] as a `turbine_ui::App`.

pub mod agent;
pub mod dock;
pub mod file_dialog;
pub mod filekind;
pub mod filetypes;
pub mod diffview;
pub mod markdown;
pub mod mcp;
pub mod palette;
pub mod server;
pub mod structview;
pub mod tools;
pub mod views;
pub mod vim;
pub mod workbench;
pub mod xtree;

/// Call first in a front end's main: as a grammar's trial child (a
/// grammar is tried in a child before it's loaded), try it and exit.
pub fn grammars_check_from_args() {
    turbine_syntax::grammars::check_from_args();
}
