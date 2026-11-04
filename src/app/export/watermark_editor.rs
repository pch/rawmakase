//! Lightroom's Watermark Editor, as a sheet over the Export dialog: the open
//! photo with the mark on it, and the mark's style, text or image options,
//! shadow and effects. Watermarks are kept as presets, a graphic one with
//! its own copy of the image.
use super::Editor;
use crate::app::ui::design::{Tone, button, radius, size, space, text};
use crate::app::ui::theme;
use crate::app::ui::widgets::{modal_frame, primary_button};
use crate::rendered::unit_to_u8;
use crate::watermark::{self, Align, Anchor, Size, Style, Watermark};
use eframe::egui::{self, Color32, Sense, Vec2};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

const WIDTH: f32 = 920.;
const HEIGHT: f32 = 620.;
const PANEL: f32 = 300.;

/// The editor's state while it is open.
pub(super) struct WatermarkEditor {
    pub(super) watermark: Watermark,
    /// The saved preset's name when one is being edited.
    original: Option<String>,
    /// An image chosen for a graphic watermark, copied when it is saved.
    source: Option<PathBuf>,
    /// The chooser's answer, from its own thread.
    picked: Arc<Mutex<Option<PathBuf>>>,
    /// The mark as last drawn for the preview, and what it was drawn from.
    preview: Option<(String, Option<(egui::TextureHandle, egui::Rect)>)>,
    /// The image or font the preview draws with, kept while only the
    /// settings change, and what it was loaded for.
    loaded: Option<(String, watermark::Ready)>,
    notice: Notice,
}
/// What the editor says beside the name field.
#[derive(Default)]
enum Notice {
    #[default]
    None,
    /// Installed fonts are still being listed; the preview waits for them.
    LoadingFonts,
    Error(String),
}
impl Notice {
    fn text(&self) -> &str {
        match self {
            Self::None => "",
            Self::LoadingFonts => "Loading fonts…",
            Self::Error(e) => e,
        }
    }
}
impl WatermarkEditor {
    pub(super) fn new(watermark: Watermark, ctx: &egui::Context) -> Self {
        // Installed fonts are listed off the interface thread, which is woken to
        // draw them once they are.
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = watermark::fonts::families();
            ctx.request_repaint();
        });
        let original = (!watermark.name.is_empty()).then(|| watermark.name.clone());
        Self {
            watermark,
            original,
            source: None,
            picked: Default::default(),
            preview: None,
            loaded: None,
            notice: Notice::None,
        }
    }
}

