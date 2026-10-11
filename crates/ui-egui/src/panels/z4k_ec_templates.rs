//! Z4K: EffectCraft templates (`.ectemplate`) in Essential Graphics, like motion graphics
//! templates made in a compositor: they appear in Browse next to the graphics templates (with a
//! rendered thumbnail; drag onto the timeline, double-click or Apply), and Edit shows the fields
//! (text, images such as a logo) of a selected template clip. Update renders again.
//!
//! Rendering runs EffectCraft's command-line tool on a worker thread so the editor stays
//! responsive; the result is placed by the engine commands `graphics.ecTemplate.apply` /
//! `graphics.ecTemplate.set` (which agents call directly). Library ids are `ec:<path>`.
//! After each save, renders the project no longer uses are deleted.
//!
//! Automation ids (Edit tab): `ecTemplates.field.<control name>`, `ecTemplates.choose.<control
//! name>`, `ecTemplates.reset.<control name>`, `ecTemplates.update`.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use egui::{Rect, vec2};
use filmcraft_engine::graphic_templates::LibraryEntry;
use filmcraft_engine::z4k_ec_templates::{self as ec, TemplateInfo};
use filmcraft_project::gtemplate::{GraphicsTemplate, TEMPLATE_FORMAT, TEMPLATE_VERSION};
use filmcraft_time::Tick;
use serde_json::{Value, json};

use crate::FilmcraftApp;

/// Library ids of EffectCraft templates start with this.
pub const PREFIX: &str = "ec:";

pub fn is_ec(id: &str) -> bool {
    id.starts_with(PREFIX)
}

fn path_of(id: &str) -> &str {
    id.strip_prefix(PREFIX).unwrap_or(id)
}

/// What a finished render is for.
#[derive(Clone, Debug)]
enum Purpose {
    Apply { template: String, time: Option<i64>, track: Option<usize> },
    Set { clip: u64, values: BTreeMap<String, String> },
}

#[derive(Default)]
struct Shared {
    running: Option<String>,
    done: Option<(Purpose, Result<PathBuf, String>)>,
    /// Templates whose thumbnail was asked for (rendered once per session at most).
    thumbs: HashSet<String>,
}

/// Per-session state (egui memory; agents use the engine commands instead).
#[derive(Clone, Default)]
struct View {
    draft_clip: Option<u64>,
    draft: BTreeMap<String, String>,
    shared: Arc<Mutex<Shared>>,
    library: Arc<(f64, Vec<TemplateInfo>)>,
    saved_revision: Option<u64>,
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

fn running(v: &View) -> bool {
    v.shared.lock().unwrap_or_else(|e| e.into_inner()).running.is_some()
}

// ------------------------------------------------------------------------------------------------
// Library (Browse)
// ------------------------------------------------------------------------------------------------

fn templates(app: &FilmcraftApp, v: &mut View, now: f64) -> Vec<TemplateInfo> {
    if v.library.0 == 0.0 || now - v.library.0 > 3.0 {
        v.library = Arc::new((now.max(0.001), ec::library(&app.session)));
    }
    v.library.1.clone()
}

/// The EffectCraft templates as Browse library entries (id `ec:<path>`, category "EffectCraft").
pub fn library_entries(app: &FilmcraftApp, ctx: &egui::Context) -> Vec<LibraryEntry> {
    let mut v = load(ctx);
    let now = ctx.input(|i| i.time);
    let list = templates(app, &mut v, now);
    store(ctx, v);
    list.into_iter()
        .map(|t| {
            let fields = t.controls.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ");
            let secs = if t.duration.is_finite() { t.duration.clamp(0.0, 86_400.0) } else { 0.0 };
            let template = GraphicsTemplate {
                format: TEMPLATE_FORMAT.into(),
                version: TEMPLATE_VERSION,
                id: format!("{PREFIX}{}", t.path),
                name: t.name.clone(),
                category: "EffectCraft".into(),
                description: format!("EffectCraft template · {}×{} · {secs:.1} s · editable: {fields}", t.width, t.height),
                author: String::new(),
                license: String::new(),
                tags: t.controls.iter().map(|c| c.name.clone()).collect(),
                canvas: [t.width.max(1), t.height.max(1)],
                duration: Tick((secs * filmcraft_time::TICKS_PER_SECOND as f64) as i64),
                layers: Vec::new(),
                graphic: Default::default(),
                controls: Vec::new(),
                resources: Default::default(),
            };
            LibraryEntry { template, path: Some(t.path) }
        })
        .collect()
}

/// A Browse card thumbnail for an EffectCraft template: the rendered still, or None while it is
/// being made (one worker per template).
pub fn thumb_texture(app: &FilmcraftApp, ctx: &egui::Context, id: &str) -> Option<egui::TextureHandle> {
    let path = path_of(id).to_string();
    let info = ec::read_info(&path).ok()?;
    let file = ec::thumb_path(&info, &ec::thumb_dir(&app.session));
    let key = egui::Id::new(("z4k-ec-thumb", file.to_string_lossy().to_string()));
    if let Some(t) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(key)) {
        return Some(t);
    }
    if let Some(img) = load_png(&file) {
        let t = ctx.load_texture(format!("z4k-ec-thumb-{}", file.to_string_lossy()), img, egui::TextureOptions::LINEAR);
        ctx.data_mut(|d| d.insert_temp(key, t.clone()));
        return Some(t);
    }
    let v = load(ctx);
    let first = v.shared.lock().unwrap_or_else(|e| e.into_inner()).thumbs.insert(format!("{path}|{}", file.to_string_lossy()));
    if first {
        let ctx2 = ctx.clone();
        std::thread::spawn(move || {
            let _ = std::panic::catch_unwind(|| {
                if let Some(dir) = file.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = ec::render_thumbnail(&info, &file);
            });
            ctx2.request_repaint();
        });
    }
    store(ctx, v);
    None
}

