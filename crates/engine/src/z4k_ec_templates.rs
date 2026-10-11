//! Z4K: EffectCraft graphics templates (`.ectemplate`) in FilmCraft, the way motion graphics
//! templates made in a compositor work in other editors.
//!
//! An `.ectemplate` is EffectCraft's Essential Graphics template: a composition with editable
//! controls (text, media replacement). FilmCraft does not render compositions itself, so it asks
//! EffectCraft's command-line tool (`effectcraft-cli`) to render the template with the chosen
//! values into a ProRes 4444 movie with alpha, places that movie like any other clip, and keeps
//! the template and its values in the media item's metadata ([`METADATA_KEY`]). Editing a value
//! renders again and relinks the item to the new movie (one undo step).
//!
//! - Templates live next to the `.fcgt` graphics templates (`<data dir>/Graphics Templates`);
//!   `graphics.ecTemplate.install` copies one there.
//! - Renders go to `<project folder>/<project name> EffectCraft Renders/` (the data directory's
//!   `EffectCraft Renders` for an unsaved project). The same template and values reuse a render.
//! - A replaced image is scaled to cover the original image's footprint, centred, and the layer's
//!   masks follow, so a logo of any size lands in the same (e.g. circular) frame.
//! - The tool is found through `FILMCRAFT_EFFECTCRAFT_CLI`, then `~/.local/craft-cli/effectcraft-cli`,
//!   then `PATH`.
//!
//! Commands: `graphics.ecTemplate.list|install|apply|controls|set`. `apply` and `set` accept a
//! movie already rendered with [`render_job`] as `render` (the UI renders on a worker thread).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use filmcraft_project::{ClipId, ItemId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad, has_seq, str_p, u64_p};
use crate::{EngineError, Result, Session};

/// File extension of EffectCraft templates.
pub const EXTENSION: &str = "ectemplate";
/// Media item metadata key holding the template link ([`Link`] as JSON).
pub const METADATA_KEY: &str = "Z4K EffectCraft Template";

const CLI_ENV: &str = "FILMCRAFT_EFFECTCRAFT_CLI";
/// Largest number of editable controls read from a manifest (manifests are untrusted).
const MAX_CONTROLS: usize = 64;
/// Longest text value accepted for a control.
const MAX_TEXT: usize = 4096;

/// One editable control of a template.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Control {
    pub name: String,
    /// "text" or "media".
    pub kind: String,
    /// Layer id in the template's composition.
    pub layer: u64,
    /// Property path (text controls).
    pub path: String,
    /// Footage item id (media controls).
    pub item: u64,
    /// The template's own value (text controls).
    pub default: String,
}

/// A template's manifest, as FilmCraft uses it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TemplateInfo {
    pub path: String,
    pub name: String,
    pub comp_name: String,
    pub width: u32,
    pub height: u32,
    pub frame_rate: f64,
    pub duration: f64,
    pub controls: Vec<Control>,
}

impl TemplateInfo {
    fn control(&self, key: &str) -> Option<&Control> {
        self.controls.iter().find(|c| c.name == key).or_else(|| self.controls.iter().find(|c| c.name.eq_ignore_ascii_case(key)))
    }

    /// Values for every control: the template's defaults overlaid with `values` (media controls
    /// without a value keep the template's own media and have no entry).
    pub fn effective_values(&self, values: &BTreeMap<String, String>) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        for c in &self.controls {
            if let Some(v) = values.get(&c.name) {
                out.insert(c.name.clone(), v.clone());
            } else if c.kind == "text" {
                out.insert(c.name.clone(), c.default.clone());
            }
        }
        out
    }
}

/// What a placed template clip remembers (stored in the media item's metadata).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Link {
    pub template: String,
    pub values: BTreeMap<String, String>,
}

impl Link {
    pub fn from_metadata(md: &BTreeMap<String, String>) -> Option<Link> {
        md.get(METADATA_KEY).and_then(|s| serde_json::from_str(s).ok())
    }
}

fn other(msg: impl Into<String>) -> EngineError {
    EngineError::Other(msg.into())
}

// ------------------------------------------------------------------------------------------------
// The EffectCraft command-line tool
// ------------------------------------------------------------------------------------------------

fn cli_exe_name() -> &'static str {
    if cfg!(windows) { "effectcraft-cli.exe" } else { "effectcraft-cli" }
}

