//! Lens Corrections › Profile's Lensfun parts: the row that offers Lensfun's
//! community profiles where no imported profile fits (a download first, then the
//! lens this photo matches), the Lensfun lenses in the Make, Model and Profile menus,
//! and resolving or updating the lens an edit names. Choosing one is an edit like
//! any other, one History step on the photo shown; the download only installs.
use super::{LABEL_GAP, control_left, label_column};
use crate::app::design::{Tone, radius, space, text};
use crate::app::lensfun::Job;
use crate::app::theme;
use crate::camera_data::Metadata;
use crate::lens::lensfun::choice::{LICENSE, LICENSE_URI, camera_for, frame_size, photo_of};
use crate::lens::lensfun::installed::Installed;
use crate::lens::lensfun::matching::{self, Outcome};
use crate::lens::lensfun::{Lens, LensfunChoice};
use crate::model::recipe::{ProfileCorrections, Recipe};
use crate::optics::lensfun::Components;
use eframe::egui;
use std::sync::Arc;

/// What the panel knows of Lensfun's database this frame.
pub(in crate::app) struct View {
    pub db: Option<Arc<Installed>>,
    pub running: Option<Job>,
    pub progress: Option<(u64, u64)>,
    pub failure: Option<String>,
    pub suggest: bool,
}

/// What the user asked for that the panel can't do itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Action {
    Download,
    Cancel,
}

/// The lenses a photo may choose from: those fitting its camera and frame.
pub(super) fn choices<'d>(db: &'d Installed, m: &Metadata) -> Vec<&'d Lens> {
    let photo = photo_of(m, frame_size(m));
    matching::choices(&db.db, &photo)
        .1
        .into_iter()
        .map(|i| &db.db.lenses[i])
        .filter(|l| l.refusal().is_none())
        .collect()
}

/// The menu label of a Lensfun lens: "Lensfun", with its calibration when the lens
/// has several.
pub(super) fn profile_label(lens: &Lens, all: &[&Lens]) -> String {
    let same = all
        .iter()
        .filter(|l| l.maker.id == lens.maker.id && l.model.id == lens.model.id)
        .count();
    if same > 1 {
        format!(
            "Lensfun (crop {:.2}, {})",
            lens.calibration.crop,
            aspect_text(lens.calibration.aspect)
        )
    } else {
        "Lensfun".into()
    }
}
fn aspect_text(aspect: f32) -> String {
    for (name, ratio) in [
        ("3:2", 1.5f32),
        ("4:3", 4. / 3.),
        ("16:9", 16. / 9.),
        ("1:1", 1.),
    ] {
        if (aspect - ratio).abs() < 0.01 {
            return name.into();
        }
    }
    format!("{aspect:.2}:1")
}

/// Chooses `lens` for photo `m`: Custom, with its calibration copied, and Enable
/// Profile Corrections on.
pub(super) fn choose(r: &mut Recipe, m: &Metadata, db: &Installed, lens: &Lens) {
    r.set_profile_corrections(m, ProfileCorrections::On);
    r.lens_profile_choice
        .choose_lensfun(LensfunChoice::of(lens, &db.db, camera_for(&db.db, m)));
}