impl Editor {
    pub(super) fn watermark_editor(&mut self, ctx: &egui::Context) {
        let palette = theme::palette(ctx);
        let Some(mut state) = self.exports.watermark_editor.take() else {
            return;
        };
        if let Some(path) = state.picked.lock().ok().and_then(|mut p| p.take()) {
            state.source = Some(path);
            state.watermark.style = Style::Graphic;
            state.preview = None;
        }
        let photo = self
            .preview
            .texture
            .as_ref()
            .map(|p| (p.id(), p.size_vec2()));
        let mut close = None;
        let mut delete = false;
        let response = egui::Modal::new(egui::Id::new("watermark-editor"))
            .backdrop_color(Color32::from_black_alpha(140))
            .frame(modal_frame(&palette))
            .show(ctx, |ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::new(WIDTH, HEIGHT), Sense::hover());
                ui.painter().text(
                    rect.left_top() + Vec2::new(24., 26.),
                    egui::Align2::LEFT_CENTER,
                    "Watermark Editor",
                    egui::FontId::proportional(text::TITLE),
                    palette.tone(Tone::STRONG),
                );
                let preview = egui::Rect::from_min_max(
                    rect.left_top() + Vec2::new(24., 52.),
                    egui::pos2(rect.right() - PANEL - 40., rect.bottom() - 72.),
                );
                draw_preview(ui, &mut state, preview, photo);
                let panel = egui::Rect::from_min_max(
                    egui::pos2(rect.right() - PANEL - 24., rect.top() + 52.),
                    rect.right_bottom() - Vec2::new(24., 72.),
                );
                let mut content = ui.new_child(egui::UiBuilder::new().max_rect(panel));
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(&mut content, |ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(space::M, space::S);
                        controls(ui, &mut state, ctx);
                    });
                let footer = egui::Rect::from_min_max(
                    egui::pos2(rect.left() + 24., rect.bottom() - 56.),
                    egui::pos2(rect.right() - 24., rect.bottom() - 12.),
                );
                let mut bar = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(footer)
                        .layout(egui::Layout::right_to_left(egui::Align::Center)),
                );
                bar.spacing_mut().button_padding = button::DIALOG;
                if primary_button(&mut bar, "Save").clicked() {
                    close = Some(true);
                }
                if bar
                    .add(egui::Button::new("Cancel").min_size(Vec2::new(84., 30.)))
                    .clicked()
                {
                    close = Some(false);
                }
                bar.add_space(space::L);
                bar.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    if state.original.is_some()
                        && ui
                            .add(egui::Button::new("Delete").frame(false))
                            .on_hover_text("Delete this watermark and its image")
                            .clicked()
                    {
                        close = Some(false);
                        delete = true;
                    }
                    ui.label(egui::RichText::new("Name").color(palette.tone(Tone::SECONDARY)));
                    ui.add(
                        egui::TextEdit::singleline(&mut state.watermark.name)
                            .desired_width(200.)
                            .hint_text("Watermark name"),
                    );
                    let notice = state.notice.text();
                    if !notice.is_empty() {
                        ui.label(
                            egui::RichText::new(notice)
                                .size(text::LABEL)
                                .color(Color32::from_rgb(230, 120, 100)),
                        );
                    }
                });
            });
        match close.or(response.should_close().then_some(false)) {
            Some(true) => {
                let graphic = state.watermark.style == Style::Graphic;
                let source = state.source.clone().filter(|_| graphic);
                if graphic && source.is_none() && state.watermark.image.is_none() {
                    state.notice = Notice::Error(format!(
                        "Choose a {} image",
                        watermark::image_formats_named()
                    ));
                    self.exports.watermark_editor = Some(state);
                    return;
                }
                let saved = watermark::save(
                    &state.watermark,
                    state.original.as_deref(),
                    source.as_deref(),
                );
                match saved {
                    Ok(saved) => {
                        // Export with Previous follows a renamed watermark.
                        if let Some(old) = &state.original
                            && *old != saved.name
                        {
                            self.update_previous(old, |previous| {
                                previous.watermark_name = saved.name.clone();
                            });
                        }
                        self.exports.draft.watermark = true;
                        self.exports.draft.watermark_name = saved.name;
                        self.exports.watermarks = watermark::presets();
                    }
                    Err(e) => {
                        state.notice = Notice::Error(format!("{e:#}"));
                        self.exports.watermark_editor = Some(state);
                    }
                }
            }
            Some(false) if delete => {
                let original = state.original.clone().unwrap_or_default();
                let saved = self
                    .exports
                    .watermarks
                    .iter()
                    .find(|w| w.name == original)
                    .cloned();
                if let Some(w) = saved
                    && let Err(e) = watermark::delete(&w)
                {
                    // Nothing else changes; the editor stays open to say so.
                    state.notice = Notice::Error(format!("Not deleted: {e:#}"));
                    self.exports.watermark_editor = Some(state);
                    return;
                }
                // A deleted watermark is never swapped for another.
                if self.exports.draft.watermark_name == original {
                    self.exports.draft.watermark = false;
                    self.exports.draft.watermark_name = watermark::SIMPLE_COPYRIGHT.into();
                }
                // Export with Previous neither uses it.
                self.update_previous(&original, |previous| {
                    previous.watermark = false;
                    previous.watermark_name = watermark::SIMPLE_COPYRIGHT.into();
                });
                self.exports.watermarks = watermark::presets();
            }
            Some(false) => {}
            None => self.exports.watermark_editor = Some(state),
        }
    }
    /// Changes the settings Export with Previous uses, when they use the
    /// watermark `name`.
    fn update_previous(
        &mut self,
        name: &str,
        change: impl FnOnce(&mut crate::export_settings::ExportSettings),
    ) {
        if let Some(mut previous) = crate::export_settings::ExportSettings::load()
            && previous.watermark_name == name
        {
            change(&mut previous);
            if let Err(e) = previous.save() {
                self.messages
                    .fail(format!("Export settings not saved: {e:#}"));
            }
        }
    }
}

