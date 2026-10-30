//! Z4K: File ▸ Import… (Cmd+I) on macOS: an open panel that takes files *and* folders, as
//! Premiere's Import does. A chosen folder imports as a bin (see the engine's `z4k_folder_import`).

use objc2::MainThreadMarker;
use objc2_app_kit::{NSModalResponseOK, NSOpenPanel};
use objc2_foundation::{NSArray, NSString};

/// Show the panel; the chosen paths, or none when cancelled (or off the main thread).
pub fn pick(exts: &[&str]) -> Vec<String> {
    let Some(mtm) = MainThreadMarker::new() else {
        return Vec::new();
    };
    let panel = NSOpenPanel::openPanel(mtm);
    panel.setCanChooseFiles(true);
    panel.setCanChooseDirectories(true);
    panel.setAllowsMultipleSelection(true);
    panel.setPrompt(Some(&NSString::from_str("Import")));
    // "public.folder" keeps folders selectable: with only file extensions listed, newer macOS greys them out
    let types: Vec<_> = exts.iter().copied().chain(["public.folder"]).map(NSString::from_str).collect();
    // allowedContentTypes needs UTTypes (another crate); the extension list does the same here
    #[allow(deprecated)]
    panel.setAllowedFileTypes(Some(&NSArray::from_retained_slice(&types)));
    if panel.runModal() != NSModalResponseOK {
        return Vec::new();
    }
    panel.URLs().iter().filter_map(|u| u.path()).map(|p| p.to_string()).collect()
}
