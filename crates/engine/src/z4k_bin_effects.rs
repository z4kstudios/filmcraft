//! Z4K: Lumetri Color on an adjustment layer in the Project panel, before it is on a timeline.
//! The effect lives in `Project::z4k_bin_effects`; every clip placed from the item starts with a
//! copy (`Project::make_track_item`), so later bin edits leave clips already placed as they are.

use filmcraft_project::{EffectInstance, ItemId, ItemKind, Param, ParamValue, Project};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, json_to_param};
use crate::{Result, Session};

fn bad(cmd: &str, m: impl Into<String>) -> crate::EngineError {
    crate::commands::bad(cmd, m)
}

fn item_p(s: &Session, p: &Value, cmd: &str) -> Result<ItemId> {
    let id = ItemId(p.get("item").and_then(Value::as_u64).ok_or_else(|| bad(cmd, "need `item`"))?);
    match s.project.item(id).map(|i| &i.kind) {
        Some(ItemKind::AdjustmentLayer { .. }) => Ok(id),
        _ => Err(bad(cmd, "`item` must be an adjustment layer")),
    }
}

/// The item's Lumetri, added when it has none yet.
fn lumetri<'a>(pr: &'a mut Project, item: ItemId, cmd: &str) -> Result<&'a mut EffectInstance> {
    let stack = pr.z4k_bin_effects.entry(item).or_default();
    if !stack.iter().any(|e| e.effect == "lumetri") {
        stack.push(filmcraft_project::find_effect("lumetri").ok_or_else(|| bad(cmd, "no Lumetri"))?.instance());
    }
    stack.iter_mut().find(|e| e.effect == "lumetri").ok_or_else(|| bad(cmd, "no Lumetri"))
}

fn apply(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "z4k.binLumetri.apply";
    let item = item_p(s, p, CMD)?;
    s.edit("Add Lumetri Color", |pr, _| lumetri(pr, item, CMD).map(|_| ()))?;
    Ok(json!({"item": item.0}))
}

fn remove(s: &mut Session, p: &Value) -> Result<Value> {
    let item = item_p(s, p, "z4k.binLumetri.remove")?;
    s.edit("Remove Lumetri Color", |pr, _| {
        pr.z4k_bin_effects.remove(&item);
        Ok(())
    })?;
    Ok(Value::Null)
}

fn toggle(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "z4k.binLumetri.toggle";
    let item = item_p(s, p, CMD)?;
    s.edit("Toggle Effect", |pr, _| {
        let e = lumetri(pr, item, CMD)?;
        e.enabled = !e.enabled;
        Ok(())
    })?;
    Ok(Value::Null)
}

fn set_param(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "z4k.binLumetri.setParam";
    let item = item_p(s, p, CMD)?;
    let pid = p.get("param").and_then(Value::as_str).ok_or_else(|| bad(CMD, "need `param`"))?.to_string();
    let val = p.get("value").cloned().ok_or_else(|| bad(CMD, "need `value`"))?;
    // a drag is one undo step, as `effects.setParam`
    if p.get("begin").and_then(Value::as_bool).unwrap_or(false) {
        s.history.merge_key = None;
    }
    let body = |pr: &mut Project, _: &mut crate::EditorState| {
        let e = lumetri(pr, item, CMD)?;
        let def = e.def().and_then(|d| d.param(&pid)).ok_or_else(|| bad(CMD, format!("no param `{pid}`")))?;
        let kind = def.kind.clone();
        let prm = e.params.entry(pid.clone()).or_insert_with(|| Param::new(def.default.clone()));
        let v = json_to_param(&prm.value, &val).ok_or_else(|| bad(CMD, "value has the wrong type"))?;
        if let (filmcraft_project::ParamKind::Float { min, max, .. }, ParamValue::Float(x)) = (&kind, &v)
            && !(*min..=*max).contains(x)
        {
            return Err(bad(CMD, format!("`{pid}` must be between {min} and {max}, got {x}")));
        }
        prm.value = v;
        Ok(())
    };
    if p.get("merge").and_then(Value::as_bool).unwrap_or(false) {
        s.edit_merged("Change Effect Parameter", &format!("z4kBin:{}:{pid}", item.0), body)?;
    } else {
        s.edit("Change Effect Parameter", body)?;
    }
    Ok(Value::Null)
}

fn set_lut(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "z4k.binLumetri.setLut";
    let item = item_p(s, p, CMD)?;
    let input = p.get("input").and_then(Value::as_bool).unwrap_or(true);
    let r = crate::color::lut_reference(s, p, CMD)?;
    s.edit(if input { "Input LUT" } else { "Creative Look" }, |pr, _| {
        let param = if input { "input_lut" } else { "look_lut" };
        let e = lumetri(pr, item, CMD)?;
        let v = ParamValue::Text(r.clone());
        e.params.entry(param.to_string()).or_insert_with(|| Param::new(v.clone())).value = v.clone();
        Ok(())
    })?;
    Ok(json!({"item": item.0, "lut": r}))
}

fn set_section(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "z4k.binLumetri.setSection";
    let item = item_p(s, p, CMD)?;
    let sec = p.get("section").and_then(Value::as_str).ok_or_else(|| bad(CMD, "need `section`"))?;
    let param = crate::color::section_param(sec).ok_or_else(|| bad(CMD, format!("unknown section `{sec}`")))?;
    let on = p.get("on").and_then(Value::as_bool);
    s.edit("Lumetri Section", |pr, _| {
        let e = lumetri(pr, item, CMD)?;
        let cur = e.param(param).and_then(|v| v.value.as_bool()).unwrap_or(param != "hsl_on");
        let v = ParamValue::Bool(on.unwrap_or(!cur));
        e.params.entry(param.to_string()).or_insert_with(|| Param::new(v.clone())).value = v.clone();
        Ok(())
    })?;
    Ok(Value::Null)
}

pub fn commands() -> Vec<CommandSpec> {
    let c = |id, label, params, run| CommandSpec { id, label, menu: &[], shortcut: None, params, enabled: always, run, journal: true, named: crate::commands::NamedKeys::NONE };
    vec![
        c("z4k.binLumetri.apply", "Add Lumetri to Bin Item", r#"{"item":id}"#, apply),
        c("z4k.binLumetri.remove", "Remove Lumetri from Bin Item", r#"{"item":id}"#, remove),
        c("z4k.binLumetri.toggle", "Toggle Lumetri on Bin Item", r#"{"item":id}"#, toggle),
        c("z4k.binLumetri.setParam", "Set Lumetri on Bin Item", r#"{"item":id,"param":str,"value":any}"#, set_param),
        c("z4k.binLumetri.setLut", "Set LUT on Bin Item", r#"{"item":id,"input":bool=true,"lut":str?,"path":str?}"#, set_lut),
        c("z4k.binLumetri.setSection", "Toggle Lumetri Section on Bin Item", r#"{"item":id,"section":str,"on":bool?}"#, set_section),
    ]
}
