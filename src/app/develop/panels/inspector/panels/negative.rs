//! The Negative panel: a scanned film negative turned into a positive before
//! everything else renders (`develop::negative`). The film type, the film base
//! picker and Auto's levels; each is one History step, named "Negative".
use super::super::{control_row, eyedropper_icon};
use super::Action;
use crate::app::develop::negative::{STEP, refusal_text};
use crate::app::develop::state::{Tool, ViewState};
use crate::app::develop::step_name::StepName;
use crate::app::ui::design::{Tone, radius, size, space, text};
use crate::app::ui::theme;
use crate::app::ui::widgets::{collapsed_adjustment_section, segmented, toolbar_action_sized};
use crate::app::worker::AutoKind;
use crate::model::negative::{FilmType, NegativeConversion};
use crate::model::recipe::Recipe;
use eframe::egui::{self, Sense, Vec2};

/// What the Negative panel shows of the editor this frame.
#[derive(Clone, Copy)]
pub(in crate::app::develop::panels::inspector) struct View {
    /// The photo is decoded, which the picker and Auto read.
    pub decoded: bool,
    /// An Auto estimate is running (any panel's: one runs at a time).
    pub auto_running: bool,
    /// The photo is an HDR file, whose HDR master renders no conversion
    /// (`model::hdr::check_hdr_file`).
    pub hdr_file: bool,
    /// The last Auto found part of the film holder in this crop
    /// (`ViewState::negative_holder`).
    pub holder_in_crop: bool,
}

/// The film type as the panel's switch shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Film {
    Off,
    Colour,
    BlackWhite,
}
impl Film {
    fn of(r: &Recipe) -> Self {
        match r.negative.as_ref().map(|n| n.film) {
            None => Film::Off,
            Some(FilmType::Colour) => Film::Colour,
            Some(FilmType::BlackWhite) => Film::BlackWhite,
        }
    }
    /// As the switch and History name it.
    fn label(self) -> &'static str {
        match self {
            Film::Off => "Off",
            Film::Colour => "Color",
            Film::BlackWhite => "B&W",
        }
    }
}

