//! Application operations shared by UI, MIDI and the authenticated local API.
//! Adapters translate input; only the Editor executes commands and owns history.
mod output;
mod parameter;
mod preset;
mod reply;
use crate::app::Module;
use crate::app::panels::{PanelChange, WorkspacePanel};
use reply::{
    Capabilities, CurveCapabilities, Curves, MaskState, PhotoIdentity, PhotoSummary, State,
};
pub(super) use reply::{Outcome, Reply};

#[derive(Default)]
pub(super) struct Automation {
    pub revision: u64,
    observed: Option<(u64, crate::model::recipe::Recipe)>,
    outputs: output::Outputs,
    turn: Option<(std::time::Instant, TurnScope)>,
}
impl Automation {
    /// Whether output job `id` has finished, or does not exist.
    pub(super) fn job_finished(&self, id: u64) -> bool {
        self.outputs.finished(id)
    }
    /// Whether an export or preview job a command started is still running, and
    /// not cancelled.
    pub(super) fn outputs_running(&self) -> bool {
        self.outputs.any_running()
    }
    /// An output job that runs until cancelled, for tests.
    #[cfg(test)]
    pub(super) fn hold_running_output(&mut self) {
        self.outputs.hold_running();
    }
    /// Cancels the export and preview jobs commands started.
    pub(super) fn cancel_outputs(&self) {
        self.outputs.cancel_all();
    }
    /// The export and preview jobs commands started, cancelled, to wait for at exit.
    pub(super) fn stop_outputs(&mut self) -> Vec<super::task::Stopping> {
        self.outputs.stop()
    }
}
pub(super) use parameter::Param;
pub(super) use preset::PresetTarget;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Source {
    Socket,
    #[cfg(any(test, target_os = "macos", target_os = "windows"))]
    Midi(u64, u64),
}
#[derive(Clone, Copy, PartialEq)]
struct TurnScope(Source, Param, Option<usize>, u64, usize);
impl Automation {
    pub(super) fn turning(&self) -> bool {
        self.turn
            .is_some_and(|(at, _)| at.elapsed() < std::time::Duration::from_millis(400))
    }
    pub(super) fn has_turn(&self) -> bool {
        self.turn.is_some()
    }
    pub(super) fn end_turn(&mut self) {
        self.turn = None;
    }
}

use super::{Editor, state::Tool};
use crate::catalog::{Photo, PhotoId};
use eframe::egui;
use serde::Serialize;

pub(super) use rawmakase_protocol::PROTOCOL;

/// A command's [`rawmakase_protocol::Target`], its photo id typed.
#[derive(Clone, Debug, Default)]
pub(super) struct Target {
    pub photo_id: Option<PhotoId>,
    pub generation: Option<u64>,
    pub revision: Option<u64>,
    /// A mask index from state. Requires generation and revision guards, since
    /// masks have no persistent IDs and can be removed or reordered by the UI.
    pub mask: Option<usize>,
}

impl From<rawmakase_protocol::Target> for Target {
    fn from(t: rawmakase_protocol::Target) -> Self {
        Self {
            photo_id: t.photo_id.map(PhotoId),
            generation: t.generation,
            revision: t.revision,
            mask: t.mask,
        }
    }
}
pub(super) use rawmakase_protocol::CurveChannel;

#[derive(Clone, Debug)]
pub(super) struct Command {
    pub operation: Operation,
    pub target: Target,
}
impl Command {
    pub(crate) fn new(operation: Operation) -> Self {
        Self {
            operation,
            target: Target::default(),
        }
    }
}

#[derive(Clone, Debug)]
pub(super) enum Operation {
    State,
    Capabilities,
    Photos {
        query: String,
        offset: usize,
        limit: usize,
    },
    /// The develop presets, of one group or all.
    Presets {
        group: Option<String>,
    },
    Preset(PresetTarget),
    Set(Param, f32),
    Curve(CurveChannel, Vec<[f32; 2]>),
    Adjust(Param, i32),
    ControlValue(Param, u8),
    Action(Action),
    Metadata {
        edit: super::photo_metadata::Edit,
        advance: bool,
    },
    Open(PhotoTarget),
    Search(String),
    Module(bool),
    Navigate(i32),
    DeviceNavigate(i32),
    Save,
    Output {
        path: std::path::PathBuf,
        max_edge: u32,
    },
    Job(u64),
    /// Builds previews of the selection, as the thumbnail menu does.
    BuildPreviews(crate::catalog::preview_cache::PreviewKind),
    /// Answers once `Until` holds, or after the time given, so a client waiting
    /// for a photo or an output is woken rather than polling.
    Wait(Until, std::time::Duration),
}

/// What a `wait` command waits for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Until {
    /// The photo is loaded, or no longer the one opened (another replaced it).
    Loaded(PhotoId),
    /// The output job has finished, or does not exist.
    Job(u64),
}