/// Path of `effectcraft-cli`.
pub fn cli_path() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os(CLI_ENV).filter(|p| !p.is_empty()) {
        return Ok(PathBuf::from(p));
    }
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        let p = PathBuf::from(home).join(".local").join("craft-cli").join(cli_exe_name());
        if p.is_file() {
            return Ok(p);
        }
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for d in std::env::split_paths(&paths) {
            let p = d.join(cli_exe_name());
            if p.is_file() {
                return Ok(p);
            }
        }
    }
    Err(other(format!("EffectCraft's command-line tool ({}) was not found; install it or set {CLI_ENV}", cli_exe_name())))
}

/// Run `effectcraft-cli` with `args`; its standard output on success.
fn run_cli(args: &[&str]) -> Result<String> {
    let cli = cli_path()?;
    let out = std::process::Command::new(&cli).args(args).output().map_err(|e| other(format!("can't run {}: {e}", cli.display())))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let msg = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("failed");
        return Err(other(format!("EffectCraft: {}", msg.trim())));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn run_cli_json(args: &[&str]) -> Result<Value> {
    let out = run_cli(args)?;
    serde_json::from_str(out.trim()).map_err(|e| other(format!("EffectCraft returned something unexpected: {e}")))
}

// ------------------------------------------------------------------------------------------------
// Templates
// ------------------------------------------------------------------------------------------------

/// Parse the manifest JSON that `essential.templateInfo` returns.
pub fn parse_info(path: &str, v: &Value) -> Result<TemplateInfo> {
    if v.get("format").and_then(Value::as_str).is_some_and(|f| f != "effectcraft-template") {
        return Err(other(format!("{path} is not an EffectCraft template")));
    }
    let num = |k: &str| v.get(k).and_then(Value::as_f64).filter(|x| x.is_finite() && *x >= 0.0);
    let dim = |k: &str| num(k).map(|x| x.min(16384.0) as u32).unwrap_or(0);
    let mut controls = Vec::new();
    for c in v.get("controls").and_then(Value::as_array).into_iter().flatten().take(MAX_CONTROLS) {
        let name = c.get("name").and_then(Value::as_str).unwrap_or("").to_string();
        let ty = c.get("type").and_then(Value::as_str).unwrap_or("");
        if name.is_empty() || !(ty == "text" || ty == "media") {
            continue;
        }
        let u = |k: &str| c.get(k).and_then(Value::as_u64).unwrap_or(0);
        controls.push(Control {
            name,
            kind: ty.to_string(),
            layer: u("layer"),
            path: c.get("path").and_then(Value::as_str).unwrap_or("").to_string(),
            item: u("item"),
            default: c.get("value").and_then(Value::as_str).unwrap_or("").to_string(),
        });
    }
    Ok(TemplateInfo {
        path: path.to_string(),
        name: v.get("name").and_then(Value::as_str).unwrap_or("EffectCraft Template").to_string(),
        comp_name: v.get("compName").and_then(Value::as_str).unwrap_or("").to_string(),
        width: dim("width"),
        height: dim("height"),
        frame_rate: num("frameRate").unwrap_or(0.0),
        duration: num("duration").unwrap_or(0.0),
        controls,
    })
}

fn file_stamp(path: &Path) -> Option<(u64, u64)> {
    let m = std::fs::metadata(path).ok()?;
    let t = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    Some((t, m.len()))
}

/// Read a template's manifest (cached while the file is unchanged).
pub fn read_info(path: &str) -> Result<TemplateInfo> {
    static CACHE: OnceLock<Mutex<HashMap<String, ((u64, u64), TemplateInfo)>>> = OnceLock::new();
    if !path.to_ascii_lowercase().ends_with(&format!(".{EXTENSION}")) {
        return Err(other(format!("{path} is not an EffectCraft template (.{EXTENSION})")));
    }
    let stamp = file_stamp(Path::new(path)).ok_or_else(|| other(format!("can't read {path}")))?;
    if let Some((s, i)) = CACHE.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner()).get(path)
        && *s == stamp
    {
        return Ok(i.clone());
    }
    let params = json!({ "path": path }).to_string();
    let v = run_cli_json(&["exec", "essential.templateInfo", "--params", &params, "--empty"])?;
    let info = parse_info(path, &v)?;
    let mut c = CACHE.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    if c.len() > 64 {
        c.clear();
    }
    c.insert(path.to_string(), (stamp, info.clone()));
    Ok(info)
}

/// Folder that holds installed templates (shared with the `.fcgt` templates).
pub fn templates_dir(s: &Session) -> Option<PathBuf> {
    crate::graphic_templates::user_templates_dir(s)
}