/// The Lensfun offer under the Profile menus: "Lensfun" in the label column and,
/// beside it on a faint accent band, what it offers (a lens, the download, a menu
/// of lenses), what that does or why it can't, and its button. The text wraps
/// within the panel. Returns what it can't do itself.
pub(super) fn card(ui: &mut egui::Ui, r: &mut Recipe, m: &Metadata, view: &View) -> Option<Action> {
    let mut action = None;
    band(ui, "Lensfun", |ui| action = contents(ui, r, m, view));
    action
}
fn contents(ui: &mut egui::Ui, r: &mut Recipe, m: &Metadata, view: &View) -> Option<Action> {
    if m.is_rendered() {
        return None;
    }
    if let Some(chosen) = r.lens_profile_choice.lensfun().cloned() {
        return chosen_offer(ui, r, m, view, &chosen);
    }
    // Only where no imported Adobe profile fits this lens, and while suggestions
    // are on.
    if !view.suggest || m.lens_profiles.auto(m).is_some() {
        return None;
    }
    let builtin = [m.lens.as_ref(), m.lens_stand_in.as_ref()]
        .into_iter()
        .flatten()
        .map(Components::of)
        .fold(Components::default(), |a, b| Components {
            distortion: a.distortion || b.distortion,
            chromatic: a.chromatic || b.chromatic,
            vignetting: a.vignetting || b.vignetting,
        });
    if builtin.distortion && builtin.chromatic && builtin.vignetting {
        return None;
    }
    let Some(db) = &view.db else {
        return download_offer(ui, view);
    };
    let photo = photo_of(m, frame_size(m));
    let matched = matching::find(&db.db, &photo);
    match &matched.outcome {
        Outcome::Unique(i) => {
            let lens = &db.db.lenses[*i];
            if let Some(why) = lens.refusal() {
                title(ui, &lens_name(lens, m));
                note(
                    ui,
                    &format!("Can't be applied: Lensfun's profile is {why}."),
                );
                return None;
            }
            let gives = given(lens, db, m);
            let adds = Components {
                distortion: gives.distortion && !builtin.distortion,
                chromatic: gives.chromatic && !builtin.chromatic,
                vignetting: gives.vignetting && !builtin.vignetting,
            };
            if !adds.any() {
                return None;
            }
            title(ui, &lens_name(lens, m));
            note(ui, &format!("Corrects {}.", gives.describe()));
            if accent_button(ui, "Use")
                .on_hover_text("Correct this photo with Lensfun's profile of this lens")
                .clicked()
            {
                choose(r, m, db, lens);
            }
        }
        Outcome::Ambiguous(all) => {
            let lenses: Vec<&Lens> = all.iter().map(|i| &db.db.lenses[*i]).collect();
            title(ui, &format!("{} possible lenses", lenses.len()));
            note(
                ui,
                "The lens name fits more than one profile: choose the one used.",
            );
            pick(ui, r, m, db, "lensfun-ambiguous", "Choose a lens…", &lenses);
        }
        Outcome::UnknownCamera if matched.crop.is_none() => {
            title(ui, "Unknown camera");
            note(
                ui,
                "Lensfun doesn't know this camera, and the photo records no 35 mm equivalent focal length to tell its crop factor.",
            );
        }
        Outcome::NoMatch | Outcome::NoLensName | Outcome::UnknownCamera => {
            title(
                ui,
                match matched.outcome {
                    Outcome::UnknownCamera => "Unknown camera",
                    Outcome::NoLensName => "Lens not named",
                    _ => "No matching profile",
                },
            );
            note(
                ui,
                match matched.outcome {
                    Outcome::UnknownCamera => {
                        "Lensfun doesn't know this camera; choose the lens used."
                    }
                    Outcome::NoLensName => "The photo doesn't name its lens; choose the lens used.",
                    _ => "No profile matched this lens's name; choose the lens used.",
                },
            );
            let lenses = choices(db, m);
            pick(ui, r, m, db, "lensfun-manual", "Choose a lens…", &lenses);
        }
    }
    None
}

/// How the panel names a Lensfun lens: a compact's "fixed lens" by its camera.
fn lens_name(lens: &Lens, m: &Metadata) -> String {
    let label = lens.label();
    if label.eq_ignore_ascii_case("fixed lens") && !m.model.is_empty() {
        format!("{} {} lens", m.make, m.model).trim().to_string()
    } else {
        label
    }
}

/// The corrections `lens` gives photo `m`.
fn given(lens: &Lens, db: &Installed, m: &Metadata) -> Components {
    LensfunChoice::of(lens, &db.db, camera_for(&db.db, m))
        .apply(m)
        .correction
        .as_ref()
        .map(Components::of)
        .unwrap_or_default()
}

