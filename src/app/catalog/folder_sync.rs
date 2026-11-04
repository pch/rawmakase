//! Synchronize Folder (issue #432) in the app: the scan, on a thread of its
//! own while the Library and Develop stay usable, shown in the top bar with
//! a stop button; the review dialog; and applying what the user accepted as
//! a folder change. The photo open in Develop keeps its pending edit: a
//! verified move of its file relinks it in place (see `catalog_ready`).
//!
//! Find on Disk… (issue #693) runs through the same scan slot, review and
//! apply: the user picks an offline photo's file, and the other offline
//! photos of its folder found beside it are offered with it.
use crate::app::Editor;
use crate::app::ui::design::{self, Tone, button, inset, radius, space, text};
use crate::app::ui::icons::Icon;
use crate::app::ui::theme;
use crate::app::worker::Event;
use crate::catalog::folder_sync::{
    Accepted, Applied, PlanKind, SyncPlan, SyncScope, find_on_disk, scan,
};
use crate::catalog::{CatalogLocation, PhotoId};
use eframe::egui;
use rawmakase_cancel::Cancel;
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// The synchronization under way or waiting for review.
#[derive(Default)]
pub(in crate::app) struct FolderSync {
    scan: Option<Scan>,
    review: Option<Review>,
    /// The review dialog is showing.
    showing: bool,
    /// Counts scans, so a result of one stopped or superseded is ignored.
    started: u64,
    /// What is being applied is Find on Disk's, for its messages.
    applying_find: bool,
}

impl FolderSync {
    pub(in crate::app) fn is_scanning(&self) -> bool {
        self.scan.is_some()
    }
    /// Whether the review dialog or its confirmation is showing: keys and
    /// commands go to it, never to the photos behind it.
    pub(in crate::app) fn is_reviewing(&self) -> bool {
        self.showing || self.review.as_ref().is_some_and(|r| r.confirming)
    }
}

