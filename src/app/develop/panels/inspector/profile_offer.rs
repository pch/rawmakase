//! Basic › Profile's offer of community camera profiles from the catalogue
//! (`camera_profiles::catalogue`), under the Profile menu as Lens Corrections
//! offers Lensfun's. It leads with each profile for this camera: its name and
//! terms, who made it and where from, then what it is to this photo and its one
//! action: Download & Apply, or Apply when it is installed already. Nothing is
//! fetched or changed until that is clicked; the editor (`app::develop::profile_download`)
//! does it.
use super::lensfun::{band, note, warning};
use crate::app::develop::profile_download::{Offer, OfferAction, Offered};
use crate::app::ui::design::{Tone, radius, size, space, text};
use crate::app::ui::theme;
use crate::camera_profiles::catalogue::Status;
use eframe::egui;

/// What the offer says when a profile can be downloaded: that one is available.
pub(super) const AVAILABLE: &str = "A free profile made for this camera is available.";

/// The offer on its band, labelled "Community". Returns what was clicked.
pub(super) fn card(ui: &mut egui::Ui, offer: &Offer) -> Option<OfferAction> {
    let mut action = None;
    band(ui, "Community", |ui| action = contents(ui, offer));
    action
}

fn contents(ui: &mut egui::Ui, offer: &Offer) -> Option<OfferAction> {
    let mut action = None;
    for (i, offered) in offer.profiles.iter().enumerate() {
        if i > 0 {
            ui.add_space(space::S);
            ui.separator();
            ui.add_space(space::S);
        }
        ui.push_id(i, |ui| {
            if let Some(clicked) = profile(ui, offered, offer) {
                action = Some(clicked);
            }
        });
    }
    // One Dismiss for the camera, under the last profile's action; none while one
    // of its profiles downloads, which keeps the offer up.
    let running_here = offer
        .running
        .is_some_and(|(id, ..)| offer.profiles.iter().any(|p| p.entry.profile.id == id));
    ui.add_space(space::XXS);
    if !running_here && ui.vertical_centered(dismiss).inner {
        action = Some(OfferAction::Dismiss);
    }
    action
}

fn profile(ui: &mut egui::Ui, offered: &Offered, offer: &Offer) -> Option<OfferAction> {
    let p = &offered.entry.profile;
    // Name, credit and terms as one tight group: rows as tall as their text.
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = space::XXS;
        ui.spacing_mut().interact_size.y = 0.;
        heading(ui, offered);
        credit(ui, offered);
    });
    ui.add_space(space::S);

    if let Some((id, fraction, cancellable)) = offer.running
        && id == p.id
    {
        if cancellable {
            note(ui, "Downloading…");
            let done = (fraction * p.size as f32) as u64;
            return downloading(ui, done, p.size).then_some(OfferAction::Cancel);
        }
        note(ui, "Installing…");
        return None;
    }
    if offered.in_use {
        note(ui, "This photo uses it.");
        return None;
    }
    if let Some(why) = &offered.failure {
        warning(ui, &format!("The download failed: {why}"));
    }
    let another = offer.running.is_some();
    match &offered.status {
        Status::Installed(_) => {
            note(ui, "It is in your profile library.");
            ui.add_space(space::S);
            primary(ui, "Apply", true)
                .on_hover_text("Use this installed profile for this photo")
                .clicked()
                .then_some(OfferAction::Apply(p.id.as_str()))
        }
        Status::Conflict => {
            warning(
                ui,
                &format!(
                    "A different file named “{}” is in your profile library. Move or rename it to download this one.",
                    p.filename
                ),
            );
            None
        }
        Status::Available => {
            note(
                ui,
                if another {
                    "Another profile is downloading; this one can follow when it finishes."
                } else {
                    AVAILABLE
                },
            );
            ui.add_space(space::S);
            let label = if offered.failure.is_some() {
                "Retry".to_string()
            } else {
                "Download & Apply".to_string()
            };
            primary(ui, &label, !another)
                .on_hover_text(
                    "Downloads this exact file, checks it, adds it to your profile library and uses it for this photo",
                )
                .clicked()
                .then_some(OfferAction::Download(p.id.as_str()))
        }
    }
}

