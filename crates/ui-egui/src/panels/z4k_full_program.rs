//! Z4K: Full Screen Program Monitor (⌘`): the window goes full screen and shows only the program
//! picture on black, like Premiere's full-screen video. Playback keys keep working; ⌘` or Esc
//! goes back to the normal layout (and leaves window full screen if it was already on). The mouse
//! pointer is hidden while it is on.
//! State lives in egui temp memory, so nothing is saved in the project or the workspace.

use egui::{Color32, Rect, vec2};
use serde_json::{Value, json};

use crate::FilmcraftApp;
use crate::menus::UiCommand;
use crate::panels::monitor::{self, Which};

pub const ID: &str = "z4k.view.fullScreenProgram";

pub const COMMANDS: &[UiCommand] = &[UiCommand { id: ID, label: "Full Screen Program Monitor", menu: &["View"], shortcut: Some("Cmd+`") }];

/// On: Some(whether the window was already full screen before).
fn state_id() -> egui::Id {
    egui::Id::unique("z4k-full-program")
}

pub fn active(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(state_id())).is_some()
}

/// Run the command; None when `id` is not this one.
pub fn route(ctx: &egui::Context, id: &str) -> Option<Result<Value, String>> {
    (id == ID).then(|| Ok(toggle(ctx)))
}

fn toggle(ctx: &egui::Context) -> Value {
    if let Some(was_full) = ctx.data_mut(|d| d.remove_temp::<bool>(state_id())) {
        if !was_full {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        }
        ctx.request_repaint();
        return json!({"fullScreenProgram": false});
    }
    let was_full = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
    ctx.data_mut(|d| d.insert_temp(state_id(), was_full));
    if !was_full {
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
    }
    ctx.request_repaint();
    json!({"fullScreenProgram": true})
}

/// Draw the program picture over the whole window. Called instead of the panels while active.
pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        toggle(&ctx);
        return;
    }
    let screen = ctx.content_rect();
    ctx.set_cursor_icon(egui::CursorIcon::None); // no mouse pointer over the picture
    egui::Area::new(egui::Id::unique("z4k-full-program-area")).order(egui::Order::Foreground).fixed_pos(screen.min).show(&ctx, |ui| {
        ui.set_clip_rect(screen);
        ui.painter().rect_filled(screen, 0.0, Color32::BLACK);
        // The monitor keeps a 4 px margin and its time/transport rows (88 px at this width) under
        // the picture: give it a rect that puts those outside the screen.
        let rect = Rect::from_min_max(screen.min - vec2(4.0, 4.0), screen.max + vec2(4.0, 4.0 + 88.0));
        let tokens = app.tokens;
        app.tokens.panel_bg = Color32::BLACK;
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).id_salt("z4k-full-program"));
        child.set_clip_rect(screen);
        monitor::show(app, &mut child, rect, Which::Program);
        app.tokens = tokens;
        app.auto.add("z4k.fullScreenProgram", screen, "Full Screen Program Monitor");
    });
}