/// A scan running on its own thread.
struct Scan {
    id: u64,
    name: String,
    /// The folder synchronized, on this computer; none for all of them.
    root: Option<PathBuf>,
    /// Find on Disk's search, not a synchronization.
    finding: bool,
    catalog: CatalogLocation,
    /// The session's structural generation it started under.
    generation: u64,
    files: Arc<AtomicUsize>,
    cancel: Arc<AtomicBool>,
    /// Set once the thread has sent its result, so a thread that ended
    /// without one (a panic, a lost channel) is noticed.
    sent: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// A finished scan waiting for the user, and what they ticked.
pub(in crate::app) struct Review {
    name: String,
    catalog: CatalogLocation,
    generation: u64,
    plan: SyncPlan,
    /// Per move of the plan: accepted. Verified ones start ticked.
    moves: Vec<bool>,
    /// Per choice: the option picked, if any.
    picks: Vec<Option<usize>>,
    /// Per addition: accepted. New files start ticked, copies don't.
    additions: Vec<bool>,
    /// Per missing photo: to remove. None start ticked.
    removals: Vec<bool>,
    /// The sections whose list is shown.
    expanded: BTreeSet<&'static str>,
    /// What each photo holds that a removal loses: rating, edit, copies.
    badges: HashMap<PhotoId, Vec<String>>,
    /// The folder synchronized, which rows are shown from; none for all.
    root: Option<PathBuf>,
    /// Asking to confirm removing photos that hold edits or metadata.
    confirming: bool,
}

/// What a scan thread sends back.
pub(crate) struct Scanned {
    id: u64,
    result: Result<SyncPlan, String>,
}
/// What applying sends back: the catalog opened again, and what changed.
pub(crate) struct Synced {
    /// The catalog opened again, or why it could not be.
    library: Result<Box<crate::app::library::Library>, String>,
    applied: Applied,
    /// The photo open in Develop, still in the catalog, and its new file
    /// when it was found moved and verified: the open document stays,
    /// following it.
    relink: Option<(PhotoId, Option<PathBuf>)>,
}

impl Editor {
    /// Starts synchronizing `scope`, named `name`, in the background.
    pub(in crate::app) fn start_folder_sync(&mut self, scope: SyncScope, name: String) {
        if self.folder_sync.scan.is_some() {
            self.messages.warn("A synchronization is already running.");
            return;
        }
        let Some(library) = &self.library else {
            return;
        };
        let inputs = match library.session.catalog().sync_inputs(scope) {
            Ok(inputs) => inputs,
            Err(e) => {
                self.messages
                    .fail(format!("Synchronize Folder failed: {e:#}"));
                return;
            }
        };
        let root = match scope {
            SyncScope::Folder(folder) => library
                .session
                .folders()
                .iter()
                .find(|f| f.id == folder)
                .map(|f| f.path.clone()),
            SyncScope::Root(root) => library
                .session
                .roots()
                .iter()
                .find(|(id, ..)| *id == root)
                .map(|(_, original, located)| {
                    PathBuf::from(located.as_deref().unwrap_or(original))
                }),
            SyncScope::All => None,
        };
        self.spawn_scan(name, root, false, move |stopped, progress| {
            scan(&inputs, &crate::volume_mounted, stopped, progress)
        });
    }
    /// Find on Disk…: asks for the file of `photo`, offline, starting where
    /// it was last.
    pub(in crate::app) fn choose_file_on_disk(&mut self, photo: PhotoId, ctx: &egui::Context) {
        let Some(path) = self
            .library
            .as_ref()
            .and_then(|l| l.photo(photo))
            .map(|p| p.path.clone())
        else {
            return;
        };
        if self.folder_sync.scan.is_some() {
            self.messages.warn("A synchronization is already running.");
            return;
        }
        if !self.activity.begin_dialog() {
            self.messages
                .warn("Find on Disk waits for the work in progress; try again once it is done.");
            return;
        }
        let start = picker_start(&path);
        let title = format!("Find {}", file_name(&path));
        crate::app::task::spawn(
            self.tx.clone(),
            ctx.clone(),
            move |tx| {
                let mut dialog = rfd::FileDialog::new().set_title(title);
                if let Some(start) = start {
                    dialog = dialog.set_directory(start);
                }
                let event = dialog
                    .pick_file()
                    .map(|file| Event::FileOnDiskPicked(photo, file))
                    .unwrap_or(Event::DialogClosed);
                let _ = tx.send(event);
            },
            |tx, _| {
                let _ = tx.send(Event::DialogClosed);
            },
        );
    }
    /// Starts Find on Disk for `photo` (a virtual copy stands for its
    /// master), whose file the user picked at `file`, in the background as a
    /// synchronization's scan: one runs at a time.
    pub(in crate::app) fn start_find_on_disk(&mut self, photo: PhotoId, file: PathBuf) {
        if self.folder_sync.scan.is_some() {
            self.messages.warn("A synchronization is already running.");
            return;
        }
        let Some(library) = &self.library else {
            return;
        };
        let inputs = match library.session.catalog().find_inputs(photo, &file) {
            Ok(inputs) => inputs,
            Err(e) => {
                self.messages.fail(format!("Find on Disk: {e:#}"));
                return;
            }
        };
        let name = library
            .photo(photo)
            .map(|p| p.filename.clone())
            .unwrap_or_default();
        let root = file.parent().map(PathBuf::from);
        self.spawn_scan(name, root, true, move |stopped, progress| {
            find_on_disk(&inputs, &crate::volume_mounted, stopped, progress)
        });
    }
    /// Runs `work`, a synchronization's scan or Find on Disk's (`finding`),
    /// on a thread of its own, shown in the top bar until its plan is back.
    fn spawn_scan(
        &mut self,
        name: String,
        root: Option<PathBuf>,
        finding: bool,
        work: impl FnOnce(Cancel<'_>, &dyn Fn(usize)) -> anyhow::Result<SyncPlan> + Send + 'static,
    ) {
        let Some(library) = &self.library else {
            return;
        };
        self.folder_sync.review = None;
        self.folder_sync.showing = false;
        self.folder_sync.started += 1;
        let id = self.folder_sync.started;
        let files = Arc::new(AtomicUsize::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let sent = Arc::new(AtomicBool::new(false));
        let (tx, ctx) = (self.tx.clone(), self.context.clone());
        let (counted, stopped, sending) = (files.clone(), cancel.clone(), sent.clone());
        let thread = std::thread::Builder::new()
            .name("folder-sync-scan".into())
            .spawn(move || {
                let progress = |n: usize| {
                    counted.store(n, Ordering::Relaxed);
                    ctx.request_repaint();
                };
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    work(Cancel::new(&stopped), &progress)
                }));
                let result = match result {
                    Ok(Ok(plan)) => Ok(plan),
                    Ok(Err(e)) => Err(format!("{e:#}")),
                    Err(_) => Err("Stopped unexpectedly".into()),
                };
                if tx
                    .send(Event::FolderSyncScanned(Box::new(Scanned { id, result })))
                    .is_ok()
                {
                    sending.store(true, Ordering::Release);
                }
                ctx.request_repaint();
            })
            .ok();
        if thread.is_none() {
            let text: String = if finding {
                "Find on Disk could not start".into()
            } else {
                "Synchronize Folder could not start".into()
            };
            self.messages.fail(text);
            return;
        }
        self.folder_sync.scan = Some(Scan {
            id,
            name,
            root,
            finding,
            catalog: library.session.catalog().location().clone(),
            generation: library.session.generation(),
            files,
            cancel,
            sent,
            thread,
        });
    }
    /// A scan finished: its plan waits for review.
    pub(in crate::app) fn folder_sync_scanned(&mut self, scanned: Scanned) {
        let Some(scan) = self.folder_sync.scan.take_if(|s| s.id == scanned.id) else {
            return;
        };
        let current = self.library.as_ref().is_some_and(|l| {
            *l.session.catalog().location() == scan.catalog
                && l.session.generation() == scan.generation
        });
        if scan.finding {
            match scanned.result {
                Err(_) if scan.cancel.load(Ordering::Relaxed) => {
                    self.messages.note("Find on Disk stopped");
                }
                Err(e) => self.messages.fail(format!("Find on Disk: {e}")),
                Ok(_) if !current => {
                    self.messages
                        .warn("The catalog changed during the search; Find on Disk again");
                }
                Ok(plan) => self.found_on_disk(scan, plan),
            }
            return;
        }
        match scanned.result {
            Err(e) if scan.cancel.load(Ordering::Relaxed) => {
                let _ = e;
                self.messages.note("Synchronization stopped");
            }
            Err(e) => self
                .messages
                .fail(format!("Synchronize Folder failed: {e}")),
            Ok(_) if !current => {
                self.messages
                    .warn("The catalog changed during the scan; synchronize again");
            }
            Ok(plan) => {
                if nothing_to_do(&plan) {
                    let mut said = format!("\"{}\" is in sync", scan.name);
                    if plan.not_checked.is_empty() {
                        self.messages.done(said);
                    } else {
                        said.push_str(&format!(" · {} not checked", plan.not_checked.len()));
                        self.messages.warn(said);
                    }
                    return;
                }
                let badges = self.badges(&plan);
                self.folder_sync.review = Some(Review::new(scan, plan, badges));
                self.folder_sync.showing = false;
            }
        }
    }
    /// Find on Disk's plan is back. The photo open in Develop moves only on
    /// a verified match: an unverified file picked for it is refused, its
    /// edit left pending, whichever module shows. When that edit can't be
    /// saved, its file gone, opening another photo can't free it: discarding
    /// the change is asked instead (issue #702). The file picked, verified,
    /// with nothing else found and its old file observed missing, is
    /// relinked at once; anything else is reviewed.
    fn found_on_disk(&mut self, scan: Scan, plan: SyncPlan) {
        let Some(picked) = plan.picked().cloned() else {
            self.messages
                .warn("Find on Disk: the file picked was not found");
            return;
        };
        if self.open_master() == Some(picked.photo) && !picked.verified {
            // Asked only over no other dialog, whose state it would replace.
            if self.unsaved_on_missing_file() && self.modal.is_none() {
                let badges = self.badges(&plan);
                self.modal = Some(crate::app::Modal::DiscardForFind(Box::new(Review::new(
                    scan, plan, badges,
                ))));
                return;
            }
            self.messages.warn(format!(
                "{} is open in Develop. Open another photo in Develop, then Find on Disk again",
                file_name(&picked.from)
            ));
            return;
        }
        let alone = plan.moves.len() == 1 && plan.choices.is_empty();
        let badges = self.badges(&plan);
        self.folder_sync.review = Some(Review::new(scan, plan, badges));
        self.folder_sync.showing = false;
        if alone && picked.verified && !picked.offline {
            let ctx = self.context.clone();
            self.apply_folder_sync(&ctx);
        }
    }
    /// Asks before discarding the unsaved change of the photo open in
    /// Develop, so that Find on Disk's unverified file can be reviewed for
    /// it (see [`found_on_disk`](Self::found_on_disk)).
    pub(in crate::app) fn discard_for_find_window(&mut self, ctx: &egui::Context) {
        let Some(crate::app::Modal::DiscardForFind(review)) = &self.modal else {
            return;
        };
        let name = review
            .plan
            .picked()
            .map(|m| file_name(&m.from))
            .unwrap_or_default();
        // Only a click or the focused button confirms: a stray Return must
        // never discard an edit.
        let Some(discard) = crate::app::ui::widgets::confirm_modal(
            ctx,
            "discard-for-find",
            &format!("Discard the unsaved change to {name}?"),
            &format!(
                "{name} is open in Develop with a change that can't be saved: \
                 its file can't be found. The file picked can't be verified as {name}, \
                 so the open photo can't follow it. Discarding closes the photo \
                 and reviews the file picked. To keep the change, Cancel and \
                 save it as a preset, or put the file back where it was."
            ),
            false,
            &[("Cancel", false), ("Discard Change", true)],
            false,
        ) else {
            return;
        };
        self.answer_discard_for_find(discard);
    }
    fn answer_discard_for_find(&mut self, discard: bool) {
        let Some(crate::app::Modal::DiscardForFind(review)) = self.modal.take() else {
            return;
        };
        let Some((picked, name)) = review.plan.picked().map(|m| (m.photo, file_name(&m.from)))
        else {
            return;
        };
        if !discard {
            self.messages.warn(format!(
                "{name} keeps its unsaved change. Save it as a preset or put the file back, then Find on Disk again"
            ));
            return;
        }
        let current = self
            .library
            .as_ref()
            .is_some_and(|l| *l.session.catalog().location() == review.catalog);
        if !current {
            self.messages
                .warn("The catalog changed during the search; Find on Disk again");
            return;
        }
        // Asked about once more as things are now: a change that can be
        // saved again (its file back) is never discarded.
        if self.open_master() == Some(picked) {
            if !self.unsaved_on_missing_file() {
                self.messages.warn(format!(
                    "{name} is open in Develop. Open another photo in Develop, then Find on Disk again"
                ));
                return;
            }
            self.close_removed_document();
        }
        // The review checks the catalog is unchanged before applying.
        self.folder_sync.review = Some(*review);
        self.folder_sync.showing = false;
    }
    /// The open edit needs saving but can't be saved: its file is not found.
    fn unsaved_on_missing_file(&self) -> bool {
        self.document.edit.save_state().needs_save()
            && self.document.path.as_ref().is_some_and(|p| !p.exists())
    }
    /// The master of the photo loaded in Develop, shown or not.
    fn open_master(&self) -> Option<PhotoId> {
        let id = self.document.catalog_photo?;
        let master = self
            .library
            .as_ref()?
            .photo(id)
            .map(|p| p.master.unwrap_or(id));
        Some(master.unwrap_or(id))
    }
    /// Notices a scan thread that ended without sending its result.
    pub(in crate::app) fn poll_folder_sync(&mut self) {
        let lost = self.folder_sync.scan.as_ref().is_some_and(|s| {
            s.thread.as_ref().is_some_and(|t| t.is_finished()) && !s.sent.load(Ordering::Acquire)
        });
        if lost {
            let finding = self.folder_sync.scan.take().is_some_and(|s| s.finding);
            let text: String = if finding {
                "Find on Disk stopped unexpectedly".into()
            } else {
                "Synchronize Folder stopped unexpectedly".into()
            };
            self.messages.fail(text);
        }
        // The review opens on its own in either mode once nothing else
        // is asked.
        if self.folder_sync.review.is_some()
            && !self.folder_sync.showing
            && !self
                .folder_sync
                .review
                .as_ref()
                .is_some_and(|r| r.confirming)
            && self.modal.is_none()
            && !self.command_modal()
            && self.folder_sync.review.as_ref().is_some_and(|r| !r.seen())
        {
            self.folder_sync.showing = true;
            if let Some(review) = &mut self.folder_sync.review {
                review.mark_seen();
            }
        }
    }
    /// Asks the scan to stop at its next check, for quitting.
    pub(in crate::app) fn stop_folder_sync(&mut self) -> Option<crate::app::task::Stopping> {
        let scan = self.folder_sync.scan.as_mut()?;
        scan.cancel.store(true, Ordering::Relaxed);
        Some(crate::app::task::Stopping::new(scan.thread.take()))
    }
    /// The top bar's part: the scan's progress and stop button, or the
    /// button that opens a review waiting, as the export's progress shows.
    pub(in crate::app) fn folder_sync_progress(&mut self, ui: &mut egui::Ui) {
        let palette = theme::palette(ui.ctx());
        if let Some(scan) = &self.folder_sync.scan {
            let files = scan.files.load(Ordering::Relaxed);
            let (rect, _) = ui.allocate_exact_size(egui::vec2(250., 28.), egui::Sense::hover());
            let painter = ui.painter();
            painter.text(
                rect.left_center() + egui::vec2(0., -6.),
                egui::Align2::LEFT_CENTER,
                if scan.finding {
                    format!("Finding photos beside \u{201c}{}\u{201d}", scan.name)
                } else {
                    format!("Synchronizing \u{201c}{}\u{201d}", scan.name)
                },
                egui::FontId::proportional(text::LABEL),
                palette.tone(Tone::SOFT),
            );
            painter.text(
                rect.left_center() + egui::vec2(0., 8.),
                egui::Align2::LEFT_CENTER,
                format!("{} files looked at", thousands(files)),
                egui::FontId::proportional(text::SMALL),
                palette.tone(Tone::MUTED),
            );
            let close = egui::Rect::from_center_size(
                rect.right_center() - egui::vec2(10., 0.),
                egui::vec2(20., 20.),
            );
            let stop = ui
                .interact(
                    close,
                    ui.id().with("stop-folder-sync"),
                    egui::Sense::click(),
                )
                .on_hover_text(if scan.finding {
                    "Stop Find on Disk"
                } else {
                    "Stop synchronizing"
                });
            let tint = palette.tone(if stop.hovered() {
                Tone::STRONG
            } else {
                Tone::SECONDARY
            });
            crate::app::ui::icons::paint_at(ui.painter(), Icon::Close, close.center(), 13., tint);
            if stop.clicked() {
                scan.cancel.store(true, Ordering::Relaxed);
            }
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(250));
            return;
        }
        if let Some(review) = &self.folder_sync.review
            && !self.folder_sync.showing
        {
            let label = if review.finding() {
                format!("Review photos found beside \u{201c}{}\u{201d}", review.name)
            } else {
                format!("Review synchronization of \u{201c}{}\u{201d}", review.name)
            };
            let button = egui::Button::new(
                egui::RichText::new(label)
                    .size(text::LABEL)
                    .color(palette.accent()),
            )
            .frame(false);
            if ui
                .add(button)
                .on_hover_text("Open the changes found")
                .clicked()
            {
                self.folder_sync.showing = true;
            }
        }
    }
    /// What each photo of `plan` holds, for the review to show.
    fn badges(&self, plan: &SyncPlan) -> HashMap<PhotoId, Vec<String>> {
        let Some(library) = &self.library else {
            return HashMap::new();
        };
        let session = &library.session;
        let edited = session.catalog().edit_times().unwrap_or_default();
        let photos = plan
            .moves
            .iter()
            .map(|m| m.photo)
            .chain(plan.missing.iter().map(|m| m.photo))
            .chain(
                plan.choices
                    .iter()
                    .flat_map(|c| c.options.iter().map(|m| m.photo)),
            );
        photos
            .map(|id| {
                let copies = session
                    .photos()
                    .iter()
                    .filter(|p| p.master == Some(id))
                    .count();
                let photo = session.photos().iter().find(|p| p.id == id);
                let rating = photo.map_or(0, |p| p.rating);
                let keywords = photo.is_some_and(|p| !p.keywords.is_empty());
                let collections = session
                    .collection_photos()
                    .values()
                    .filter(|members| members.contains(&id))
                    .count();
                let mut parts = Vec::new();
                if rating > 0 {
                    parts.push("\u{2605}".repeat(rating as usize));
                }
                if edited.contains_key(&id) {
                    parts.push("Edited".into());
                }
                if keywords {
                    parts.push("Keywords".into());
                }
                if collections > 0 {
                    parts.push(crate::app::ui::widgets::plural(
                        collections,
                        "collection",
                        "collections",
                    ));
                }
                if copies > 0 {
                    parts.push(crate::app::ui::widgets::plural(
                        copies,
                        "virtual copy",
                        "virtual copies",
                    ));
                }
                (id, parts)
            })
            .collect()
    }
    /// The review dialog: one card for each kind of change, each saying
    /// what it does, with its photos below.
    pub(in crate::app) fn folder_sync_window(&mut self, ctx: &egui::Context) {
        if !self.folder_sync.showing {
            return;
        }
        let open = self.document.catalog_photo;
        let open_master = open.and_then(|id| {
            let photo = self.library.as_ref()?.photo(id)?;
            Some(photo.master.unwrap_or(id))
        });
        let thumbnails: HashMap<PhotoId, egui::TextureHandle> =
            match (&self.library, &self.folder_sync.review) {
                (Some(library), Some(review)) => review
                    .photos()
                    .filter_map(|id| Some((id, library.thumbnail(id)?.clone())))
                    .collect(),
                _ => HashMap::new(),
            };
        let Some(review) = &mut self.folder_sync.review else {
            self.folder_sync.showing = false;
            return;
        };
        let palette = theme::palette(ctx);
        let mut answer: Option<bool> = None;
        let response = egui::Modal::new(egui::Id::new("folder-sync-review"))
            .backdrop_color(egui::Color32::from_black_alpha(140))
            .frame(crate::app::ui::widgets::modal_frame(&palette))
            .show(ctx, |ui| {
                ui.set_width(WIDTH);
                ui.spacing_mut().item_spacing = egui::vec2(space::M, space::M);
                // Header: what was looked at, and what was found.
                egui::Frame::new()
                    .inner_margin(egui::Margin {
                        left: inset::DIALOG as i8,
                        right: inset::DIALOG as i8,
                        top: inset::DIALOG as i8,
                        bottom: space::XL as i8,
                    })
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(review.title())
                                .size(text::TITLE)
                                .color(palette.tone(Tone::STRONG)),
                        );
                        ui.add_space(space::XXS);
                        ui.label(
                            egui::RichText::new(review.summary())
                                .size(text::LABEL)
                                .color(palette.tone(Tone::SECONDARY)),
                        );
                    });
                separator(ui, &palette);
                egui::ScrollArea::vertical()
                    .max_height((ctx.content_rect().height() - 220.).max(240.))
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        egui::Frame::new().inner_margin(inset::CARD).show(ui, |ui| {
                            ui.spacing_mut().item_spacing = egui::vec2(space::M, space::L);
                            review.cards(ui, open_master, &thumbnails);
                        });
                    });
                separator(ui, &palette);
                // Footer: the promise about the disk, then the buttons.
                egui::Frame::new()
                    .inner_margin(egui::Margin {
                        left: inset::DIALOG as i8,
                        right: inset::DIALOG as i8,
                        top: space::L as i8,
                        bottom: space::XL as i8,
                    })
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(
                                    "Only the catalog changes. Files on disk stay as they are.",
                                )
                                .size(text::LABEL)
                                .color(palette.tone(Tone::MUTED)),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.spacing_mut().button_padding = button::DIALOG;
                                    let removing = review.removals.iter().filter(|r| **r).count();
                                    let nothing = review.accepted_nothing();
                                    let label = if review.finding() {
                                        format!(
                                            "Relink {}",
                                            crate::app::ui::widgets::plural(
                                                review.ticked(),
                                                "Photo",
                                                "Photos"
                                            )
                                        )
                                    } else if removing > 0 {
                                        format!("Synchronize and Remove {removing}")
                                    } else {
                                        "Synchronize".into()
                                    };
                                    let confirm = ui.add_enabled_ui(!nothing, |ui| {
                                        crate::app::ui::widgets::primary_button(ui, &label)
                                    });
                                    if confirm.inner.clicked() {
                                        answer = Some(true);
                                    }
                                    if nothing {
                                        confirm.response.on_hover_text("Nothing is ticked");
                                    }
                                    if ui
                                        .add(
                                            egui::Button::new("Cancel")
                                                .min_size(egui::vec2(84., 30.)),
                                        )
                                        .clicked()
                                    {
                                        answer = Some(false);
                                    }
                                },
                            );
                        });
                    });
            });
        if answer.is_none() && response.should_close() && !review.confirming {
            answer = Some(false);
        }
        match answer {
            Some(false) => {
                self.folder_sync.showing = false;
                self.folder_sync.review = None;
            }
            Some(true) => {
                let review = self.folder_sync.review.as_mut().expect("shown");
                if !review.removal_losses().is_empty() && !review.confirming {
                    review.confirming = true;
                } else {
                    self.apply_folder_sync(ctx);
                }
            }
            None => {}
        }
        self.removal_confirm_window(ctx);
    }
    /// Confirms removing photos that hold edits, ratings or copies.
    fn removal_confirm_window(&mut self, ctx: &egui::Context) {
        let Some(review) = &mut self.folder_sync.review else {
            return;
        };
        if !review.confirming {
            return;
        }
        let losing = review.removal_losses();
        let detail = format!(
            "These photos are removed from the catalog with what they hold:\n{}",
            losing.join("\n")
        );
        match crate::app::ui::widgets::confirm_modal(
            ctx,
            "folder-sync-remove",
            "Remove missing photos?",
            &detail,
            false,
            &[("Cancel", false), ("Remove", true)],
            false,
        ) {
            Some(true) => {
                review.confirming = false;
                self.apply_folder_sync(ctx);
            }
            Some(false) => review.confirming = false,
            None => {}
        }
    }
    /// Applies the review's choices as a folder change, then opens the
    /// catalog again.
    fn apply_folder_sync(&mut self, ctx: &egui::Context) {
        let Some(review) = self.folder_sync.review.take() else {
            return;
        };
        self.folder_sync.showing = false;
        let current = self.library.as_ref().is_some_and(|l| {
            *l.session.catalog().location() == review.catalog
                && l.session.generation() == review.generation
        });
        if !current {
            self.messages
                .warn("The catalog changed since the scan; synchronize again");
            return;
        }
        let accepted = review.accepted();
        // The open photo, and its file found moved, as accepted.
        let open = self.document.catalog_photo;
        let open_master = open.and_then(|id| {
            let photo = self.library.as_ref()?.photo(id)?;
            Some(photo.master.unwrap_or(id))
        });
        let open_move = open_master.and_then(|master| {
            let to = accepted
                .moves
                .iter()
                .find(|(id, _)| *id == master)?
                .1
                .clone();
            let verified = review
                .plan
                .moves
                .iter()
                .chain(review.plan.choices.iter().flat_map(|c| c.options.iter()))
                .any(|m| m.photo == master && m.to == to && m.verified);
            verified.then_some(to)
        });
        // The open edit is saved first. Saving fails when its own file was
        // moved: then only a verified move of it lets synchronizing go on,
        // keeping the edit pending, saved on the new path afterwards.
        if self.activity.is_busy() {
            self.messages
                .warn("Finish what is running first, then synchronize.");
            self.folder_sync.review = Some(review);
            return;
        }
        // Edits made in the Library are saved first, whatever happens.
        if !self.commit_library_drafts() {
            self.folder_sync.review = Some(review);
            return;
        }
        let file_gone = self.document.path.as_ref().is_some_and(|p| !p.exists());
        if !self.flush() && !(file_gone && open_move.is_some()) {
            self.folder_sync.review = Some(review);
            return;
        }
        if !self.activity.begin_folder_change() {
            self.folder_sync.review = Some(review);
            return;
        }
        // What the readers bring back could land on photos given removed
        // photos' ids.
        if let Some(library) = &mut self.library {
            library.stop_background_reads();
        }
        let location = review.catalog.clone();
        self.folder_sync.applying_find = review.finding();
        let plan = review.plan;
        // The open photo's master, as the review saw it: what moves is the
        // master's family.
        let relink = open.map(|id| (id, open_master.unwrap_or(id), open_move));
        let ctx = ctx.clone();
        crate::app::task::spawn(
            self.tx.clone(),
            ctx.clone(),
            move |tx| {
                let result = (|| -> anyhow::Result<Synced> {
                    let mut catalog = crate::catalog::Catalog::open(&location)?;
                    let applied = catalog.apply_folder_sync(
                        &plan,
                        &accepted,
                        &crate::volume_mounted,
                        open,
                        &crate::catalog_session::new_photo_defaults(),
                    )?;
                    drop(catalog);
                    // Applied: what follows must not lose it, nor where the
                    // open photo's file went.
                    let opened = crate::catalog_session::CatalogSession::open(&location)
                        .map(|opened| crate::app::library::Library::new(opened, ctx.clone()));
                    let library = match opened {
                        Ok(mut library) => {
                            synced_message(&mut library, &plan, &applied);
                            Ok(Box::new(library))
                        }
                        Err(e) => Err(format!("{e:#}")),
                    };
                    // The open photo stays open while it is in the catalog;
                    // it follows only a move that went through.
                    let relink = relink.and_then(|(id, master, to)| {
                        // Gone from the catalog: nothing to keep open.
                        if let Ok(l) = &library {
                            l.photo(id)?;
                        }
                        Some((id, to.filter(|_| applied.moved.contains(&master))))
                    });
                    Ok(Synced {
                        library,
                        applied,
                        relink,
                    })
                })();
                let _ = tx.send(Event::FolderSyncApplied(
                    result.map(Box::new).map_err(|e| format!("{e:#}")),
                ));
            },
            |tx, message| {
                let _ = tx.send(Event::FolderSyncApplied(Err(message)));
            },
        );
    }
    /// Applying finished: forgets removed photos, moves preview requests
    /// with moved ones, and shows the catalog again, the open photo
    /// following its file.
    /// Forgets photos a synchronization removed: their previews requested,
    /// their undo, and the reference if it was one of them.
    fn forget_removed(&mut self, removed: &[PhotoId]) {
        for &id in removed {
            self.forget_preview_intent(id);
            self.undo_log.forget_photo(id);
        }
        if self.reference.photo.is_some_and(|id| removed.contains(&id)) {
            self.clear_reference();
        }
    }
    pub(in crate::app) fn folder_synced(&mut self, result: Result<Box<Synced>, String>) {
        let synced = match result {
            Ok(synced) => synced,
            Err(e) => {
                self.activity.finish_dialog();
                // Nothing applied: the reads applying stopped go on.
                if let Some(library) = &mut self.library {
                    library.start_background_reads();
                }
                let text: String = if self.folder_sync.applying_find {
                    format!("Find on Disk failed: {e}")
                } else {
                    format!("Synchronize Folder failed: {e}")
                };
                self.messages.fail(text);
                return;
            }
        };
        let Synced {
            library,
            applied,
            relink,
        } = *synced;
        let library = match library {
            Ok(library) => library,
            Err(e) => {
                // Applied, but not shown again: the open photo still
                // follows its file, so its edit saves there.
                self.activity.finish_dialog();
                if let Some((id, Some(path))) = relink
                    && self.document.catalog_photo == Some(id)
                {
                    self.document.relink(path);
                }
                self.forget_removed(&applied.removed);
                // The lists shown must not keep photos removed, whose ids
                // new photos may have: read again here, or emptied. The
                // Library stays, so an edit still unsaved saves to it.
                let reread = self
                    .library
                    .as_mut()
                    .map(|l| l.reread_after(&applied.removed));
                if let Some(Err(again)) = reread {
                    self.messages.fail(format!(
                        "Synchronized, but the catalog could not be read again: {again:#}. Open it again."
                    ));
                } else {
                    let library = self.library.as_ref().expect("read again");
                    let moved = moved_files(library.session.photos(), &applied);
                    let catalog = library.session.catalog().location().clone();
                    self.move_preview_intents(&catalog, &moved);
                    self.messages.fail(format!(
                        "Synchronized, but the catalog could not be opened again: {e}. It was read again."
                    ));
                }
                return;
            }
        };
        self.forget_removed(&applied.removed);
        let moved = moved_files(library.session.photos(), &applied);
        self.move_preview_intents(library.session.catalog().location(), &moved);
        self.catalog_arrived(Ok(library), relink);
    }
}