/// Before Lensfun is installed: the download, its progress, and its terms.
fn download_offer(ui: &mut egui::Ui, view: &View) -> Option<Action> {
    let mut action = None;
    title(ui, "Community lens profiles");
    note(
        ui,
        "Lensfun's free database may have a profile for this lens. Nothing changes until you choose one.",
    );
    if let Some(why) = &view.failure {
        warning(ui, &format!("The download failed: {why}"));
    }
    if let Some((done, total)) = view.progress.filter(|_| view.running.is_some()) {
        if progress(ui, done, total) {
            action = Some(Action::Cancel);
        }
    } else {
        let label = if view.failure.is_some() {
            "Retry".to_string()
        } else {
            format!(
                "Download · {:.1} MB",
                crate::lensfun_install::ABOUT_MEGABYTES
            )
        };
        if ui
            .add_enabled_ui(view.running.is_none(), |ui| accent_button(ui, &label))
            .inner
            .clicked()
        {
            action = Some(Action::Download);
        }
    }
    terms(ui);
    action
}

/// The lens the edit chose: unresolved ones offer the download or resolving it,
/// resolved ones a newer calibration when the database has one.
fn chosen_offer(
    ui: &mut egui::Ui,
    r: &mut Recipe,
    m: &Metadata,
    view: &View,
    chosen: &LensfunChoice,
) -> Option<Action> {
    let found = view
        .db
        .as_ref()
        .and_then(|db| db.db.find(&chosen.lens).map(|lens| (db, lens)));
    if !chosen.is_resolved() {
        let mut action = None;
        title(ui, chosen.name());
        match (&view.db, found) {
            (None, _) => {
                note(
                    ui,
                    "This edit names a Lensfun profile: download Lensfun's profiles to apply it.",
                );
                if let Some((done, total)) = view.progress.filter(|_| view.running.is_some()) {
                    if progress(ui, done, total) {
                        action = Some(Action::Cancel);
                    }
                } else if ui
                    .add_enabled_ui(view.running.is_none(), |ui| {
                        accent_button(ui, "Download Lensfun Profiles")
                    })
                    .inner
                    .clicked()
                {
                    action = Some(Action::Download);
                }
                terms(ui);
            }
            (Some(db), Some(_)) => {
                note(
                    ui,
                    "Not applied yet: resolve it against the installed profiles.",
                );
                if accent_button(ui, "Resolve").clicked()
                    && let Some(resolved) = chosen.resolved(&db.db, m)
                {
                    r.lens_profile_choice.choose_lensfun(resolved);
                }
            }
            (Some(_), None) => note(
                ui,
                "Not in the installed Lensfun profiles; a newer version may have it (Preferences › Profiles & Presets).",
            ),
        }
        return action;
    }
    // A newer calibration of the same lens.
    if let Some((db, lens)) = found
        && chosen.calibration.as_deref() != Some(&lens.calibration)
        && db.db.timestamp > chosen.provenance.database
    {
        title(ui, "Newer calibration");
        note(
            ui,
            &format!(
                "Lensfun's version of {} has a newer calibration of this lens.",
                db.manifest.date()
            ),
        );
        if accent_button(ui, "Update").clicked()
            && let Some(resolved) = chosen.resolved(&db.db, m)
        {
            r.lens_profile_choice.choose_lensfun(resolved);
        }
    }
    None
}

/// A menu of `lenses` that chooses the one picked.
fn pick(
    ui: &mut egui::Ui,
    r: &mut Recipe,
    m: &Metadata,
    db: &Installed,
    id: &str,
    label: &str,
    lenses: &[&Lens],
) {
    if lenses.is_empty() {
        note(
            ui,
            "Lensfun has no profile for this camera's mount and sensor size.",
        );
        return;
    }
    let mut chosen = None;
    egui::ComboBox::from_id_salt(id)
        .width(ui.available_width())
        .selected_text(label)
        .height(320.)
        .show_ui(ui, |ui| {
            for (i, lens) in lenses.iter().enumerate() {
                let text = format!("{} · {}", lens.label(), profile_label(lens, lenses));
                if ui.selectable_label(false, text).clicked() {
                    chosen = Some(i);
                }
            }
        });
    if let Some(i) = chosen {
        choose(r, m, db, lenses[i]);
    }
}

/// Lensfun's terms, as every download offer shows them.
pub(in crate::app) fn terms(ui: &mut egui::Ui) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.;
        small(ui, "Lensfun database by Lensfun contributors, ");
        ui.hyperlink_to(egui::RichText::new(LICENSE).size(text::SMALL), LICENSE_URI);
        small(ui, " · from ");
        ui.hyperlink_to(
            egui::RichText::new("lensfun.github.io").size(text::SMALL),
            "https://lensfun.github.io",
        );
    });
}