/// Installed templates, by file name (files that can't be read are skipped).
pub fn library(s: &Session) -> Vec<TemplateInfo> {
    let Some(dir) = templates_dir(s) else { return Vec::new() };
    let Ok(rd) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut paths: Vec<PathBuf> =
        rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case(EXTENSION))).collect();
    paths.sort();
    paths.into_iter().take(500).filter_map(|p| read_info(&p.to_string_lossy()).ok()).collect()
}

/// Find a template by path or by name among the installed ones.
pub fn find(s: &Session, key: &str) -> Result<TemplateInfo> {
    if key.to_ascii_lowercase().ends_with(&format!(".{EXTENSION}")) {
        return read_info(key);
    }
    library(s).into_iter().find(|t| t.name.eq_ignore_ascii_case(key)).ok_or_else(|| bad("graphics.ecTemplate", format!("no EffectCraft template `{key}`")))
}

fn info_json(t: &TemplateInfo, values: Option<&BTreeMap<String, String>>) -> Value {
    let vals = values.map(|v| t.effective_values(v));
    json!({
        "path": t.path, "name": t.name, "comp": t.comp_name, "width": t.width, "height": t.height,
        "frameRate": t.frame_rate, "seconds": t.duration,
        "controls": t.controls.iter().map(|c| {
            let mut j = json!({"name": c.name, "kind": c.kind, "default": c.default});
            if let Some(v) = &vals {
                j["value"] = v.get(&c.name).map(|x| json!(x)).unwrap_or(Value::Null);
            }
            j
        }).collect::<Vec<_>>(),
    })
}

// ------------------------------------------------------------------------------------------------
// Rendering
// ------------------------------------------------------------------------------------------------

fn fnv(h: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(h, |h, b| (h ^ *b as u64).wrapping_mul(0x100_0000_01b3))
}

/// Key of a render: template file + values (+ the replaced files' stamps).
fn render_key(t: &TemplateInfo, values: &BTreeMap<String, String>) -> String {
    let mut h = fnv(0xcbf2_9ce4_8422_2325, t.path.as_bytes());
    if let Some((a, b)) = file_stamp(Path::new(&t.path)) {
        h = fnv(h, &a.to_le_bytes());
        h = fnv(h, &b.to_le_bytes());
    }
    for (k, v) in values {
        h = fnv(h, k.as_bytes());
        h = fnv(h, &[0]);
        h = fnv(h, v.as_bytes());
        h = fnv(h, &[1]);
        if t.control(k).is_some_and(|c| c.kind == "media")
            && let Some((a, b)) = file_stamp(Path::new(v))
        {
            h = fnv(h, &a.to_le_bytes());
            h = fnv(h, &b.to_le_bytes());
        }
    }
    format!("{h:016x}")
}

/// A file-name-safe version of `name`.
fn safe_name(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' { c } else { '_' }).take(60).collect();
    let s = s.trim().to_string();
    if s.is_empty() { "EffectCraft Template".into() } else { s }
}

/// Where renders go for this session's project.
pub fn render_dir(s: &Session) -> Result<PathBuf> {
    if let Some(p) = s.path.as_deref().map(Path::new)
        && let (Some(dir), Some(stem)) = (p.parent(), p.file_stem())
    {
        return Ok(dir.join(format!("{} EffectCraft Renders", stem.to_string_lossy())));
    }
    if let Some(d) = s.prefs_path.as_ref().and_then(|p| p.parent()) {
        return Ok(d.join("EffectCraft Renders"));
    }
    Ok(std::env::temp_dir().join("FilmCraft EffectCraft Renders"))
}

fn arr2(v: &Value) -> Option<[f64; 2]> {
    let a = v.as_array()?;
    let x = a.first()?.as_f64()?;
    let y = a.get(1)?.as_f64()?;
    (x.is_finite() && y.is_finite()).then_some([x, y])
}

fn map_points(v: &Value, f: impl Fn([f64; 2]) -> [f64; 2]) -> Value {
    Value::Array(v.as_array().into_iter().flatten().map(|p| arr2(p).map(|q| json!(f(q))).unwrap_or_else(|| p.clone())).collect())
}