/// The open photo with the mark where an export puts it.
fn draw_preview(
    ui: &mut egui::Ui,
    state: &mut WatermarkEditor,
    area: egui::Rect,
    photo: Option<(egui::TextureId, Vec2)>,
) {
    let palette = theme::palette(ui.ctx());
    ui.painter()
        .rect_filled(area, radius::SMALL, palette.tone(Tone::WINDOW));
    let Some((texture, size)) = photo.filter(|(_, s)| s.x > 0. && s.y > 0.) else {
        ui.painter().text(
            area.center(),
            egui::Align2::CENTER_CENTER,
            "Open a photo to preview the watermark",
            egui::FontId::proportional(text::LABEL),
            palette.tone(Tone::FAINT),
        );
        return;
    };
    let scale = (area.width() / size.x).min(area.height() / size.y);
    let shown = egui::Rect::from_center_size(area.center(), size * scale);
    ui.painter().image(
        texture,
        shown,
        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
        Color32::WHITE,
    );
    // Drawn again only when the watermark or the preview's size changed.
    let key = format!(
        "{}|{:?}|{}x{}",
        serde_json::to_string(&state.watermark).unwrap_or_default(),
        state.source,
        shown.width().round(),
        shown.height().round()
    );
    if state.preview.as_ref().is_none_or(|(k, _)| *k != key) {
        let mark = mark_texture(ui.ctx(), state, shown);
        // Tried again next frame while fonts are still being listed.
        let loading = mark.is_none() && matches!(state.notice, Notice::LoadingFonts);
        state.preview = (!loading).then_some((key, mark));
    }
    if let Some((_, Some((texture, rect)))) = &state.preview {
        // Cut at the photo's edges, as the export is.
        ui.painter().with_clip_rect(shown).image(
            texture.id(),
            rect.translate(shown.min.to_vec2()),
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
            Color32::WHITE,
        );
    }
}

/// The mark for a photo shown at `shown`'s size, as a texture and where it
/// goes relative to the photo's corner.
fn mark_texture(
    ctx: &egui::Context,
    state: &mut WatermarkEditor,
    shown: egui::Rect,
) -> Option<(egui::TextureHandle, egui::Rect)> {
    let mut w = state.watermark.clone();
    let assets = format!(
        "{:?}|{:?}|{:?}|{}|{}",
        w.style, state.source, w.image, w.family, w.face
    );
    let ready = match &state.loaded {
        Some((key, ready)) if *key == assets => ready.with(&w),
        // An installed font is found once the fonts are listed, off this
        // thread; drawn after that.
        _ if w.style == Style::Text
            && w.family != watermark::fonts::INTER
            && watermark::fonts::families_if_listed().is_none() =>
        {
            state.notice = Notice::LoadingFonts;
            return None;
        }
        _ => {
            // An image not saved yet is previewed from where it was chosen.
            let loaded = match (&state.source, w.style) {
                (Some(source), Style::Graphic) => {
                    let dir = source.parent()?.to_path_buf();
                    w.image = source.file_name().map(|n| n.to_string_lossy().into_owned());
                    w.ready_in(&dir)
                }
                _ => w.ready(),
            };
            match loaded {
                Ok(r) => {
                    state.loaded = Some((assets, r.clone()));
                    r
                }
                Err(e) => {
                    state.notice = Notice::Error(format!("{e:#}"));
                    return None;
                }
            }
        }
    };
    state.notice = Notice::None;
    let (pw, ph) = (shown.width().round() as u32, shown.height().round() as u32);
    let placed = ready.place(pw.max(1), ph.max(1))?;
    let pixels: Vec<Color32> = placed
        .rgba
        .iter()
        .map(|[r, g, b, a]| {
            Color32::from_rgba_unmultiplied(
                unit_to_u8(*r),
                unit_to_u8(*g),
                unit_to_u8(*b),
                unit_to_u8(*a),
            )
        })
        .collect();
    let image = egui::ColorImage {
        size: [placed.width, placed.height],
        source_size: Vec2::new(placed.width as f32, placed.height as f32),
        pixels,
    };
    let texture = ctx.load_texture("watermark-preview", image, egui::TextureOptions::LINEAR);
    let rect = egui::Rect::from_min_size(
        egui::pos2(placed.x as f32, placed.y as f32),
        Vec2::new(placed.width as f32, placed.height as f32),
    );
    Some((texture, rect))
}

/// The panel's grid: a label column, then every control from the same x,
/// on rows of one height, so nothing is out of line.
const LABEL: f32 = 76.;
const ROW: f32 = size::FIELD;
const GAP: f32 = space::M;
/// The number box after a slider.
const VALUE: f32 = 48.;

