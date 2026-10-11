//! Z4K: Essential Graphics ▸ EffectCraft — EffectCraft templates (`.ectemplate`) as editable
//! graphics, like motion graphics templates made in a compositor.
//!
//! The tab lists the installed templates (Install… adds one); Apply renders the selected one and
//! places it at the playhead. When the selected clip was made from a template, its fields (text,
//! and images such as a logo) are shown; Update renders again with the new values and relinks the
//! clip. Rendering runs EffectCraft's command-line tool on a worker thread, so the editor stays
//! responsive; the result is then placed by the engine commands `graphics.ecTemplate.apply` /
//! `graphics.ecTemplate.set` (which agents can call directly).
//!
//! Automation ids: `ecTemplates.install`, `ecTemplates.item.<n>`, `ecTemplates.apply`,
//! `ecTemplates.field.<control name>`, `ecTemplates.choose.<control name>`,
//! `ecTemplates.reset.<control name>`, `ecTemplates.update`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use egui::{Rect, vec2};
use filmcraft_engine::z4k_ec_templates::{self as ec, TemplateInfo};
use serde_json::{Value, json};

use crate::FilmcraftApp;

/// What a finished render is for.
#[derive(Clone, Debug)]
enum Purpose {
    Apply { template: String, values: BTreeMap<String, String> },
    Set { clip: u64, values: BTreeMap<String, String> },
}

#[derive(Default)]
struct Slot {
    running: Option<Purpose>,
    done: Option<(Purpose, Result<PathBuf, String>)>,
}

/// Per-session view state (egui memory; agents use the engine commands instead).
#[derive(Clone, Default)]
struct View {
    selected: Option<String>,
    /// Clip whose fields are in `draft`.
    draft_clip: Option<u64>,
    draft: BTreeMap<String, String>,
    status: String,
    job: Arc<Mutex<Slot>>,
    library: Arc<(f64, Vec<TemplateInfo>)>,
}

fn view_id() -> egui::Id {
    egui::Id::new("z4k-ec-templates")
}

fn load(ctx: &egui::Context) -> View {
    ctx.data(|d| d.get_temp::<View>(view_id())).unwrap_or_default()
}

fn store(ctx: &egui::Context, v: View) {
    ctx.data_mut(|d| d.insert_temp(view_id(), v));
}

/// The selected timeline clip was made from an EffectCraft template.
pub fn clip_selected(app: &FilmcraftApp) -> bool {
    ec::target(&app.session, &Value::Null).is_some()
}

fn busy(v: &View) -> bool {
    v.job.lock().unwrap_or_else(|e| e.into_inner()).running.is_some()
}

/// Render on a worker thread; the result is picked up by [`poll`].
/// `values` are what EffectCraft renders with; `purpose` says what to do with the movie.
fn start(app: &FilmcraftApp, v: &mut View, ctx: &egui::Context, info: TemplateInfo, values: BTreeMap<String, String>, purpose: Purpose) {
    let dir = match ec::render_dir(&app.session) {
        Ok(d) => d,
        Err(e) => {
            v.status = e.to_string();
            return;
        }
    };
    {
        let mut slot = v.job.lock().unwrap_or_else(|e| e.into_inner());
        if slot.running.is_some() {
            return;
        }
        slot.running = Some(purpose.clone());
    }
    v.status = format!("Rendering {} with EffectCraft…", info.name);
    let job = v.job.clone();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let r = std::panic::catch_unwind(|| ec::render_job(&info, &values, &dir).map_err(|e| e.to_string()))
            .unwrap_or_else(|_| Err("the EffectCraft render stopped unexpectedly".into()));
        let mut slot = job.lock().unwrap_or_else(|e| e.into_inner());
        slot.running = None;
        slot.done = Some((purpose, r));
        drop(slot);
        ctx.request_repaint();
    });
}

