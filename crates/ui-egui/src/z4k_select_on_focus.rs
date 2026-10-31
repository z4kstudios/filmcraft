//! Z4K: clicking into a single-line text or number field selects its whole value, so typing
//! replaces it (like Premiere and macOS fields), in every panel. The selection is made when the
//! click is released, so dragging across the text still selects just that part; multi-line
//! editors keep the caret where they were clicked.

use egui::text::{CCursor, CCursorRange};
use egui::text_edit::TextEditState;
use egui::{Context, Id};

/// Taller than this is a multi-line editor (notes, caption text): left alone.
const SINGLE_LINE_MAX_H: f32 = 40.0;

fn last_focus_id() -> Id {
    Id::unique("z4k-select-on-focus.last")
}
fn pending_id() -> Id {
    Id::unique("z4k-select-on-focus.pending")
}

/// Call once per frame, after the panels have run.
pub fn after_frame(ctx: &Context) {
    let focused = ctx.memory(|m| m.focused());
    let last = ctx.data(|d| d.get_temp::<Option<Id>>(last_focus_id())).flatten();
    ctx.data_mut(|d| d.insert_temp(last_focus_id(), focused));
    let Some(id) = focused else {
        ctx.data_mut(|d| d.remove::<Id>(pending_id()));
        return;
    };
    if last != Some(id) {
        // focus just moved here: only text fields have a TextEditState
        let single_line = ctx.read_response(id).is_some_and(|r| r.rect.height() <= SINGLE_LINE_MAX_H);
        if TextEditState::load(ctx, id).is_some() && single_line {
            ctx.data_mut(|d| d.insert_temp(pending_id(), id));
        } else {
            ctx.data_mut(|d| d.remove::<Id>(pending_id()));
        }
    }
    if ctx.data(|d| d.get_temp::<Id>(pending_id())) != Some(id) || ctx.input(|i| i.pointer.any_down()) {
        return;
    }
    ctx.data_mut(|d| d.remove::<Id>(pending_id()));
    let Some(mut st) = TextEditState::load(ctx, id) else { return };
    // a range the user dragged out themselves stays
    if st.cursor.char_range().is_some_and(|r| r.primary != r.secondary) {
        return;
    }
    // past-the-end cursors are clamped to the text by the field
    st.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(0), CCursor::new(usize::MAX))));
    st.store(ctx, id);
    ctx.request_repaint();
}
