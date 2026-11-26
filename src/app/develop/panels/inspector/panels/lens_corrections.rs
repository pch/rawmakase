//! The Lens Corrections panel: Profile (Adobe lens profiles, the lens data in
//! the RAW, and Lensfun's, `inspector::lens_profile` and `inspector::lensfun`),
//! then Lightroom's Manual tab: Distortion, Defringe and Vignetting.
use super::super::{
    PanelSwitch, checkbox_row, control_row, defringe_sliders, eyedropper_icon, lens_profile,
    lensfun, setting_control, subheading, subheading_tool_rect, switch_row,
};
use super::Action;
use crate::app::develop::state::{Tool, ViewState};
use crate::app::develop::step_name::StepName;
use crate::app::presets::bulk_import::ImportKind;
use crate::app::ui::design::{Tone, radius, text};
use crate::app::ui::theme;
use crate::app::ui::widgets::switched_section;
use crate::camera_data::Metadata;
use crate::model::panels::Panel;
use crate::model::params::ParameterId;
use crate::model::recipe::Recipe;
use eframe::egui::{self, Sense};

/// Draws the panel for photo `metadata`; `lensfun_view` is what Lensfun's
/// database holds this frame.
pub(in crate::app::develop::panels::inspector) fn show(
    ui: &mut egui::Ui,
    step: &mut StepName,
    r: &mut Recipe,
    view: &mut ViewState,
    metadata: Option<&Metadata>,
    lensfun_view: &lensfun::View,
    palette: &theme::Palette,
) -> Vec<Action> {
    let mut actions = Vec::new();
    let mut switch = PanelSwitch::new(r, Panel::LensCorrections);
    if switched_section(
        ui,
        step,
        "Lens Corrections",
        &mut switch.state,
        |ui, step| {
            let photo = metadata;
            subheading(ui, "Profile");
            checkbox_row(ui, |ui| {
                ui.checkbox(&mut r.lens_ca, "Remove Chromatic Aberration")
                .on_hover_text("Remove red/cyan and blue/yellow fringes toward the edges of the frame, measured from the photo itself.");
            });
            checkbox_row(ui, |ui| {
                let mut on = r.lens_profile;
                if ui
                .checkbox(&mut on, "Enable Profile Corrections")
                .on_hover_text("Correct distortion and vignetting with an Adobe lens profile, or the lens data the camera stored in the RAW.")
                .changed()
                && let Some(m) = metadata
            {
                let state = if on {
                    crate::model::recipe::ProfileCorrections::On
                } else {
                    crate::model::recipe::ProfileCorrections::Off
                };
                r.set_profile_corrections(m, state);
            }
            });
            let lens = metadata
                .map(|m| m.lens_model.clone())
                .filter(|l| !l.is_empty());
            control_row(ui, "Lens", |ui| {
                ui.label(
                    egui::RichText::new(lens.as_deref().unwrap_or("Unknown"))
                        .size(text::SMALL)
                        .color(palette.tone(Tone::SOFT)),
                );
            });
            lens_profile::profile_menus(ui, r, metadata, lensfun_view);
            // The file's own correction, which Enable Profile Corrections doesn't turn
            // off, has a switch of its own. While a profile or the camera's stand-in
            // renders in its place the switch changes nothing, so it is disabled then
            // and the note says why.
            if let Some(m) = metadata
                && r.file_lens_correction(m).is_some()
            {
                let replaced = r.as_rendered().file_lens_correction_replaced(m);
                ui.add_enabled_ui(!replaced, |ui| {
                    switch_row(
                        ui,
                        &mut r.lens_file_correction,
                        FILE_CORRECTION_LABEL,
                        "Correct the photo with the lens data stored in the file. Off, \
                         the photo renders without it.",
                    );
                });
            }
            // Said here as well as on import, so a missing profile is never silent; and
            // in the window, not only in a hover tip, what the file's own correction
            // does, so toggling Enable Profile Corrections doesn't look broken.
            if let Some(note) = metadata.and_then(|m| correction_note(r, m)) {
                control_row(ui, "", |ui| {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(note)
                                .size(text::SMALL)
                                .color(palette.tone(Tone::SECONDARY)),
                        )
                        .wrap(),
                    );
                });
            }
            if let Some(m) = metadata
                && let Some(action) = lensfun::card(ui, r, m, lensfun_view)
            {
                actions.push(Action::Lensfun(action));
            }
            if r.lens_profile {
                subheading(ui, "Amount");
                ui.push_id("lens-amount", |ui| {
                    setting_control(ui, step, r, ParameterId::LensDistortion, 1., photo);
                    setting_control(ui, step, r, ParameterId::LensVignetting, 1., photo);
                });
            }
            control_row(ui, "", |ui| {
                if ui
                .small_button("Import Lens Profiles…")
                .on_hover_text("Import Adobe .lcp lens profiles, e.g. from /Library/Application Support/Adobe/CameraRaw/LensProfiles")
                .clicked()
            {
                actions.push(Action::ImportLens);
            }
                if ui
                    .small_button("Import Folder…")
                    .on_hover_text("Import every .lcp lens profile in a folder and its subfolders")
                    .clicked()
                {
                    actions.push(Action::ImportFolder(ImportKind::LensProfiles));
                }
            });
            // Lightroom's Manual tab: Distortion, then Defringe and Vignetting.
            subheading(ui, "Distortion");
            ui.push_id("manual-distortion", |ui| {
                setting_control(ui, step, r, ParameterId::ManualDistortion, 0., photo);
            });
            // The Fringe Color Selector, at the heading row's far right, as White
            // Balance's sits at the right of its caption.
            let rect = subheading_tool_rect(subheading(ui, "Defringe"));
            let response = ui.interact(rect, ui.id().with("fringe-selector"), Sense::click());
            let picking = view.is(Tool::Defringe);
            if picking || response.hovered() {
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
            eyedropper_icon(ui.painter(), rect.center(), picking || response.hovered());
            if response
                .on_hover_text("Fringe color selector: click a purple or green fringe in the photo")
                .clicked()
            {
                view.toggle(Tool::Defringe);
            }
            defringe_sliders(ui, step, &mut r.effects);
            subheading(ui, "Vignetting");
            ui.push_id("lens-vignette", |ui| {
                setting_control(ui, step, r, ParameterId::LensVignetteAmount, 0., photo);
                setting_control(ui, step, r, ParameterId::LensVignetteMidpoint, 0.5, photo);
            });
        },
    ) {
        if let Some(m) = metadata {
            r.lens_builtin = m.lens.as_ref().is_none_or(|l| l.default_on);
        }
        let defaults = Recipe::default();
        r.lens_file_correction = defaults.lens_file_correction;
        r.lens_ca = defaults.lens_ca;
        r.lens_profile = defaults.lens_profile;
        r.lens_profile_choice = defaults.lens_profile_choice;
        r.lens_distortion = defaults.lens_distortion;
        r.lens_vignetting = defaults.lens_vignetting;
        r.lens_manual_distortion = defaults.lens_manual_distortion;
        let d = Recipe::default().effects;
        r.effects.defringe = d.defringe;
        r.effects.defringe_ranges = d.defringe_ranges;
        r.effects.lens_vignette = d.lens_vignette;
        r.effects.lens_vignette_midpoint = d.lens_vignette_midpoint;
    }
    switch.finish(r);
    actions
}