/// Place or relink a finished render.
fn poll(app: &mut FilmcraftApp, v: &mut View) {
    let done = v.job.lock().unwrap_or_else(|e| e.into_inner()).done.take();
    let Some((purpose, r)) = done else { return };
    let movie = match r {
        Ok(p) => p.to_string_lossy().to_string(),
        Err(e) => {
            v.status = e;
            return;
        }
    };
    let res = match purpose {
        Purpose::Apply { template, values } => {
            app.session.execute("graphics.ecTemplate.apply", json!({"template": template, "values": values, "render": movie}))
        }
        Purpose::Set { clip, values } => app.session.execute("graphics.ecTemplate.set", json!({"clip": clip, "values": values, "render": movie})),
    };
    match res {
        Ok(r) => {
            if let Some(c) = r.get("clip").and_then(Value::as_u64) {
                let _ = app.session.execute("timeline.select", json!({"clips": [c]}));
            }
            v.status = "Done.".into();
            v.draft_clip = None; // reload the fields from the clip
        }
        Err(e) => v.status = e.to_string(),
    }
}

fn library(app: &FilmcraftApp, v: &mut View, now: f64) -> Vec<TemplateInfo> {
    if now - v.library.0 > 3.0 || v.library.0 == 0.0 {
        v.library = Arc::new((now.max(0.001), ec::library(&app.session)));
    }
    v.library.1.clone()
}