/// Property changes that make a layer whose footage changed from `old` to `new` pixels cover the
/// old footprint, centred, with its masks following. `props` is the layer's flat property list
/// (`effectcraft-cli props … --flat --json`); animated properties are left alone.
pub fn fit_props(props: &[Value], old: (f64, f64), new: (f64, f64)) -> Vec<(String, Value)> {
    let ok = |x: f64| x.is_finite() && x > 0.0;
    if !(ok(old.0) && ok(old.1) && ok(new.0) && ok(new.1)) {
        return Vec::new();
    }
    let k = (old.0 / new.0).max(old.1 / new.1);
    if !ok(k) {
        return Vec::new();
    }
    let (c0, c1) = ([old.0 / 2.0, old.1 / 2.0], [new.0 / 2.0, new.1 / 2.0]);
    let pt = |p: [f64; 2]| [(p[0] - c0[0]) / k + c1[0], (p[1] - c0[1]) / k + c1[1]];
    let vec = |p: [f64; 2]| [p[0] / k, p[1] / k];
    let mut out = Vec::new();
    for p in props {
        let Some(path) = p.get("path").and_then(Value::as_str) else { continue };
        if p.get("keys").and_then(Value::as_u64).is_some_and(|n| n > 0) || p.get("expression").is_some_and(|e| !e.is_null()) {
            continue;
        }
        let Some(v) = p.get("value") else { continue };
        let third = |v: &Value| v.as_array().and_then(|a| a.get(2)).and_then(Value::as_f64).unwrap_or(0.0);
        match path {
            "transform/anchor" => {
                if let Some(a) = arr2(v) {
                    let n = pt(a);
                    out.push((path.to_string(), json!([n[0], n[1], third(v)])));
                }
            }
            "transform/scale" => {
                if let Some(s) = arr2(v) {
                    out.push((path.to_string(), json!([s[0] * k, s[1] * k, third(v)])));
                }
            }
            _ if path.starts_with("masks/") && path.ends_with("/path") => {
                let mut nv = v.clone();
                if let Some(inner) = nv.get_mut("v").and_then(Value::as_object_mut) {
                    if let Some(x) = inner.get("vertices") {
                        let m = map_points(x, pt);
                        inner.insert("vertices".into(), m);
                    }
                    for key in ["in_tangents", "out_tangents"] {
                        if let Some(x) = inner.get(key) {
                            let m = map_points(x, vec);
                            inner.insert(key.into(), m);
                        }
                    }
                    out.push((path.to_string(), nv));
                }
            }
            _ if path.starts_with("masks/") && path.ends_with("/feather") => {
                if let Some(f) = arr2(v) {
                    out.push((path.to_string(), json!(vec(f))));
                }
            }
            _ if path.starts_with("masks/") && path.ends_with("/expansion") => {
                if let Some(e) = v.as_f64().filter(|x| x.is_finite()) {
                    out.push((path.to_string(), json!(e / k)));
                }
            }
            _ => {}
        }
    }
    out
}

fn item_sizes(project: &str) -> Result<HashMap<u64, (f64, f64)>> {
    let v = run_cli_json(&["info", "--json", "--project", project])?;
    let mut m = HashMap::new();
    for it in v.get("project").and_then(|p| p.get("items")).and_then(Value::as_array).into_iter().flatten() {
        if let (Some(id), Some(sz)) = (it.get("id").and_then(Value::as_u64), it.get("size").and_then(arr2)) {
            m.insert(id, (sz[0], sz[1]));
        }
    }
    Ok(m)
}

/// Render template `t` with `values` (control name → text, or image path for media controls)
/// into `out_dir`; returns the movie (ProRes 4444 with alpha). Reuses an earlier identical render.
/// Runs EffectCraft's command-line tool several times; call it off the UI thread.
pub fn render_job(t: &TemplateInfo, values: &BTreeMap<String, String>, out_dir: &Path) -> Result<PathBuf> {
    for (k, v) in values {
        let c = t.control(k).ok_or_else(|| bad("graphics.ecTemplate", format!("the template has no control `{k}`")))?;
        if v.len() > MAX_TEXT {
            return Err(bad("graphics.ecTemplate", format!("`{k}` is too long")));
        }
        if c.kind == "media" && !Path::new(v).is_file() {
            return Err(bad("graphics.ecTemplate", format!("`{k}`: {v} is not a file")));
        }
    }
    if t.comp_name.is_empty() {
        return Err(other(format!("{} has no composition", t.path)));
    }
    let key = render_key(t, values);
    let out = out_dir.join(format!("{} {}.mov", safe_name(&t.name), &key[..12.min(key.len())]));
    if out.metadata().is_ok_and(|m| m.len() > 0) {
        return Ok(out);
    }
    let job = out_dir.join(".jobs").join(&key);
    std::fs::create_dir_all(&job).map_err(|e| other(format!("can't create {}: {e}", job.display())))?;
    let r = render_in(t, values, &job, &out);
    let _ = std::fs::remove_dir_all(&job);
    r.map(|_| out)
}

