//! Z4K: `file.import` takes folders. A folder dropped from Finder or chosen in Import… (Cmd+I)
//! becomes a bin named after it, each sub-folder a bin inside that, as `mediaBrowser.import`
//! (and Premiere's Import) does. Plain file lists are left to `file.import` itself.

use serde_json::{Value, json};

use crate::{Result, Session};

/// True when `path` is a folder (and not a media "file" the host lists as a directory).
fn is_folder(s: &Session, path: &str) -> bool {
    matches!(s.services.list_entries(path), Some(Ok(_))) && crate::media_browser::kind_of(path).is_none()
}

/// When `paths` holds a folder, import everything through `mediaBrowser.import` and return its
/// result; `None` lets `file.import` carry on.
pub fn route(s: &mut Session, paths: &[String], bin: Option<u64>, image_sequence: bool) -> Option<Result<Value>> {
    if image_sequence || !paths.iter().any(|p| is_folder(s, p)) {
        return None;
    }
    let mut q = json!({"paths": paths, "imageSequence": false});
    if let Some(b) = bin {
        q["bin"] = json!(b);
    }
    Some(s.execute("mediaBrowser.import", q))
}