/// Each moved photo and its copies, at their new file.
fn moved_files(photos: &[crate::catalog::Photo], applied: &Applied) -> Vec<(PhotoId, PathBuf)> {
    photos
        .iter()
        .filter(|p| applied.moved.contains(&p.master.unwrap_or(p.id)))
        .map(|p| (p.id, p.path.clone()))
        .collect()
}

/// Where Find on Disk's picker starts for a photo last at `path`: the
/// nearest folder above it that is still there.
fn picker_start(path: &std::path::Path) -> Option<PathBuf> {
    path.ancestors()
        .skip(1)
        .find(|dir| !dir.as_os_str().is_empty() && dir.is_dir())
        .map(PathBuf::from)
}

/// Whether `plan` offers nothing to do.
fn nothing_to_do(plan: &SyncPlan) -> bool {
    plan.gone_folders.is_empty()
        && plan.moves.is_empty()
        && plan.choices.is_empty()
        && plan.additions.is_empty()
        && plan.missing.is_empty()
        && plan.conflicts.is_empty()
        && plan.ambiguous.is_empty()
}

/// The top bar after applying: "Synchronized · 14 updated · 32 added ·
/// 5 still missing", or Find on Disk's "Relinked 9 photos", with every
/// detail on hover.
fn synced_message(library: &mut crate::app::library::Library, plan: &SyncPlan, applied: &Applied) {
    if matches!(plan.kind(), PlanKind::FindOnDisk { .. }) {
        let mut summary = format!(
            "Relinked {}",
            crate::app::ui::widgets::plural(applied.moved.len(), "photo", "photos")
        );
        if !applied.skipped.is_empty() {
            summary.push_str(&format!(" \u{b7} {} skipped", applied.skipped.len()));
        }
        let mut detail: Vec<String> = applied
            .skipped
            .iter()
            .map(|s| format!("Skipped {}: {}", s.path.display(), s.reason))
            .collect();
        if !applied.protected.is_empty() {
            detail.push(format!(
                "{} kept their edit protected: it was saved for another version of the file",
                crate::app::ui::widgets::plural(applied.protected.len(), "photo", "photos")
            ));
        }
        if detail.is_empty() {
            library.messages.done(summary);
        } else {
            library
                .messages
                .warn_with_detail(summary, detail.join("\n"));
        }
        library.offer_previous_import = false;
        return;
    }
    let still_missing = plan
        .missing
        .iter()
        .filter(|m| !applied.removed.contains(&m.photo))
        .count();
    let mut summary = vec![
        format!("{} updated", applied.moved.len()),
        format!("{} added", applied.added),
    ];
    if !applied.removed.is_empty() {
        summary.push(format!("{} removed", applied.removed.len()));
    }
    if still_missing > 0 {
        summary.push(format!("{still_missing} still missing"));
    }
    if !applied.skipped.is_empty() {
        summary.push(format!("{} skipped", applied.skipped.len()));
    }
    let mut detail: Vec<String> = Vec::new();
    for s in &applied.skipped {
        detail.push(format!("Skipped {}: {}", s.path.display(), s.reason));
    }
    if !applied.protected.is_empty() {
        detail.push(format!(
            "{} kept their edit protected: it was saved for another version of the file",
            crate::app::ui::widgets::plural(applied.protected.len(), "photo", "photos")
        ));
    }
    for n in &plan.not_checked {
        detail.push(format!("Not checked {}: {}", n.path.display(), n.reason));
    }
    if let Some(e) = &applied.add_error {
        summary.push("adding new files failed".into());
        detail.push(format!("Adding new files failed: {e:#}"));
    }
    let report = applied.report.details();
    if !report.is_empty() {
        detail.push(report);
        detail.push(
            "Read Metadata from Files reads their metadata again once the files can be read."
                .into(),
        );
    }
    let said = format!("Synchronized · {}", summary.join(" · "));
    let nothing_done = applied.moved.is_empty() && applied.added == 0 && applied.removed.is_empty();
    if applied.add_error.is_some() && nothing_done {
        library.messages.fail_with_detail(said, detail.join("\n"));
    } else if applied.add_error.is_some()
        || !applied.skipped.is_empty()
        || !applied.protected.is_empty()
        || !applied.report.details().is_empty()
        || !plan.not_checked.is_empty()
    {
        library.messages.warn_with_detail(said, detail.join("\n"));
    } else {
        library.messages.done_with_detail(said, detail.join("\n"));
    }
    library.offer_previous_import = applied.added > 0;
}