fn load_png(path: &Path) -> Option<egui::ColorImage> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() > 32 * 1024 * 1024 {
        return None;
    }
    let img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png).ok()?.to_rgba8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    (w > 0 && h > 0 && w <= 4096 && h <= 4096).then(|| egui::ColorImage::from_rgba_unmultiplied([w, h], img.as_raw()))
}

// ------------------------------------------------------------------------------------------------
// Rendering jobs
// ------------------------------------------------------------------------------------------------

/// Render on a worker thread; [`poll`] places the result.
fn start(app: &mut FilmcraftApp, ctx: &egui::Context, info: TemplateInfo, values: BTreeMap<String, String>, purpose: Purpose) {
    let dir = match ec::render_dir(&app.session) {
        Ok(d) => d,
        Err(e) => {
            app.ui.status = e.to_string();
            return;
        }
    };
    let v = load(ctx);
    {
        let mut sh = v.shared.lock().unwrap_or_else(|e| e.into_inner());
        if sh.running.is_some() {
            app.ui.status = "An EffectCraft template is still rendering; try again in a moment.".into();
            return;
        }
        sh.running = Some(info.name.clone());
    }
    let res = ec::resolution_for(&info, ec::sequence_size(&app.session));
    app.ui.status = format!("Rendering {} with EffectCraft…", info.name);
    let shared = v.shared.clone();
    let ctx2 = ctx.clone();
    std::thread::spawn(move || {
        let r = std::panic::catch_unwind(|| ec::render_job(&info, &values, res, &dir).map_err(|e| e.to_string()))
            .unwrap_or_else(|_| Err("the EffectCraft render stopped unexpectedly".into()));
        let mut sh = shared.lock().unwrap_or_else(|e| e.into_inner());
        sh.running = None;
        sh.done = Some((purpose, r));
        drop(sh);
        ctx2.request_repaint();
    });
    store(ctx, v);
}

/// Apply EffectCraft template `id` (`ec:<path>`), at `time` / on video track `track` when given
/// (a drop on the timeline), else at the playhead.
pub fn start_apply(app: &mut FilmcraftApp, ctx: &egui::Context, id: &str, time: Option<i64>, track: Option<usize>) {
    let path = path_of(id).to_string();
    match ec::read_info(&path) {
        Ok(info) => start(app, ctx, info, BTreeMap::new(), Purpose::Apply { template: path, time, track }),
        Err(e) => app.ui.status = e.to_string(),
    }
}