/// Lays `add` out beside the label column, from the rail start to the panel's edge
/// less a margin, wrapping; when it adds anything, puts `label` in the label
/// column at its first line and a faint accent band with an outline behind both.
/// Adds nothing to the panel when `add` shows nothing. Every offer card uses it.
pub(super) fn band(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    const PAD: f32 = space::M;
    let background = ui.painter().add(egui::Shape::Noop);
    let full = ui.available_rect_before_wrap();
    let content = egui::Rect::from_min_max(
        egui::pos2(full.left() + control_left(ui), full.top() + PAD),
        egui::pos2(full.right() - PAD, full.bottom()),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(content)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
    child.spacing_mut().item_spacing.y = space::XS;
    add(&mut child);
    let used = child.min_rect();
    if used.height() <= 0. {
        return;
    }
    let palette = theme::palette(ui.ctx());
    let rect = egui::Rect::from_x_y_ranges(full.x_range(), full.top()..=used.bottom() + PAD);
    ui.painter().set(
        background,
        egui::epaint::RectShape::new(
            rect,
            radius::CONTROL,
            palette.accent().gamma_multiply(0.10),
            egui::Stroke::new(1., palette.accent().gamma_multiply(0.40)),
            egui::StrokeKind::Inside,
        ),
    );
    ui.painter().text(
        egui::pos2(full.left() + label_column(ui) - LABEL_GAP, used.top()),
        egui::Align2::RIGHT_TOP,
        label,
        egui::FontId::proportional(text::SMALL),
        palette.tone(Tone::SOFT),
    );
    ui.allocate_rect(rect, egui::Sense::hover());
    ui.add_space(space::XS);
}
/// The offer's first line: a lens, or what the offer is about.
pub(super) fn title(ui: &mut egui::Ui, text: &str) {
    ui.add(
        egui::Label::new(
            egui::RichText::new(text)
                .size(text::SMALL)
                .color(theme::palette(ui.ctx()).tone(Tone::STRONG)),
        )
        .wrap(),
    );
}
/// The offer's one action, in the accent colour.
pub(super) fn accent_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    let fill = if ui.is_enabled() {
        palette.accent()
    } else {
        palette.tone(Tone::RAISED)
    };
    ui.add_space(space::XXS);
    ui.spacing_mut().button_padding = egui::vec2(space::L, space::XXS);
    ui.add(
        egui::Button::new(
            egui::RichText::new(label)
                .size(text::SMALL)
                .color(palette.on_accent()),
        )
        .fill(fill)
        .min_size(egui::vec2(0., 22.)),
    )
}
/// The download's bar and Cancel. Returns whether Cancel was clicked.
pub(super) fn progress(ui: &mut egui::Ui, done: u64, total: u64) -> bool {
    let fraction = if total > 0 {
        done as f32 / total as f32
    } else {
        0.
    };
    ui.add(
        egui::ProgressBar::new(fraction)
            .desired_width(ui.available_width())
            .desired_height(4.),
    );
    let mut cancel = false;
    ui.horizontal(|ui| {
        small(ui, &format!("{:.1} MB", done as f32 / 1e6));
        cancel = ui.small_button("Cancel").clicked();
    });
    cancel
}
pub(super) fn warning(ui: &mut egui::Ui, text: &str) {
    ui.add(
        egui::Label::new(
            egui::RichText::new(text)
                .size(text::SMALL)
                .color(theme::palette(ui.ctx()).warning()),
        )
        .wrap(),
    );
}
pub(super) fn note(ui: &mut egui::Ui, text: &str) {
    ui.add(
        egui::Label::new(
            egui::RichText::new(text)
                .size(text::SMALL)
                .color(theme::palette(ui.ctx()).tone(Tone::SECONDARY)),
        )
        .wrap(),
    );
}
pub(super) fn small(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .size(text::SMALL)
            .color(theme::palette(ui.ctx()).tone(Tone::SOFT)),
    );
}