impl Review {
    fn new(scan: Scan, plan: SyncPlan, badges: HashMap<PhotoId, Vec<String>>) -> Self {
        // Ticked as the plan says: verified moves, and Find on Disk's file
        // picked.
        let ticked = plan.default_accepted().moves;
        Self {
            moves: plan
                .moves
                .iter()
                .map(|m| ticked.contains(&(m.photo, m.to.clone())))
                .collect(),
            picks: vec![None; plan.choices.len()],
            additions: plan
                .additions
                .iter()
                .map(|a| a.copy_of.is_empty())
                .collect(),
            removals: vec![false; plan.missing.len()],
            expanded: BTreeSet::new(),
            badges,
            confirming: false,
            name: scan.name,
            root: scan.root.or_else(|| common_folder(&plan)),
            catalog: scan.catalog,
            generation: scan.generation,
            plan,
        }
    }
    /// Whether this is Find on Disk's review, not a synchronization's.
    fn finding(&self) -> bool {
        matches!(self.plan.kind(), PlanKind::FindOnDisk { .. })
    }
    /// The photos ticked or picked.
    fn ticked(&self) -> usize {
        self.moves.iter().filter(|m| **m).count() + self.picks.iter().flatten().count()
    }
    /// The dialog's title: Find on Disk's says how many other photos were
    /// found beside the one picked.
    fn title(&self) -> String {
        if !self.finding() {
            return format!("Synchronize \u{201c}{}\u{201d}", self.name);
        }
        let picked = self.plan.picked().map(|m| m.photo);
        let mut others: Vec<PhotoId> = self.photos().filter(|id| Some(*id) != picked).collect();
        others.sort();
        others.dedup();
        match others.len() {
            0 => format!("Relink \u{201c}{}\u{201d}", self.name),
            n => format!(
                "Found {} in this folder",
                crate::app::ui::widgets::plural(n, "other missing photo", "other missing photos")
            ),
        }
    }
    /// Whether the review was opened once already.
    fn seen(&self) -> bool {
        self.expanded.contains("seen")
    }
    fn mark_seen(&mut self) {
        self.expanded.insert("seen");
    }
    /// What the user accepted.
    fn accepted(&self) -> Accepted {
        let mut moves: Vec<(PhotoId, PathBuf)> = self
            .plan
            .moves
            .iter()
            .zip(&self.moves)
            .filter(|(_, on)| **on)
            .map(|(m, _)| (m.photo, m.to.clone()))
            .collect();
        for (choice, pick) in self.plan.choices.iter().zip(&self.picks) {
            if let Some(m) = pick.and_then(|i| choice.options.get(i)) {
                moves.push((m.photo, m.to.clone()));
            }
        }
        Accepted {
            moves,
            additions: self
                .plan
                .additions
                .iter()
                .zip(&self.additions)
                .filter(|(_, on)| **on)
                .map(|(a, _)| a.file.clone())
                .collect(),
            removals: self
                .plan
                .missing
                .iter()
                .zip(&self.removals)
                .filter(|(_, on)| **on)
                .map(|(m, _)| m.photo)
                .collect(),
        }
    }
    /// The photos to remove that hold something, each with what.
    fn removal_losses(&self) -> Vec<String> {
        self.plan
            .missing
            .iter()
            .zip(&self.removals)
            .filter(|(_, on)| **on)
            .filter_map(|(m, _)| {
                let badge = self
                    .badges
                    .get(&m.photo)
                    .filter(|b| !b.is_empty())?
                    .join(", ");
                Some(format!(
                    "{}: {badge}",
                    m.path.file_name().unwrap_or_default().to_string_lossy()
                ))
            })
            .collect()
    }
    /// Every photo the review shows, for their thumbnails.
    fn photos(&self) -> impl Iterator<Item = PhotoId> + '_ {
        self.plan
            .moves
            .iter()
            .map(|m| m.photo)
            .chain(self.plan.missing.iter().map(|m| m.photo))
            .chain(
                self.plan
                    .choices
                    .iter()
                    .flat_map(|c| c.options.iter().map(|m| m.photo)),
            )
    }
    /// "4 changes found in 1,240 files" and the like, under the title; for
    /// Find on Disk, where the file picked is and what it adds.
    fn summary(&self) -> String {
        let p = &self.plan;
        if let Some(picked) = p.picked() {
            let mut parts = vec![format!(
                "{} found in {}",
                file_name(&picked.from),
                picked
                    .to
                    .parent()
                    .map(crate::app::ui::widgets::pretty_path)
                    .unwrap_or_default()
            )];
            if let Some(root) = picked.new_root() {
                parts.push(format!(
                    "{} is added as a root folder",
                    root.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| root.display().to_string())
                ));
            }
            return parts.join(" \u{b7} ");
        }
        let mut parts = Vec::new();
        let moved = p.moves.len();
        if moved > 0 {
            parts.push(crate::app::ui::widgets::plural(
                moved,
                "photo moved",
                "photos moved",
            ));
        }
        let new = p.additions.len();
        if new > 0 {
            parts.push(crate::app::ui::widgets::plural(
                new,
                "new file",
                "new files",
            ));
        }
        if !p.missing.is_empty() {
            parts.push(format!("{} missing", p.missing.len()));
        }
        if !p.choices.is_empty() {
            parts.push(crate::app::ui::widgets::plural(
                p.choices.len(),
                "needs a choice",
                "need a choice",
            ));
        }
        if !p.gone_folders.is_empty() {
            parts.push(crate::app::ui::widgets::plural(
                p.gone_folders.len(),
                "folder gone",
                "folders gone",
            ));
        }
        if parts.is_empty() {
            "Nothing to change".into()
        } else {
            parts.join(" \u{b7} ")
        }
    }
    /// Whether nothing at all is ticked or picked, nor any folder left
    /// empty to remove.
    fn accepted_nothing(&self) -> bool {
        self.plan.gone_folders.is_empty()
            && !self.moves.iter().any(|m| *m)
            && !self.additions.iter().any(|a| *a)
            && !self.removals.iter().any(|r| *r)
            && self.picks.iter().all(Option::is_none)
    }
    /// Where `path` is, from the folder synchronized when it is below it.
    fn shown(&self, path: &std::path::Path) -> String {
        match self
            .root
            .as_deref()
            .and_then(|root| path.strip_prefix(root).ok())
        {
            Some(rest) => rest.display().to_string(),
            None => crate::app::ui::widgets::pretty_path(path),
        }
    }
    /// The folder `dir` by name: its path from the folder synchronized, or
    /// that folder's own name.
    fn place(&self, dir: Option<&std::path::Path>) -> String {
        let Some(dir) = dir else {
            return String::new();
        };
        let shown = self.shown(dir);
        if shown.is_empty() {
            dir.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or(shown)
        } else {
            shown
        }
    }
    /// One card for each kind of change found.
    fn cards(
        &mut self,
        ui: &mut egui::Ui,
        open_master: Option<PhotoId>,
        thumbnails: &HashMap<PhotoId, egui::TextureHandle>,
    ) {
        let palette = theme::palette(ui.ctx());
        // Find on Disk from a drive that isn't connected: the old files
        // can't be checked, so the catalog switches on the checksum alone.
        let offline = self.plan.moves.iter().filter(|m| m.offline).count();
        if offline > 0 {
            ui.horizontal_wrapped(|ui| {
                ui.add_space(space::XS);
                ui.label(
                    egui::RichText::new(format!(
                        "\u{26a0}  The old drive of {} isn't connected. The files found here match by checksum; \
                         the catalog uses them from now on, even if the old ones are still there.",
                        crate::app::ui::widgets::plural(offline, "photo", "photos")
                    ))
                    .size(text::LABEL)
                    .color(palette.tone(Tone::SOFT)),
                );
            });
        }
        let verified: Vec<usize> = (0..self.plan.moves.len())
            .filter(|i| self.plan.moves[*i].verified)
            .collect();
        let probable: Vec<usize> = (0..self.plan.moves.len())
            .filter(|i| !self.plan.moves[*i].verified)
            .collect();
        for (key, kind, indices) in [
            ("verified", Kind::Moved, verified),
            ("probable", Kind::Probable, probable),
        ] {
            if indices.is_empty() {
                continue;
            }
            let mut all = indices.iter().all(|i| self.moves[*i]);
            let some = indices.iter().any(|i| self.moves[*i]);
            let open_list = self.expanded.contains(key);
            let (toggled, expand) =
                card_header(ui, &palette, kind, indices.len(), &mut all, some, open_list);
            if toggled {
                for &i in &indices {
                    let open = open_master == Some(self.plan.moves[i].photo);
                    // A probable match can't take the open photo along.
                    self.moves[i] = all && !(open && kind == Kind::Probable);
                }
            }
            self.toggle(key, expand);
            if self.expanded.contains(key) {
                list(ui, &palette, |ui| {
                    for &i in &indices {
                        let m = self.plan.moves[i].clone();
                        let open = open_master == Some(m.photo);
                        let enabled = !(open && !m.verified);
                        let mut on = self.moves[i];
                        let note = if open && !m.verified {
                            Some("Open in Develop, not verified")
                        } else if m.offline {
                            Some("Its old drive isn't connected")
                        } else if m.by_name {
                            Some("Same name, not verified")
                        } else if !m.verified && self.plan.picked() == Some(&m) {
                            Some("Picked, not verified")
                        } else {
                            None
                        };
                        row(
                            ui,
                            &palette,
                            Row {
                                on: Some((&mut on, enabled)),
                                thumbnail: thumbnails.get(&m.photo),
                                title: &file_name(&m.from),
                                detail: &format!("now {}", self.shown(&m.to)),
                                hover: &format!(
                                    "{}\n\u{2192} {}",
                                    m.from.display(),
                                    m.to.display()
                                ),
                                chips: self.badges.get(&m.photo).map(Vec::as_slice).unwrap_or(&[]),
                                note,
                            },
                        );
                        self.moves[i] = on;
                    }
                });
            }
        }
        if !self.plan.choices.is_empty() {
            self.choice_card(ui, &palette, open_master, thumbnails);
        }
        let new: Vec<usize> = (0..self.plan.additions.len())
            .filter(|i| self.plan.additions[*i].copy_of.is_empty())
            .collect();
        let copies: Vec<usize> = (0..self.plan.additions.len())
            .filter(|i| !self.plan.additions[*i].copy_of.is_empty())
            .collect();
        for (key, kind, indices) in [("new", Kind::New, new), ("copies", Kind::Copies, copies)] {
            if indices.is_empty() {
                continue;
            }
            let mut all = indices.iter().all(|i| self.additions[*i]);
            let some = indices.iter().any(|i| self.additions[*i]);
            let open_list = self.expanded.contains(key);
            let (toggled, expand) =
                card_header(ui, &palette, kind, indices.len(), &mut all, some, open_list);
            if toggled {
                for &i in &indices {
                    self.additions[i] = all;
                }
            }
            self.toggle(key, expand);
            if self.expanded.contains(key) {
                list(ui, &palette, |ui| {
                    for &i in &indices {
                        let a = self.plan.additions[i].clone();
                        let detail = format!("in {}", self.place(a.file.parent()));
                        let mut on = self.additions[i];
                        let copy_of = a
                            .copy_of
                            .first()
                            .map(|id| format!("Same as a photo already here (#{})", id.0));
                        row(
                            ui,
                            &palette,
                            Row {
                                on: Some((&mut on, true)),
                                thumbnail: None,
                                title: &file_name(&a.file),
                                detail: &detail,
                                hover: &a.file.display().to_string(),
                                chips: &[],
                                note: copy_of.as_deref(),
                            },
                        );
                        self.additions[i] = on;
                    }
                });
            }
        }
        if !self.plan.missing.is_empty() {
            let mut all = self.removals.iter().all(|r| *r);
            let some = self.removals.iter().any(|r| *r);
            let open_list = self.expanded.contains("missing");
            let (toggled, expand) = card_header(
                ui,
                &palette,
                Kind::Missing,
                self.plan.missing.len(),
                &mut all,
                some,
                open_list,
            );
            if toggled {
                for (i, m) in self.plan.missing.iter().enumerate() {
                    self.removals[i] = all && open_master != Some(m.photo);
                }
            }
            self.toggle("missing", expand);
            if self.expanded.contains("missing") {
                list(ui, &palette, |ui| {
                    for i in 0..self.plan.missing.len() {
                        let m = self.plan.missing[i].clone();
                        let open = open_master == Some(m.photo);
                        let mut on = self.removals[i];
                        let detail = format!("was in {}", self.place(m.path.parent()));
                        row(
                            ui,
                            &palette,
                            Row {
                                on: Some((&mut on, !open)),
                                thumbnail: thumbnails.get(&m.photo),
                                title: &file_name(&m.path),
                                detail: &detail,
                                hover: &m.path.display().to_string(),
                                chips: self.badges.get(&m.photo).map(Vec::as_slice).unwrap_or(&[]),
                                note: open.then_some("Open in Develop"),
                            },
                        );
                        self.removals[i] = on;
                    }
                });
            }
        }
        if !self.plan.gone_folders.is_empty() {
            let n = self.plan.gone_folders.len();
            ui.add_space(space::XXS);
            ui.horizontal(|ui| {
                ui.add_space(space::XS);
                ui.label(
                    egui::RichText::new(format!(
                        "\u{24d8}  {} no longer on disk {} removed from the catalog once {} no photos.",
                        crate::app::ui::widgets::plural(n, "folder", "folders"),
                        if n == 1 { "is" } else { "are" },
                        if n == 1 { "it holds" } else { "they hold" }
                    ))
                    .size(text::LABEL)
                    .color(palette.tone(Tone::MUTED)),
                );
            });
        }
        let not_checked =
            self.plan.not_checked.len() + self.plan.conflicts.len() + self.plan.ambiguous.len();
        if not_checked > 0 {
            let mut lines: Vec<String> = self
                .plan
                .not_checked
                .iter()
                .map(|n| format!("{}: {}", n.path.display(), n.reason))
                .collect();
            lines.extend(self.plan.conflicts.iter().map(|c| c.message()));
            lines.extend(
                self.plan
                    .ambiguous
                    .iter()
                    .map(|d| format!("{}: more than one catalog folder could be it", d.display())),
            );
            ui.add_space(space::XXS);
            ui.horizontal(|ui| {
                ui.add_space(space::XS);
                ui.label(
                    egui::RichText::new(format!(
                        "\u{24d8}  {} could not be checked here, so nothing in {} counts as missing.",
                        crate::app::ui::widgets::plural(not_checked, "place", "places"),
                        if not_checked == 1 { "it" } else { "them" }
                    ))
                    .size(text::LABEL)
                    .color(palette.tone(Tone::MUTED)),
                )
                .on_hover_text(lines.join("\n"));
            });
        }
    }
    /// Shows or hides the list of the card `key`.
    fn toggle(&mut self, key: &'static str, flip: bool) {
        if flip && !self.expanded.remove(key) {
            self.expanded.insert(key);
        }
    }
    /// The card for photos found more than once: for each, the files it may
    /// be, to pick one or leave it.
    fn choice_card(
        &mut self,
        ui: &mut egui::Ui,
        palette: &theme::Palette,
        open_master: Option<PhotoId>,
        thumbnails: &HashMap<PhotoId, egui::TextureHandle>,
    ) {
        card(ui, palette, |ui| {
            ui.horizontal(|ui| {
                kind_icon(ui, palette, Kind::Choice);
                ui.label(
                    egui::RichText::new(Kind::Choice.title(self.plan.choices.len()))
                        .size(text::BODY)
                        .color(palette.tone(Tone::TEXT)),
                );
            });
            ui.label(
                egui::RichText::new(Kind::Choice.explanation())
                    .size(text::LABEL)
                    .color(palette.tone(Tone::MUTED)),
            );
            for c in 0..self.plan.choices.len() {
                let choice = self.plan.choices[c].clone();
                // The photos this choice is about, each with what it holds.
                let mut photos: Vec<PhotoId> = choice.options.iter().map(|m| m.photo).collect();
                photos.dedup();
                ui.add_space(space::XS);
                for photo in &photos {
                    let from = choice
                        .options
                        .iter()
                        .find(|m| m.photo == *photo)
                        .map(|m| m.from.clone())
                        .unwrap_or_default();
                    row(
                        ui,
                        palette,
                        Row {
                            on: None,
                            thumbnail: thumbnails.get(photo),
                            title: &file_name(&from),
                            detail: &format!("missing from {}", self.place(from.parent())),
                            hover: &from.display().to_string(),
                            chips: self.badges.get(photo).map(Vec::as_slice).unwrap_or(&[]),
                            note: None,
                        },
                    );
                }
                ui.indent(("folder-sync-choice", c), |ui| {
                    ui.spacing_mut().item_spacing.y = space::XS;
                    let leave = self.picks[c].is_none();
                    if option(
                        ui,
                        palette,
                        leave,
                        "Decide later",
                        "stays missing; these files are offered again next time",
                        true,
                    )
                    .clicked()
                    {
                        self.picks[c] = None;
                    }
                    for (i, m) in choice.options.iter().enumerate() {
                        let enabled = !(open_master == Some(m.photo) && !m.verified);
                        let same_name = m.to.file_name() == m.from.file_name();
                        let how = match (m.verified, same_name) {
                            (true, true) => "same content and name",
                            (true, false) => "same content, renamed",
                            (false, _) => "probably the same, not verified",
                        };
                        let detail = format!("{how} \u{b7} in {}", self.place(m.to.parent()));
                        let title = if photos.len() > 1 {
                            format!("{} is {}", file_name(&m.to), file_name(&m.from))
                        } else {
                            file_name(&m.to)
                        };
                        if option(
                            ui,
                            palette,
                            self.picks[c] == Some(i),
                            &title,
                            &detail,
                            enabled,
                        )
                        .on_hover_text(m.to.display().to_string())
                        .clicked()
                        {
                            self.picks[c] = Some(i);
                        }
                    }
                });
            }
        });
    }
}

