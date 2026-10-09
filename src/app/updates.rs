//! The update notice: a small card under the toolbar when a newer release
//! is out, and the About rows in Preferences. fastframe-update does the
//! work on a thread of its own (`crate::updates`); this is what the user sees.
use super::Editor;
use super::widgets::{modal_frame, primary_button};
use crate::app::theme;
use crate::updates::{INTERVAL, Launch, Prepared, Release, Unsupported, Updater};
use eframe::egui::{self, Color32, Sense, Vec2};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

/// Launch settles before the first check.
const FIRST_CHECK: Duration = Duration::from_secs(5);
const WIDTH: f32 = 300.;

enum Request {
    Check { manual: bool },
    Download(Release),
    Install,
}
enum Reply {
    Checked {
        found: Result<Option<Release>, String>,
        installable: Result<(), Unsupported>,
        manual: bool,
    },
    Progress(u64, u64),
    Downloaded(Result<(), String>),
    Installing(Result<(), String>),
}

/// Where a release is on its way to being installed.
#[derive(Default)]
enum Download {
    #[default]
    Idle,
    Downloading(u64, u64),
    Ready,
    Installing,
    Failed(String),
}

#[derive(Default)]
pub(super) struct Updates {
    /// None in isolated tests, which never reach the network.
    requests: Option<Sender<Request>>,
    replies: Option<Receiver<Reply>>,
    automatic_flag: Arc<AtomicBool>,
    pub(super) automatic: bool,
    pub(super) skipped: Option<String>,
    available: Option<Release>,
    /// Why this copy cannot replace itself, e.g. a package manager owns it.
    unsupported: Option<Unsupported>,
    download: Download,
    /// Closed for this launch; a newer release shows it again.
    dismissed: bool,
    /// Check Now: None until pressed, then the check's result.
    manual: Option<Option<Result<(), String>>>,
    /// Acknowledged after the first frame, so the helper keeps the update.
    receipt: Option<fastframe_update::Receipt>,
}
impl Updates {
    pub(super) fn new(session: &crate::app::session::Session, ctx: Option<&egui::Context>) -> Self {
        let automatic = !session.no_update_checks;
        let mut updates = Self {
            automatic,
            automatic_flag: Arc::new(AtomicBool::new(automatic)),
            skipped: session.skipped_version.clone(),
            ..Default::default()
        };
        if let Some(ctx) = ctx {
            let (requests, incoming) = mpsc::channel();
            let (outgoing, replies) = mpsc::channel();
            let enabled = updates.automatic_flag.clone();
            let ctx = ctx.clone();
            std::thread::Builder::new()
                .name("updates".into())
                .spawn(move || worker(crate::updates::updater(), incoming, outgoing, enabled, ctx))
                .ok();
            updates.requests = Some(requests);
            updates.replies = Some(replies);
        }
        updates
    }
    /// Takes what the update helper left for this launch: a receipt to
    /// acknowledge, or why it restored the previous version.
    pub(super) fn launched(&mut self, launch: Launch, status: &mut String) {
        self.receipt = launch.receipt;
        if let Some(error) = launch.error {
            *status = error;
        }
    }
    /// Drops the update checker's channels at exit. It ends after the request it is
    /// working on; a download cannot be cancelled, so it is not waited for.
    pub(super) fn close(&mut self) {
        self.requests = None;
        self.replies = None;
    }
    fn send(&self, request: Request) {
        if let Some(requests) = &self.requests {
            let _ = requests.send(request);
        }
    }
}

/// Checks at launch, hourly while enabled and on request; downloads and
/// hands off when asked. The prepared update never leaves this thread.
fn worker(
    updater: Updater,
    incoming: Receiver<Request>,
    outgoing: Sender<Reply>,
    enabled: Arc<AtomicBool>,
    ctx: egui::Context,
) {
    let reply = |reply| {
        let sent = outgoing.send(reply).is_ok();
        ctx.request_repaint();
        sent
    };
    let mut next = Instant::now() + FIRST_CHECK;
    let mut prepared: Option<Prepared> = None;
    loop {
        let request = match incoming.recv_timeout(next.saturating_duration_since(Instant::now())) {
            Ok(request) => request,
            Err(RecvTimeoutError::Timeout) => {
                next = Instant::now() + INTERVAL;
                if !enabled.load(Ordering::Relaxed) {
                    continue;
                }
                Request::Check { manual: false }
            }
            Err(RecvTimeoutError::Disconnected) => return,
        };
        let sent = match request {
            Request::Check { manual } => reply(Reply::Checked {
                found: updater.check().map_err(|e| format!("{e:#}")),
                installable: updater.installation().map(|_| ()),
                manual,
            }),
            Request::Download(release) => {
                let result = updater.download(&release, |received, total| {
                    reply(Reply::Progress(received, total));
                });
                let result = result.map(|p| prepared = Some(p));
                reply(Reply::Downloaded(result.map_err(|e| format!("{e:#}"))))
            }
            Request::Install => {
                let result = match prepared.take() {
                    Some(p) => updater.handoff(p, Vec::new()),
                    None => Err(anyhow::anyhow!("The update is no longer ready")),
                };
                reply(Reply::Installing(result.map_err(|e| format!("{e:#}"))))
            }
        };
        if !sent {
            return;
        }
    }
}

