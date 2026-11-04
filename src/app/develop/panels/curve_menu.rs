//! The Tone Curve panel's Point Curve menu: Lightroom's Linear, Medium Contrast and
//! Strong Contrast, the curves saved here, and Save… to save the photo's whole point
//! curve under a name. Choosing a curve is one History step.
use crate::app::ui::design::{self, Tone, button, inset, space, text};
use crate::app::ui::widgets::{modal_frame, primary_button};
use crate::app::{Editor, ui::theme};
use crate::model::panels::{Panel, PanelState};
use crate::presets::curves::{BuiltinCurve, PointCurve, SavedCurve, SavedCurves};
use eframe::egui::{self, Color32};

/// The saved curves, read once when the menu is first shown, and the name being
/// typed in the Save Point Curve window while it is open.
#[derive(Default)]
pub(in crate::app) struct CurveMenu {
    saved: Option<Vec<SavedCurve>>,
    saving: Option<CurveSave>,
}

/// The Save Point Curve window's name field, which takes the keyboard once.
struct CurveSave {
    name: String,
    focused: bool,
}

/// What was chosen in the menu.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::app) enum CurveChoice {
    Builtin(BuiltinCurve),
    Saved(SavedCurve),
    Save,
}

/// What the Save Point Curve window asked for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SaveChoice {
    Save,
    Cancel,
}