fn render_in(t: &TemplateInfo, values: &BTreeMap<String, String>, job: &Path, out: &Path) -> Result<()> {
    let project = job.join("job.ecproj");
    let ps = project.to_string_lossy().to_string();
    let create = json!({"path": t.path, "projectPath": ps}).to_string();
    run_cli(&["exec", "templates.create", "--params", &create, "--empty"])?;
    let media: Vec<(&Control, &String)> = values.iter().filter_map(|(k, v)| t.control(k).filter(|c| c.kind == "media").map(|c| (c, v))).collect();
    let before = if media.is_empty() { HashMap::new() } else { item_sizes(&ps)? };
    // values: one `run` with every change
    let mut steps: Vec<String> = vec!["run".into()];
    for (k, v) in values {
        let Some(c) = t.control(k) else { continue };
        if c.kind == "text" {
            steps.push("prop.set".into());
            steps.push(json!({"layer": c.layer, "path": c.path, "value": v}).to_string());
        } else {
            steps.push("file.replaceFootage".into());
            steps.push(json!({"path": v, "item": c.item}).to_string());
        }
    }
    if steps.len() > 1 {
        steps.extend(["--project".to_string(), ps.clone(), "--save".to_string()]);
        let a: Vec<&str> = steps.iter().map(String::as_str).collect();
        run_cli(&a)?;
    }
    // fit replaced images to the footprint of the originals
    if !media.is_empty() {
        let after = item_sizes(&ps)?;
        let mut fit: Vec<String> = vec!["run".into()];
        for (c, _) in &media {
            let (Some(old), Some(new)) = (before.get(&c.item), after.get(&c.item)) else { continue };
            if old == new {
                continue;
            }
            let props = run_cli_json(&["props", &t.comp_name, &c.layer.to_string(), "--flat", "--json", "--project", &ps])?;
            let list = props.get("properties").and_then(Value::as_array).cloned().unwrap_or_default();
            for (path, value) in fit_props(&list, *old, *new) {
                fit.push("prop.set".into());
                fit.push(json!({"layer": c.layer, "path": path, "value": value}).to_string());
            }
        }
        if fit.len() > 1 {
            fit.extend(["--project".to_string(), ps.clone(), "--save".to_string()]);
            let a: Vec<&str> = fit.iter().map(String::as_str).collect();
            run_cli(&a)?;
        }
    }
    let tmp = job.join("render.mov");
    let ts = tmp.to_string_lossy().to_string();
    run_cli(&[
        "render",
        "--comp",
        &t.comp_name,
        "--out",
        &ts,
        "--format",
        "prores",
        "--prores",
        "4444",
        "--channels",
        "rgba",
        "--work-area",
        "--audio",
        "on",
        "--project",
        &ps,
    ])?;
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).map_err(|e| other(format!("can't create {}: {e}", dir.display())))?;
    }
    std::fs::rename(&tmp, out).or_else(|_| std::fs::copy(&tmp, out).map(|_| ())).map_err(|e| other(format!("can't write {}: {e}", out.display())))
}

// ------------------------------------------------------------------------------------------------
// Commands
// ------------------------------------------------------------------------------------------------

fn values_p(cmd: &str, p: &Value) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    let Some(v) = p.get("values") else { return Ok(out) };
    let o = v.as_object().ok_or_else(|| bad(cmd, "`values` must be an object of control name → text or file path"))?;
    for (k, v) in o.iter().take(MAX_CONTROLS) {
        let s = v.as_str().ok_or_else(|| bad(cmd, format!("`{k}` must be a string")))?;
        out.insert(k.clone(), s.to_string());
    }
    Ok(out)
}

/// Control names in `values` normalised to the template's spelling.
fn normalise(t: &TemplateInfo, cmd: &str, values: BTreeMap<String, String>) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for (k, v) in values {
        let c = t.control(&k).ok_or_else(|| bad(cmd, format!("the template has no control `{k}`")))?;
        out.insert(c.name.clone(), v);
    }
    Ok(out)
}

/// A movie the caller already rendered (`render`), or a render now.
fn rendered(s: &Session, cmd: &str, p: &Value, t: &TemplateInfo, values: &BTreeMap<String, String>) -> Result<String> {
    if let Some(r) = str_p(p, "render") {
        if !Path::new(r).is_file() {
            return Err(bad(cmd, format!("{r} is not a file")));
        }
        return Ok(r.to_string());
    }
    Ok(render_job(t, values, &render_dir(s)?)?.to_string_lossy().to_string())
}