enum Action {
    Dismiss,
    OpenPage,
    Download,
    Install,
    Skip,
}

impl Editor {
    pub(super) fn poll_updates(&mut self, ctx: &egui::Context) {
        if let Some(receipt) = self.updates.receipt.take() {
            // The window is up, so the helper keeps the new version.
            std::thread::spawn(move || {
                let _ = receipt.acknowledge();
            });
        }
        let updates = &mut self.updates;
        let Some(replies) = &updates.replies else {
            return;
        };
        let mut install = false;
        while let Ok(reply) = replies.try_recv() {
            match reply {
                Reply::Checked {
                    found,
                    installable,
                    manual,
                } => {
                    match &found {
                        Ok(Some(release)) => {
                            if updates.available.as_ref() != Some(release) {
                                updates.dismissed = false;
                                updates.download = Download::Idle;
                            }
                            updates.available = Some(release.clone());
                        }
                        Ok(None) => updates.available = None,
                        // Offline or GitHub unreachable: keep what we knew.
                        Err(_) => {}
                    }
                    updates.unsupported = installable.err();
                    if manual {
                        updates.manual = Some(Some(found.map(|_| ())));
                    }
                }
                Reply::Progress(received, total) => {
                    updates.download = Download::Downloading(received, total);
                }
                Reply::Downloaded(result) => {
                    updates.download = match result {
                        Ok(()) => Download::Ready,
                        Err(e) => Download::Failed(e),
                    };
                }
                Reply::Installing(Ok(())) => install = true,
                Reply::Installing(Err(e)) => updates.download = Download::Failed(e),
            }
        }
        if install {
            // The helper waits for this process to exit, then installs and
            // relaunches. Quitting goes through the usual close, which saves.
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    /// Lightroom-style notice under the toolbar on the right, over the panels
    /// so the layout never moves. Hidden behind modal windows and first-run
    /// setup.
    pub(super) fn update_notice(&mut self, ctx: &egui::Context, modal: bool) {
        let palette = theme::palette(ctx);
        let updates = &self.updates;
        let Some(release) = updates.available.clone() else {
            return;
        };
        if updates.dismissed
            || updates.skipped.as_deref() == Some(release.version.as_str())
            || modal
            || self.onboarding.visible
        {
            return;
        }
        let mut action = None;
        egui::Area::new(egui::Id::new("update-notice"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::RIGHT_TOP, Vec2::new(-16., 112.))
            .show(ctx, |ui| {
                modal_frame(&palette)
                    .inner_margin(egui::Margin::same(16))
                    .shadow(egui::Shadow {
                        offset: [0, 6],
                        blur: 24,
                        spread: 0,
                        color: Color32::from_black_alpha(110),
                    })
                    .show(ui, |ui| {
                        ui.set_width(WIDTH);
                        ui.spacing_mut().item_spacing = Vec2::new(8., 6.);
                        action = notice_contents(ui, updates, &release);
                    });
            });
        match action {
            Some(Action::Dismiss) => self.updates.dismissed = true,
            Some(Action::OpenPage) => {
                let _ = crate::platform::web::open(&release.url);
                self.updates.dismissed = true;
            }
            Some(Action::Download) => self.download_update(release),
            Some(Action::Install) => self.install_update(),
            Some(Action::Skip) => {
                self.updates.skipped = Some(release.version);
                let _ = self.save_session();
            }
            None => {}
        }
    }

    fn download_update(&mut self, release: Release) {
        self.updates.download = Download::Downloading(0, 0);
        self.updates.send(Request::Download(release));
    }

    fn install_update(&mut self) {
        if self.exporting() {
            self.status = "Finish exporting before updating".into();
            return;
        }
        // Edits are saved before the app quits for the helper.
        if !self.flush() {
            return;
        }
        self.updates.download = Download::Installing;
        self.updates.send(Request::Install);
    }

    /// The version row's controls in Preferences > General: Check Now and
    /// what it found, or the update itself.
    pub(super) fn update_status(&mut self, ui: &mut egui::Ui) {
        if self.updates.requests.is_none() {
            return;
        }
        if let Some(release) = self.updates.available.clone() {
            if ui
                .button(format!("Update to {}", release.version))
                .clicked()
            {
                if self.updates.unsupported.is_some() {
                    let _ = crate::platform::web::open(&release.url);
                } else {
                    // The notice carries the progress; show it again.
                    self.updates.dismissed = false;
                    self.updates.skipped = None;
                    self.preferences.open = false;
                    if matches!(self.updates.download, Download::Idle | Download::Failed(_)) {
                        self.download_update(release);
                    }
                }
            }
            return;
        }
        let updates = &mut self.updates;
        let checking = matches!(updates.manual, Some(None));
        if ui
            .add_enabled(!checking, egui::Button::new("Check Now"))
            .clicked()
        {
            updates.manual = Some(None);
            updates.send(Request::Check { manual: true });
        }
        match &updates.manual {
            Some(None) => {
                ui.add(egui::Spinner::new().size(12.));
                note(ui, "Checking…");
            }
            Some(Some(Ok(()))) => {
                note(ui, "RAWmakase is up to date.");
            }
            Some(Some(Err(error))) => {
                note(ui, "Couldn’t reach GitHub.").on_hover_text(error);
            }
            None => {}
        }
    }

    pub(super) fn automatic_updates_checkbox(&mut self, ui: &mut egui::Ui) {
        let updates = &mut self.updates;
        if ui
            .checkbox(&mut updates.automatic, "Check for updates automatically")
            .changed()
        {
            updates
                .automatic_flag
                .store(updates.automatic, Ordering::Relaxed);
            if updates.automatic {
                updates.send(Request::Check { manual: false });
            }
            let _ = self.save_session();
        }
    }
}

/// The notice's text and buttons for where the update is.
fn notice_contents(ui: &mut egui::Ui, updates: &Updates, release: &Release) -> Option<Action> {
    let palette = theme::palette(ui.ctx());
    let mut action = None;
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Update available")
                .size(14.)
                .color(palette.gray(236)),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if close_button(ui).on_hover_text("Remind me later").clicked() {
                action = Some(Action::Dismiss);
            }
        });
    });
    note(
        ui,
        &format!(
            "RAWmakase {} is available. You have {}.",
            release.version,
            crate::updates::config().current_version
        ),
    );
    ui.add_space(6.);
    ui.spacing_mut().button_padding = Vec2::new(12., 5.);
    match &updates.download {
        Download::Downloading(received, total) => {
            let progress = if *total > 0 {
                *received as f32 / *total as f32
            } else {
                0.
            };
            ui.add(
                egui::ProgressBar::new(progress)
                    .desired_height(6.)
                    .fill(palette.accent()),
            );
            note(ui, "Downloading…");
        }
        Download::Ready => {
            note(ui, "Ready to install. RAWmakase restarts to finish.");
            if primary_button(ui, "Restart to Update").clicked() {
                action = Some(Action::Install);
            }
        }
        Download::Installing => {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(12.));
                note(ui, "Preparing to restart…");
            });
        }
        Download::Idle | Download::Failed(_) => {
            let failed = matches!(updates.download, Download::Failed(_));
            if let Download::Failed(error) = &updates.download {
                ui.label(egui::RichText::new(error).size(12.).color(palette.danger()));
            }
            if let Some(reason) = &updates.unsupported {
                note(ui, &reason.to_string());
            }
            ui.horizontal(|ui| {
                let (label, pressed) = match (&updates.unsupported, failed) {
                    (Some(_), _) => ("Download", Action::OpenPage),
                    (None, true) => ("Try Again", Action::Download),
                    (None, false) => ("Update", Action::Download),
                };
                if primary_button(ui, label).clicked() {
                    action = Some(pressed);
                }
                let skip = egui::Button::new("Skip This Version").min_size(Vec2::new(0., 30.));
                if ui.add(skip).clicked() {
                    action = Some(Action::Skip);
                }
            });
        }
    }
    action
}

fn note(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Label::new(
            egui::RichText::new(text)
                .size(12.)
                .color(theme::palette(ui.ctx()).gray(160)),
        )
        .wrap(),
    )
}

/// An unframed ×.
fn close_button(ui: &mut egui::Ui) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(20.), Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, 4., palette.gray(48));
    }
    let color = palette.gray(if response.hovered() { 235 } else { 150 });
    super::icons::paint_at(
        ui.painter(),
        super::icons::Icon::Close,
        rect.center(),
        14.,
        color,
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}
