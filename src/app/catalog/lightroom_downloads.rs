//! Download from Lightroom: choosing the albums of the Lightroom library a
//! catalog was imported from whose originals, in Adobe's cloud, Lightroom
//! is to download, and having it do so (`catalog::lightroom::downloads`).
//!
//! The library is read and written only while Lightroom has it closed; both
//! run in a task of their own, as the copy of its catalog takes a few
//! seconds. Lightroom is then opened, and it downloads the originals to
//! where the photos wait for them; the Library shows each as it arrives.
use crate::app::Editor;
use crate::app::ui::design::{self, Tone, button, inset, radius, space, text};
use crate::app::ui::theme;
use crate::app::ui::widgets::{modal_frame, primary_button};
use crate::app::worker::Event;
use crate::catalog::lightroom::{LibraryAlbum, Marked};
use eframe::egui::{self, Color32, Sense, Vec2};
use std::collections::HashSet;

/// The library's albums as read for the dialog: with the catalog's photos in
/// the cloud and their bytes, and the bytes free on the library's disk, where
/// Lightroom downloads them (None when that can't be told).
pub(crate) struct Albums {
    albums: Vec<LibraryAlbum>,
    in_cloud: (usize, u64),
    free: Option<u64>,
}

/// Room left on the library's disk after a download, beyond the 2 GB
/// Lightroom keeps free itself: the disk is the user's system disk too.
const SPARE: u64 = 10_000_000_000;

/// The albums to choose from, by their folders, and those chosen.
pub(crate) struct DownloadDialog {
    albums: Vec<Choice>,
    /// The albums of each folder (the top level first, "" for none), in order.
    groups: Vec<(String, Vec<usize>)>,
    chosen: HashSet<usize>,
    /// The catalog's photos here, and in the cloud with their originals' bytes.
    here: usize,
    in_cloud: (usize, u64),
    /// The bytes free on the library's disk, where Lightroom downloads.
    free: Option<u64>,
}

/// An album as listed, under its folder's path ("2. Travel / Iceland").
struct Choice {
    album: LibraryAlbum,
    folder: String,
}

/// What the dialog was closed with.
enum Close {
    Cancel,
    Download,
}

/// The group of the photos in no album.
const NOT_IN_AN_ALBUM: &str = "Not in an album";

/// The dialog's width, and a row's height.
const WIDTH: f32 = 560.;
const ROW: f32 = 30.;

impl Editor {
    /// Reads the albums of the Lightroom library this catalog was imported
    /// from and shows them to choose from.
    pub(in crate::app) fn open_lightroom_downloads(&mut self) {
        if !self.ready_for_catalog() || !self.activity.begin_dialog() {
            self.messages
                .warn("Download from Lightroom waits for the work in progress");
            return;
        }
        let Some(library) = &self.library else {
            self.activity.finish_dialog();
            return;
        };
        let catalog = library.session.catalog().location().clone();
        let in_cloud = library.session.cloud_photos().len();
        let in_cloud_bytes: u64 = library
            .session
            .cloud_photos()
            .values()
            .map(|c| c.size)
            .sum();
        let ctx = self.context.clone();
        let _ = self.tx.send(Event::CatalogWorking(
            "Reading the Lightroom library's albums…".into(),
        ));
        crate::app::task::spawn(
            self.tx.clone(),
            ctx,
            move |tx| {
                let result = crate::catalog::Catalog::open(&catalog)
                    .and_then(|mut c| c.lightroom_library_albums())
                    .map(|albums| {
                        // The space Lightroom downloads into: the library's disk.
                        let free = albums.first().and_then(|a| {
                            rawmakase_fs::volume_facts(&a.account)
                                .ok()
                                .map(|f| f.available)
                        });
                        Box::new(Albums {
                            albums,
                            in_cloud: (in_cloud, in_cloud_bytes),
                            free,
                        })
                    })
                    .map_err(|e| format!("{e:#}"));
                let _ = tx.send(Event::LightroomAlbums(result));
            },
            |tx, error| {
                let _ = tx.send(Event::LightroomAlbums(Err(error)));
            },
        );
    }