/// Keep only the first of the undo steps made since `before`, under `label`.
fn fold_undo(s: &mut Session, before: usize, label: &str) {
    if s.history.undo.len() > before + 1 {
        s.history.undo.truncate(before + 1);
    }
    if let Some(e) = s.history.undo.get_mut(before) {
        e.0 = label.to_string();
    }
}

/// The selected (or `clip`) timeline clip made from an EffectCraft template.
pub fn target(s: &Session, p: &Value) -> Option<(ClipId, ItemId, Link)> {
    let q = s.active_sequence()?;
    let link_of = |c: ClipId| {
        let (_, it) = q.find_item(c)?;
        let link = Link::from_metadata(&s.project.item(it.item)?.metadata)?;
        Some((c, it.item, link))
    };
    if let Some(c) = u64_p(p, "clip").map(ClipId) {
        return link_of(c);
    }
    s.state.selection.iter().find_map(|c| link_of(*c))
}

fn has_target(s: &Session) -> std::result::Result<(), String> {
    has_seq(s)?;
    target(s, &Value::Null).map(|_| ()).ok_or_else(|| "select a clip made from an EffectCraft template".into())
}

fn set_link(s: &mut Session, item: ItemId, link: &Link, label: &str) -> Result<()> {
    let j = serde_json::to_string(link).map_err(|e| other(e.to_string()))?;
    s.edit(label, |pr, _| {
        let it = pr.item_mut(item).ok_or_else(|| bad("graphics.ecTemplate", "no such item"))?;
        it.metadata.insert(METADATA_KEY.into(), j);
        Ok(())
    })
}

fn list(s: &mut Session, _: &Value) -> Result<Value> {
    Ok(Value::Array(library(s).iter().map(|t| info_json(t, None)).collect()))
}

fn install(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_p(p, "path").ok_or_else(|| bad("graphics.ecTemplate.install", "need `path`"))?;
    let info = read_info(path)?;
    let dir = templates_dir(s).ok_or_else(|| other("no data directory to install templates into"))?;
    std::fs::create_dir_all(&dir).map_err(|e| other(format!("can't create {}: {e}", dir.display())))?;
    let name = Path::new(path).file_name().ok_or_else(|| bad("graphics.ecTemplate.install", "no file name"))?;
    let dest = dir.join(name);
    if dest != Path::new(path) {
        std::fs::copy(path, &dest).map_err(|e| other(format!("can't copy to {}: {e}", dest.display())))?;
    }
    Ok(json!({"path": dest.to_string_lossy(), "name": info.name}))
}

fn apply(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "graphics.ecTemplate.apply";
    let key = str_p(p, "template").ok_or_else(|| bad(CMD, "need `template` (a path or the name of an installed template)"))?;
    let t = find(s, key)?;
    let values = normalise(&t, CMD, values_p(CMD, p)?)?;
    let movie = rendered(s, CMD, p, &t, &values)?;
    let before = s.history.undo.len();
    let mut q = json!({"path": movie});
    for k in ["time", "seconds", "frame", "timecode", "track"] {
        if let Some(v) = p.get(k) {
            q[k] = v.clone();
        }
    }
    let r = s.execute("graphics.newFromFile", q)?;
    let item = u64_p(&r, "item").map(ItemId).ok_or_else(|| other("the render was not placed"))?;
    let clip = u64_p(&r, "clip");
    let link = Link { template: t.path.clone(), values: values.clone() };
    set_link(s, item, &link, "Apply EffectCraft Template")?;
    let name = t.name.clone();
    let _ = s.edit("Apply EffectCraft Template", |pr, _| {
        if let Some(it) = pr.item_mut(item) {
            it.name = name;
        }
        Ok(())
    });
    fold_undo(s, before, "Apply EffectCraft Template");
    Ok(json!({"clip": clip, "item": item.0, "render": movie, "template": info_json(&t, Some(&values))}))
}

fn controls(s: &mut Session, p: &Value) -> Result<Value> {
    let Some((clip, item, link)) = target(s, p) else {
        return Ok(json!({"clip": Value::Null, "template": Value::Null}));
    };
    let t = read_info(&link.template)?;
    Ok(json!({"clip": clip.0, "item": item.0, "template": info_json(&t, Some(&link.values))}))
}

