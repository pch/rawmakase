//! Basic › Profile's offer of community camera profiles from the catalogue
//! (`camera_profiles::catalogue`), under the Profile menu as Lens Corrections
//! offers Lensfun's: each profile for this camera, who made it and on what terms,
//! and its one action: Download & Apply, or Apply when it is installed already.
//! Nothing is fetched or changed until that is clicked; the editor
//! (`app::profile_download`) does it.
use super::lensfun::{accent_button, band, note, progress, small, title, warning};
use crate::app::design::text;
use crate::app::profile_download::{Offer, OfferAction, Offered};
use crate::camera_profiles::catalogue::Status;
use eframe::egui;

/// The offer on its band, labelled "Community". Returns what was clicked.
pub(super) fn card(ui: &mut egui::Ui, offer: &Offer) -> Option<OfferAction> {
    let mut action = None;
    band(ui, "Community", |ui| action = contents(ui, offer));
    action
}

fn contents(ui: &mut egui::Ui, offer: &Offer) -> Option<OfferAction> {
    let mut action = None;
    let n = offer.profiles.len();
    title(
        ui,
        &if n == 1 {
            "Community camera profile".to_string()
        } else {
            format!("{n} community camera profiles")
        },
    );
    note(
        ui,
        "Made for this camera and free to use. Nothing is downloaded or changed until you choose.",
    );
    if offer
        .running
        .is_some_and(|(id, ..)| offer.profiles.iter().all(|p| p.entry.profile.id != id))
    {
        note(
            ui,
            "Another camera's profile is downloading; this one can follow.",
        );
    }
    for (i, offered) in offer.profiles.iter().enumerate() {
        ui.push_id(i, |ui| {
            if let Some(clicked) = profile(ui, offered, offer) {
                action = Some(clicked);
            }
        });
    }
    if ui
        .small_button("Dismiss")
        .on_hover_text(
            "Hide this offer for this camera until RAWmakase restarts. Profile › Show community profiles for this camera brings it back.",
        )
        .clicked()
    {
        action = Some(OfferAction::Dismiss);
    }
    action
}

fn profile(ui: &mut egui::Ui, offered: &Offered, offer: &Offer) -> Option<OfferAction> {
    let entry = offered.entry;
    let p = &entry.profile;
    ui.add_space(crate::app::design::space::XXS);
    ui.add(egui::Label::new(egui::RichText::new(&p.name).size(text::SMALL).strong()).wrap());
    credit(ui, offered);
    let mut action = None;
    if let Some((id, fraction, cancellable)) = offer.running
        && id == p.id
    {
        if cancellable {
            let done = (fraction * p.size as f32) as u64;
            if progress(ui, done, p.size) {
                action = Some(OfferAction::Cancel);
            }
        } else {
            note(ui, "Installing…");
        }
        return action;
    }
    if offered.in_use {
        note(ui, "This photo uses it.");
        return None;
    }
    if let Some(why) = &offered.failure {
        warning(ui, &format!("The download failed: {why}"));
    }
    match &offered.status {
        Status::Installed(_) => {
            if accent_button(ui, "Apply")
                .on_hover_text("Use this installed profile for this photo")
                .clicked()
            {
                action = Some(OfferAction::Apply(p.id.as_str()));
            }
        }
        Status::Conflict => warning(
            ui,
            &format!(
                "A different file named “{}” is in your profile library. Move or rename it to download this one.",
                p.filename
            ),
        ),
        Status::Available => {
            let label = if offered.failure.is_some() {
                "Retry".to_string()
            } else {
                format!("Download & Apply · {}", size(p.size))
            };
            if ui
                .add_enabled_ui(offer.running.is_none(), |ui| accent_button(ui, &label))
                .inner
                .on_hover_text(
                    "Downloads this exact file, checks it, adds it to your profile library and uses it for this photo",
                )
                .clicked()
            {
                action = Some(OfferAction::Download(p.id.as_str()));
            }
        }
    }
    action
}

/// Who made it, on what terms, and where it comes from: "Profile by RawTherapee
/// contributors, CC0 1.0 · from RawTherapee", with the terms and the pinned file
/// linked. The file's own notice is in the terms' hover text.
fn credit(ui: &mut egui::Ui, offered: &Offered) {
    let entry = offered.entry;
    let terms = entry.profile.terms;
    let notice = format!("The file's own notice: “{}”", entry.profile.notice);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.;
        small(ui, &format!("Profile by {}, ", entry.source.credit));
        let terms_text = egui::RichText::new(terms.label()).size(text::SMALL);
        match terms.url() {
            Some(url) => ui.hyperlink_to(terms_text, url).on_hover_text(notice),
            None => ui.label(terms_text).on_hover_text(notice),
        };
        small(ui, " · from ");
        ui.hyperlink_to(
            egui::RichText::new(&entry.source.name).size(text::SMALL),
            entry.page(),
        )
        .on_hover_text(format!(
            "{} at revision {}",
            entry.profile.file,
            &entry.source.revision[..8]
        ));
    });
}

/// A download's size as the button gives it: "65 KB", "1.1 MB".
fn size(bytes: u64) -> String {
    if bytes < 1_000_000 {
        format!("{} KB", bytes.div_ceil(1000))
    } else {
        format!("{:.1} MB", bytes as f64 / 1e6)
    }
}