/// The review dialog's width.
const WIDTH: f32 = 660.;

/// The kinds of change the review shows, each a card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Moved,
    Probable,
    New,
    Copies,
    Missing,
    Choice,
}
impl Kind {
    fn title(self, n: usize) -> String {
        match self {
            Self::Moved => format!(
                "Update {} moved or renamed",
                crate::app::ui::widgets::plural(n, "photo", "photos")
            ),
            Self::Probable => format!(
                "{} probably moved",
                crate::app::ui::widgets::plural(n, "photo", "photos")
            ),
            Self::New => format!(
                "Add {}",
                crate::app::ui::widgets::plural(n, "new photo", "new photos")
            ),
            Self::Copies => format!(
                "Add {}",
                crate::app::ui::widgets::plural(
                    n,
                    "copy of a photo already here",
                    "copies of photos already here"
                )
            ),
            Self::Missing => format!(
                "Remove {} from the catalog",
                crate::app::ui::widgets::plural(n, "missing photo", "missing photos")
            ),
            Self::Choice => format!(
                "{} found more than once",
                crate::app::ui::widgets::plural(n, "photo", "photos")
            ),
        }
    }
    /// What the card does, in a line.
    fn explanation(self) -> &'static str {
        match self {
            Self::Moved => {
                "Found elsewhere with exactly the same content. They keep their ratings, keywords, edits and virtual copies."
            }
            Self::Probable => {
                "Look like photos that moved, but no full checksum was on record to prove it. Tick the ones you recognize; their edits open only if the file passes the usual check."
            }
            Self::New => "Added as Add Photo Folder adds them, with their sidecar metadata.",
            Self::Copies => {
                "Exact copies of photos already in the catalog, usually duplicates. Left out unless you tick them."
            }
            Self::Missing => {
                "Not found on disk. Removing takes their edits, ratings and keywords out of the catalog; files elsewhere are not touched."
            }
            Self::Choice => {
                "Each of these photos is missing, and more than one file has exactly its content. Pick the file that is the photo, or leave it to decide later."
            }
        }
    }
    fn icon(self) -> Icon {
        match self {
            Self::Moved | Self::Probable => Icon::Folder,
            Self::New | Self::Copies => Icon::FolderPlus,
            Self::Missing => Icon::Close,
            Self::Choice => Icon::Check,
        }
    }
    fn tint(self, palette: &theme::Palette) -> egui::Color32 {
        match self {
            Self::Moved | Self::New => palette.accent(),
            Self::Probable | Self::Copies => palette.tone(Tone::LABEL),
            Self::Missing => palette.danger(),
            Self::Choice => palette.warning(),
        }
    }
}

/// A thin rule across the dialog.
fn separator(ui: &mut egui::Ui, palette: &theme::Palette) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.), egui::Sense::hover());
    ui.painter().hline(
        rect.x_range(),
        rect.center().y,
        egui::Stroke::new(1., palette.tone(Tone::RAISED)),
    );
}

/// A card: a rounded panel a little lighter than the dialog.
fn card(ui: &mut egui::Ui, palette: &theme::Palette, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(palette.tone(Tone::FIELD))
        .stroke(egui::Stroke::new(1., palette.tone(Tone::OUTLINE)))
        .corner_radius(radius::CARD)
        .inner_margin(inset::CARD)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = egui::vec2(space::M, space::S);
            add(ui);
        });
}

/// The kind's icon in a soft disc of its colour.
fn kind_icon(ui: &mut egui::Ui, palette: &theme::Palette, kind: Kind) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(22., 22.), egui::Sense::hover());
    let tint = kind.tint(palette);
    ui.painter()
        .circle_filled(rect.center(), 11., tint.gamma_multiply(0.18));
    if kind == Kind::Choice {
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "?",
            egui::FontId::proportional(text::BODY),
            tint,
        );
    } else {
        crate::app::ui::icons::paint_at(ui.painter(), kind.icon(), rect.center(), 13., tint);
    }
}

/// A card's header: its box (mixed when only some rows are ticked), icon,
/// title and the explanation, and a Show/Hide toggle for its rows. Returns
/// whether the box was toggled and whether the list was.
fn card_header(
    ui: &mut egui::Ui,
    palette: &theme::Palette,
    kind: Kind,
    n: usize,
    all: &mut bool,
    some: bool,
    open: bool,
) -> (bool, bool) {
    let mut toggled = false;
    let mut expand = false;
    card(ui, palette, |ui| {
        ui.horizontal(|ui| {
            if check(ui, palette, *all, some && !*all, true).clicked() {
                *all = !*all;
                toggled = true;
            }
            kind_icon(ui, palette, kind);
            ui.label(
                egui::RichText::new(kind.title(n))
                    .size(text::BODY)
                    .color(palette.tone(Tone::TEXT)),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let text = if open { "Hide" } else { "Show" };
                let link = ui.add(
                    egui::Button::new(
                        egui::RichText::new(text)
                            .size(text::LABEL)
                            .color(palette.tone(Tone::LABEL)),
                    )
                    .frame(false),
                );
                expand = link.clicked();
            });
        });
        ui.horizontal(|ui| {
            ui.add_space(space::XXXL);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(kind.explanation())
                        .size(text::LABEL)
                        .color(palette.tone(Tone::MUTED)),
                )
                .wrap(),
            );
        });
    });
    (toggled, expand)
}

/// A card's rows, indented under it.
fn list(ui: &mut egui::Ui, palette: &theme::Palette, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(palette.tone(Tone::CARD))
        .corner_radius(radius::CARD)
        .inner_margin(design::margin(space::L, space::S))
        .outer_margin(egui::Margin {
            left: space::XL as i8,
            right: 0,
            top: -(space::XS as i8),
            bottom: 0,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = egui::vec2(space::M, space::XXS);
            add(ui);
        });
}