fn set(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "graphics.ecTemplate.set";
    let (clip, item, mut link) = target(s, p).ok_or_else(|| bad(CMD, "select a clip made from an EffectCraft template"))?;
    let t = read_info(&link.template)?;
    for (k, v) in normalise(&t, CMD, values_p(CMD, p)?)? {
        if v.is_empty() && t.control(&k).is_some_and(|c| c.kind == "media") {
            link.values.remove(&k); // back to the template's own media
        } else {
            link.values.insert(k, v);
        }
    }
    let movie = rendered(s, CMD, p, &t, &link.values)?;
    let before = s.history.undo.len();
    s.execute("media.relink", json!({"item": item.0, "path": movie, "force": true, "relinkOthers": false}))?;
    set_link(s, item, &link, "Change EffectCraft Template")?;
    fold_undo(s, before, "Change EffectCraft Template");
    Ok(json!({"clip": clip.0, "item": item.0, "render": movie, "template": info_json(&t, Some(&link.values))}))
}

pub fn commands() -> Vec<CommandSpec> {
    let q = |id, label, params, run| CommandSpec { id, label, menu: &[], shortcut: None, params, enabled: always, run, journal: false };
    vec![
        q("graphics.ecTemplate.list", "EffectCraft Templates", "{} → installed .ectemplate templates and their controls", list),
        CommandSpec {
            id: "graphics.ecTemplate.install",
            label: "Install EffectCraft Template…",
            menu: &[],
            shortcut: None,
            params: r#"{"path":str} (an .ectemplate file; copied into the Graphics Templates folder)"#,
            enabled: always,
            run: install,
            journal: true,
        },
        CommandSpec {
            id: "graphics.ecTemplate.apply",
            label: "Apply EffectCraft Template",
            menu: &[],
            shortcut: None,
            params: r#"{"template":path|name,"values":{control name: text | image path}?,"time":ticks?,"track":index?,"render":path? (a movie made by render_job)} (renders with EffectCraft and places the movie at the playhead)"#,
            enabled: has_seq,
            run: apply,
            journal: true,
        },
        q(
            "graphics.ecTemplate.controls",
            "EffectCraft Template Controls",
            r#"{"clip":id?} → the template and current values of the selected template clip"#,
            controls,
        ),
        CommandSpec {
            id: "graphics.ecTemplate.set",
            label: "Change EffectCraft Template",
            menu: &[],
            shortcut: None,
            params: r#"{"values":{control name: text or image path, empty for the template image},"clip":id?,"render":path?}"#,
            enabled: has_target,
            run: set,
            journal: true,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn circle(c: f64, r: f64) -> Value {
        let t = r * 0.5523;
        json!({"t": "Path", "v": {"closed": true,
            "in_tangents": [[-t, 0.0], [0.0, -t], [t, 0.0], [0.0, t]],
            "out_tangents": [[t, 0.0], [0.0, t], [-t, 0.0], [0.0, -t]],
            "vertices": [[c, c - r], [c + r, c], [c, c + r], [c - r, c]]}})
    }

    #[test]
    fn fit_keeps_the_footprint_and_centres_a_wide_logo() {
        let props = vec![
            json!({"path": "masks/mask/path", "type": "path", "value": circle(120.0, 120.0)}),
            json!({"path": "masks/mask/feather", "type": "vec2", "value": [10.0, 10.0]}),
            json!({"path": "transform/anchor", "type": "vec3", "value": [120.0, 120.0, 0.0]}),
            json!({"path": "transform/scale", "type": "vec3", "value": [67.5, 67.5, 0.0]}),
            json!({"path": "transform/position", "type": "vec3", "value": [-417.0, 51.0, 0.0]}),
        ];
        let out: HashMap<String, Value> = fit_props(&props, (240.0, 240.0), (800.0, 500.0)).into_iter().collect();
        // k = max(240/800, 240/500) = 0.48
        assert_eq!(out["transform/anchor"], json!([400.0, 250.0, 0.0]));
        let s = arr2(&out["transform/scale"]).unwrap();
        assert!((s[0] - 32.4).abs() < 1e-9 && (s[1] - 32.4).abs() < 1e-9);
        let v = &out["masks/mask/path"]["v"]["vertices"];
        assert_eq!(arr2(&v[0]).unwrap(), [400.0, 0.0]);
        assert_eq!(arr2(&v[1]).unwrap(), [650.0, 250.0]);
        let f = arr2(&out["masks/mask/feather"]).unwrap();
        assert!((f[0] - 10.0 / 0.48).abs() < 1e-9);
        assert!(!out.contains_key("transform/position"), "position is untouched");
        // a point maps to the same screen place: (p - a) * s is unchanged
        let (p_old, a_old, s_old) = (240.0, 120.0, 67.5);
        let p_new = arr2(&v[1]).unwrap()[0];
        assert!(((p_old - a_old) * s_old - (p_new - 400.0) * 32.4).abs() < 1e-9);
    }

    #[test]
    fn fit_refuses_hostile_sizes_and_skips_animated_properties() {
        let props = vec![json!({"path": "transform/scale", "value": [100.0, 100.0, 100.0], "keys": 2})];
        assert!(fit_props(&props, (240.0, 240.0), (100.0, 100.0)).is_empty());
        let props = vec![json!({"path": "transform/scale", "value": [100.0, 100.0, 100.0]})];
        for (o, n) in [((0.0, 240.0), (100.0, 100.0)), ((240.0, 240.0), (0.0, 1.0)), ((f64::NAN, 1.0), (1.0, 1.0)), ((1.0, 1.0), (-5.0, 1.0))] {
            assert!(fit_props(&props, o, n).is_empty());
        }
        // garbage values are skipped, not panicked on
        let junk = vec![json!({"path": "transform/anchor", "value": "x"}), json!({"path": "masks/m/path", "value": {"v": {"vertices": [1, [2]]}}}), json!(7)];
        let _ = fit_props(&junk, (10.0, 10.0), (20.0, 20.0));
    }

    #[test]
    fn manifest_parsing_keeps_text_and_media_controls() {
        let v = json!({"format": "effectcraft-template", "name": "CTA", "compName": "YouTube CTA", "width": 1920, "height": 608,
            "frameRate": 60.0, "duration": 3.7, "controls": [
                {"name": "Channel Name", "type": "text", "layer": 167, "path": "text/sourceText", "value": "Zak"},
                {"name": "Logo", "type": "media", "layer": 139, "item": 1},
                {"name": "Size", "type": "slider", "layer": 1},
                {"type": "text"}]});
        let t = parse_info("/x/CTA.ectemplate", &v).unwrap();
        assert_eq!(t.controls.len(), 2);
        assert_eq!(t.control("channel name").unwrap().default, "Zak");
        assert_eq!(t.control("Logo").unwrap().item, 1);
        let vals = t.effective_values(&BTreeMap::new());
        assert_eq!(vals.get("Channel Name").map(String::as_str), Some("Zak"));
        assert!(!vals.contains_key("Logo"));
        assert!(parse_info("/x/a.ectemplate", &json!({"format": "other"})).is_err());
        // hostile numbers
        let t = parse_info("/x/a.ectemplate", &json!({"width": -5, "height": 1e300, "frameRate": "x", "controls": "no"})).unwrap();
        assert_eq!((t.width, t.height, t.controls.len()), (0, 16384, 0));
    }

    #[test]
    fn link_round_trips_through_metadata() {
        let mut md = BTreeMap::new();
        assert!(Link::from_metadata(&md).is_none());
        md.insert(METADATA_KEY.to_string(), "not json".into());
        assert!(Link::from_metadata(&md).is_none());
        let l = Link { template: "/a.ectemplate".into(), values: [("Name".to_string(), "Z".to_string())].into() };
        md.insert(METADATA_KEY.to_string(), serde_json::to_string(&l).unwrap());
        assert_eq!(Link::from_metadata(&md), Some(l));
    }

    #[test]
    fn render_keys_change_with_values_and_names_are_safe() {
        let t = TemplateInfo { path: "/nonexistent/a.ectemplate".into(), name: "A".into(), ..Default::default() };
        let a = render_key(&t, &[("N".to_string(), "1".to_string())].into());
        let b = render_key(&t, &[("N".to_string(), "2".to_string())].into());
        assert_ne!(a, b);
        assert_eq!(safe_name("a/b:c"), "a_b_c");
        assert_eq!(safe_name("  "), "EffectCraft Template");
    }

    #[test]
    fn commands_refuse_bad_parameters() {
        let mut s = Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        assert!(s.execute("graphics.ecTemplate.install", json!({})).is_err());
        assert!(s.execute("graphics.ecTemplate.install", json!({"path": "/no/such.fcgt"})).is_err());
        assert!(s.execute("graphics.ecTemplate.controls", json!({"clip": 99})).is_ok());
        assert!(s.execute("graphics.ecTemplate.set", json!({"values": {"a": "b"}})).is_err());
        assert!(values_p("x", &json!({"values": {"a": 3}})).is_err());
        assert!(values_p("x", &json!({"values": [1]})).is_err());
    }
}