    /// Shows the albums read, or why they couldn't be.
    pub(in crate::app) fn lightroom_albums(&mut self, result: Result<Box<Albums>, String>) {
        self.catalog_work = None;
        self.activity.finish_dialog();
        match result {
            Ok(read) => {
                let Albums {
                    albums,
                    in_cloud,
                    free,
                } = *read;
                // The catalog's other photos are here.
                let here = self.library.as_ref().map_or(0, |l| {
                    let masters = l.session.photos().iter().filter(|p| p.master.is_none());
                    masters.count().saturating_sub(in_cloud.0)
                });
                let mut dialog = DownloadDialog::new(albums, here, in_cloud);
                dialog.free = free;
                self.modal = Some(crate::app::Modal::LightroomDownload(Box::new(dialog)));
            }
            Err(e) => self.messages.warn(format!("Download from Lightroom: {e}")),
        }
    }

    pub(in crate::app) fn lightroom_download_window(&mut self, ctx: &egui::Context) {
        let Some(crate::app::Modal::LightroomDownload(dialog)) = &mut self.modal else {
            return;
        };
        let palette = theme::palette(ctx);
        let mut close = None;
        let response = egui::Modal::new(egui::Id::new("lightroom-download"))
            .backdrop_color(Color32::from_black_alpha(140))
            .frame(modal_frame(&palette).inner_margin(design::margin_same(inset::DIALOG)))
            .show(ctx, |ui| {
                ui.set_width(WIDTH);
                ui.label(
                    egui::RichText::new("Bring Your Photos from Adobe's Cloud")
                        .size(text::TITLE)
                        .color(palette.tone(Tone::STRONG)),
                );
                ui.add_space(space::S);
                ui.label(
                    egui::RichText::new(
                        "Choose albums for Lightroom to download. RAWmakase marks them, as \
                         Store Album Locally does, and opens Lightroom: keep it open, and \
                         each photo appears here as it arrives.",
                    )
                    .size(text::LABEL)
                    .color(palette.tone(Tone::SECONDARY)),
                );
                ui.add_space(space::L);
                figures(ui, &palette, dialog);
                ui.add_space(space::L);
                let height = (ctx.content_rect().height() * 0.85 - 300.).clamp(140., 420.);
                egui::Frame::new()
                    .fill(palette.tone(Tone::CARD))
                    .stroke(egui::Stroke::new(1., palette.tone(Tone::RAISED)))
                    .corner_radius(radius::CARD)
                    .inner_margin(design::margin(space::XS, space::XS))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        egui::ScrollArea::vertical()
                            .max_height(height)
                            .auto_shrink([false, true])
                            .show(ui, |ui| album_list(ui, &palette, dialog));
                    });
                ui.add_space(space::XL);
                let (photos, bytes) = dialog.totals();
                let fits = dialog.fits();
                if let Err((needed, free)) = fits {
                    ui.label(
                        egui::RichText::new(format!(
                            "Too much for the library's disk: {} needed, {} free. Lightroom \
                             downloads into its library; choose fewer albums, or move the \
                             library to a larger disk in Lightroom first.",
                            size(needed),
                            size(free)
                        ))
                        .size(text::LABEL)
                        .color(Color32::from_rgb(220, 120, 90)),
                    );
                    ui.add_space(space::S);
                }
                // One line, the summary centred on the buttons.
                let line = Vec2::new(ui.available_width(), 34.);
                ui.allocate_ui_with_layout(
                    line,
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.spacing_mut().button_padding = button::DIALOG;
                        let albums = dialog
                            .chosen
                            .iter()
                            .filter(|&&i| !dialog.albums[i].album.loose)
                            .count();
                        let loose = albums < dialog.chosen.len();
                        let summary = match (albums, loose) {
                            (0, false) => "No albums chosen".to_string(),
                            (n, loose) => {
                                let what = match (n, loose) {
                                    (0, _) => "Photos in no album".to_string(),
                                    (n, false) => plural(n, "album", "albums"),
                                    (n, true) => format!(
                                        "{} and photos in no album",
                                        plural(n, "album", "albums")
                                    ),
                                };
                                format!(
                                    "{what} · {} · {}",
                                    plural(photos, "photo", "photos"),
                                    size(bytes)
                                )
                            }
                        };
                        ui.label(
                            egui::RichText::new(summary)
                                .size(text::LABEL)
                                .color(palette.tone(Tone::SOFT)),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let download = ui.add_enabled_ui(photos > 0 && fits.is_ok(), |ui| {
                                primary_button(ui, "Download")
                            });
                            if download.inner.clicked() {
                                close = Some(Close::Download);
                            }
                            if ui.button("Not Now").clicked() {
                                close = Some(Close::Cancel);
                            }
                        });
                    },
                );
            });
        if response.should_close() && close.is_none() {
            close = Some(Close::Cancel);
        }
        match close {
            None => {}
            Some(Close::Cancel) => self.modal = None,
            Some(Close::Download) => {
                let Some(crate::app::Modal::LightroomDownload(dialog)) = self.modal.take() else {
                    return;
                };
                let chosen: Vec<LibraryAlbum> = dialog
                    .chosen
                    .iter()
                    .map(|&i| dialog.albums[i].album.clone())
                    .collect();
                self.mark_lightroom_albums(chosen);
            }
        }
    }

    /// Marks `albums` in Lightroom's library and opens Lightroom to download
    /// them.
    fn mark_lightroom_albums(&mut self, albums: Vec<LibraryAlbum>) {
        if !self.activity.begin_dialog() {
            self.messages
                .warn("Download from Lightroom waits for the work in progress");
            return;
        }
        let (photos, bytes) = totals(albums.iter());
        let _ = self.tx.send(Event::CatalogWorking(
            "Marking the albums in Lightroom's library…".into(),
        ));
        crate::app::task::spawn(
            self.tx.clone(),
            self.context.clone(),
            move |tx| {
                let backups = crate::storage::data_dir().join("Lightroom backups");
                let result = crate::catalog::lightroom::mark_albums_for_download(&albums, &backups)
                    .map(|marked| {
                        let opened = crate::platform::lightroom::open().is_ok();
                        (marked, opened, photos, bytes)
                    })
                    .map_err(|e| format!("{e:#}"));
                let _ = tx.send(Event::LightroomMarked(result));
            },
            |tx, error| {
                let _ = tx.send(Event::LightroomMarked(Err(error)));
            },
        );
    }

    /// Says what marking the albums did.
    pub(in crate::app) fn lightroom_marked(
        &mut self,
        result: Result<(Marked, bool, usize, u64), String>,
    ) {
        self.catalog_work = None;
        self.activity.finish_dialog();
        match result {
            Ok((marked, opened, photos, bytes)) => {
                let mut line = format!(
                    "Lightroom will download {} ({})",
                    plural(photos, "photo", "photos"),
                    size(bytes)
                );
                line.push_str(if opened {
                    "; keep it open until they are here"
                } else {
                    ": open Lightroom and keep it open until they are here"
                });
                if marked.new_album.is_some() {
                    line.push_str(&format!(
                        " · those in no album are in album \"{}\"",
                        crate::catalog::lightroom::NEW_ALBUM
                    ));
                }
                if marked.unsettled > 0 {
                    line.push_str(&format!(
                        " · {} photos Lightroom hasn't finished syncing were left out",
                        marked.unsettled
                    ));
                }
                self.messages.note(line);
            }
            Err(e) => self.messages.warn(format!("Download from Lightroom: {e}")),
        }
    }
}