#[derive(Clone, Debug)]
pub(super) enum PhotoTarget {
    Name(String),
    Id(PhotoId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    Undo,
    Redo,
    Rating(u8),
    Flag(i8),
    Label(u8),
    ToggleLabel(u8),
    Copy,
    Paste,
    PastePrevious,
    Sync,
    Reset,
    AutoTone,
    AutoWhiteBalance,
    CurveLinear,
    CurveMedium,
    CurveStrong,
    ExportDialog,
    ExportPrevious,
    ToggleMono,
    Treatment(bool),
    Compare,
    CameraThumbnail,
    Clipping,
    Zoom,
    Fit,
    Crop,
    Remove,
    WhiteBalance,
    Mask,
    Mixer(usize),
    Library,
    Develop,
    Loupe,
    Next,
    Previous,
    /// Hides or shows the open module's panels, as Tab, Shift+Tab and F6–F8 do.
    Panels(PanelChange),
    /// Hides or shows everything but the photo, as F does.
    Chrome,
}
impl Action {
    const NAMED: &[(&'static str, Self)] = &[
        ("undo", Self::Undo),
        ("redo", Self::Redo),
        ("rating:0", Self::Rating(0)),
        ("rating:1", Self::Rating(1)),
        ("rating:2", Self::Rating(2)),
        ("rating:3", Self::Rating(3)),
        ("rating:4", Self::Rating(4)),
        ("rating:5", Self::Rating(5)),
        ("pick", Self::Flag(1)),
        ("reject", Self::Flag(-1)),
        ("unflag", Self::Flag(0)),
        ("label:red", Self::Label(0)),
        ("label:yellow", Self::Label(1)),
        ("label:green", Self::Label(2)),
        ("label:blue", Self::Label(3)),
        ("label:purple", Self::Label(4)),
        ("label:none", Self::Label(5)),
        ("toggle_label:red", Self::ToggleLabel(0)),
        ("toggle_label:yellow", Self::ToggleLabel(1)),
        ("toggle_label:green", Self::ToggleLabel(2)),
        ("toggle_label:blue", Self::ToggleLabel(3)),
        ("toggle_label:purple", Self::ToggleLabel(4)),
        ("copy", Self::Copy),
        ("paste", Self::Paste),
        ("paste_previous", Self::PastePrevious),
        ("sync", Self::Sync),
        ("reset", Self::Reset),
        ("auto_tone", Self::AutoTone),
        ("auto_white_balance", Self::AutoWhiteBalance),
        ("curve:linear", Self::CurveLinear),
        ("curve:medium_contrast", Self::CurveMedium),
        ("curve:strong_contrast", Self::CurveStrong),
        ("export_dialog", Self::ExportDialog),
        ("export_previous", Self::ExportPrevious),
        ("toggle:bw", Self::ToggleMono),
        ("treatment:bw", Self::Treatment(true)),
        ("treatment:color", Self::Treatment(false)),
        ("compare", Self::Compare),
        ("camera_thumbnail", Self::CameraThumbnail),
        ("clipping", Self::Clipping),
        ("zoom", Self::Zoom),
        ("fit", Self::Fit),
        ("crop", Self::Crop),
        ("remove", Self::Remove),
        ("white_balance", Self::WhiteBalance),
        ("mask", Self::Mask),
        ("mixer:hue", Self::Mixer(0)),
        ("mixer:sat", Self::Mixer(1)),
        ("mixer:lum", Self::Mixer(2)),
        ("library", Self::Library),
        ("develop", Self::Develop),
        ("loupe", Self::Loupe),
        ("next", Self::Next),
        ("previous", Self::Previous),
        ("panels:sides", Self::Panels(PanelChange::Sides)),
        ("panels:all", Self::Panels(PanelChange::All)),
        (
            "panels:left",
            Self::Panels(PanelChange::Toggle(WorkspacePanel::Left)),
        ),
        (
            "panels:right",
            Self::Panels(PanelChange::Toggle(WorkspacePanel::Right)),
        ),
        (
            "panels:filmstrip",
            Self::Panels(PanelChange::Toggle(WorkspacePanel::Filmstrip)),
        ),
        (
            "panels:presets",
            Self::Panels(PanelChange::Toggle(WorkspacePanel::Presets)),
        ),
        ("chrome", Self::Chrome),
    ];
    pub(super) fn metadata(self) -> Option<super::photo_metadata::Edit> {
        use super::photo_metadata::{Edit, LABELS};
        Some(match self {
            Self::Rating(n) => Edit::Rating(i32::from(n)),
            Self::Flag(n) => Edit::Flag(i32::from(n)),
            Self::Label(n) => Edit::Label(LABELS.get(usize::from(n)).copied().unwrap_or("").into()),
            Self::ToggleLabel(n) => {
                Edit::ToggleLabel(LABELS.get(usize::from(n)).copied().unwrap_or("").into())
            }
            _ => return None,
        })
    }
    pub(crate) fn parse(name: &str) -> Option<Self> {
        Self::NAMED
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, action)| *action)
    }
    pub(crate) fn name(self) -> &'static str {
        Self::NAMED
            .iter()
            .find(|(_, action)| *action == self)
            .expect("named action")
            .0
    }
    pub(crate) fn names() -> Vec<&'static str> {
        Self::NAMED.iter().map(|(name, _)| *name).collect()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(super) struct Error {
    pub code: &'static str,
    pub message: String,
}
impl Error {
    pub(crate) fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}