/// The caption of the switch for the file's own lens correction.
const FILE_CORRECTION_LABEL: &str = "Lens correction stored in the file";

/// What the panel says under the Profile menus, of the settings as they render (a
/// switched-off panel renders its defaults): why the profile it should use can't
/// render ([`Recipe::missing_lens_profile`]), and what the correction stored in the
/// file does ([`Recipe::file_lens_correction`]). It renders whether Enable Profile
/// Corrections is on or off ([`Recipe::fixed_lens_correction`]) unless its own
/// switch ([`Recipe::lens_file_correction`]) is off or something renders in its
/// place ([`Recipe::file_lens_correction_replaced`]). Where the file's other tables
/// follow Enable Profile Corrections (Sony's), only part of it does.
fn correction_note(r: &Recipe, m: &Metadata) -> Option<String> {
    let r = r.as_rendered();
    let missing = r.missing_lens_profile(m);
    if r.file_lens_correction(m).is_none() {
        return missing;
    }
    if r.file_lens_correction_replaced(m) {
        let used = missing.map_or(String::new(), |missing| format!("{missing}. "));
        let rendering = r.lens_correction(m);
        let stand_in = (m.lens_stand_in.as_ref()).filter(|s| rendering.as_deref() == Some(*s));
        let by = stand_in.map_or("The profile in use replaces", |s| s.source.as_str());
        let verb = if stand_in.is_some() { " includes" } else { "" };
        return Some(format!(
            "{used}{by}{verb} the lens correction stored in this file, so its switch \
             changes nothing while Enable Profile Corrections is on."
        ));
    }
    if !r.lens_file_correction {
        let off =
            "The lens correction stored in this file is off, so the photo renders without it.";
        return Some(match missing {
            Some(missing) => format!("{missing}. {off}"),
            None => off.into(),
        });
    }
    // An imported profile or Lensfun lens would replace it once Enable Profile
    // Corrections is on: nothing to say of it then.
    let Some(fixed) = r.fixed_lens_correction(m) else {
        return missing;
    };
    // A stand-in that holds no more than the fixed correction (a Sony lens that
    // stores only its CA) leaves nothing for Enable Profile Corrections to add. A
    // missing profile can't be said with a fuller stand-in: that one is in use.
    let part = m.lens_stand_in.as_ref().is_some_and(|s| s != fixed);
    // With a missing profile the note ends by naming the file's correction, which is
    // what renders; the clause then says the rest.
    Some(match (missing, part) {
        (Some(missing), _) => format!(
            "{missing}, which applies in full, whether Enable Profile Corrections is on or off."
        ),
        (None, false) => "The lens correction stored in this file applies in full, whether Enable Profile Corrections is on or off.".into(),
        (None, true) => "Part of the lens correction stored in this file applies, whether Enable Profile Corrections is on or off.".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::panels::{Panel, PanelState};
    use crate::model::recipe::ProfileCorrections::{self, Off, On};
    use crate::optics::{LensCorrection, Radial};

    /// A photo whose file stores a distortion `source`; with `stand_in`, fuller
    /// data (vignetting too) stands in for a profile, as a Sony file's tables do.
    fn photo(source: &str, default_on: bool, stand_in: bool) -> Metadata {
        let lens = LensCorrection {
            source: source.into(),
            default_on,
            distortion: Radial::new(vec![0., 1.], vec![1., 1.02]),
            ..Default::default()
        };
        Metadata {
            lens_stand_in: stand_in.then(|| LensCorrection {
                vignetting: Radial::new(vec![0., 1.], vec![1., 1.8]),
                ..lens.clone()
            }),
            lens: Some(lens),
            ..Default::default()
        }
    }
    fn note(m: &Metadata, epc: ProfileCorrections, file: bool) -> String {
        let mut r = Recipe {
            lens_file_correction: file,
            ..Recipe::for_metadata(m)
        };
        r.set_profile_corrections(m, epc);
        correction_note(&r, m).unwrap_or_default()
    }
    const APPLIES: &str = "applies in full, whether Enable Profile Corrections is on or off.";
    const OFF: &str = "is off, so the photo renders without it.";
    const NOTHING: &str = "so its switch changes nothing while Enable Profile Corrections is on.";

    /// The note says the file's correction applies whatever Enable Profile
    /// Corrections says while its switch is on, that it is off when it is, "part"
    /// where the rest of the file's tables follow Enable Profile Corrections, and
    /// that the switch changes nothing while something renders in its place (#1289).
    #[test]
    fn the_note_says_what_the_files_correction_does() {
        let dng = photo(LensCorrection::DNG_OPCODES, true, false);
        assert!(note(&dng, Off, true).starts_with("The lens correction stored in this file"));
        assert!(note(&dng, Off, true).ends_with(APPLIES));
        assert!(note(&dng, On, true).starts_with("No matching imported Adobe lens profile"));
        assert!(note(&dng, On, true).ends_with(APPLIES));
        assert!(note(&dng, Off, false).ends_with(OFF));
        assert!(note(&dng, On, false).contains("no lens correction. "));
        assert!(note(&dng, On, false).ends_with(OFF));
        let sony = photo(LensCorrection::SONY_BUILTIN, true, true);
        assert!(note(&sony, Off, true).starts_with("Part of"));
        assert!(note(&sony, Off, false).ends_with(OFF));
        for file in [true, false] {
            assert!(note(&sony, On, file).contains("Sony built-in includes the lens correction"));
            assert!(note(&sony, On, file).ends_with(NOTHING));
        }
        // A stand-in no fuller than what always applies adds nothing.
        let same = Metadata {
            lens_stand_in: sony.lens.clone(),
            ..sony
        };
        assert!(note(&same, Off, true).ends_with(APPLIES));
        // Nikon's optional profile follows Enable Profile Corrections: only a
        // missing profile is said.
        let nikon = photo("Nikon built-in", false, false);
        assert_eq!(note(&nikon, Off, true), "");
        assert!(note(&nikon, On, true).ends_with("using Nikon built-in"));
        // An embedded "Camera Settings" profile names the file's own correction: off,
        // it is said to be off, not missing.
        let mut camera = Recipe {
            lens_file_correction: false,
            ..Recipe::for_metadata(&dng)
        };
        camera.set_profile_corrections(&dng, On);
        camera.lens_profile_choice.id = Some(crate::lens::choice::LensProfileRef::Adobe(
            crate::lens::choice::LensProfileId {
                name: "Camera Settings".into(),
                embedded: true,
                ..Default::default()
            },
        ));
        assert_eq!(camera.missing_lens_profile(&dng), None);
        assert!(
            correction_note(&camera, &dng)
                .unwrap()
                .starts_with("The lens correction stored")
        );
        // The panel switched off renders its defaults: the correction applies.
        let mut bypassed = Recipe {
            lens_file_correction: false,
            ..Recipe::for_metadata(&dng)
        };
        bypassed.panels.set(Panel::LensCorrections, PanelState::Off);
        assert!(correction_note(&bypassed, &dng).unwrap().ends_with(APPLIES));
    }

    /// Beside an imported profile for the lens the file's correction renders while
    /// Enable Profile Corrections is off, so its switch is there and the note says
    /// when it is off; with the profile in use the switch changes nothing (#1289).
    #[test]
    fn the_note_follows_what_renders_beside_an_imported_profile() {
        let lens = "Testcam 35mm F2";
        let mut m = Metadata {
            make: "Testcam".into(),
            lens_model: lens.into(),
            focal: 35.,
            aperture: 2.,
            width: 600,
            height: 400,
            ..photo(LensCorrection::DNG_OPCODES, true, false)
        };
        let lcp = crate::lens::lcp::test_profile("Testcam", lens, "Adobe (Testcam)", -0.05, -0.5);
        m.lens_profiles =
            crate::lens::lcp::Library::from_texts([("T.lcp", lcp.as_str())]).for_photo(&m);
        let r = Recipe::for_metadata(&m);
        assert!(r.file_lens_correction(&m).is_some() && r.fixed_lens_correction(&m).is_none());
        assert_eq!(note(&m, Off, true), "");
        assert!(note(&m, Off, false).ends_with(OFF));
        assert!(note(&m, On, false).starts_with("The profile in use replaces"));
        assert!(note(&m, On, false).ends_with(NOTHING));
    }
}