fn heading(ui: &mut egui::Ui, title: &str) {
    ui.add_space(space::L);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 18.), Sense::hover());
    ui.painter().text(
        rect.left_center(),
        egui::Align2::LEFT_CENTER,
        title,
        egui::FontId::proportional(text::LABEL),
        theme::palette(ui.ctx()).tone(Tone::TEXT),
    );
    ui.add_space(space::XXS);
}
/// One row: `label` right-aligned in the label column, then `contents`,
/// vertically centered, in the rest of the width.
fn row<R>(ui: &mut egui::Ui, label: &str, contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    row_of(ui, label, ROW, contents)
}
fn row_of<R>(
    ui: &mut egui::Ui,
    label: &str,
    height: f32,
    contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
    ui.painter().text(
        egui::pos2(rect.left() + LABEL, rect.top() + ROW / 2.),
        egui::Align2::RIGHT_CENTER,
        label,
        egui::FontId::proportional(text::LABEL),
        theme::palette(ui.ctx()).tone(Tone::SECONDARY),
    );
    let controls = egui::Rect::from_min_max(
        egui::pos2(rect.left() + LABEL + GAP, rect.top()),
        rect.right_bottom(),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(controls)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.set_min_height(height);
    contents(&mut child)
}
/// A slider across the row with its value in a box at the right edge.
fn slider(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    suffix: &str,
) {
    row(ui, label, |ui| {
        ui.spacing_mut().slider_width = (ui.available_width() - VALUE - GAP).max(40.);
        ui.add(egui::Slider::new(value, range.clone()).show_value(false));
        ui.add_sized(
            Vec2::new(VALUE, 20.),
            egui::DragValue::new(value)
                .range(range)
                .fixed_decimals(0)
                .suffix(suffix),
        );
    });
}
/// `slider` for a fraction, shown in percent.
fn percent_slider(
    ui: &mut egui::Ui,
    label: &str,
    fraction: &mut f32,
    range: std::ops::RangeInclusive<f32>,
) {
    let mut percent = *fraction * 100.;
    slider(ui, label, &mut percent, range, "%");
    *fraction = percent / 100.;
}
/// Choices side by side, as a segmented control.
fn choices<T: PartialEq + Copy>(ui: &mut egui::Ui, value: &mut T, options: &[(T, &str)]) {
    ui.spacing_mut().item_spacing.x = space::XS;
    for (option, text) in options {
        ui.selectable_value(value, *option, *text);
    }
}

/// Style, image or text options, shadow and effects, as Lightroom's
/// editor has them.
fn controls(ui: &mut egui::Ui, state: &mut WatermarkEditor, ctx: &egui::Context) {
    ui.spacing_mut().item_spacing.y = space::XS;
    let w = &mut state.watermark;
    heading(ui, "Watermark Style");
    row(ui, "", |ui| {
        choices(
            ui,
            &mut w.style,
            &[(Style::Text, "Text"), (Style::Graphic, "Graphic")],
        )
    });
    match w.style {
        Style::Graphic => {
            heading(ui, "Image Options");
            row(ui, "Image", |ui| {
                let name = state
                    .source
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .or_else(|| w.image.clone())
                    .unwrap_or_else(|| format!("A {} image", watermark::image_formats_named()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Choose…").clicked() {
                        let picked = state.picked.clone();
                        let ctx = ctx.clone();
                        std::thread::spawn(move || {
                            if let Some(file) = rfd::FileDialog::new()
                                .add_filter(
                                    watermark::image_formats_named(),
                                    &watermark::image_extensions(),
                                )
                                .pick_file()
                                && let Ok(mut slot) = picked.lock()
                            {
                                *slot = Some(file);
                            }
                            ctx.request_repaint();
                        });
                    }
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(name)
                                .size(text::LABEL)
                                .color(theme::palette(ui.ctx()).tone(Tone::LABEL)),
                        )
                        .truncate(),
                    );
                });
            });
        }
        Style::Text => {
            heading(ui, "Text Options");
            row_of(ui, "Text", 52., |ui| {
                ui.add_sized(
                    Vec2::new(ui.available_width(), 48.),
                    egui::TextEdit::multiline(&mut w.text).desired_rows(2),
                );
            });
            // Installed fonts are still being listed at first: Inter until then.
            let listed = watermark::fonts::families_if_listed();
            let inter = [watermark::fonts::inter()];
            let families: &[watermark::fonts::Family] = listed.unwrap_or(&inter);
            row(ui, "Font", |ui| {
                egui::ComboBox::from_id_salt("watermark-font")
                    .width(ui.available_width())
                    .selected_text(&w.family)
                    .show_ui(ui, |ui| {
                        for f in families {
                            if ui.selectable_label(w.family == f.name, &f.name).clicked() {
                                w.family = f.name.clone();
                                w.face =
                                    f.faces.first().map(|x| x.name.clone()).unwrap_or_default();
                            }
                        }
                    });
            });
            row(ui, "Style", |ui| {
                let faces = families
                    .iter()
                    .find(|f| f.name == w.family)
                    .map(|f| f.faces.clone())
                    .unwrap_or_default();
                egui::ComboBox::from_id_salt("watermark-face")
                    .width(ui.available_width())
                    .selected_text(&w.face)
                    .show_ui(ui, |ui| {
                        for face in &faces {
                            ui.selectable_value(&mut w.face, face.name.clone(), &face.name);
                        }
                    });
            });
            row(ui, "Align", |ui| {
                choices(
                    ui,
                    &mut w.align,
                    &[
                        (Align::Left, "Left"),
                        (Align::Center, "Center"),
                        (Align::Right, "Right"),
                    ],
                )
            });
            row(ui, "Color", |ui| {
                // The picker in sRGB, as the color is stored and laid over
                // the photo.
                let mut srgb = w.color.map(unit_to_u8);
                if ui.color_edit_button_srgb(&mut srgb).changed() {
                    w.color = srgb.map(|c| c as f32 / 255.);
                }
            });
            heading(ui, "Shadow");
            row(ui, "", |ui| {
                ui.checkbox(&mut w.shadow.enabled, "Drop shadow")
            });
            ui.add_enabled_ui(w.shadow.enabled, |ui| {
                percent_slider(ui, "Opacity", &mut w.shadow.opacity, 0.0..=100.);
                percent_slider(ui, "Offset", &mut w.shadow.offset, 0.0..=40.);
                percent_slider(ui, "Radius", &mut w.shadow.radius, 0.0..=40.);
                slider(ui, "Angle", &mut w.shadow.angle, -180.0..=180., "°");
            });
        }
    }
    heading(ui, "Watermark Effects");
    percent_slider(ui, "Opacity", &mut w.opacity, 0.0..=100.);
    row(ui, "Size", |ui| {
        ui.spacing_mut().item_spacing.x = space::XS;
        let proportional = matches!(w.size, Size::Proportional(_));
        if ui.selectable_label(proportional, "Proportional").clicked() && !proportional {
            w.size = Size::default();
        }
        ui.selectable_value(&mut w.size, Size::Fit, "Fit");
        ui.selectable_value(&mut w.size, Size::Fill, "Fill");
    });
    // Always a row, so choosing Fit or Fill moves nothing below.
    let mut percent = match w.size {
        Size::Proportional(p) => p,
        _ => 0.2,
    };
    ui.add_enabled_ui(matches!(w.size, Size::Proportional(_)), |ui| {
        percent_slider(ui, "Amount", &mut percent, 1.0..=100.);
    });
    if let Size::Proportional(p) = &mut w.size {
        *p = percent;
    }
    heading(ui, "Inset");
    percent_slider(ui, "Horizontal", &mut w.inset[0], 0.0..=50.);
    percent_slider(ui, "Vertical", &mut w.inset[1], 0.0..=50.);
    heading(ui, "Position");
    row_of(ui, "Anchor", 3. * 20. + 4., |ui| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(space::S, space::XXS);
            for v in 0..3 {
                ui.horizontal(|ui| {
                    for h in 0..3 {
                        ui.radio_value(&mut w.anchor, Anchor(h, v), "");
                    }
                });
            }
        });
    });
    row(ui, "Rotate", |ui| {
        if ui
            .add(egui::Button::new("⟲").frame(false))
            .on_hover_text("Rotate left")
            .clicked()
        {
            w.rotation = (w.rotation + 1) % 4;
        }
        if ui
            .add(egui::Button::new("⟳").frame(false))
            .on_hover_text("Rotate right")
            .clicked()
        {
            w.rotation = (w.rotation + 3) % 4;
        }
    });
}