impl DownloadDialog {
    fn new(albums: Vec<LibraryAlbum>, here: usize, in_cloud: (usize, u64)) -> Self {
        let names: std::collections::HashMap<&str, (&str, Option<&str>)> = albums
            .iter()
            .map(|a| (a.id.as_str(), (a.name.as_str(), a.parent.as_deref())))
            .collect();
        // The path of the folders an album is in, "" for none.
        let folder = |album: &LibraryAlbum| {
            let mut parts = Vec::new();
            let mut parent = album.parent.as_deref();
            // Bounded, for folders that are each other's parents.
            for _ in 0..16 {
                let Some((name, up)) = parent.and_then(|p| names.get(p)) else {
                    break;
                };
                parts.push(*name);
                parent = *up;
            }
            parts.reverse();
            parts.join(" / ")
        };
        // An album with no photos here is nothing to choose.
        let mut choices: Vec<Choice> = albums
            .iter()
            .filter(|a| !a.set && a.photos > 0)
            .map(|a| Choice {
                folder: if a.loose {
                    NOT_IN_AN_ALBUM.into()
                } else {
                    folder(a)
                },
                album: a.clone(),
            })
            .collect();
        choices.sort_by_key(|c| {
            (
                c.album.loose,
                c.folder.to_lowercase(),
                c.album.name.to_lowercase(),
            )
        });
        let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
        for (i, choice) in choices.iter().enumerate() {
            match groups.last_mut() {
                Some((folder, members)) if *folder == choice.folder => members.push(i),
                _ => groups.push((choice.folder.clone(), vec![i])),
            }
        }
        // Albums in no folder after the folders, as a group of their own,
        // and photos in no album last.
        groups.sort_by_key(|(folder, _)| (folder == NOT_IN_AN_ALBUM, folder.is_empty()));
        Self {
            albums: choices,
            groups,
            chosen: HashSet::new(),
            here,
            in_cloud,
            free: None,
        }
    }