/// Every frame: place finished renders, and clean up unused renders after a save.
pub fn poll(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let mut v = load(ctx);
    let done = v.shared.lock().unwrap_or_else(|e| e.into_inner()).done.take();
    if let Some((purpose, r)) = done {
        match r {
            Err(e) => app.ui.status = e,
            Ok(p) => {
                let movie = p.to_string_lossy().to_string();
                let res = match purpose {
                    Purpose::Apply { template, time, track } => {
                        let mut q = json!({"template": template, "render": movie});
                        if let Some(t) = time {
                            q["time"] = json!(t);
                        }
                        if let Some(t) = track {
                            q["track"] = json!(t);
                        }
                        app.session.execute("graphics.ecTemplate.apply", q)
                    }
                    Purpose::Set { clip, values } => app.session.execute("graphics.ecTemplate.set", json!({"clip": clip, "values": values, "render": movie})),
                };
                match res {
                    Ok(r) => {
                        if let Some(c) = r.get("clip").and_then(Value::as_u64) {
                            let _ = app.session.execute("timeline.select", json!({"clips": [c]}));
                        }
                        app.ui.status = "EffectCraft template rendered.".into();
                        v.draft_clip = None; // reload the fields from the clip
                    }
                    Err(e) => app.ui.status = e.to_string(),
                }
            }
        }
    }
    // after a save, drop renders nothing uses any more
    let saved = app.session.saved_revision;
    if app.session.path.is_some() && v.saved_revision.is_some_and(|r| r != saved) && saved == app.session.revision {
        let n = ec::cleanup_renders(&app.session);
        if n > 0 {
            app.ui.status = format!("Removed {n} unused EffectCraft render{}.", if n == 1 { "" } else { "s" });
        }
    }
    v.saved_revision = Some(saved);
    store(ctx, v);
}

// ------------------------------------------------------------------------------------------------
// Edit tab
// ------------------------------------------------------------------------------------------------

/// Essential Graphics ▸ Edit for a clip made from an EffectCraft template.
pub fn edit(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let ctx = ui.ctx().clone();
    let t = app.tokens;
    let Some((clip, _, link)) = ec::target(&app.session, &Value::Null) else { return };
    let mut v = load(&ctx);
    let busy = running(&v);
    if busy {
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }
    let mut elems: Vec<(String, Rect, String)> = Vec::new();
    let mut b = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(12.0, 8.0))).id_salt("z4k-ec-edit"));
    b.set_clip_rect(rect);
    let info = match ec::read_info(&link.template) {
        Ok(i) => i,
        Err(e) => {
            b.label(egui::RichText::new(format!("Template unavailable: {e}")).color(t.text_faint));
            store(&ctx, v);
            return;
        }
    };
    if v.draft_clip != Some(clip.0) {
        v.draft_clip = Some(clip.0);
        v.draft = info.effective_values(&link.values);
    }
    b.label(egui::RichText::new(&info.name).strong());
    b.label(egui::RichText::new("EffectCraft template").color(t.text_faint).size(11.0));
    b.add_space(8.0);
    let mut pick: Option<String> = None;
    egui::Grid::new("z4k-ec-fields").num_columns(2).spacing(vec2(12.0, 8.0)).show(&mut b, |ui| {
        for c in &info.controls {
            ui.label(egui::RichText::new(&c.name).color(t.text_dim));
            if c.kind == "text" {
                let mut s = v.draft.get(&c.name).cloned().unwrap_or_default();
                let r = ui.add(egui::TextEdit::singleline(&mut s).desired_width((rect.width() - 170.0).clamp(120.0, 320.0)));
                elems.push((format!("ecTemplates.field.{}", c.name), r.rect, c.name.clone()));
                v.draft.insert(c.name.clone(), s);
            } else {
                ui.horizontal(|ui| {
                    let cur = v.draft.get(&c.name).filter(|p| !p.is_empty());
                    let shown =
                        cur.and_then(|p| Path::new(p).file_name().map(|n| n.to_string_lossy().to_string())).unwrap_or_else(|| "(template image)".into());
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
    let current = info.effective_values(&link.values);
    let changed = v.draft.iter().any(|(k, val)| current.get(k).map(String::as_str).unwrap_or("") != val.as_str());
    let mut update = false;
    b.add_space(10.0);
    b.horizontal(|ui| {
        let r = ui.add_enabled(changed && !busy, egui::Button::new("Update"));
        elems.push(("ecTemplates.update".into(), r.rect, "Update".into()));
        update = r.clicked();
        if busy {
            ui.spinner();
            ui.label(egui::RichText::new("Rendering…").color(t.text_dim));
        } else if changed {
            ui.label(egui::RichText::new("renders the graphic with the new values").color(t.text_faint));
        }
    });
    b.add_space(6.0);
    b.label(
        egui::RichText::new(
            "The template file isn't changed: these values belong to this clip. To change the design, edit the template in EffectCraft and export it again.",
        )
        .color(t.text_faint)
        .size(11.0),
    );
    for (id, r, l) in elems {
        app.auto.add(&id, r, &l);
    }
    let values = v.draft.clone();
    store(&ctx, v);
    if update {
        // an empty image field means "the template's own image": not rendered, but sent to the
        // engine command so the clip forgets the old one
        let mut render = values.clone();
        render.retain(|k, val| !(val.is_empty() && info.controls.iter().any(|c| &c.name == k && c.kind == "media")));
        start(app, &ctx, info, render, Purpose::Set { clip: clip.0, values });
    }
}