/// One photo or file in a list.
struct Row<'a> {
    /// Its box and whether it can be changed; none for a row without one.
    on: Option<(&'a mut bool, bool)>,
    thumbnail: Option<&'a egui::TextureHandle>,
    title: &'a str,
    detail: &'a str,
    hover: &'a str,
    /// What the photo holds: rating, Edited, Keywords…
    chips: &'a [String],
    /// Why it can't be ticked, or what it is.
    note: Option<&'a str>,
}
fn row(ui: &mut egui::Ui, palette: &theme::Palette, row: Row<'_>) {
    ui.horizontal(|ui| {
        ui.set_min_height(44.);
        if let Some((on, enabled)) = row.on
            && check(ui, palette, *on, false, enabled).clicked()
            && enabled
        {
            *on = !*on;
        }
        let (rect, _) = ui.allocate_exact_size(egui::vec2(52., 36.), egui::Sense::hover());
        match row.thumbnail {
            Some(texture) => {
                let size = texture.size_vec2();
                let fit = (rect.width() / size.x).min(rect.height() / size.y);
                let image = egui::Rect::from_center_size(rect.center(), size * fit);
                ui.painter().image(
                    texture.id(),
                    image,
                    egui::Rect::from_min_max(egui::pos2(0., 0.), egui::pos2(1., 1.)),
                    egui::Color32::WHITE,
                );
            }
            None => {
                ui.painter().rect_filled(
                    rect.shrink(2.),
                    radius::CONTROL,
                    palette.tone(Tone::RAISED),
                );
            }
        }
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = space::HAIR;
            ui.add_space(space::XS);
            ui.horizontal(|ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(row.title)
                            .size(text::BODY)
                            .color(palette.tone(Tone::TEXT)),
                    )
                    .truncate(),
                )
                .on_hover_text(row.hover);
                for chip_text in row.chips {
                    chip(ui, palette, chip_text);
                }
            });
            let mut detail = row.detail.to_string();
            if let Some(note) = row.note {
                if !detail.is_empty() {
                    detail.push_str(" \u{b7} ");
                }
                detail.push_str(note);
            }
            ui.add(
                egui::Label::new(
                    egui::RichText::new(detail)
                        .size(text::LABEL)
                        .color(palette.tone(Tone::MUTED)),
                )
                .truncate(),
            );
        });
    });
}

/// A small rounded label: "★★★", "Edited".
fn chip(ui: &mut egui::Ui, palette: &theme::Palette, text: &str) {
    egui::Frame::new()
        .fill(palette.tone(Tone::OUTLINE))
        .corner_radius(radius::CONTROL)
        .inner_margin(design::margin(space::S, space::HAIR))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(text)
                    .size(text::SMALL)
                    .color(palette.tone(Tone::SOFT)),
            );
        });
}

/// A box drawn as the app draws its own: accent when on, a dash when only
/// some of what it covers is on.
fn check(
    ui: &mut egui::Ui,
    palette: &theme::Palette,
    on: bool,
    mixed: bool,
    enabled: bool,
) -> egui::Response {
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(18., 18.), sense);
    let rect = egui::Rect::from_center_size(rect.center(), egui::vec2(16., 16.));
    let painter = ui.painter();
    let filled = (on || mixed) && enabled;
    if filled {
        painter.rect_filled(rect, radius::CONTROL, palette.accent());
    } else {
        let stroke = if response.hovered() {
            palette.tone(Tone::SECONDARY)
        } else {
            palette.tone(Tone::DISABLED)
        };
        painter.rect_filled(rect, radius::CONTROL, palette.tone(Tone::CARD));
        painter.rect_stroke(
            rect,
            4.,
            egui::Stroke::new(1., stroke),
            egui::StrokeKind::Inside,
        );
    }
    if filled && on {
        crate::app::ui::icons::paint_at(
            painter,
            Icon::Check,
            rect.center(),
            12.,
            palette.on_accent(),
        );
    } else if filled {
        painter.hline(
            rect.shrink(4.).x_range(),
            rect.center().y,
            egui::Stroke::new(2., palette.on_accent()),
        );
    }
    response.on_hover_cursor(if enabled {
        egui::CursorIcon::PointingHand
    } else {
        egui::CursorIcon::NotAllowed
    })
}

/// One answer to a choice, on one line: a round button, the answer, and
/// in grey why it is one.
fn option(
    ui: &mut egui::Ui,
    palette: &theme::Palette,
    selected: bool,
    title: &str,
    detail: &str,
    enabled: bool,
) -> egui::Response {
    let fill = if selected {
        palette.accent().gamma_multiply(0.14)
    } else {
        palette.tone(Tone::CARD)
    };
    let stroke = if selected {
        palette.accent().gamma_multiply(0.8)
    } else {
        palette.tone(Tone::RAISED)
    };
    let inner = egui::Frame::new()
        .fill(fill)
        .stroke(egui::Stroke::new(1., stroke))
        .corner_radius(radius::CARD)
        .inner_margin(design::margin(space::L, space::S))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(16., 16.), egui::Sense::hover());
                let painter = ui.painter();
                painter.circle_stroke(
                    rect.center(),
                    6.5,
                    egui::Stroke::new(
                        1.5,
                        if selected {
                            palette.accent()
                        } else {
                            palette.tone(Tone::DISABLED)
                        },
                    ),
                );
                if selected {
                    painter.circle_filled(rect.center(), 3.5, palette.accent());
                }
                let color = if enabled {
                    palette.tone(Tone::TEXT)
                } else {
                    palette.tone(Tone::FAINT)
                };
                ui.label(egui::RichText::new(title).size(text::LABEL).color(color));
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(detail)
                            .size(text::LABEL)
                            .color(palette.tone(Tone::MUTED)),
                    )
                    .truncate(),
                );
            });
        });
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    ui.interact(inner.response.rect, ui.id().with((title, detail)), sense)
        .on_hover_cursor(if enabled {
            egui::CursorIcon::PointingHand
        } else {
            egui::CursorIcon::NotAllowed
        })
}

/// The deepest folder holding every file the plan names, for showing
/// paths from it; none when they share no more than the disk.
fn common_folder(plan: &SyncPlan) -> Option<PathBuf> {
    let mut dirs = plan
        .moves
        .iter()
        .chain(plan.choices.iter().flat_map(|c| c.options.iter()))
        .flat_map(|m| [m.from.parent(), m.to.parent()])
        .chain(plan.additions.iter().map(|a| a.file.parent()))
        .chain(plan.missing.iter().map(|m| m.path.parent()))
        .flatten();
    let mut common = dirs.next()?.to_path_buf();
    for dir in dirs {
        while !dir.starts_with(&common) {
            if !common.pop() {
                return None;
            }
        }
    }
    (common.components().count() > 2).then_some(common)
}

/// A file's name.
fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