    /// The photos still in the cloud of the albums chosen, and their bytes;
    /// a photo in several of them once.
    fn totals(&self) -> (usize, u64) {
        totals(self.chosen.iter().map(|&i| &self.albums[i].album))
    }

    /// Whether the photos chosen fit on the library's disk, with room to
    /// spare; and if not, what they need and what is free.
    fn fits(&self) -> Result<(), (u64, u64)> {
        let (_, bytes) = self.totals();
        match self.free {
            Some(free) if bytes.saturating_add(SPARE) > free => Err((bytes, free)),
            _ => Ok(()),
        }
    }

    /// Whether album `i` can be chosen: it has photos in the cloud.
    fn choosable(&self, i: usize) -> bool {
        self.albums[i].album.in_cloud > 0
    }
}

/// What is here and what is in the cloud, side by side.
fn figures(ui: &mut egui::Ui, palette: &theme::Palette, dialog: &DownloadDialog) {
    let (in_cloud, bytes) = dialog.in_cloud;
    ui.columns(2, |columns| {
        for (ui, (title, value, detail)) in columns.iter_mut().zip([
            ("On this computer", count(dialog.here), "photos".to_string()),
            (
                "In Adobe's cloud",
                count(in_cloud),
                format!("photos · {}", size(bytes)),
            ),
        ]) {
            egui::Frame::new()
                .fill(palette.tone(Tone::WELL))
                .corner_radius(radius::CARD)
                .inner_margin(design::margin(space::L, space::M))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(
                        egui::RichText::new(title)
                            .size(text::SMALL)
                            .color(palette.tone(Tone::MUTED)),
                    );
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(value)
                                .size(text::HERO)
                                .color(palette.tone(Tone::STRONG)),
                        );
                        ui.label(
                            egui::RichText::new(detail)
                                .size(text::LABEL)
                                .color(palette.tone(Tone::SECONDARY)),
                        );
                    });
                });
        }
    });
}