pub(super) type Result<T> = std::result::Result<T, Error>;

impl Editor {
    /// Reconcile at command/frame boundaries as worker results and undo can edit
    /// the recipe outside an edit frame. Revisions identify observed recipe states.
    pub(super) fn sync_command_revision(&mut self) {
        let generation = self.load.id();
        if let Some((seen_generation, recipe)) = &self.automation.observed
            && *seen_generation == generation
            && *recipe == *self.document.edit.recipe()
        {
            return;
        }
        if self.automation.observed.is_some() {
            self.automation.revision = self.automation.revision.wrapping_add(1);
        }
        self.automation.observed = Some((generation, self.document.edit.recipe().clone()));
    }
    pub(super) fn command_state(&mut self) -> State {
        self.sync_command_revision();
        let develop = self.module == Module::Develop && self.document.metadata.is_some();
        let mut recipe = self.document.edit.recipe().clone();
        let mut values = std::collections::BTreeMap::new();
        if develop {
            for (name, param) in Param::all() {
                values.insert(
                    name,
                    param.shown(&mut recipe, self.view.mixer_adjust.min(2)),
                );
            }
        }
        let masks: Vec<MaskState> = recipe
            .masks
            .iter()
            .enumerate()
            .map(|(index, mask)| {
                let values: std::collections::BTreeMap<String, f32> = Param::NAMED
                    .iter()
                    .filter_map(|(name, param)| {
                        param.local_shown(&mask.adjust).map(|v| ((*name).into(), v))
                    })
                    .collect();
                MaskState {
                    index,
                    name: mask.name.clone(),
                    hidden: mask.hidden,
                    values,
                }
            })
            .collect();
        State {
            protocol: PROTOCOL,
            message: self.messages.line().to_string(),
            mode: if self.module == Module::Library {
                "library"
            } else {
                "develop"
            },
            photo: develop
                .then(|| self.document.path.as_ref().map(|p| p.display().to_string()))
                .flatten(),
            photo_id: develop.then_some(self.document.catalog_photo).flatten(),
            generation: self.load.id(),
            revision: self.automation.revision,
            loaded: develop && self.document.full().is_some() && !self.load.is_running(),
            open_failed: self.open_failed(),
            open_error: self
                .open_failed()
                .and_then(|_| self.document.failed.clone()),
            busy: self.activity.is_busy(),
            modal: self.command_modal(),
            black_and_white: develop && recipe.effects.monochrome,
            mixer_channel: (["hue", "sat", "lum"][self.view.mixer_adjust.min(2)]),
            // A photo's imported Lightroom edit names no preset in the list.
            preset: develop
                .then(|| {
                    self.presets
                        .library
                        .presets
                        .iter()
                        .find(|p| !p.name.is_empty() && p.name == recipe.preset_name)
                })
                .flatten()
                .map(|p| crate::presets::display_name(&p.name)),
            values,
            masks,
            tone_curve: Curves {
                rgb: recipe.curve,
                red: recipe.effects.channels[0].clone(),
                green: recipe.effects.channels[1].clone(),
                blue: recipe.effects.channels[2].clone(),
            },
            selected_mask: self.view.masking.selected,
            exporting: self.exporting(),
            building_previews: self
                .preview_builds
                .progress()
                .filter(super::preview_build::Progress::running)
                .map(|p| super::commands::reply::BuildProgress {
                    done: p.done,
                    total: p.total,
                }),
            auto_running: self.document.auto.is_running(),
            treatment_pending: self.document.pending_treatment.is_some(),
            panels: self.shown_panels(),
            chrome: if self.chrome.hidden(self.module) {
                "hidden"
            } else {
                "shown"
            },
            save_state: match self.document.edit.save_state() {
                super::save_state::SaveState::Clean => "saved",
                super::save_state::SaveState::Pending(_) => "pending",
                super::save_state::SaveState::Saving { .. } => "saving",
                super::save_state::SaveState::Failed { .. } => "failed",
                super::save_state::SaveState::Protected(_) => "protected",
            },
        }
    }
    /// The photo Develop shows as one it could not open.
    pub(super) fn open_failed(&self) -> Option<crate::catalog::PhotoId> {
        (self.module == Module::Develop
            && self.document.failed.is_some()
            && !self.load.is_running())
        .then_some(self.document.catalog_photo)
        .flatten()
    }
    pub(super) fn command_modal(&self) -> bool {
        self.preferences.open
            || self.export_modal()
            || self.modal.is_some()
            || self.view.shortcuts
            || self.report_issue.is_open()
            || self.onboarding.visible
            || self.curve_save_open()
            || self.lut_import_open()
            || self.folder_sync.is_reviewing()
            || self.file_ops.dialog_open()
            || self.auto_stack_open()
    }
    fn check_target(&self, target: &Target) -> Result<()> {
        if target
            .photo_id
            .is_some_and(|id| self.document.catalog_photo != Some(id))
            || target.generation.is_some_and(|n| n != self.load.id())
            || target
                .revision
                .is_some_and(|n| n != self.automation.revision)
        {
            return Err(Error::new(
                "stale_target",
                "The photo or edit changed; read state again",
            ));
        }
        if target.mask.is_some() && (target.generation.is_none() || target.revision.is_none()) {
            return Err(Error::new(
                "target_required",
                "Mask edits require generation and revision from state",
            ));
        }
        Ok(())
    }
    fn require_presets(&self) -> Result<()> {
        if !self.presets.scanned {
            return Err(Error::new("not_ready", "Presets are still loading"));
        }
        Ok(())
    }
    pub(super) fn require_develop(&self) -> Result<()> {
        if self.module == Module::Library || self.document.metadata.is_none() {
            return Err(Error::new("no_document", "Open a photo in Develop first"));
        }
        if self.load.is_running() || self.document.full().is_none() {
            return Err(Error::new("not_ready", "The photo is still loading"));
        }
        Ok(())
    }
    /// Each command is a complete edit transaction. The reply is built only
    /// afterward, including history and rendering invalidation, on the UI thread.
    #[cfg(test)]
    pub(super) fn execute_command(
        &mut self,
        command: Command,
        ctx: &egui::Context,
    ) -> Result<Outcome> {
        self.execute_command_from(command, Source::Socket, ctx)
    }
    pub(super) fn execute_command_from(
        &mut self,
        command: Command,
        source: Source,
        ctx: &egui::Context,
    ) -> Result<Outcome> {
        self.sync_command_revision();
        let Command { operation, target } = command;
        match operation {
            // A wait sent by a device answers at once; the control socket keeps it.
            Operation::State | Operation::Wait(..) => return Ok(Outcome::Empty),
            Operation::Job(id) => return self.automation.outputs.state(id).map(Outcome::Output),
            Operation::Capabilities => {
                return Ok(Outcome::Capabilities(Capabilities {
                    protocol: PROTOCOL,
                    actions: Action::names(),
                    parameters: Param::capabilities(),
                    commands: &[
                        "state",
                        "capabilities",
                        "photos",
                        "presets",
                        "preset",
                        "curve",
                        "set",
                        "turn",
                        "action",
                        "open",
                        "search",
                        "module",
                        "photo",
                        "save",
                        "export",
                        "preview",
                        "job",
                        "wait",
                        "build_previews",
                    ],
                    tone_curve: CurveCapabilities {
                        channels: ["rgb", "red", "green", "blue"],
                        point_count: rawmakase_protocol::curve::POINT_COUNT,
                        coordinate_range: [0, 1],
                        minimum_input_spacing: rawmakase_protocol::curve::MINIMUM_INPUT_SPACING,
                        interpolation: "natural_cubic",
                    },
                    target_fields: &["photo_id", "generation", "revision", "mask"],
                    completion: "applied; loaded/exporting/save_state describe asynchronous work",
                    headless: false,
                }));
            }

            Operation::Photos {
                query,
                offset,
                limit,
            } => {
                let library = self
                    .library
                    .as_ref()
                    .ok_or_else(|| Error::new("no_catalog", "No catalog is open"))?;
                let query = query.to_lowercase();
                let all: Vec<_> = library
                    .session
                    .photos
                    .iter()
                    .filter(|p| query.is_empty() || p.filename.to_lowercase().contains(&query))
                    .collect();
                let photos = all
                    .iter()
                    .skip(offset)
                    .take(limit)
                    .map(|p| PhotoSummary::from(*p))
                    .collect();
                return Ok(Outcome::Photos {
                    total: all.len(),
                    offset,
                    photos,
                });
            }
            Operation::BuildPreviews(kind) => {
                if self.activity.is_busy() || self.command_modal() {
                    return Err(Error::new(
                        "busy",
                        "Close the dialog or wait for the current operation",
                    ));
                }
                let ids = self.preview_command_scope();
                if ids.is_empty() {
                    return Err(Error::new(
                        "no_selection",
                        "Select photos to build previews of",
                    ));
                }
                let queued = self
                    .build_previews(&ids, kind)
                    .map_err(|e| Error::new("not_queued", e))?;
                self.messages
                    .note(super::preview_build::queued_message(queued));
                return Ok(Outcome::Previews {
                    queued: queued.queued,
                    skipped: queued.skipped,
                });
            }
            Operation::Presets { group } => {
                self.require_presets()?;
                // The issues are the open photo's, and the Library has none open.
                let develop = self.module == Module::Develop && self.document.metadata.is_some();
                let issues = if develop {
                    &self.presets.issues[..]
                } else {
                    &[]
                };
                return Ok(Outcome::Presets {
                    presets: preset::summaries(
                        &self.presets.library.presets,
                        issues,
                        group.as_deref(),
                    ),
                });
            }
            _ => {}
        }
        let library_metadata = self.module == Module::Library
            && matches!(operation, Operation::Action(a) if a.metadata().is_some());
        let mut checked = target.clone();
        if library_metadata && let Some(id) = target.photo_id {
            if !self.library.as_ref().is_some_and(|l| l.photo(id).is_some()) {
                return Err(Error::new("not_found", "No catalog photo has this ID"));
            }
            checked.photo_id = None;
        }
        self.check_target(&checked)?;
        if self.activity.is_busy() || self.command_modal() {
            return Err(Error::new(
                "busy",
                "Close the dialog or wait for the current operation",
            ));
        }
        if target.mask.is_some()
            && !matches!(
                operation,
                Operation::Set(..) | Operation::Adjust(..) | Operation::ControlValue(..)
            )
        {
            return Err(Error::new(
                "unsupported_scope",
                "This command does not operate on a mask",
            ));
        }
        let turn = match operation {
            Operation::Adjust(param, _) | Operation::ControlValue(param, _) => Some(TurnScope(
                source,
                param,
                target.mask,
                self.load.id(),
                self.view.mixer_adjust.min(2),
            )),
            _ => None,
        };
        if turn.is_none()
            || !(self.automation.turning() && self.automation.turn.map(|(_, scope)| scope) == turn)
        {
            self.automation.end_turn();
            self.finish_gesture();
        }
        self.automation.turn = turn.map(|scope| (std::time::Instant::now(), scope));
        let save = matches!(operation, Operation::Save);
        let mut frame = self.begin_edit_frame();
        frame.command_adjust = turn.is_some();
        let result = self.apply_command(operation, &target, ctx);
        self.finish_edit_frame(frame, ctx);
        self.sync_undo();
        if save && result.is_ok() {
            if !self.flush()
                || self.document.edit.save_state().needs_save()
                || self.document.edit.save_state().is_protected()
            {
                return Err(Error::new("save_failed", "The edit could not be saved"));
            }
            return Ok(Outcome::Saved { saved: true });
        }
        result
    }
    fn apply_command(
        &mut self,
        operation: Operation,
        target: &Target,
        ctx: &egui::Context,
    ) -> Result<Outcome> {
        match operation {
            Operation::Set(param, value) => {
                let param = self.for_photo(param, target);
                self.command_parameter(param, Some(value), 0, target, ctx)?
            }
            Operation::Preset(target) => {
                self.require_develop()?;
                self.require_presets()?;
                let i = preset::find(&self.presets.library.presets, &target)?;
                let before = self.document.edit.recipe().clone();
                let applied = self
                    .apply_preset(i)
                    .map_err(|e| Error::new("not_applied", format!("{e:#}")))?;
                // Named here: applying the preset already applied, after other edits,
                // changes no preset name for History to recognize it by. Only a change
                // is named, or the name would be left for the next edit.
                if *self.document.edit.recipe() != before {
                    super::widgets::name_frame_step(ctx, "Preset".into(), applied.name.clone());
                }
                return Ok(Outcome::Preset { applied });
            }
            Operation::Curve(channel, points) => {
                self.require_develop()?;
                let curve = crate::color::curve::ToneCurve {
                    points,
                    ..Default::default()
                };
                curve
                    .validate()
                    .map_err(|e| Error::new("invalid_curve", e.to_string()))?;
                let current = match channel {
                    CurveChannel::Rgb => &mut self.document.edit.recipe_mut().curve,
                    CurveChannel::Red => &mut self.document.edit.recipe_mut().effects.channels[0],
                    CurveChannel::Green => &mut self.document.edit.recipe_mut().effects.channels[1],
                    CurveChannel::Blue => &mut self.document.edit.recipe_mut().effects.channels[2],
                };
                if *current != curve {
                    *current = curve;
                    super::widgets::name_frame_step(
                        ctx,
                        "Point Curve".into(),
                        format!("{channel:?}"),
                    );
                }
            }
            Operation::ControlValue(param, value) => {
                let param = self.for_photo(param, target);
                self.command_parameter(
                    param,
                    Some(param.control_value(value, target.mask.is_some())),
                    0,
                    target,
                    ctx,
                )?;
                ctx.request_repaint_after(std::time::Duration::from_millis(450));
            }
            Operation::Adjust(param, ticks) => {
                let param = self.for_photo(param, target);
                self.command_parameter(param, None, ticks, target, ctx)?;
                ctx.request_repaint_after(std::time::Duration::from_millis(450));
            }
            Operation::Action(action) => return self.execute_action(action, target.photo_id),
            Operation::Metadata { edit, advance } => {
                self.command_metadata(edit, target.photo_id, advance)?
            }
            Operation::Open(photo) => return self.command_open(photo),
            Operation::Search(text) => {
                let library = self
                    .library
                    .as_mut()
                    .ok_or_else(|| Error::new("no_catalog", "No catalog is open"))?;
                let mut layout = library.layout();
                layout.query = text.clone();
                layout.filters_off = false;
                library.apply_layout(&layout);
                return Ok(Outcome::Search { query: text });
            }
            Operation::Module(develop) => return self.command_module(develop),
            Operation::Navigate(step) => return self.command_navigate(step),
            Operation::DeviceNavigate(step) => {
                if self.module == Module::Develop
                    || self.library.as_ref().is_some_and(|l| l.loupe_open())
                {
                    return self.command_navigate(step);
                }
            }
            Operation::Output { path, max_edge } => {
                self.require_develop()?;
                let photo = self
                    .export_photo()
                    .ok_or_else(|| Error::new("not_ready", "The photo is not ready to export"))?;
                return self
                    .automation
                    .outputs
                    .start(
                        photo,
                        path,
                        max_edge,
                        self.load.id(),
                        self.automation.revision,
                        ctx.clone(),
                    )
                    .map(Outcome::Output);
            }
            Operation::Save => {
                self.require_develop()?;
                if self.document.edit.save_state().is_protected() {
                    return Err(Error::new(
                        "protected",
                        "This edit is protected from saving",
                    ));
                }
                if self.library.is_none()
                    || self.document.catalog_photo.is_none()
                    || self.document.path.is_none()
                {
                    return Err(Error::new(
                        "no_catalog",
                        "The photo must belong to an open catalog",
                    ));
                }
                // Flush after the transaction has finalized pending recipe changes.
            }
            Operation::State
            | Operation::Capabilities
            | Operation::Photos { .. }
            | Operation::Presets { .. }
            | Operation::Job(_)
            | Operation::BuildPreviews(_)
            | Operation::Wait(..) => unreachable!(),
        }
        Ok(Outcome::Empty)
    }
    /// `param` as the open photo holds it (see `ParameterId::for_photo`): a JPEG,
    /// PNG or TIFF's Temp and Tint are relative. A mask's are its own.
    fn for_photo(&self, param: Param, target: &Target) -> Param {
        match param {
            Param::Setting(id) if target.mask.is_none() => {
                Param::Setting(id.for_photo(self.document.metadata.as_ref()))
            }
            other => other,
        }
    }
    fn command_parameter(
        &mut self,
        param: Param,
        value: Option<f32>,
        ticks: i32,
        target: &Target,
        ctx: &egui::Context,
    ) -> Result<()> {
        self.require_develop()?;
        if value.is_some_and(|v| !v.is_finite()) {
            return Err(Error::new("invalid_value", "Value must be finite"));
        }
        let before_recipe = self.document.edit.recipe().clone();
        let channel = self.view.mixer_adjust.min(2);
        let shown = if let Some(index) = target.mask {
            let mask = self
                .document
                .edit
                .recipe_mut()
                .masks
                .get_mut(index)
                .ok_or_else(|| Error::new("unknown_mask", "The mask does not exist"))?;
            param.local_set(&mut mask.adjust, value, ticks)?
        } else {
            let recipe = self.document.edit.recipe_mut();
            let change = |recipe: &mut crate::model::recipe::Recipe| match value {
                Some(value) => param.set(recipe, value, channel),
                None => param.turn(recipe, ticks, channel),
            };
            match param {
                Param::Setting(id) => {
                    let photo = self.document.metadata.as_ref();
                    crate::model::edit::change_setting(recipe, id, photo, change)
                }
                _ => change(recipe),
            }
        };
        let label = if let Some(index) = target.mask {
            format!("Mask {} {}", index + 1, param.label(channel))
        } else {
            param.label(channel)
        };
        if before_recipe != *self.document.edit.recipe() {
            ctx.data_mut(|d| d.insert_temp(super::widgets::history_step_id(), (label, shown)));
        }
        Ok(())
    }
    pub(super) fn execute_action(
        &mut self,
        action: Action,
        photo: Option<PhotoId>,
    ) -> Result<Outcome> {
        use Action::*;
        if let Some(edit) = action.metadata() {
            if self.module == Module::Library && photo.is_none() {
                return Err(Error::new(
                    "target_required",
                    "Library metadata actions require an explicit photo_id; use photos to find it",
                ));
            }
            self.command_metadata(edit, photo, false)?;
            return Ok(Outcome::Empty);
        }
        match action {
            Undo | Redo => {
                if !self.command_history(action == Redo) {
                    return Err(Error::new(
                        "history_unavailable",
                        "No history step could be applied",
                    ));
                }
            }
            Library => return self.command_module(false),
            Develop => return self.command_module(true),
            Next => return self.command_navigate(1),
            Previous => return self.command_navigate(-1),
            Panels(change) => {
                if !self.request_panels(change) {
                    return Err(Error::new("save_failed", self.messages.line().to_string()));
                }
            }
            Chrome => {
                if !self.toggle_chrome() {
                    return Err(Error::new("save_failed", self.messages.line().to_string()));
                }
            }
            Loupe => {
                self.command_module(false)?;
                if let Some(library) = &mut self.library {
                    library.open_loupe();
                }
            }
            _ => {
                self.require_develop()?;
                match action {
                    Copy => self.open_copy_dialog(super::settings_transfer::Transfer::Copy),
                    Paste => {
                        if self.clipboard.is_none() {
                            return Err(Error::new(
                                "empty_clipboard",
                                "No settings have been copied",
                            ));
                        }
                        self.paste_settings();
                    }
                    PastePrevious => {
                        if self.previous_settings.is_none() {
                            return Err(Error::new(
                                "no_previous",
                                "No previous photo settings are available",
                            ));
                        }
                        self.paste_previous();
                    }
                    Sync => {
                        if self.sync_targets().is_empty() {
                            return Err(Error::new(
                                "no_selection",
                                "Select destination photos to synchronize",
                            ));
                        }
                        self.open_copy_dialog(super::settings_transfer::Transfer::Sync);
                    }
                    Reset => self.reset_settings(),
                    AutoTone | AutoWhiteBalance => {
                        if self.document.auto.is_running() {
                            return Err(Error::new(
                                "busy",
                                "An automatic adjustment is already running",
                            ));
                        }
                        let kind = if action == AutoTone {
                            super::worker::AutoKind::Settings
                        } else {
                            super::worker::AutoKind::WhiteBalance
                        };
                        self.start_auto(kind);
                    }
                    CurveLinear | CurveMedium | CurveStrong => {
                        use crate::presets::curves::BuiltinCurve;
                        let curve = match action {
                            CurveLinear => BuiltinCurve::Linear,
                            CurveMedium => BuiltinCurve::MediumContrast,
                            _ => BuiltinCurve::StrongContrast,
                        };
                        self.choose_point_curve(super::curve_menu::CurveChoice::Builtin(curve));
                    }
                    ExportDialog => self.open_export_dialog(),
                    ExportPrevious => self.export_with_previous(),
                    ToggleMono => self.toggle_treatment(),
                    Treatment(bw) => self.set_treatment(if bw {
                        crate::model::recipe::Treatment::BlackWhite
                    } else {
                        crate::model::recipe::Treatment::Color
                    }),
                    Compare => self.set_compare(
                        self.view
                            .compare
                            .toggled(super::before_after::Compare::BeforeOnly),
                    ),
                    CameraThumbnail => self.toggle_camera_thumbnail(),
                    Clipping => self.view.clipping.toggle_both(),
                    Zoom => self.view.zoom.on = !self.view.zoom.on,
                    Fit => self.view.zoom.on = false,
                    Crop => self.view.toggle(Tool::Crop),
                    Remove => self.view.toggle(Tool::Remove),
                    WhiteBalance => self.view.toggle(Tool::WhiteBalance),
                    Mask => self.view.toggle(Tool::Mask),
                    Mixer(c) => {
                        self.view.mixer_adjust = c;
                        self.view.mixer_color = false;
                    }
                    _ => unreachable!(),
                }
            }
        }
        Ok(Outcome::Empty)
    }
    pub(super) fn command_metadata(
        &mut self,
        edit: super::photo_metadata::Edit,
        photo: Option<PhotoId>,
        advance: bool,
    ) -> Result<()> {
        let library = self
            .library
            .as_mut()
            .ok_or_else(|| Error::new("no_catalog", "No catalog is open"))?;
        let id = photo.or(if self.module == Module::Library {
            None
        } else {
            self.document.catalog_photo
        });
        let next = if let Some(id) = id {
            library.edit_metadata(id, edit, advance)
        } else if self.module == Module::Library {
            library.edit_shown(edit, advance).map(|_| None)
        } else {
            return Err(Error::new("no_document", "No catalog photo is open"));
        };
        let next = next.map_err(|e| Error::new("save_failed", e.to_string()))?;
        self.messages.absorb(&mut library.messages);
        self.sync_undo();
        // The Library shows the photo made active itself; Develop follows it
        // when the edit advanced or the filter hid the photo open.
        let next = next.filter(|_| advance || self.module == Module::Develop);
        if let Some(next) = next {
            self.develop_catalog_photo(next);
            if self.document.catalog_photo != Some(next)
                && let (Some(library), Some(id)) = (&mut self.library, id)
            {
                library.make_active(id);
            }
        }
        Ok(())
    }
    fn command_module(&mut self, develop: bool) -> Result<Outcome> {
        if develop {
            if self.module == Module::Develop {
                return Ok(Outcome::Empty);
            }
            let id = self
                .library
                .as_mut()
                .and_then(|l| l.selected_or_first())
                .ok_or_else(|| Error::new("no_document", "Select a photo first"))?;
            self.command_open(PhotoTarget::Id(id))
        } else {
            if self.library.is_none() {
                return Err(Error::new("no_catalog", "No catalog is open"));
            }
            if !self.flush() {
                return Err(Error::new("save_failed", "The edit could not be saved"));
            }
            self.module = Module::Library;
            if let Some(library) = &mut self.library {
                library.show_grid();
            }
            Ok(Outcome::Empty)
        }
    }
    fn command_navigate(&mut self, step: i32) -> Result<Outcome> {
        let library = self
            .library
            .as_ref()
            .ok_or_else(|| Error::new("no_catalog", "No catalog is open"))?;
        let current = if self.module == Module::Library {
            library.selected()
        } else {
            self.document.catalog_photo
        };
        let next = current
            .and_then(|id| {
                if self.module == Module::Library {
                    library.navigate(id, step)
                } else {
                    self.develop_step(id, step)
                }
            })
            .ok_or_else(|| Error::new("end_of_list", "No next photo in this direction"))?;
        if self.module == Module::Library {
            self.library
                .as_mut()
                .expect("checked above")
                .make_active(next);
            return Ok(Outcome::Empty);
        }
        self.command_open(PhotoTarget::Id(next))
    }
    fn command_open(&mut self, target: PhotoTarget) -> Result<Outcome> {
        let library = self
            .library
            .as_ref()
            .ok_or_else(|| Error::new("no_catalog", "No catalog is open"))?;
        let id = match target {
            PhotoTarget::Id(id) => id,
            PhotoTarget::Name(name) => match find_photos(&library.session.photos, &name)[..] {
                [] => return Err(Error::new("not_found", "No photo matches this name")),
                [photo] => photo.id,
                ref many => {
                    return Err(Error::new(
                        "ambiguous",
                        format!("{} photos match; use a catalog ID", many.len()),
                    ));
                }
            },
        };
        let photo = library
            .photo(id)
            .cloned()
            .ok_or_else(|| Error::new("not_found", "No photo has this ID"))?;
        if let Some(refusal) = super::library::develop_refusal(&photo, photo.path.is_file()) {
            return Err(Error::new("not_editable", refusal.detail()));
        }
        if !self.flush() {
            return Err(Error::new("save_failed", "The edit could not be saved"));
        }
        self.develop_catalog_photo(id);
        if self.module == Module::Library || self.document.catalog_photo != Some(id) {
            return Err(Error::new(
                "open_failed",
                "The requested photo could not be opened",
            ));
        }
        if let Some(library) = &mut self.library {
            library.reveal(id);
        }
        Ok(Outcome::Opened {
            opened: PhotoIdentity::from(&photo),
        })
    }
}