/// `n` with thousands separated: 1,240.
fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Modal;
    use crate::app::library::Module;
    use crate::app::shell::messages::Level;
    use crate::catalog::ReadChecksum;
    use crate::storage::{ChecksumRead, FileChecksum};
    use std::time::{Duration, Instant};

    /// Distinct synthetic bytes, larger than the quick hash's blocks.
    fn bytes(seed: u8) -> Vec<u8> {
        (0..300_000u32)
            .map(|i| (i as u8).wrapping_mul(seed) ^ seed)
            .collect()
    }
    /// An editor on a catalog of a folder "trip" with two photos, their
    /// checksums on record.
    fn editor() -> anyhow::Result<(tempfile::TempDir, Editor, PathBuf)> {
        let dir = tempfile::tempdir()?;
        let trip = dir.path().canonicalize()?.join("trip");
        std::fs::create_dir_all(&trip)?;
        std::fs::write(trip.join("a.dng"), bytes(1))?;
        std::fs::write(trip.join("b.dng"), bytes(2))?;
        let path = dir.path().join("photos.rawmakase");
        let mut catalog = crate::catalog::Catalog::create(&path)?;
        catalog.add_folder(&trip)?;
        let read: Vec<ReadChecksum> = catalog
            .photos()?
            .into_iter()
            .map(|p| {
                let ChecksumRead::Done(checksum) = FileChecksum::read_full(&p.path, |_| true)?
                else {
                    anyhow::bail!("not read");
                };
                Ok(ReadChecksum {
                    photo: p.id,
                    folder: p.folder,
                    filename: p.filename,
                    path: p.path,
                    checksum,
                })
            })
            .collect::<anyhow::Result<_>>()?;
        catalog.save_checksums(&read)?;
        drop(catalog);
        let ctx = egui::Context::default();
        let mut e = Editor::with_context(
            &ctx,
            None,
            crate::app::preferences::session::Session::default(),
            None,
        );
        e.library = Some(Box::new(crate::app::library::Library::load(&path, ctx)?));
        e.module = Module::Library;
        Ok((dir, e, trip))
    }
    fn id(e: &Editor, name: &str) -> PhotoId {
        let library = e.library.as_ref().unwrap();
        library
            .session
            .photos()
            .iter()
            .find(|p| p.filename == name)
            .unwrap()
            .id
    }
    /// Delivers events until the scan is over.
    fn scanned(e: &mut Editor) {
        let ctx = e.context.clone();
        let until = Instant::now() + Duration::from_secs(10);
        while e.folder_sync.scan.is_some() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
            e.events(&ctx);
            e.poll_folder_sync();
        }
        assert!(e.folder_sync.scan.is_none(), "the scan never finished");
    }
    /// Delivers events until applying is over.
    fn applied(e: &mut Editor) {
        let ctx = e.context.clone();
        let until = Instant::now() + Duration::from_secs(10);
        while e.activity.is_busy() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
            e.events(&ctx);
        }
        assert!(!e.activity.is_busy(), "applying never finished");
    }

    #[test]
    fn a_renamed_photo_is_reviewed_and_relinked_with_its_rating() -> anyhow::Result<()> {
        let (_dir, mut e, trip) = editor()?;
        let a = id(&e, "a.dng");
        e.library
            .as_mut()
            .unwrap()
            .session
            .set_ratings(&[(a, 4, 0, String::new())])?;
        std::fs::rename(trip.join("a.dng"), trip.join("renamed.dng"))?;
        std::fs::write(trip.join("new.dng"), bytes(3))?;
        e.start_folder_sync(SyncScope::All, "trip".into());
        scanned(&mut e);
        // The review opens on its own in the Library.
        e.poll_folder_sync();
        assert!(e.folder_sync.showing);
        let ctx = e.context.clone();
        e.apply_folder_sync(&ctx);
        applied(&mut e);
        let library = e.library.as_ref().unwrap();
        let photo = library.photo(a).unwrap();
        assert_eq!(
            (photo.path.clone(), photo.rating),
            (trip.join("renamed.dng"), 4)
        );
        assert!(
            e.messages
                .line()
                .starts_with("Synchronized · 1 updated · 1 added"),
            "{}",
            e.messages.line()
        );
        // The photo added is Previous Import.
        assert_eq!(library.session.previous_import().len(), 1);
        assert!(library.offer_previous_import);
        Ok(())
    }

    #[test]
    fn a_folder_the_scan_could_not_read_makes_the_result_a_warning() -> anyhow::Result<()> {
        let (_dir, mut e, trip) = editor()?;
        std::fs::write(trip.join("new.dng"), bytes(3))?;
        e.start_folder_sync(SyncScope::All, "trip".into());
        scanned(&mut e);
        e.poll_folder_sync();
        let review = e.folder_sync.review.as_mut().expect("a review");
        review
            .plan
            .not_checked
            .push(crate::catalog::folder_sync::NotChecked {
                path: trip.join("locked"),
                reason: "permission denied".into(),
            });
        let ctx = e.context.clone();
        e.apply_folder_sync(&ctx);
        applied(&mut e);
        e.take_library_messages();
        assert_eq!(e.messages.level(), Level::Warning, "{}", e.messages.line());
        Ok(())
    }

    #[test]
    fn a_scan_from_before_a_structural_change_is_not_offered() -> anyhow::Result<()> {
        let (_dir, mut e, trip) = editor()?;
        std::fs::rename(trip.join("a.dng"), trip.join("renamed.dng"))?;
        e.start_folder_sync(SyncScope::All, "trip".into());
        // A folder change while it scans.
        e.library.as_mut().unwrap().session.reload()?;
        scanned(&mut e);
        assert!(e.folder_sync.review.is_none());
        assert!(
            e.messages.line().contains("changed during the scan"),
            "{}",
            e.messages.line()
        );
        Ok(())
    }

    #[test]
    fn a_failed_synchronization_starts_the_reads_it_stopped_again() -> anyhow::Result<()> {
        let (_dir, mut e, _) = editor()?;
        let library = e.library.as_mut().unwrap();
        // A batch whose bursts are still to be stacked.
        library.session.catalog().db_for_tests().execute_batch(
            "INSERT INTO import_burst_stacks(batch, seconds) SELECT id, 2.0 FROM import_batches",
        )?;
        // Applying stops them; it fails.
        library.stop_background_reads();
        assert!(e.activity.begin_folder_change());
        e.folder_synced(Err("boom".into()));
        assert!(!e.activity.is_busy());
        assert!(
            e.library.as_ref().unwrap().session.reading_import_stacks(),
            "read again"
        );
        Ok(())
    }

    #[test]
    fn a_scan_thread_lost_without_a_result_clears_the_top_bar() -> anyhow::Result<()> {
        let (_dir, mut e, _) = editor()?;
        let thread = std::thread::spawn(|| {});
        while !thread.is_finished() {
            std::thread::yield_now();
        }
        e.folder_sync.scan = Some(Scan {
            id: 99,
            name: "trip".into(),
            root: None,
            finding: false,
            catalog: e
                .library
                .as_ref()
                .unwrap()
                .session
                .catalog()
                .location()
                .clone(),
            generation: 0,
            files: Default::default(),
            cancel: Default::default(),
            sent: Default::default(),
            thread: Some(thread),
        });
        e.poll_folder_sync();
        assert!(!e.folder_sync.is_scanning());
        assert!(e.messages.line().contains("stopped unexpectedly"));
        Ok(())
    }

    #[test]
    fn the_open_photo_moved_outside_keeps_its_pending_edit_and_saves_it_there() -> anyhow::Result<()>
    {
        let (_dir, mut e, trip) = editor()?;
        let a = id(&e, "a.dng");
        // Open in Develop with an edit not yet saved.
        e.module = Module::Develop;
        e.document.catalog_photo = Some(a);
        e.document.opened_at(trip.join("a.dng"));
        e.document.edit.change(None, |r| r.exposure = 0.8);
        let steps = e.document.edit.history().steps().0.len();
        std::fs::rename(trip.join("a.dng"), trip.join("moved.dng"))?;
        // Saving to the old path fails now.
        assert!(!e.flush());
        e.start_folder_sync(SyncScope::All, "trip".into());
        scanned(&mut e);
        let ctx = e.context.clone();
        e.apply_folder_sync(&ctx);
        applied(&mut e);
        assert_eq!(
            e.document.path.as_deref(),
            Some(trip.join("moved.dng").as_path())
        );
        assert_eq!(e.document.catalog_photo, Some(a));
        assert_eq!(e.document.edit.history().steps().0.len(), steps);
        assert!(e.module == Module::Develop);
        // The save goes through on the new path.
        assert!(e.flush());
        let library = e.library.as_ref().unwrap();
        let saved = library
            .session
            .catalog()
            .load_edit(a, &trip.join("moved.dng"))?
            .expect("saved");
        assert_eq!(saved.recipe.exposure, 0.8);
        Ok(())
    }

    #[test]
    fn the_open_photo_whose_move_is_skipped_stays_open_with_its_edit() -> anyhow::Result<()> {
        let (_dir, mut e, trip) = editor()?;
        let a = id(&e, "a.dng");
        e.module = Module::Develop;
        e.document.catalog_photo = Some(a);
        e.document.opened_at(trip.join("a.dng"));
        e.document.edit.change(None, |r| r.exposure = 0.6);
        std::fs::rename(trip.join("a.dng"), trip.join("moved.dng"))?;
        e.start_folder_sync(SyncScope::All, "trip".into());
        scanned(&mut e);
        // The file found changes before applying: its move is skipped.
        let later = std::time::SystemTime::now() + Duration::from_secs(30);
        std::fs::File::options()
            .write(true)
            .open(trip.join("moved.dng"))?
            .set_modified(later)?;
        let ctx = e.context.clone();
        e.apply_folder_sync(&ctx);
        applied(&mut e);
        assert_eq!(e.document.catalog_photo, Some(a));
        assert_eq!(
            e.document.path.as_deref(),
            Some(trip.join("a.dng").as_path())
        );
        assert_eq!(e.document.edit.recipe().exposure, 0.6);
        assert!(e.document.edit.save_state().needs_save());
        assert!(e.module == Module::Develop);
        Ok(())
    }

    #[test]
    fn removing_a_photo_with_keywords_asks_first() -> anyhow::Result<()> {
        let (_dir, mut e, trip) = editor()?;
        let a = id(&e, "a.dng");
        let library = e.library.as_mut().unwrap();
        library
            .session
            .catalog_mut_for_tests()
            .add_keywords(&[a], &[vec!["Trip".into()]])?;
        library.session.reload()?;
        std::fs::remove_file(trip.join("a.dng"))?;
        e.start_folder_sync(SyncScope::All, "trip".into());
        scanned(&mut e);
        let review = e.folder_sync.review.as_mut().expect("a review");
        review.removals.iter_mut().for_each(|r| *r = true);
        let losses = review.removal_losses();
        assert_eq!(losses.len(), 1);
        assert!(losses[0].contains("Keywords"), "{losses:?}");
        Ok(())
    }

    #[test]
    fn review_opens_in_both_modes_after_other_dialogs_close() -> anyhow::Result<()> {
        for module in [Module::Library, Module::Develop] {
            let (_dir, mut e, trip) = editor()?;
            e.module = module;
            e.preferences.open = true;
            std::fs::remove_file(trip.join("a.dng"))?;
            e.start_folder_sync(SyncScope::All, "trip".into());
            scanned(&mut e);
            assert!(e.folder_sync.review.is_some());
            assert!(!e.folder_sync.showing);

            e.preferences.open = false;
            e.poll_folder_sync();
            assert!(e.folder_sync.showing);
            assert!(e.command_modal());
            assert!(e.module == module);

            // A dismissed review must not reopen on the next frame.
            e.folder_sync.showing = false;
            e.poll_folder_sync();
            assert!(!e.folder_sync.showing);
        }
        Ok(())
    }

    #[test]
    fn the_review_is_modal() -> anyhow::Result<()> {
        let (_dir, mut e, trip) = editor()?;
        std::fs::remove_file(trip.join("a.dng"))?;
        e.start_folder_sync(SyncScope::All, "trip".into());
        scanned(&mut e);
        e.poll_folder_sync();
        assert!(e.folder_sync.showing);
        // Keys and commands go to the dialog, not the photos behind it.
        assert!(e.command_modal());
        Ok(())
    }

    #[test]
    fn a_reference_photo_removed_is_no_longer_shown() -> anyhow::Result<()> {
        let (_dir, mut e, trip) = editor()?;
        let b = id(&e, "b.dng");
        e.reference.photo = Some(b);
        std::fs::remove_file(trip.join("b.dng"))?;
        e.start_folder_sync(SyncScope::All, "trip".into());
        scanned(&mut e);
        let review = e.folder_sync.review.as_mut().expect("a review");
        review.removals.iter_mut().for_each(|r| *r = true);
        let ctx = e.context.clone();
        e.apply_folder_sync(&ctx);
        applied(&mut e);
        assert_eq!(e.reference.photo, None);
        Ok(())
    }

    #[test]
    fn an_empty_folder_gone_from_disk_is_offered_and_removed() -> anyhow::Result<()> {
        let (_dir, mut e, trip) = editor()?;
        // A folder whose one photo moved away, then the folder itself.
        std::fs::create_dir_all(trip.join("day 2"))?;
        std::fs::write(trip.join("day 2/c.dng"), bytes(5))?;
        e.library
            .as_mut()
            .unwrap()
            .session
            .catalog_mut_for_tests()
            .add_folder(&trip)?;
        e.library.as_mut().unwrap().session.reload()?;
        let c = id(&e, "c.dng");
        e.library
            .as_mut()
            .unwrap()
            .session
            .catalog()
            .db_for_tests()
            .execute("DELETE FROM photos WHERE id=?", [c.0])?;
        e.library.as_mut().unwrap().session.reload()?;
        std::fs::remove_dir_all(trip.join("day 2"))?;
        e.start_folder_sync(SyncScope::All, "trip".into());
        scanned(&mut e);
        let review = e.folder_sync.review.as_ref().expect("offered, not in sync");
        assert_eq!(review.plan.gone_folders.len(), 1);
        assert!(!review.accepted_nothing());
        let ctx = e.context.clone();
        e.apply_folder_sync(&ctx);
        applied(&mut e);
        let folders = e.library.as_ref().unwrap().session.folders();
        assert!(!folders.iter().any(|f| f.relative == "day 2"));
        Ok(())
    }

    /// Renders the review of a scenario with every kind of change to
    /// `target/folder-sync-review.png`, for looking at the design.
    /// Moves `name` out of the trip folder to a directory outside every
    /// root, as another volume would: new time. Returns its new path.
    fn move_out(trip: &std::path::Path, name: &str) -> anyhow::Result<PathBuf> {
        let out = trip.with_file_name("moved");
        std::fs::create_dir_all(&out)?;
        let to = out.join(name);
        std::fs::copy(trip.join(name), &to)?;
        let later = std::time::SystemTime::now() + Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(&to)?
            .set_modified(later)?;
        std::fs::remove_file(trip.join(name))?;
        Ok(to)
    }
    /// Opens `id` at `path` in Develop as `catalog_ready` leaves it.
    fn open_in_develop(e: &mut Editor, id: PhotoId, path: PathBuf) {
        e.module = Module::Develop;
        e.document.catalog_photo = Some(id);
        e.document.opened_at(path);
    }
    fn forget_checksum(e: &Editor, id: PhotoId) -> anyhow::Result<()> {
        let library = e.library.as_ref().unwrap();
        library
            .session
            .catalog()
            .db_for_tests()
            .execute("DELETE FROM photo_checksums WHERE photo=?", [id.0])?;
        Ok(())
    }

    #[test]
    fn the_picker_starts_in_the_nearest_folder_still_there() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let shoot = dir.path().join("shoot");
        std::fs::create_dir_all(&shoot)?;
        assert_eq!(picker_start(&shoot.join("a.dng")), Some(shoot.clone()));
        assert_eq!(
            picker_start(&shoot.join("gone/deeper/a.dng")),
            Some(shoot.clone())
        );
        Ok(())
    }

    #[test]
    fn a_verified_file_with_nothing_else_found_is_relinked_at_once() -> anyhow::Result<()> {
        let (_dir, mut e, trip) = editor()?;
        let a = id(&e, "a.dng");
        let to = move_out(&trip, "a.dng")?;
        let previous = e
            .library
            .as_ref()
            .unwrap()
            .session
            .previous_import()
            .clone();
        e.start_find_on_disk(a, to.clone());
        scanned(&mut e);
        applied(&mut e);
        assert!(e.folder_sync.review.is_none());
        let library = e.library.as_ref().unwrap();
        assert_eq!(library.photo(a).unwrap().path, to);
        assert!(
            e.messages.line().starts_with("Relinked 1 photo"),
            "{}",
            e.messages.line()
        );
        assert_eq!(library.session.previous_import(), &previous);
        assert!(!library.offer_previous_import);
        Ok(())
    }

    #[test]
    fn other_photos_found_beside_it_are_reviewed() -> anyhow::Result<()> {
        let (_dir, mut e, trip) = editor()?;
        let (a, b) = (id(&e, "a.dng"), id(&e, "b.dng"));
        let to = move_out(&trip, "a.dng")?;
        let other = move_out(&trip, "b.dng")?;
        e.start_find_on_disk(a, to.clone());
        scanned(&mut e);
        e.poll_folder_sync();
        assert!(e.folder_sync.showing);
        let review = e.folder_sync.review.as_ref().unwrap();
        assert_eq!(review.title(), "Found 1 other missing photo in this folder");
        assert_eq!(review.ticked(), 2);
        let ctx = e.context.clone();
        e.apply_folder_sync(&ctx);
        applied(&mut e);
        let library = e.library.as_ref().unwrap();
        assert_eq!(library.photo(a).unwrap().path, to);
        assert_eq!(library.photo(b).unwrap().path, other);
        assert!(
            e.messages.line().starts_with("Relinked 2 photos"),
            "{}",
            e.messages.line()
        );
        Ok(())
    }

    #[test]
    fn a_photo_whose_drive_is_gone_is_always_reviewed() -> anyhow::Result<()> {
        let (_dir, mut e, trip) = editor()?;
        let a = id(&e, "a.dng");
        // The whole folder went elsewhere: its old place can't be listed,
        // so the photo is unavailable, not observed missing.
        let elsewhere = trip.with_file_name("elsewhere");
        std::fs::rename(&trip, &elsewhere)?;
        e.start_find_on_disk(a, elsewhere.join("a.dng"));
        scanned(&mut e);
        e.poll_folder_sync();
        assert!(!e.activity.is_busy());
        assert!(e.folder_sync.showing, "{}", e.messages.line());
        let review = e.folder_sync.review.as_ref().unwrap();
        assert!(review.plan.moves.iter().all(|m| m.offline && m.verified));
        Ok(())
    }

    #[test]
    fn a_different_file_is_refused_and_nothing_stays_busy() -> anyhow::Result<()> {
        let (_dir, mut e, trip) = editor()?;
        let a = id(&e, "a.dng");
        std::fs::remove_file(trip.join("a.dng"))?;
        let out = trip.with_file_name("moved");
        std::fs::create_dir_all(&out)?;
        std::fs::write(out.join("a.dng"), bytes(9))?;
        e.start_find_on_disk(a, out.join("a.dng"));
        scanned(&mut e);
        assert!(e.folder_sync.review.is_none() && !e.folder_sync.is_scanning());
        assert!(!e.activity.is_busy());
        assert!(
            e.messages.line().contains("a different photo than a.dng"),
            "{}",
            e.messages.line()
        );
        Ok(())
    }

    #[test]
    fn a_stopped_panicked_or_lost_search_clears_the_top_bar() -> anyhow::Result<()> {
        let (_dir, mut e, _) = editor()?;
        // Panicked.
        e.spawn_scan("a.dng".into(), None, true, |_, _| panic!("synthetic"));
        scanned(&mut e);
        assert!(
            e.messages.line().contains("Stopped unexpectedly"),
            "{}",
            e.messages.line()
        );
        // Stopped.
        e.spawn_scan("a.dng".into(), None, true, |stopped, _| {
            while !stopped.is_cancelled() {
                std::thread::sleep(Duration::from_millis(5));
            }
            anyhow::bail!("Find on Disk stopped")
        });
        e.folder_sync
            .scan
            .as_ref()
            .unwrap()
            .cancel
            .store(true, Ordering::Relaxed);
        scanned(&mut e);
        assert_eq!(e.messages.line(), "Find on Disk stopped");
        // Lost without a result.
        let thread = std::thread::spawn(|| {});
        while !thread.is_finished() {
            std::thread::yield_now();
        }
        e.folder_sync.scan = Some(Scan {
            id: 99,
            name: "a.dng".into(),
            root: None,
            finding: true,
            catalog: e
                .library
                .as_ref()
                .unwrap()
                .session
                .catalog()
                .location()
                .clone(),
            generation: 0,
            files: Default::default(),
            cancel: Default::default(),
            sent: Default::default(),
            thread: Some(thread),
        });
        e.poll_folder_sync();
        assert!(!e.folder_sync.is_scanning());
        assert!(
            e.messages
                .line()
                .contains("Find on Disk stopped unexpectedly")
        );
        assert!(!e.activity.is_busy());
        Ok(())
    }

    #[test]
    fn the_open_photo_follows_a_verified_relink_and_saves_its_edit_there() -> anyhow::Result<()> {
        let (_dir, mut e, trip) = editor()?;
        let a = id(&e, "a.dng");
        open_in_develop(&mut e, a, trip.join("a.dng"));
        e.document.edit.change(None, |r| r.exposure = 0.7);
        let to = move_out(&trip, "a.dng")?;
        assert!(!e.flush());
        e.start_find_on_disk(a, to.clone());
        scanned(&mut e);
        applied(&mut e);
        assert_eq!(e.document.path.as_deref(), Some(to.as_path()));
        assert_eq!(e.document.catalog_photo, Some(a));
        assert!(e.module == Module::Develop);
        assert!(e.flush());
        let library = e.library.as_ref().unwrap();
        let saved = library.session.catalog().load_edit(a, &to)?.expect("saved");
        assert_eq!(saved.recipe.exposure, 0.7);
        // A save that fails at the new path keeps the edit pending.
        e.document.edit.change(None, |r| r.exposure = 0.9);
        std::fs::remove_file(&to)?;
        assert!(!e.flush());
        assert!(e.document.edit.save_state().needs_save());
        assert_eq!(e.document.edit.recipe().exposure, 0.9);
        Ok(())
    }

    #[test]
    fn an_unverified_file_for_the_open_photo_is_refused_until_another_is_opened()
    -> anyhow::Result<()> {
        for open_copy in [false, true] {
            let (_dir, mut e, trip) = editor()?;
            let (a, b) = (id(&e, "a.dng"), id(&e, "b.dng"));
            let open = if open_copy {
                let copy = e
                    .library
                    .as_mut()
                    .unwrap()
                    .session
                    .catalog_mut_for_tests()
                    .create_virtual_copy(a)?;
                e.library.as_mut().unwrap().session.reload()?;
                copy
            } else {
                a
            };
            forget_checksum(&e, a)?;
            open_in_develop(&mut e, open, trip.join("a.dng"));
            e.document.edit.change(None, |r| r.exposure = 0.4);
            let to = move_out(&trip, "a.dng")?;
            let refused = |e: &mut Editor| {
                e.start_find_on_disk(a, to.clone());
                scanned(e);
                assert!(e.folder_sync.review.is_none());
                assert!(!e.activity.is_busy());
                assert!(
                    e.messages.line().contains("is open in Develop"),
                    "{}",
                    e.messages.line()
                );
                let library = e.library.as_ref().unwrap();
                assert_eq!(library.photo(a).unwrap().path, trip.join("a.dng"));
            };
            // Its edit can't be saved, its file gone: discarding it is
            // asked, and Cancel keeps it.
            e.start_find_on_disk(a, to.clone());
            scanned(&mut e);
            assert!(matches!(e.modal, Some(Modal::DiscardForFind(_))));
            e.answer_discard_for_find(false);
            assert!(e.modal.is_none());
            assert!(e.folder_sync.review.is_none());
            assert!(
                e.messages.line().contains("keeps its unsaved change"),
                "{}",
                e.messages.line()
            );
            assert_eq!(e.document.edit.recipe().exposure, 0.4);
            assert!(e.document.edit.save_state().needs_save());
            assert_eq!(
                e.document.path.as_deref(),
                Some(trip.join("a.dng").as_path())
            );
            assert_eq!(
                e.library.as_ref().unwrap().photo(a).unwrap().path,
                trip.join("a.dng")
            );
            // The Library shown, the document still loaded, its edit clean.
            e.document.edit.save_state_mut().saved();
            e.module = Module::Library;
            refused(&mut e);
            // Another family open in Develop: the same file relinks.
            open_in_develop(&mut e, b, trip.join("b.dng"));
            e.start_find_on_disk(a, to.clone());
            scanned(&mut e);
            // Not verified: reviewed, never applied at once.
            assert!(!e.activity.is_busy());
            e.poll_folder_sync();
            assert!(e.folder_sync.showing, "{}", e.messages.line());
            let ctx = e.context.clone();
            e.apply_folder_sync(&ctx);
            applied(&mut e);
            let library = e.library.as_ref().unwrap();
            assert_eq!(
                library.photo(a).unwrap().path,
                to,
                "{:?}",
                library.messages.detail()
            );
        }
        Ok(())
    }

    #[test]
    fn an_unsaved_change_on_a_gone_file_can_be_discarded_for_an_unverified_file()
    -> anyhow::Result<()> {
        let (_dir, mut e, trip) = editor()?;
        let a = id(&e, "a.dng");
        forget_checksum(&e, a)?;
        open_in_develop(&mut e, a, trip.join("a.dng"));
        e.document.edit.change(None, |r| r.exposure = 0.4);
        let to = move_out(&trip, "a.dng")?;
        // Leaving the photo saves first, which fails: it can't be left.
        assert!(!e.flush());
        e.start_find_on_disk(a, to.clone());
        scanned(&mut e);
        assert!(matches!(e.modal, Some(Modal::DiscardForFind(_))));
        // The file put back meanwhile: the change can be saved again, and
        // is never discarded.
        std::fs::copy(&to, trip.join("a.dng"))?;
        e.answer_discard_for_find(true);
        assert!(e.modal.is_none());
        assert_eq!(e.document.catalog_photo, Some(a));
        assert_eq!(e.document.edit.recipe().exposure, 0.4);
        assert!(e.document.edit.save_state().needs_save());
        assert!(e.folder_sync.review.is_none());
        assert!(
            e.messages.line().contains("is open in Develop"),
            "{}",
            e.messages.line()
        );
        std::fs::remove_file(trip.join("a.dng"))?;
        // Asked over no other dialog: the status says why instead.
        e.modal = Some(Modal::RemoveCopy(a));
        e.start_find_on_disk(a, to.clone());
        scanned(&mut e);
        assert!(matches!(e.modal, Some(Modal::RemoveCopy(_))));
        assert!(
            e.messages.line().contains("is open in Develop"),
            "{}",
            e.messages.line()
        );
        e.modal = None;
        e.start_find_on_disk(a, to.clone());
        scanned(&mut e);
        assert!(matches!(e.modal, Some(Modal::DiscardForFind(_))));
        // Nothing is reviewed while the question is open.
        e.poll_folder_sync();
        assert!(!e.folder_sync.showing);
        e.answer_discard_for_find(true);
        // The photo is closed, its change gone, and the file reviewed.
        assert!(e.modal.is_none());
        assert_eq!(e.document.catalog_photo, None);
        assert!(!e.document.edit.save_state().needs_save());
        assert!(e.module == Module::Library);
        e.poll_folder_sync();
        assert!(e.folder_sync.showing, "{}", e.messages.line());
        let ctx = e.context.clone();
        e.apply_folder_sync(&ctx);
        applied(&mut e);
        let library = e.library.as_ref().unwrap();
        assert_eq!(library.photo(a).unwrap().path, to);
        // The discarded change was never saved.
        assert!(library.session.catalog().load_edit(a, &to)?.is_none());
        Ok(())
    }

    #[test]
    #[ignore = "writes an image to look at"]
    fn render_the_review() -> anyhow::Result<()> {
        let (_dir, mut e, trip) = editor()?;
        let ctx = e.context.clone();
        crate::app::install_fonts(&ctx, &fastframe_text::TextRendering::platform_default());
        crate::app::ui::icons::install(&ctx);
        let b = id(&e, "b.dng");
        e.library
            .as_mut()
            .unwrap()
            .session
            .set_ratings(&[(b, 3, 0, String::new())])?;
        // Moved, new, a copy, a missing one and one found twice.
        std::fs::create_dir_all(trip.join("selects"))?;
        std::fs::rename(trip.join("a.dng"), trip.join("selects/keep-01.dng"))?;
        std::fs::write(trip.join("new-1.dng"), bytes(7))?;
        std::fs::write(trip.join("new-2.dng"), bytes(8))?;
        std::fs::copy(
            trip.join("selects/keep-01.dng"),
            trip.join("keep-01 copy.dng"),
        )?;
        std::fs::copy(trip.join("b.dng"), trip.join("selects/b.dng"))?;
        std::fs::rename(trip.join("b.dng"), trip.join("b-renamed.dng"))?;
        std::fs::write(trip.join("c.dng"), bytes(9))?;
        e.start_folder_sync(SyncScope::All, "Sony 7CR".into());
        scanned(&mut e);
        e.poll_folder_sync();
        let review = e.folder_sync.review.as_mut().expect("a review");
        review
            .expanded
            .extend(["verified", "new", "copies", "missing"]);
        let mut textures = Default::default();
        let image = crate::app::ui::ui_snapshot::render(
            &ctx,
            &mut textures,
            egui::vec2(1100., 1500.),
            2.,
            4,
            |ui| e.draw(ui),
        );
        let out =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/folder-sync-review.png");
        image.save(&out)?;
        eprintln!("{}", out.display());
        // The other cards, their rows shown.
        let review = e.folder_sync.review.as_mut().expect("a review");
        review.plan.choices.clear();
        review.picks.clear();
        let image = crate::app::ui::ui_snapshot::render(
            &ctx,
            &mut textures,
            egui::vec2(1100., 1500.),
            2.,
            4,
            |ui| e.draw(ui),
        );
        image.save(out.with_file_name("folder-sync-review-2.png"))?;
        Ok(())
    }
}