/// The profile's name. The band's label is drawn at this line's top in the same
/// size, so the two read as one line.
fn heading(ui: &mut egui::Ui, offered: &Offered) {
    let palette = theme::palette(ui.ctx());
    ui.add(
        egui::Label::new(
            egui::RichText::new(&offered.entry.profile.name)
                .size(text::SMALL)
                .strong()
                .color(palette.tone(Tone::STRONG)),
        )
        .wrap(),
    );
}

/// Who made it, linked to the exact file at its pinned revision, then the terms
/// the file's notice states (CC0 linked to its deed; the notice itself in the
/// hover text) and the file's size: "by RawTherapee contributors", "Public
/// domain · 66 KB".
fn credit(ui: &mut egui::Ui, offered: &Offered) {
    let entry = offered.entry;
    let palette = theme::palette(ui.ctx());
    let muted = |text: &str| {
        egui::RichText::new(text)
            .size(text::SMALL)
            .color(palette.tone(Tone::MUTED))
    };
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.;
        ui.label(muted("by "));
        ui.hyperlink_to(
            egui::RichText::new(&entry.source.credit).size(text::SMALL),
            entry.page(),
        )
        .on_hover_text(format!(
            "{} from {} at revision {}",
            entry.profile.file,
            entry.source.name,
            &entry.source.revision[..8]
        ));
    });
    let terms = entry.profile.terms;
    let notice = format!("The file's own notice: “{}”", entry.profile.notice);
    // The download's size here, so the button's label stays on one line.
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.;
        match terms.url() {
            Some(url) => ui.hyperlink_to(egui::RichText::new(terms.label()).size(text::SMALL), url),
            None => ui.label(muted(terms.label())),
        }
        .on_hover_text(notice);
        ui.label(muted(&format!(" · {}", size(entry.profile.size))));
    });
}

/// A running download: its bar, "23 of 66 KB" and Cancel. Returns whether Cancel
/// was clicked.
fn downloading(ui: &mut egui::Ui, done: u64, total: u64) -> bool {
    let palette = theme::palette(ui.ctx());
    ui.add(
        egui::ProgressBar::new(if total > 0 {
            done as f32 / total as f32
        } else {
            0.
        })
        .desired_width(ui.available_width())
        .desired_height(space::XS),
    );
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!("{} of {}", size(done), size(total)))
                .size(text::SMALL)
                .color(palette.tone(Tone::MUTED)),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            quiet(ui, "Cancel").clicked()
        })
        .inner
    })
    .inner
}

/// The offer's one action, in the accent colour across the card's width.
fn primary(ui: &mut egui::Ui, label: &str, enabled: bool) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    let fill = if enabled {
        palette.accent()
    } else {
        palette.tone(Tone::RAISED)
    };
    // Justified and centred: the button spans the card, its label in the middle.
    ui.vertical_centered_justified(|ui| {
        ui.add_enabled(
            enabled,
            egui::Button::new(
                egui::RichText::new(label)
                    .size(text::SMALL)
                    .color(palette.on_accent()),
            )
            .fill(fill)
            .corner_radius(radius::CONTROL)
            .min_size(egui::vec2(0., size::FIELD)),
        )
    })
    .inner
}

/// A quiet text button: hides the offer for this camera, also after a restart.
fn dismiss(ui: &mut egui::Ui) -> bool {
    quiet(ui, "Dismiss")
        .on_hover_text(
            "Hide this offer for this camera. Profile › Show community profiles for this camera brings it back.",
        )
        .clicked()
}

/// A secondary action as text without a frame: Dismiss, Cancel.
fn quiet(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    ui.add(
        egui::Button::new(
            egui::RichText::new(label)
                .size(text::SMALL)
                .color(palette.tone(Tone::SECONDARY)),
        )
        .frame(false),
    )
}

/// A size as the offer gives it: "65 KB", "1.1 MB".
fn size(bytes: u64) -> String {
    if bytes < 1_000_000 {
        format!("{} KB", bytes.div_ceil(1000))
    } else {
        format!("{:.1} MB", bytes as f64 / 1e6)
    }
}
