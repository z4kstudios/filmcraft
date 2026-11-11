//! Z4K: View ▸ Full Screen Program Monitor (⌘`). The mode itself is upstream's
//! `view.fullScreenProgram` (cinema mode, #3652); upstream gives it no menu item and no default
//! key, so this only adds both and runs their command. Our own drawing code was retired
//! 2026-11-10 (tag `fallback/filmcraft-full-screen-program`).

use serde_json::{Value, json};

use crate::FilmcraftApp;
use crate::menus::UiCommand;

pub const ID: &str = "z4k.view.fullScreenProgram";

pub const COMMANDS: &[UiCommand] = &[UiCommand { id: ID, label: "Full Screen Program Monitor", menu: &["View"], shortcut: Some("Cmd+`") }];

/// Run the command; None when `id` is not this one.
pub fn route(app: &mut FilmcraftApp, ctx: &egui::Context, id: &str) -> Option<Result<Value, String>> {
    (id == ID).then(|| {
        crate::panels::keyboard::route(app, ctx, "view.fullScreenProgram", &json!({}))
            .unwrap_or_else(|| Err("view.fullScreenProgram is not available".to_owned()))
    })
}
