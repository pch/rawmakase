//! Photo info (camera, lens, exposure, size) for the Metadata panel and the
//! Loupe. Photos imported from Lightroom have it from their catalog; for
//! photos added from folders it is read from the files in the background and
//! kept in the catalog, a row of nothing for a file without any.
use super::Library;
use crate::app::ui::design::Tone;
use crate::app::ui::theme;
use crate::metadata::PhotoInfo;
use eframe::egui;

impl Library {
    /// The active photo's info, read from the catalog once per photo.
    pub(super) fn active_info(&mut self) -> Option<PhotoInfo> {
        let id = self.selection.active?;
        if self.info.as_ref().is_none_or(|(at, _)| *at != id) {
            let info = self.session.catalog().photo_info(id).ok().flatten();
            self.info = Some((id, info));
        }
        self.info.as_ref().and_then(|(_, info)| info.clone())
    }
    /// A grid cell's hover, as Lightroom's: file name, capture time and
    /// dimensions. The info is read from the catalog once per hovered photo.
    pub(super) fn hover_text(&mut self, photo: &crate::catalog::Photo) -> String {
        if self
            .hover_info
            .as_ref()
            .is_none_or(|(id, _)| *id != photo.id)
        {
            let info = self.session.catalog().photo_info(photo.id).ok().flatten();
            self.hover_info = Some((photo.id, info));
        }
        let info = self.hover_info.as_ref().and_then(|(_, info)| info.as_ref());
        [
            Some(format!(
                "{}{}",
                photo.filename,
                super::cell::copy_suffix(photo)
            )),
            Some(photo.capture_text()).filter(|t| !t.is_empty()),
            info.and_then(PhotoInfo::dimensions_text),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n")
    }
    /// An expanded grid cell's details: dimensions and capture date, e.g.
    /// "6000 × 4000 · 29/06/2016". The info is read once per photo.
    pub(super) fn cell_details(&mut self, photo: &crate::catalog::Photo) -> String {
        let catalog = self.session.catalog();
        let info = self
            .cell_info
            .entry(photo.id)
            .or_insert_with(|| catalog.photo_info(photo.id).ok().flatten());
        let date = photo
            .capture_text()
            .get(..10)
            .unwrap_or_default()
            .to_string();
        [
            info.as_ref().and_then(PhotoInfo::dimensions_text),
            Some(date).filter(|d| !d.is_empty()),
            Some(photo.format.clone()).filter(|f| !f.is_empty()),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ")
    }
    /// Reads the info of the photos that have none, unless already reading.
    pub(super) fn start_photo_info(&mut self) {
        if self.reads_suspended {
            return;
        }
        let availability = &self.availability;
        self.session
            .start_photo_info(|path| availability.is_available(path));
    }
    /// Whether photo info is still being read from files.
    pub(in crate::app) fn reading_photo_info(&self) -> bool {
        self.session.reading_photo_info()
    }
    /// Counts each save of info read from files.
    pub(in crate::app) fn photo_info_saves(&self) -> u64 {
        self.info_saves
    }
    /// Saves the info read so far.
    pub(super) fn poll_photo_info(&mut self) {
        let polled = self.session.poll_photo_info();
        if polled.start_again {
            self.start_photo_info();
        }
        if let Some(e) = polled.error {
            self.messages
                .warn(format!("Photo info could not be saved: {e}"));
        }
        if polled.saved > 0 {
            self.info_saves += 1;
            self.info = None;
            self.hover_info = None;
            self.cell_info.clear();
            // Sorted by aspect ratio, the new sizes find their places.
            if self.filters.sort == super::sort::Sort::AspectRatio {
                self.resort_in_place(|library| library.sort_keys = None);
            }
        }
    }
}

/// What the Loupe's Info overlay (I) shows, as Lightroom's Info 1 and 2.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) enum Overlay {
    #[default]
    Off,
    /// File name, capture time and dimensions.
    Info1,
    /// File name, exposure, camera and lens.
    Info2,
}
impl Overlay {
    pub(super) fn next(self) -> Self {
        match self {
            Self::Off => Self::Info1,
            Self::Info1 => Self::Info2,
            Self::Info2 => Self::Off,
        }
    }
}

impl Library {
    /// I: the Loupe's Info overlay goes to Info 1, Info 2, then off. Develop shows
    /// the same one over the photo being edited.
    pub(in crate::app) fn cycle_loupe_info(&mut self) {
        self.loupe_info = self.loupe_info.next();
    }
    /// Which Info overlay shows, by name.
    #[cfg(test)]
    pub(in crate::app) fn loupe_info(&self) -> String {
        format!("{:?}", self.loupe_info)
    }
    /// Draws the Loupe's Info overlay in the top left of `rect`.
    pub(in crate::app) fn loupe_overlay(&mut self, painter: &egui::Painter, rect: egui::Rect) {
        if self.loupe_info == Overlay::Off {
            return;
        }
        let Some(photo) = self.selection.active.and_then(|id| self.photo(id)).cloned() else {
            return;
        };
        let info = self.active_info().unwrap_or_default();
        let name = format!("{}{}", photo.filename, super::cell::copy_suffix(&photo));
        let lines: Vec<String> = match self.loupe_info {
            Overlay::Info1 => vec![
                Some(name),
                Some(photo.capture_text()).filter(|t| !t.is_empty()),
                info.dimensions_text(),
            ],
            _ => vec![
                Some(name),
                Some(
                    [info.exposure_text(), info.focal_text(), info.iso_text()]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(", "),
                )
                .filter(|s| !s.is_empty()),
                Some(
                    [info.camera.clone(), info.lens]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(" · "),
                )
                .filter(|s| !s.is_empty()),
            ],
        }
        .into_iter()
        .flatten()
        .collect();
        let text = theme::palette(painter.ctx()).tone(Tone::STRONG);
        let mut y = rect.top() + 12.;
        for (i, line) in lines.iter().enumerate() {
            let size = if i == 0 { 15. } else { 12. };
            let font = egui::FontId::proportional(size);
            let at = egui::pos2(rect.left() + 14., y);
            // A shadow keeps the text readable on any photo.
            painter.text(
                at + egui::vec2(1., 1.),
                egui::Align2::LEFT_TOP,
                line,
                font.clone(),
                egui::Color32::from_black_alpha(200),
            );
            painter.text(at, egui::Align2::LEFT_TOP, line, font, text);
            y += size + 5.;
        }
    }
}