/// The albums by folder: each folder a heading that chooses all of its
/// albums, each album a row with what of it is in the cloud.
fn album_list(ui: &mut egui::Ui, palette: &theme::Palette, dialog: &mut DownloadDialog) {
    ui.spacing_mut().item_spacing.y = 0.;
    let groups = dialog.groups.clone();
    for (n, (folder, members)) in groups.iter().enumerate() {
        if n > 0 {
            ui.add_space(space::S);
        }
        let choosable: Vec<usize> = members
            .iter()
            .copied()
            .filter(|&i| dialog.choosable(i))
            .collect();
        let chosen = choosable
            .iter()
            .filter(|i| dialog.chosen.contains(i))
            .count();
        let title = if folder.is_empty() { "Albums" } else { folder };
        folder_row(ui, palette, title, chosen, &choosable, &mut dialog.chosen);
        for &i in members {
            album_row(ui, palette, dialog, i);
        }
    }
}

/// How far a check is checked.
#[derive(Clone, Copy, PartialEq)]
enum Check {
    Off,
    Some,
    On,
}

/// A check centred at `at`, as the album rows and folder headings share it;
/// returns where it is.
fn draw_check(
    painter: &egui::Painter,
    palette: &theme::Palette,
    at: egui::Pos2,
    check: Check,
    enabled: bool,
) -> egui::Rect {
    let rect = egui::Rect::from_center_size(at, Vec2::splat(14.));
    let (fill, stroke) = match (enabled, check) {
        (true, Check::Off) => (
            palette.tone(Tone::FIELD),
            palette.tone(Tone::OUTLINE_STRONG),
        ),
        (true, _) => (palette.accent(), palette.accent()),
        (false, _) => (palette.tone(Tone::CARD), palette.tone(Tone::OUTLINE)),
    };
    painter.rect(
        rect,
        radius::SMALL,
        fill,
        egui::Stroke::new(1., stroke),
        egui::StrokeKind::Inside,
    );
    let c = rect.center();
    let mark = egui::Stroke::new(1.6, palette.on_accent());
    match check {
        Check::On => {
            painter.line(
                vec![
                    c + Vec2::new(-3.5, 0.),
                    c + Vec2::new(-1., 2.5),
                    c + Vec2::new(3.5, -3.),
                ],
                mark,
            );
        }
        Check::Some => {
            painter.line_segment([c + Vec2::new(-3.5, 0.), c + Vec2::new(3.5, 0.)], mark);
        }
        Check::Off => {}
    }
    rect
}

/// A folder's heading, whose check chooses all of its albums that can be
/// chosen (`choosable`), `chosen` of which are.
fn folder_row(
    ui: &mut egui::Ui,
    palette: &theme::Palette,
    title: &str,
    chosen: usize,
    choosable: &[usize],
    chosen_set: &mut HashSet<usize>,
) {
    let enabled = !choosable.is_empty();
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), ROW),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let painter = ui.painter_at(rect);
    if enabled && response.hovered() {
        painter.rect_filled(
            rect.shrink2(Vec2::new(0., 1.)),
            radius::CONTROL,
            palette.tone(Tone::RAISED),
        );
    }
    let state = match chosen {
        0 => Check::Off,
        n if n == choosable.len() => Check::On,
        _ => Check::Some,
    };
    let check = draw_check(
        &painter,
        palette,
        egui::pos2(rect.left() + space::M + 7., rect.center().y),
        state,
        enabled,
    );
    let ink = palette.tone(if enabled { Tone::SOFT } else { Tone::FAINT });
    let galley = painter.layout_no_wrap(
        title.to_string(),
        egui::FontId::proportional(text::LABEL),
        ink,
    );
    painter.galley(
        egui::pos2(
            check.right() + space::M,
            rect.center().y - galley.size().y / 2.,
        ),
        galley,
        ink,
    );
    if response.clicked() {
        let all = state != Check::On;
        for i in choosable {
            if all {
                chosen_set.insert(*i);
            } else {
                chosen_set.remove(i);
            }
        }
    }
    if enabled {
        response.on_hover_cursor(egui::CursorIcon::PointingHand);
    }
}