/// The tab's body.
pub fn panel(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let ctx = ui.ctx().clone();
    let t = app.tokens;
    let mut v = load(&ctx);
    poll(app, &mut v);
    let now = ctx.input(|i| i.time);
    let lib = library(app, &mut v, now);
    let running = busy(&v);
    if running {
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
    }
    let mut elems: Vec<(String, Rect, String)> = Vec::new();
    let mut b = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(10.0, 6.0))).id_salt("z4k-ec"));
    b.set_clip_rect(rect);
    let mut apply: Option<String> = None;
    let mut install = false;
    b.horizontal(|ui| {
        ui.label(egui::RichText::new("EffectCraft Templates").strong());
        let r = ui.button("Install…").on_hover_text("Add an EffectCraft template (.ectemplate)");
        elems.push(("ecTemplates.install".into(), r.rect, "Install".into()));
        install = r.clicked();
    });
    b.add_space(4.0);
    egui::ScrollArea::vertical().id_salt("z4k-ec-list").max_height(150.0).auto_shrink([false, true]).show(&mut b, |ui| {
        if lib.is_empty() {
            ui.label(egui::RichText::new("No EffectCraft templates yet. Use Install… to add an .ectemplate.").color(t.text_faint));
        }
        for (i, tpl) in lib.iter().enumerate() {
            let on = v.selected.as_deref() == Some(tpl.path.as_str());
            let fields = tpl.controls.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ");
            let r = ui
                .selectable_label(on, format!("{}   ·   {}×{}, {:.1} s", tpl.name, tpl.width, tpl.height, tpl.duration))
                .on_hover_text(format!("Fields: {fields}"));
            elems.push((format!("ecTemplates.item.{i}"), r.rect, tpl.name.clone()));
            if r.double_clicked() {
                apply = Some(tpl.path.clone());
            } else if r.clicked() {
                v.selected = Some(tpl.path.clone());
            }
        }
    });
    b.horizontal(|ui| {
        let r = ui.add_enabled(v.selected.is_some() && !running, egui::Button::new("Apply"));
        elems.push(("ecTemplates.apply".into(), r.rect, "Apply".into()));
        if r.clicked() {
            apply = v.selected.clone();
        }
        ui.label(egui::RichText::new("places it at the playhead").color(t.text_faint));
    });
    b.add_space(8.0);
    b.separator();

    // the selected template clip
    let mut update: Option<(u64, TemplateInfo, BTreeMap<String, String>)> = None;
    match ec::target(&app.session, &Value::Null) {
        None => {
            b.label(egui::RichText::new("Select a clip made from an EffectCraft template to edit its fields.").color(t.text_faint));
        }
        Some((clip, _, link)) => match ec::read_info(&link.template) {
            Err(e) => {
                b.label(egui::RichText::new(format!("Template unavailable: {e}")).color(t.text_faint));
            }
            Ok(info) => {
                if v.draft_clip != Some(clip.0) {
                    v.draft_clip = Some(clip.0);
                    v.draft = info.effective_values(&link.values);
                }
                b.label(egui::RichText::new(format!("{}  (selected clip)", info.name)).strong());
                b.add_space(4.0);
                let mut pick: Option<String> = None;
                egui::Grid::new("z4k-ec-fields").num_columns(2).spacing(vec2(10.0, 6.0)).show(&mut b, |ui| {
                    for c in &info.controls {
                        ui.label(&c.name);
                        if c.kind == "text" {
                            let mut s = v.draft.get(&c.name).cloned().unwrap_or_default();
                            let r = ui.add(egui::TextEdit::singleline(&mut s).desired_width(220.0));
                            elems.push((format!("ecTemplates.field.{}", c.name), r.rect, c.name.clone()));
                            v.draft.insert(c.name.clone(), s);
                        } else {
                            ui.horizontal(|ui| {
                                let cur = v.draft.get(&c.name).filter(|p| !p.is_empty());
                                let shown = cur
                                    .and_then(|p| std::path::Path::new(p).file_name().map(|n| n.to_string_lossy().to_string()))
                                    .unwrap_or_else(|| "(template image)".into());
                                ui.label(shown);
                                let r = ui.button("Choose…");
                                elems.push((format!("ecTemplates.choose.{}", c.name), r.rect, format!("Choose {}", c.name)));
                                if r.clicked() {
                                    pick = Some(c.name.clone());
                                }
                                let r = ui.add_enabled(cur.is_some(), egui::Button::new("Reset"));
                                elems.push((format!("ecTemplates.reset.{}", c.name), r.rect, format!("Reset {}", c.name)));
                                if r.clicked() {
                                    v.draft.insert(c.name.clone(), String::new());
                                }
                            });
                        }
                        ui.end_row();
                    }
                });
                if let Some(name) = pick
                    && let Some(f) = app.hooks.pick_open_file.as_mut()
                    && let Some(path) = f("Image", &["png", "jpg", "jpeg", "tif", "tiff", "bmp", "gif", "webp", "psd"])
                {
                    v.draft.insert(name, path);
                }
                let changed = v.draft != info.effective_values(&link.values);
                b.add_space(6.0);
                b.horizontal(|ui| {
                    let r = ui.add_enabled(changed && !running, egui::Button::new("Update"));
                    elems.push(("ecTemplates.update".into(), r.rect, "Update".into()));
                    if r.clicked() {
                        // send only the fields that differ from the template's own values
                        let defaults = info.effective_values(&BTreeMap::new());
                        let mut vals = BTreeMap::new();
                        for (k, val) in &v.draft {
                            let is_media = info.controls.iter().any(|c| &c.name == k && c.kind == "media");
                            if is_media || defaults.get(k) != Some(val) || link.values.contains_key(k) {
                                vals.insert(k.clone(), val.clone());
                            }
                        }
                        update = Some((clip.0, info.clone(), vals));
                    }
                    if changed {
                        ui.label(egui::RichText::new("renders the clip again").color(t.text_faint));
                    }
                });
            }
        },
    }
    if !v.status.is_empty() {
        b.add_space(6.0);
        b.horizontal(|ui| {
            if running {
                ui.spinner();
            }
            ui.label(egui::RichText::new(&v.status).color(t.text_dim));
        });
    }

    for (id, r, l) in elems {
        app.auto.add(&id, r, &l);
    }
    if install
        && let Some(f) = app.hooks.pick_open_file.as_mut()
        && let Some(path) = f("EffectCraft Template", &[ec::EXTENSION])
    {
        match app.session.execute("graphics.ecTemplate.install", json!({"path": path})) {
            Ok(r) => {
                v.selected = r.get("path").and_then(Value::as_str).map(str::to_string);
                v.library = Arc::new((0.0, Vec::new()));
                v.status = "Installed.".into();
            }
            Err(e) => v.status = e.to_string(),
        }
    }
    if let Some(path) = apply {
        v.selected = Some(path.clone());
        match ec::read_info(&path) {
            Ok(info) => start(app, &mut v, &ctx, info, BTreeMap::new(), Purpose::Apply { template: path, values: BTreeMap::new() }),
            Err(e) => v.status = e.to_string(),
        }
    }
    if let Some((clip, info, values)) = update {
        // an empty image field means "the template's own image": not rendered, but sent to the
        // engine command so it forgets the old one
        let mut render = values.clone();
        render.retain(|k, val| !(val.is_empty() && info.controls.iter().any(|c| &c.name == k && c.kind == "media")));
        start(app, &mut v, &ctx, info, render, Purpose::Set { clip, values });
    }
    store(&ctx, v);
}
