//! Develop's backdrop around the photo: its colour (black, Dark Gray, the
//! theme's and the default, three lighter greys or white) and the border it
//! leaves around the photo, both set from the menu a right click on the
//! backdrop (not the photo) opens, as in Lightroom, and kept in the session.
//! The Library, the Navigator and the toolbars keep the theme's backdrop and
//! their own margins.
use super::theme::{self, Palette};
use super::widgets;
use super::{Editor, Module};
use eframe::egui::{self, Color32, Rect};
use std::ops::RangeInclusive;

/// The border Develop leaves between the photo's area and the panels around
/// it, in points, unless the user changes it.
pub(super) const DEFAULT_BORDER: f32 = super::workspace::PHOTO_MARGIN;
/// Up to 100 points; egui's margins stop at 127.
pub(super) const BORDER_RANGE: RangeInclusive<f32> = 0.0..=100.0;
/// The slider snaps to 0, 20, 40, 60, 80 and 100.
const BORDER_STEP: f32 = 20.;

/// `border` in range, at the nearest step.
fn snapped(border: f32) -> f32 {
    ((border / BORDER_STEP).round() * BORDER_STEP).clamp(*BORDER_RANGE.start(), *BORDER_RANGE.end())
}
/// The border the session saved, at the nearest step, or the default when none
/// in range was.
pub(super) fn border_from_session(saved: Option<u32>) -> f32 {
    saved
        .map(|b| b as f32)
        .filter(|b| BORDER_RANGE.contains(b))
        .map_or(DEFAULT_BORDER, snapped)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Backdrop {
    Black,
    /// The default: the palette's photo backdrop, which follows the theme
    /// (Lightroom's dark grey in RAWmakase's own palette).
    #[default]
    DarkGray,
    MediumDarkGray,
    MediumLightGray,
    LightGray,
    White,
}

impl Backdrop {
    /// In the menu's order, darkest first.
    pub(super) const ALL: [Self; 6] = [
        Self::Black,
        Self::DarkGray,
        Self::MediumDarkGray,
        Self::MediumLightGray,
        Self::LightGray,
        Self::White,
    ];
    /// The stable name the session keeps; the default's is empty.
    pub(super) fn key(self) -> &'static str {
        match self {
            Self::Black => "black",
            Self::DarkGray => "",
            Self::MediumDarkGray => "medium-dark-gray",
            Self::MediumLightGray => "medium-light-gray",
            Self::LightGray => "light-gray",
            Self::White => "white",
        }
    }
    /// A name this version does not know reads as the default.
    pub(super) fn from_key(key: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|b| b.key() == key)
            .unwrap_or_default()
    }
    fn label(self) -> &'static str {
        match self {
            Self::Black => "Black",
            Self::DarkGray => "Dark Gray",
            Self::MediumDarkGray => "Medium Dark Gray",
            Self::MediumLightGray => "Medium Light Gray",
            Self::LightGray => "Light Gray",
            Self::White => "White",
        }
    }
    /// The greys are evenly spaced in sRGB, which is close to even to the eye.
    pub(super) fn color(self, palette: &Palette) -> Color32 {
        match self {
            Self::Black => Color32::BLACK,
            Self::DarkGray => palette.photo_backdrop(),
            Self::MediumDarkGray => Color32::from_gray(102),
            Self::MediumLightGray => Color32::from_gray(153),
            Self::LightGray => Color32::from_gray(204),
            Self::White => Color32::WHITE,
        }
    }
}

impl Editor {
    /// Behind the photo: the chosen backdrop in Develop, the theme's in the
    /// Library's Loupe, which shares the viewport.
    pub(super) fn develop_backdrop(&self, ctx: &egui::Context) -> Color32 {
        let palette = theme::palette(ctx);
        if self.module == Module::Develop {
            self.view.backdrop.color(&palette)
        } else {
            palette.photo_backdrop()
        }
    }
    pub(super) fn set_backdrop(&mut self, backdrop: Backdrop) {
        if backdrop != self.view.backdrop {
            self.view.backdrop = backdrop;
            let _ = self.save_session();
        }
    }
    /// The backdrop's menu, opened in Develop by a context click on `response`
    /// that is on none of `photos`: the colours, then the border's slider. A
    /// colour closes it; the slider keeps it open while it is dragged.
    pub(super) fn backdrop_menu(&mut self, response: &egui::Response, photos: &[Rect]) {
        if self.module != Module::Develop {
            return;
        }
        let off_photos = response
            .interact_pointer_pos()
            .is_some_and(|p| !photos.iter().any(|r| r.contains(p)));
        let command = if widgets::context_clicked(response) && off_photos {
            Some(egui::SetOpenCommand::Bool(true))
        } else if response.clicked() {
            Some(egui::SetOpenCommand::Bool(false))
        } else {
            None
        };
        let current = self.view.backdrop;
        let mut chosen = None;
        let mut border = self.view.border;
        egui::Popup::menu(response)
            .open_memory(command)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .at_pointer_fixed()
            .show(|ui| {
                ui.set_width(230.);
                ui.spacing_mut().item_spacing.y = 0.;
                for backdrop in Backdrop::ALL {
                    if widgets::menu_item(ui, backdrop.label(), "", true, backdrop == current) {
                        chosen = Some(backdrop);
                        ui.close();
                    }
                }
                widgets::menu_separator(ui);
                widgets::slider(ui, "Border", &mut border, BORDER_RANGE, DEFAULT_BORDER);
            });
        if let Some(backdrop) = chosen {
            self.set_backdrop(backdrop);
        }
        self.set_border(border);
        // Saved once the drag is let go, not on every step of it.
        if self.view.border_unsaved && !response.ctx.input(|i| i.pointer.any_down()) {
            self.view.border_unsaved = false;
            let _ = self.save_session();
        }
    }
    fn set_border(&mut self, border: f32) {
        let border = snapped(border);
        if border != self.view.border {
            self.view.border = border;
            self.view.border_unsaved = true;
        }
    }
}
