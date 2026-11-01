//! Z4K: the Lumetri Color panel grades an adjustment layer selected in the Project panel; see
//! `filmcraft_engine::z4k_bin_effects`. The panel keeps its target while it is being used itself.

use filmcraft_engine::project::{EffectInstance, ItemId, ItemKind};
use serde_json::{Value, json};

use crate::FilmcraftApp;
use crate::dock::PanelKind;

fn single_layer(app: &FilmcraftApp) -> Option<u64> {
    match app.session.state.project_selection.as_slice() {
        [id] if matches!(app.session.project.item(*id).map(|i| &i.kind), Some(ItemKind::AdjustmentLayer { .. })) => Some(id.0),
        _ => None,
    }
}

/// Once a frame: the Project panel takes the Lumetri panel, the Timeline takes it back.
pub fn after_frame(app: &mut FilmcraftApp) {
    match app.ui.focused {
        PanelKind::Project => app.ui.project_panel.z4k_lumetri_item = single_layer(app),
        PanelKind::Timeline => app.ui.project_panel.z4k_lumetri_item = None,
        _ => {}
    }
}

/// The bin item the panel grades now, if any.
pub fn target(app: &FilmcraftApp) -> Option<u64> {
    app.ui.project_panel.z4k_lumetri_item.filter(|i| single_layer(app) == Some(*i))
}

/// The item's name and effects.
pub fn view(app: &FilmcraftApp, item: u64) -> (String, Vec<EffectInstance>) {
    let p = &app.session.project;
    let id = ItemId(item);
    (p.item(id).map(|i| i.name.clone()).unwrap_or_default(), p.z4k_bin_effects.get(&id).cloned().unwrap_or_default())
}

/// The bin command for one of the panel's clip commands.
pub fn route(cmd: &str) -> Option<&'static str> {
    Some(match cmd {
        "effects.apply" => "z4k.binLumetri.apply",
        "effects.toggleEnabled" => "z4k.binLumetri.toggle",
        "effects.setParam" => "z4k.binLumetri.setParam",
        "lumetri.setSection" => "z4k.binLumetri.setSection",
        "lumetri.setInputLut" | "lumetri.setLook" => "z4k.binLumetri.setLut",
        _ => return None,
    })
}

/// Run one of the panel's commands on its clip, or on `bin` when it grades a bin item.
pub fn exec(app: &mut FilmcraftApp, bin: Option<u64>, cmd: &str, mut p: Value) {
    let r = match bin {
        None => app.session.execute(cmd, p).map_err(|e| e.to_string()),
        Some(item) => match route(cmd) {
            Some(to) => {
                p["item"] = json!(item);
                if cmd == "lumetri.setLook" {
                    p["input"] = json!(false);
                }
                app.session.execute(to, p).map_err(|e| e.to_string())
            }
            None => Err(tl!("Color Match needs a clip on the timeline").to_string()),
        },
    };
    if let Err(e) = r {
        app.ui.status = e;
    }
}
