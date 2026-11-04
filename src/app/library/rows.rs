//! The rows of the Library's Metadata panel, as Lightroom's: a caption
//! column, then a value or a text field, at a fixed height so the panel never
//! widens or reflows.
use crate::app::ui::design::{self, Tone, size, space, text};
use crate::app::ui::theme;
use eframe::egui::{self, Color32};

/// The height of a row.
pub(super) const ROW: f32 = size::ROW;
/// The caption's right edge, from the row's left.
const CAPTION_RIGHT: f32 = 84.;
/// A value's left edge, from the row's left.
const VALUE_LEFT: f32 = 92.;
/// A text field's left edge, from the row's left; its text lines up with
/// the values.
const FIELD_LEFT: f32 = 88.;
/// Text size of captions, values and fields.
const TEXT_SIZE: f32 = text::SMALL;
const CAPTION_GRAY: Tone = Tone::MUTED;
pub(super) const VALUE_GRAY: Tone = Tone::SOFT;
/// A value not set, shown as a dash.
const UNSET_GRAY: Tone = Tone::OUTLINE_STRONG;

/// The font of captions, values and fields.
pub(super) fn font() -> egui::FontId {
    egui::FontId::proportional(TEXT_SIZE)
}
/// The caption column of a row, beside its first line.
pub(super) fn caption_at(ui: &egui::Ui, rect: egui::Rect, key: &str) {
    ui.painter().text(
        egui::pos2(rect.left() + CAPTION_RIGHT, rect.top() + ROW / 2.),
        egui::Align2::RIGHT_CENTER,
        key,
        font(),
        theme::palette(ui.ctx()).tone(CAPTION_GRAY),
    );
}
/// A row's value, truncated to fit; dimmed when it is not `set`.
pub(super) fn value_at(ui: &egui::Ui, rect: egui::Rect, text: &str, set: bool) {
    let left = rect.left() + VALUE_LEFT;
    paint_truncated(
        ui,
        egui::pos2(left, rect.center().y),
        rect.right() - left,
        text,
        theme::palette(ui.ctx()).tone(if set { VALUE_GRAY } else { UNSET_GRAY }),
    );
}
/// The text field area of a row, right of its caption.
pub(super) fn field_rect(rect: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_max(
        egui::pos2(rect.left() + FIELD_LEFT, rect.top() + 1.),
        egui::pos2(rect.right(), rect.bottom() - 1.),
    )
}
/// A text field styled as the panel's values.
pub(super) fn panel_edit<'t>(
    palette: &theme::Palette,
    edit: egui::TextEdit<'t>,
) -> egui::TextEdit<'t> {
    edit.font(font())
        .text_color(palette.tone(VALUE_GRAY))
        .margin(design::margin(space::XS, space::HAIR))
}
/// One line of `text` from `left_center`, cut short to `width`.
pub(super) fn paint_truncated(
    ui: &egui::Ui,
    left_center: egui::Pos2,
    width: f32,
    text: &str,
    color: Color32,
) {
    let galley = egui::WidgetText::from(text).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        width.max(1.),
        font(),
    );
    ui.painter().galley(
        egui::pos2(left_center.x, left_center.y - galley.size().y / 2.),
        galley,
        color,
    );
}