impl Editor {
    /// The saved curves, read from the Curves folder the first time.
    pub(in crate::app) fn saved_curves(&mut self) -> Vec<SavedCurve> {
        if self.curves.saved.is_none() {
            self.reload_saved_curves(&SavedCurves::default());
        }
        self.curves.saved.clone().unwrap_or_default()
    }
    fn reload_saved_curves(&mut self, store: &SavedCurves) {
        let list = store.list();
        if let Some(error) = list.errors.first() {
            self.messages.fail(format!("Point curve not read: {error}"));
        }
        self.curves.saved = Some(list.curves);
    }
    /// Carries out a choice from the Point Curve menu.
    pub(in crate::app) fn choose_point_curve(&mut self, choice: CurveChoice) {
        let before = self.document.edit.recipe().clone();
        let r = self.document.edit.recipe_mut();
        let name = match choice {
            CurveChoice::Builtin(curve) => {
                curve.apply(r);
                curve.name().to_string()
            }
            CurveChoice::Saved(saved) => {
                saved.curve.apply(r);
                saved.name
            }
            CurveChoice::Save => {
                self.curves.saving = Some(CurveSave {
                    name: String::new(),
                    focused: false,
                });
                return;
            }
        };
        // On, as any change to the panel turns it on, even when the curve was already
        // this one.
        r.panels.set(Panel::ToneCurve, PanelState::On);
        // A choice that changes nothing is no step, and must not name the next one.
        // Named through the frame, as a control's step is, so a wheel resize still
        // pending is recorded under its own name first.
        if *r != before {
            self.step_name.name("Point Curve".into(), name);
        }
    }
    /// Whether the Save Point Curve window is open.
    pub(in crate::app) fn curve_save_open(&self) -> bool {
        self.curves.saving.is_some()
    }
    /// The Save Point Curve window, while open.
    pub(in crate::app) fn curve_save_window(&mut self, ctx: &egui::Context) {
        let palette = theme::palette(ctx);
        let Some(saving) = &mut self.curves.saving else {
            return;
        };
        let mut choice = None;
        let response = egui::Modal::new(egui::Id::new("save-point-curve"))
            .backdrop_color(Color32::from_black_alpha(140))
            .frame(modal_frame(&palette).inner_margin(design::margin_same(inset::DIALOG)))
            .show(ctx, |ui| {
                ui.set_width(380.);
                ui.label(
                    egui::RichText::new("Save Point Curve")
                        .size(text::TITLE)
                        .color(palette.tone(Tone::STRONG)),
                );
                ui.add_space(space::S);
                ui.label(
                    egui::RichText::new(
                        "Saves the RGB, Red, Green and Blue curves for the Point Curve menu.",
                    )
                    .size(text::SMALL)
                    .color(palette.tone(Tone::SECONDARY)),
                );
                ui.add_space(space::L);
                let field = ui.add(
                    egui::TextEdit::singleline(&mut saving.name)
                        .hint_text("Curve name")
                        .desired_width(f32::INFINITY),
                );
                if !saving.focused {
                    field.request_focus();
                    saving.focused = true;
                }
                let named = !saving.name.trim().is_empty();
                if named && field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    choice = Some(SaveChoice::Save);
                }
                ui.add_space(space::XL);
                ui.horizontal(|ui| {
                    ui.spacing_mut().button_padding = button::DIALOG;
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled_ui(named, |ui| primary_button(ui, "Save"))
                            .inner
                            .clicked()
                        {
                            choice = Some(SaveChoice::Save);
                        }
                        if ui.button("Cancel").clicked() {
                            choice = Some(SaveChoice::Cancel);
                        }
                    });
                });
            });
        if response.should_close() {
            choice = Some(SaveChoice::Cancel);
        }
        let Some(choice) = choice else {
            return;
        };
        let saving = self.curves.saving.take().expect("open above");
        if choice == SaveChoice::Save {
            self.save_point_curve(&SavedCurves::default(), &saving.name);
        }
    }
    /// Saves the photo's point curve as `name` in `store`, and lists it.
    fn save_point_curve(&mut self, store: &SavedCurves, name: &str) {
        let curve = PointCurve::of(self.document.edit.recipe());
        match store.save(name, &curve) {
            Ok(_) => {
                let saved = format!("Point curve {} saved", name.trim());
                self.messages.clear_line();
                self.reload_saved_curves(store);
                // A curve the reload could not read is said too.
                if self.messages.line().is_empty() {
                    self.messages.done(saved);
                } else {
                    // The curve not read has its own card.
                    let line = format!("{saved}. {}", self.messages.line());
                    self.messages.note(line);
                }
            }
            Err(e) => self.messages.fail(format!("Point curve not saved: {e:#}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choosing_a_curve_is_one_named_history_step_that_turns_the_panel_on() {
        let ctx = egui::Context::default();
        let mut e = Editor::with_context(
            &ctx,
            None,
            crate::app::preferences::session::Session::default(),
            None,
        );
        e.document
            .edit
            .setup_mut()
            .panels
            .set(Panel::ToneCurve, PanelState::Off);
        let before = e.document.edit.recipe().clone();
        let frame = e.begin_edit_frame();
        e.choose_point_curve(CurveChoice::Builtin(BuiltinCurve::MediumContrast));
        e.finish_edit_frame(frame, &ctx);
        let r = e.document.edit.recipe();
        assert_eq!(r.curve, BuiltinCurve::MediumContrast.curve());
        assert_eq!(r.panels.state(Panel::ToneCurve), PanelState::On);
        let (steps, applied) = e.document.edit.history().steps();
        assert_eq!(applied, 1);
        assert_eq!(
            (steps[0].name.as_str(), steps[0].value.as_str()),
            ("Point Curve", "Medium Contrast")
        );
        e.undo();
        assert_eq!(*e.document.edit.recipe(), before);

        // The same curve again, with the panel off: it turns on, as one step.
        e.document.edit.setup_mut().curve = BuiltinCurve::MediumContrast.curve();
        e.document
            .edit
            .setup_mut()
            .panels
            .set(Panel::ToneCurve, PanelState::Off);
        let frame = e.begin_edit_frame();
        e.choose_point_curve(CurveChoice::Builtin(BuiltinCurve::MediumContrast));
        e.finish_edit_frame(frame, &ctx);
        let r = e.document.edit.recipe();
        assert_eq!(r.panels.state(Panel::ToneCurve), PanelState::On);
        let (steps, applied) = e.document.edit.history().steps();
        assert_eq!((applied, steps[0].name.as_str()), (1, "Point Curve"));
        // Once more, with nothing to change: no step, and the next edit keeps its
        // own name.
        let frame = e.begin_edit_frame();
        e.choose_point_curve(CurveChoice::Builtin(BuiltinCurve::MediumContrast));
        e.finish_edit_frame(frame, &ctx);
        assert_eq!(e.document.edit.history().steps().1, 1);
        let frame = e.begin_edit_frame();
        e.document.edit.setup_mut().exposure = 0.5;
        e.finish_edit_frame(frame, &ctx);
        let (steps, applied) = e.document.edit.history().steps();
        assert_eq!(applied, 2);
        assert_ne!(steps[1].name, "Point Curve");
    }

    #[test]
    fn a_saved_curve_is_listed_and_loads_all_four_curves_in_one_step() {
        let ctx = egui::Context::default();
        let mut e = Editor::with_context(
            &ctx,
            None,
            crate::app::preferences::session::Session::default(),
            None,
        );
        let d = tempfile::tempdir().unwrap();
        let store = SavedCurves {
            dir: d.path().join("Curves"),
        };
        e.curves.saved = Some(Vec::new());
        e.document.edit.setup_mut().curve = BuiltinCurve::StrongContrast.curve();
        // Points on Lightroom's 0–255 steps, as saved curves keep them.
        e.document.edit.setup_mut().effects.channels[1].points =
            vec![[0., 0.], [128. / 255., 153. / 255.], [1., 1.]];
        let saved_recipe = e.document.edit.recipe().clone();
        e.save_point_curve(&store, "Green Lift");
        assert_eq!(e.messages.line(), "Point curve Green Lift saved");
        let saved = e.saved_curves();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].name, "Green Lift");
        // A curve file that can't be read is still said after a save.
        std::fs::write(store.dir.join("Broken.xmp"), "<not xmp").unwrap();
        e.save_point_curve(&store, "Other");
        assert!(
            e.messages.line().starts_with("Point curve Other saved"),
            "{}",
            e.messages.line()
        );
        assert!(
            e.messages.line().contains("Broken.xmp"),
            "{}",
            e.messages.line()
        );
        std::fs::remove_file(store.dir.join("Broken.xmp")).unwrap();
        // Saving again under the same name keeps the first.
        e.save_point_curve(&store, "Green Lift");
        assert!(
            e.messages.line().contains("already saved"),
            "{}",
            e.messages.line()
        );

        e.document
            .edit
            .replace(crate::model::recipe::Recipe::default());
        let frame = e.begin_edit_frame();
        e.choose_point_curve(CurveChoice::Saved(saved[0].clone()));
        e.finish_edit_frame(frame, &ctx);
        let r = e.document.edit.recipe();
        assert_eq!(r.curve, saved_recipe.curve);
        assert_eq!(r.effects.channels, saved_recipe.effects.channels);
        let (steps, applied) = e.document.edit.history().steps();
        assert_eq!(applied, 1);
        assert_eq!(steps[0].value, "Green Lift");
        assert_eq!(crate::presets::curves::shown_name(r, &saved), "Green Lift");
    }
}
