//! The backdrop behind the photo in Develop and the Library's Loupe: the
//! theme's own color, or one of Lightroom's six shades from white to black
//! for judging a high- or low-key photo against another surround. It is
//! chosen from the menu a right click on the backdrop opens, or in
//! Preferences, and kept in the session. The toolbars under the photo,
//! Compare, Survey and the grid keep the theme's color.
use crate::app::ui::design::Tone;
use crate::app::ui::theme::Palette;
use crate::app::ui::widgets::{context_menu, menu_item, menu_separator, menu_width};
use eframe::egui::{self, Color32, Rect, Sense};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Backdrop {
    /// The palette's photo backdrop, which follows the theme.
    #[default]
    Default,
    White,
    LightGray,
    MediumGray,
    DarkGray,
    DarkerGray,
    Black,
}

impl Backdrop {
    /// In the menu's order: the default, then Lightroom's shades from light to
    /// dark.
    pub(in crate::app) const ALL: [Self; 7] = [
        Self::Default,
        Self::White,
        Self::LightGray,
        Self::MediumGray,
        Self::DarkGray,
        Self::DarkerGray,
        Self::Black,
    ];

    /// The name the session keeps; the default keeps none.
    pub(in crate::app) fn key(self) -> Option<&'static str> {
        match self {
            Self::Default => None,
            Self::White => Some("white"),
            Self::LightGray => Some("light-gray"),
            Self::MediumGray => Some("medium-gray"),
            Self::DarkGray => Some("dark-gray"),
            Self::DarkerGray => Some("darker-gray"),
            Self::Black => Some("black"),
        }
    }
    /// A name this release doesn't know reads as the default.
    pub(in crate::app) fn from_key(key: Option<&str>) -> Self {
        Self::ALL
            .into_iter()
            .find(|b| b.key().is_some() && b.key() == key)
            .unwrap_or_default()
    }
    pub(in crate::app) fn label(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::White => "White",
            Self::LightGray => "Light Gray",
            Self::MediumGray => "Medium Gray",
            Self::DarkGray => "Dark Gray",
            Self::DarkerGray => "Darker Gray",
            Self::Black => "Black",
        }
    }
    /// The color drawn behind the photo with `palette`, sRGB-encoded. The
    /// shades are the grey levels Lightroom Classic 15's interface code fills
    /// with: 1, 0.82, 0.5, 0.25, 0.15 and 0.
    pub(in crate::app) fn fill(self, palette: &Palette) -> Color32 {
        match self {
            Self::Default => palette.photo_backdrop(),
            Self::White => Color32::WHITE,
            Self::LightGray => Color32::from_gray(209),
            Self::MediumGray => Color32::from_gray(128),
            Self::DarkGray => Color32::from_gray(64),
            Self::DarkerGray => Color32::from_gray(38),
            Self::Black => Color32::BLACK,
        }
    }
    /// Text drawn straight on the backdrop, such as "Loading…": the theme's
    /// `tone` on its own color, else a grey that reads on the shade whatever
    /// the theme, dark on the lighter shades and light on the darker ones.
    pub(in crate::app) fn text(self, palette: &Palette, tone: Tone) -> Color32 {
        match self {
            Self::Default => palette.tone(tone),
            Self::White | Self::LightGray | Self::MediumGray => Color32::from_gray(40),
            Self::DarkGray | Self::DarkerGray | Self::Black => Color32::from_gray(190),
        }
    }
}

/// Opens the Background Color menu on a right click (or Control-click on
/// macOS) on `response`, with `current` checked; returns the color chosen.
pub(in crate::app) fn menu(response: &egui::Response, current: Backdrop) -> Option<Backdrop> {
    let mut chosen = None;
    context_menu(response, |ui| {
        menu_width(ui, 180.);
        ui.spacing_mut().item_spacing.y = 0.;
        // A heading, greyed out as Lightroom's is.
        menu_item(ui, "Background Color", "", false, false);
        for backdrop in Backdrop::ALL {
            if menu_item(ui, backdrop.label(), "", true, backdrop == current) {
                chosen = Some(backdrop);
                ui.close();
            }
            if backdrop == Backdrop::Default {
                menu_separator(ui);
            }
        }
    });
    chosen.filter(|c| *c != current)
}

impl crate::app::Editor {
    /// Fills `outer`, a photo view's whole area, with the backdrop, and opens
    /// the backdrop's menu on a right click anywhere in it that the photo's own
    /// view, drawn over it later, doesn't take. Returns the area inside the
    /// photo's margin, where the photo goes.
    pub(in crate::app) fn backdrop_stage(&mut self, ui: &mut egui::Ui, outer: Rect) -> Rect {
        let palette = crate::app::ui::theme::palette(ui.ctx());
        ui.painter()
            .rect_filled(outer, 0., self.view.backdrop.fill(&palette));
        // Sensed before the photo's view so that, drawn over it, the view keeps
        // its own clicks.
        let stage = ui.interact(outer, ui.id().with("backdrop-stage"), Sense::click());
        self.backdrop_menu(&stage);
        outer.shrink(crate::app::shell::workspace::PHOTO_MARGIN)
    }
    /// Opens the backdrop's menu from `response` and keeps the color chosen.
    pub(in crate::app) fn backdrop_menu(&mut self, response: &egui::Response) {
        if let Some(chosen) = menu(response, self.view.backdrop) {
            self.set_backdrop(chosen);
            response.ctx.request_repaint();
        }
    }
    /// Shows `backdrop` behind the photo from now on, and saves it.
    pub(in crate::app) fn set_backdrop(&mut self, backdrop: Backdrop) {
        if backdrop != self.view.backdrop {
            self.view.backdrop = backdrop;
            let _ = self.save_session();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_read_back_and_unknown_names_read_as_the_default() {
        for backdrop in Backdrop::ALL {
            assert_eq!(Backdrop::from_key(backdrop.key()), backdrop);
        }
        assert_eq!(Backdrop::from_key(Some("mauve")), Backdrop::Default);
        assert_eq!(Backdrop::from_key(Some("")), Backdrop::Default);
        assert_eq!(Backdrop::from_key(None), Backdrop::Default);
    }

    #[test]
    fn the_default_follows_the_palette() {
        let palette = Palette::DEFAULT;
        assert_eq!(Backdrop::Default.fill(&palette), palette.photo_backdrop());
        assert_eq!(Backdrop::MediumGray.fill(&palette), Color32::from_gray(128));
    }

    #[test]
    fn text_on_a_shade_reads_on_it() {
        let palette = Palette::DEFAULT;
        for backdrop in Backdrop::ALL.into_iter().skip(1) {
            let [text, fill] = [
                backdrop.text(&palette, Tone::FAINT),
                backdrop.fill(&palette),
            ]
            .map(|c| i32::from(c.r()));
            assert!((text - fill).abs() >= 80, "{backdrop:?}");
        }
    }
}

#[cfg(test)]
#[path = "backdrop_editor_tests.rs"]
mod editor_tests;