/// One album: a check, its name, and its photos in the cloud and their size
/// (or that it is all here, or downloading); the whole row toggles it.
fn album_row(ui: &mut egui::Ui, palette: &theme::Palette, dialog: &mut DownloadDialog, i: usize) {
    let choosable = dialog.choosable(i);
    let album = &dialog.albums[i].album;
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), ROW),
        if choosable {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let on = dialog.chosen.contains(&i);
    let painter = ui.painter_at(rect);
    if choosable && (response.hovered() || on) {
        painter.rect_filled(
            rect.shrink2(Vec2::new(0., 1.)),
            radius::CONTROL,
            palette.tone(if response.hovered() {
                Tone::RAISED
            } else {
                Tone::PANEL
            }),
        );
    }
    let check = draw_check(
        &painter,
        palette,
        egui::pos2(rect.left() + space::XXL + 2., rect.center().y),
        if on { Check::On } else { Check::Off },
        choosable,
    );
    let ink = palette.tone(if choosable { Tone::TEXT } else { Tone::FAINT });
    let name_left = check.right() + space::M;
    let detail = status(album);
    let detail_galley = painter.layout_no_wrap(
        detail,
        egui::FontId::proportional(text::LABEL),
        palette.tone(if album.marked && album.in_cloud > 0 {
            Tone::SOFT
        } else {
            Tone::MUTED
        }),
    );
    let detail_left = rect.right() - space::M - detail_galley.size().x;
    let name = painter.layout_no_wrap(
        album.name.clone(),
        egui::FontId::proportional(text::BODY),
        ink,
    );
    painter
        .with_clip_rect(egui::Rect::from_x_y_ranges(
            name_left..=detail_left - space::L,
            rect.y_range(),
        ))
        .galley(
            egui::pos2(name_left, rect.center().y - name.size().y / 2.),
            name,
            ink,
        );
    painter.galley(
        egui::pos2(detail_left, rect.center().y - detail_galley.size().y / 2.),
        detail_galley,
        ink,
    );
    if response.clicked() {
        if on {
            dialog.chosen.remove(&i);
        } else {
            dialog.chosen.insert(i);
        }
    }
    let response = if choosable {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    };
    if dialog.albums[i].album.loose {
        response.on_hover_text(format!(
            "Lightroom keeps only albums on this computer: RAWmakase makes an album, \
             \"{}\", of these photos for Lightroom to download.",
            crate::catalog::lightroom::NEW_ALBUM
        ));
    }
}

/// The photos in the cloud of `albums` and their bytes, each photo once.
fn totals<'a>(albums: impl Iterator<Item = &'a LibraryAlbum>) -> (usize, u64) {
    let mut seen = HashSet::new();
    let (mut photos, mut bytes) = (0, 0);
    for album in albums {
        for (asset, size) in &album.in_cloud_assets {
            if seen.insert(asset.as_str()) {
                photos += 1;
                bytes += size;
            }
        }
    }
    (photos, bytes)
}

/// "1,330 photos · 31.0 GB", "All here", "Downloading · 30 photos".
fn status(album: &LibraryAlbum) -> String {
    match (album.in_cloud, album.marked) {
        (0, _) => "All here".into(),
        (n, true) => format!(
            "Downloading · {} · {}",
            plural(n, "photo", "photos"),
            size(album.in_cloud_bytes)
        ),
        (n, false) => format!(
            "{} · {}",
            plural(n, "photo", "photos"),
            size(album.in_cloud_bytes)
        ),
    }
}

/// `n` with thousands separated: "22,951".
fn count(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, d) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(d);
    }
    out
}

/// "1 photo", "1,330 photos".
fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{} {}", count(n), if n == 1 { one } else { many })
}