/// The photos a name stands for: the filename or the path, in any case, with or
/// without the extension; else the ones whose name contains it. A master wins
/// over its virtual copies, which share its filename.
fn find_photos<'a>(photos: &'a [Photo], name: &str) -> Vec<&'a Photo> {
    let wanted = name.trim().to_lowercase();
    let by_path = wanted.contains(['/', '\\']);
    let text = |p: &Photo| {
        if by_path {
            p.path.to_string_lossy().to_lowercase()
        } else {
            p.filename.to_lowercase()
        }
    };
    let stem = |p: &Photo| {
        std::path::Path::new(&text(p))
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
    };
    let pick = |matching: &dyn Fn(&Photo) -> bool| -> Vec<&'a Photo> {
        let all: Vec<&Photo> = photos.iter().filter(|p| matching(p)).collect();
        let masters: Vec<&Photo> = all.iter().copied().filter(|p| p.master.is_none()).collect();
        if masters.is_empty() { all } else { masters }
    };
    if wanted.is_empty() {
        return Vec::new();
    }
    let exact = pick(&|p| text(p) == wanted || (!by_path && stem(p).as_deref() == Some(&wanted)));
    if exact.is_empty() {
        pick(&|p| text(p).contains(&wanted))
    } else {
        exact
    }
}

#[cfg(test)]
mod tests;