/// Draws the panel.
pub(in crate::app::develop::panels::inspector) fn show(
    ui: &mut egui::Ui,
    step: &mut StepName,
    r: &mut Recipe,
    view: &mut ViewState,
    facts: &View,
    palette: &theme::Palette,
) -> Vec<Action> {
    let mut actions = Vec::new();
    if collapsed_adjustment_section(ui, step, "Negative", |ui, step| {
        ui.add_space(space::XS);
        let shown = Film::of(r);
        let mut film = shown;
        control_row(ui, "Film", |ui| {
            let options = [Film::Off, Film::Colour, Film::BlackWhite].map(|f| (f, f.label()));
            segmented(ui, &mut film, &options, ui.available_width())
        });
        if film != shown {
            let kind = match film {
                Film::Off => None,
                Film::Colour => Some(FilmType::Colour),
                Film::BlackWhite => Some(FilmType::BlackWhite),
            };
            // A change of film keeps what was measured of the scan.
            r.negative = kind.map(|kind| match &r.negative {
                Some(n) => n.with_film(kind),
                None => NegativeConversion::new(kind),
            });
            step.name(STEP.into(), film.label().into());
        }
        let Some(n) = &r.negative else {
            return;
        };
        let new = NegativeConversion::new(n.film);
        let soft = |ui: &mut egui::Ui, words: &str, tone: Tone| {
            ui.label(
                egui::RichText::new(words)
                    .size(text::SMALL)
                    .color(palette.tone(tone)),
            );
        };
        // The film base: what it is, and the picker at the row's end, as White
        // Balance's selector sits at its caption's.
        control_row(ui, "Film base", |ui| {
            match n.base == new.base {
                true => soft(ui, "Not picked", Tone::LABEL),
                false => soft(ui, "Picked", Tone::TEXT),
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let picking = view.is(Tool::FilmBase);
                let (rect, response) = ui.allocate_exact_size(
                    Vec2::new(26., 20.),
                    if facts.decoded {
                        Sense::click()
                    } else {
                        Sense::hover()
                    },
                );
                if picking || (facts.decoded && response.hovered()) {
                    ui.painter().rect_filled(
                        rect,
                        radius::CONTROL,
                        palette.tone(if picking {
                            Tone::PRESSED
                        } else {
                            Tone::OUTLINE
                        }),
                    );
                }
                if facts.decoded {
                    eyedropper_icon(ui.painter(), rect.center(), picking || response.hovered());
                } else {
                    // Dimmed as a disabled button's label is.
                    crate::app::ui::icons::paint_at(
                        ui.painter(),
                        crate::app::ui::icons::Icon::Eyedropper,
                        rect.center(),
                        14.,
                        palette.tone(Tone::OUTLINE_STRONG),
                    );
                }
                // A button to screen readers, pressed while the picker is out.
                response.widget_info(|| {
                    egui::WidgetInfo::selected(
                        egui::WidgetType::Button,
                        facts.decoded,
                        picking,
                        "Film base picker",
                    )
                });
                if response
                    .on_hover_text("Film base picker: click the clear film edge, beside the frame")
                    .clicked()
                {
                    view.toggle(Tool::FilmBase);
                }
            });
        });
        if view.is(Tool::FilmBase)
            && let Some(refusal) = view.film_base_refusal
        {
            note(ui, refusal_text(refusal), palette.warning());
        }
        // The levels Auto measures, and Auto at the row's end, as Basic's sits at its
        // heading's.
        control_row(ui, "Levels", |ui| {
            match (n.dmin, n.dmax) == (new.dmin, new.dmax) {
                true => soft(ui, "Not measured", Tone::LABEL),
                false => soft(ui, "Measured", Tone::TEXT),
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let auto = toolbar_action_sized(
                    ui,
                    "Auto",
                    Vec2::new(52., size::CONTROL + 2.),
                    false,
                    facts.decoded && !facts.auto_running,
                    0,
                );
                let auto = crate::app::ui::widgets::named(auto, "Auto levels");
                if auto
                    .on_hover_text("Level the frame inside the crop, leaving out its border")
                    .clicked()
                {
                    actions.push(Action::Auto(AutoKind::Negative));
                }
            });
        });
        if facts.holder_in_crop {
            note(
                ui,
                "The crop holds part of the film holder, which makes Auto too dark: \
                 crop it out and press Auto again",
                palette.warning(),
            );
        }
        // An HDR file's HDR master applies Exposure to its light before anything
        // reads it, so it renders no conversion: the photo shows its SDR rendition.
        if facts.hdr_file && r.hdr.enabled {
            note(
                ui,
                "An HDR file's HDR master doesn't render a conversion yet: \
                 the photo shows its SDR rendition",
                palette.warning(),
            );
        }
        // Why the picker and Auto are off, said where they are.
        if !facts.decoded {
            note(
                ui,
                "The picker and Auto work once the photo has loaded",
                palette.tone(Tone::SOFT),
            );
        } else if facts.auto_running {
            note(ui, "Measuring…", palette.tone(Tone::SOFT));
        }
    }) {
        r.negative = None;
    }
    // Turned off, reset or undone, the conversion takes its picker with it
    // (`Editor::finish_edit_frame` puts it away too, for undo).
    if r.negative.is_none() && view.is(Tool::FilmBase) {
        view.set_tool(Tool::None);
    }
    if r.negative.is_none() {
        view.negative_holder = None;
    }
    actions
}

/// A line under the rows, at the controls' left edge, wrapped to their width.
fn note(ui: &mut egui::Ui, words: &str, color: egui::Color32) {
    let left = crate::app::ui::widgets::control_left(ui);
    let width = ui.available_width();
    let galley = ui.painter().layout(
        words.to_owned(),
        egui::FontId::proportional(text::SMALL),
        color,
        width - left,
    );
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(width, galley.size().y), Sense::hover());
    ui.painter()
        .galley(egui::pos2(rect.left() + left, rect.top()), galley, color);
    // Read out as the text it shows.
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, words));
}