/// `n` bytes in decimal units, as a file browser shows them.
fn size(n: u64) -> String {
    let n = n as f64;
    match n {
        n if n >= 1e12 => format!("{:.1} TB", n / 1e12),
        n if n >= 1e9 => format!("{:.1} GB", n / 1e9),
        n if n >= 1e6 => format!("{:.0} MB", n / 1e6),
        n => format!("{:.0} KB", n / 1e3),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn album(id: &str, parent: Option<&str>, set: bool, cloud: &[(&str, u64)]) -> LibraryAlbum {
        LibraryAlbum {
            account: "acc".into(),
            id: id.into(),
            name: id.to_uppercase(),
            parent: parent.map(String::from),
            set,
            marked: false,
            photos: cloud.len() + 1,
            in_cloud: cloud.len(),
            in_cloud_bytes: cloud.iter().map(|(_, s)| s).sum(),
            in_cloud_assets: cloud.iter().map(|(a, s)| (a.to_string(), *s)).collect(),
            loose: false,
        }
    }

    #[test]
    fn albums_are_listed_by_their_folders_and_a_photo_in_two_counts_once() {
        let mut empty = album("empty", None, false, &[]);
        empty.photos = 0;
        let mut loose = album("loose", None, false, &[("c", 5_000_000)]);
        loose.loose = true;
        let albums = vec![
            album("travel", None, true, &[]),
            album(
                "iceland",
                Some("travel"),
                false,
                &[("a", 10_000_000), ("b", 20_000_000)],
            ),
            album("best", None, false, &[("b", 20_000_000)]),
            album("done", None, false, &[]),
            empty,
            loose,
        ];
        let mut dialog = DownloadDialog::new(albums, 7, (4, 45_000_000));
        let groups: Vec<(&str, Vec<&str>)> = dialog
            .groups
            .iter()
            .map(|(folder, members)| {
                let names = members
                    .iter()
                    .map(|&i| dialog.albums[i].album.id.as_str())
                    .collect();
                (folder.as_str(), names)
            })
            .collect();
        // Folders first, then albums in none, then the photos in no album; an
        // empty album isn't listed.
        assert_eq!(
            groups,
            [
                ("TRAVEL", vec!["iceland"]),
                ("", vec!["best", "done"]),
                (NOT_IN_AN_ALBUM, vec!["loose"])
            ]
        );
        let at = |id: &str| dialog.albums.iter().position(|c| c.album.id == id).unwrap();
        dialog.chosen = [at("iceland"), at("best")].into();
        assert_eq!(dialog.totals(), (2, 30_000_000));
        dialog.chosen.insert(at("loose"));
        assert_eq!(dialog.totals(), (3, 35_000_000));
        assert!(!dialog.choosable(at("done")));
        assert_eq!(status(&dialog.albums[at("done")].album), "All here");
        assert_eq!(
            status(&dialog.albums[at("iceland")].album),
            "2 photos · 30 MB"
        );
        assert_eq!(count(22951), "22,951");
        // What doesn't fit on the library's disk, with room to spare, can't
        // be downloaded; nor what fits only without it.
        assert_eq!(dialog.fits(), Ok(()), "the disk unknown");
        dialog.free = Some(SPARE + 35_000_000);
        assert_eq!(dialog.fits(), Ok(()));
        dialog.free = Some(SPARE + 34_000_000);
        assert_eq!(dialog.fits(), Err((35_000_000, SPARE + 34_000_000)));
        // 326 GB of photos in no album on a disk with 232 GB free.
        let mut big = album("big", None, false, &[("x", 326_000_000_000)]);
        big.loose = true;
        let mut dialog = DownloadDialog::new(vec![big], 977, (1, 326_000_000_000));
        dialog.free = Some(232_000_000_000);
        dialog.chosen.insert(0);
        assert!(dialog.fits().is_err());
        assert_eq!(count(999), "999");
    }
}
