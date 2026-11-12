//! Viewer input for commands: a `[[command]]` whose manifest declares an
//! `input` other than `none` collects it on the photo open in Develop before
//! it is sent, with the same [`Collector`] and markers as mask generators'
//! aiming.
//!
//! Running such a command, from wherever commands run, starts collecting: the
//! open tool is put away and a card on the photo names the command and what
//! to do. By kind:
//!
//! - `point`: the first click sends it;
//! - `points`: clicks and Alt-clicks add up, to [`MAX_CLICKS`]; Done or Enter
//!   sends them;
//! - `box`: the first drag of at least 1% of the photo on each side sends it;
//! - `prompt`: clicks, Alt-clicks and a box, refined together as aiming
//!   refines a mask; each change is sent, one at a time, and input made while
//!   one runs is ignored.
//!
//! Escape and Cancel end it without sending anything; so do a new photo, a
//! tool taken up, leaving Develop, and the extension disabled, removed,
//! loaded again or crashing ([`Host::lifecycle`]). For `prompt`, Done or
//! Escape ends it after what was sent. `brush` isn't collected: such a
//! command loads, but doesn't run ([`unsupported`]).
//!
//! Each send reads the command's target then and there, so the target and the
//! input are captured together, as one job's inputs
//! ([`Editor::send_extension_command`]).
//!
//! [`Host::lifecycle`]: super::Host::lifecycle
use super::RunBy;
use super::viewer_input::{Collector, Gesture};
use crate::app::develop::state::Tool;
use crate::app::ui::design::{self, radius, space, text};
use crate::app::{Editor, Module};
use crate::develop::ViewMapping;
use eframe::egui;
use rawmakase_extensions::protocol::manifest::Input;
use rawmakase_extensions::protocol::viewer::{MAX_CLICKS, ViewerInput};

/// Why a command taking `input` can't run in this version, if it can't.
pub(in crate::app) fn unsupported(input: Input) -> Option<&'static str> {
    (input == Input::Brush)
        .then_some("It needs brush input, which this version of RAWmakase doesn't collect")
}

/// A command collecting its viewer input.
#[derive(Clone, Debug)]
pub(in crate::app) struct Collecting {
    extension: String,
    command: String,
    title: String,
    kind: Input,
    by: RunBy,
    /// The document load it was started on: a new photo ends it.
    load: u64,
    /// Its extension's lifecycle then ([`super::Host::lifecycle`]).
    lifecycle: u64,
    /// The tools taken up in Develop then ([`ViewState::tools_taken`]): any
    /// tool taken up since ends it, though it closed again in the same frame
    /// (Crop and Enter).
    ///
    /// [`ViewState::tools_taken`]: crate::app::develop::state::ViewState::tools_taken
    tools: u64,
    input: Collector,
}

impl Collecting {
    /// The command, as `<extension>/<command>`.
    #[cfg(test)]
    pub(in crate::app) fn id(&self) -> String {
        format!("{}/{}", self.extension, self.command)
    }
    /// The clicks collected so far, for tests.
    #[cfg(test)]
    pub(in crate::app) fn clicks(&self) -> usize {
        self.input.clicks.len()
    }
    /// What to do on the photo.
    fn instruction(&self) -> &'static str {
        match self.kind {
            Input::Point => "Click the photo.",
            Input::Points => {
                "Click the photo, Alt-click to leave something out, then press Done or Enter."
            }
            Input::Box => "Drag a box on the photo.",
            Input::Prompt => {
                "Click the photo or drag a box around something; Alt-click to leave \
                 something out. Each change runs it again."
            }
            Input::None | Input::Brush => "",
        }
    }
}

impl Editor {
    /// Starts collecting `kind` for command `command` of extension
    /// `extension`, which then runs from the collected input. It needs a
    /// photo open in Develop; the tool open is put away, Masking with its
    /// aiming included.
    pub(in crate::app) fn begin_command_input(
        &mut self,
        extension: &str,
        command: &str,
        kind: Input,
        by: RunBy,
    ) -> crate::app::shell::commands::Result<()> {
        use crate::app::shell::commands::Error;
        if self.module != Module::Develop || self.document.full().is_none() {
            return Err(Error::new("no_document", "Open a photo in Develop first"));
        }
        let lifecycle = self
            .extensions
            .lifecycle(extension)
            .ok_or_else(|| Error::new("unknown_action", format!("No extension {extension}")))?;
        let title = self
            .extensions
            .get(extension)
            .and_then(|e| e.manifest.commands.iter().find(|c| c.id == command))
            .map_or_else(|| command.to_string(), |c| c.title.clone());
        if self.view.is(Tool::Mask) {
            self.close_masking();
        }
        self.view.set_tool(Tool::None);
        let collecting = Collecting {
            extension: extension.to_string(),
            command: command.to_string(),
            title,
            kind,
            by,
            load: self.document.load_id(),
            lifecycle,
            tools: self.view.tools_taken(),
            input: Collector::default(),
        };
        self.messages.note(format!(
            "{}: {}",
            collecting.title,
            collecting.instruction()
        ));
        self.command_input = Some(collecting);
        self.context.request_repaint();
        Ok(())
    }

    /// The command collecting its viewer input, if one is.
    #[cfg(test)]
    pub(in crate::app) fn command_input(&self) -> Option<&Collecting> {
        self.command_input.as_ref()
    }

    /// Stops collecting, sending nothing more.
    pub(in crate::app) fn end_command_input(&mut self) {
        if self.command_input.take().is_some() {
            self.context.request_repaint();
        }
    }

    /// Ends collecting once what it was for has gone: Develop left, a tool
    /// taken up, another photo (or none) open, or its extension disabled,
    /// removed, loaded again, Failed or crashed. Runs every pass, and again
    /// before collecting takes keys or the pointer, as what ends it may have
    /// happened earlier in the same pass.
    pub(in crate::app) fn end_command_input_when_stale(&mut self) {
        let Some(c) = &self.command_input else {
            return;
        };
        let stale = self.module != Module::Develop
            || !self.view.is(Tool::None)
            || self.view.tools_taken() != c.tools
            || self.document.load_id() != c.load
            || self.document.full().is_none()
            || self.extensions.lifecycle(&c.extension) != Some(c.lifecycle)
            || self.extensions.ready(&c.extension).is_err();
        if stale {
            self.end_command_input();
        }
    }

    /// Whether a `prompt` command's input is ignored now, as a command of
    /// its extension still runs. Other kinds are never held back: they are
    /// sent once, and wait behind whatever runs in the program as any command
    /// does.
    fn command_input_busy(&self) -> bool {
        self.command_input.as_ref().is_some_and(|c| {
            c.kind == Input::Prompt && self.extensions.command_running(&c.extension)
        })
    }

    /// A click on the photo at `at` (image frame, 0 to 1); an Alt-click has
    /// `positive` false.
    pub(in crate::app) fn command_input_click(&mut self, at: [f32; 2], positive: bool) {
        let busy = self.command_input_busy();
        let Some(c) = &mut self.command_input else {
            return;
        };
        let send = match c.kind {
            Input::Point => c
                .input
                .add(at, true)
                .then(|| (ViewerInput::Point { x: at[0], y: at[1] }, true)),
            Input::Points => {
                c.input.add(at, positive);
                None
            }
            Input::Prompt => {
                (!busy && c.input.click(at, positive, || None)).then(|| (c.input.input(), false))
            }
            Input::Box | Input::None | Input::Brush => None,
        };
        self.send_and_maybe_end(send);
    }

    /// A box dragged on the photo from `a` to `b` (image frame, 0 to 1).
    pub(in crate::app) fn command_input_drag(&mut self, a: [f32; 2], b: [f32; 2]) {
        let busy = self.command_input_busy();
        let Some(c) = &mut self.command_input else {
            return;
        };
        let send = match c.kind {
            Input::Box if c.input.drag(a, b) => c
                .input
                .bounds
                .map(|[x0, y0, x1, y1]| (ViewerInput::Box { x0, y0, x1, y1 }, true)),
            Input::Prompt => (!busy && c.input.drag(a, b)).then(|| (c.input.input(), false)),
            Input::Box | Input::Point | Input::Points | Input::None | Input::Brush => None,
        };
        self.send_and_maybe_end(send);
    }

    /// Sends the input of a gesture, if it made one, and ends collecting
    /// after it for the kinds sent once.
    fn send_and_maybe_end(&mut self, send: Option<(ViewerInput, bool)>) {
        let Some((input, last)) = send else {
            return;
        };
        self.send_command_input(input);
        if last {
            self.end_command_input();
        }
    }

    /// Done, or Enter: `points` sends its clicks, if it has any; `prompt`
    /// ends, after what it sent.
    pub(in crate::app) fn command_input_done(&mut self) {
        let Some(c) = &self.command_input else {
            return;
        };
        match c.kind {
            Input::Points if !c.input.clicks.is_empty() => {
                let clicks = c.input.clicks.clone();
                self.send_and_maybe_end(Some((ViewerInput::Points { clicks }, true)));
            }
            Input::Prompt => self.end_command_input(),
            _ => {}
        }
    }

    /// Escape ends collecting. Taken before Develop's other keys, which
    /// would take it to put a tool away.
    pub(in crate::app) fn command_input_escape(&mut self, ctx: &egui::Context) {
        use crate::app::shell::commands::registry::bindings as b;
        self.end_command_input_when_stale();
        if self.command_input.is_some() && self.keys(ctx).take(&b::CLOSE_TOOL).is_some() {
            self.end_command_input();
        }
    }

    /// Enter is Done. Taken after Develop's other keys, so a tool one of
    /// them took up this frame ends collecting first, and nothing is sent.
    pub(in crate::app) fn command_input_enter(&mut self, ctx: &egui::Context) {
        self.end_command_input_when_stale();
        if self.command_input.is_none() {
            return;
        }
        let keys = self.keys(ctx);
        let enter = keys
            .presses()
            .into_iter()
            .find(|p| p.key == egui::Key::Enter && p.modifiers.is_none());
        if let Some(press) = enter {
            keys.take_press(&press);
            keys.remove_press(&press);
            self.command_input_done();
        }
    }

    /// Sends the command with `input`, saying in the top bar why it can't
    /// be.
    fn send_command_input(&mut self, input: ViewerInput) {
        let Some(c) = &self.command_input else {
            return;
        };
        let (extension, command, title, by) = (
            c.extension.clone(),
            c.command.clone(),
            c.title.clone(),
            c.by,
        );
        if let Err(e) = self.send_extension_command(&extension, &command, Some(input), by) {
            self.messages.fail(format!("{title}: {}", e.message));
        }
    }

    /// Clicks, drags and markers on the photo, and the card that says what to
    /// do, while a command collects its input. Returns whether it owns the
    /// pointer: it does while it collects.
    ///
    /// The photo is drawn after Develop's keys, panels and toolbar, so a tool
    /// they took up this frame ends collecting here first: the pointer is
    /// that tool's alone, and nothing is sent.
    pub(in crate::app) fn command_input_overlay(
        &mut self,
        ui: &mut egui::Ui,
        response: &egui::Response,
        rect: egui::Rect,
        area: egui::Rect,
    ) -> bool {
        self.end_command_input_when_stale();
        if self.command_input.is_none() {
            return false;
        }
        let Some(im) = self.document.full().cloned() else {
            return true;
        };
        let recipe = self.effective_recipe();
        let map = ViewMapping::new(&im, &recipe);
        let to_screen = |p: [f32; 2]| {
            let [u, v] = map.to_view(p);
            egui::Pos2::new(
                rect.left() + u * rect.width(),
                rect.top() + v * rect.height(),
            )
        };
        let to_image = |p: egui::Pos2| {
            map.to_image(
                (p.x - rect.left()) / rect.width(),
                (p.y - rect.top()) / rect.height(),
            )
        };
        let busy = self.command_input_busy();
        let Some(c) = &mut self.command_input else {
            return true;
        };
        let drags = matches!(c.kind, Input::Box | Input::Prompt);
        let gesture = c
            .input
            .overlay(ui, response, rect, busy, drags, &to_screen, &to_image);
        match gesture {
            Some(Gesture::Click { at, positive }) => self.command_input_click(at, positive),
            Some(Gesture::Drag { from, to }) => self.command_input_drag(from, to),
            None => {}
        }
        self.command_input_card(ui, area, busy);
        true
    }

    /// The card at the top of the photo: the command, what to do, what was
    /// collected, and how to finish.
    fn command_input_card(&mut self, ui: &egui::Ui, area: egui::Rect, busy: bool) {
        let Some(c) = &self.command_input else {
            return;
        };
        let (title, instruction, kind) = (c.title.clone(), c.instruction(), c.kind);
        let summary = matches!(kind, Input::Points | Input::Prompt).then(|| c.input.summary());
        let can_send = !c.input.clicks.is_empty();
        let full = c.input.clicks.len() >= MAX_CLICKS;
        let mut done = false;
        let mut cancel = false;
        egui::Area::new(egui::Id::new("extension-command-input"))
            .order(egui::Order::Foreground)
            .fixed_pos(area.center_top() + egui::vec2(0., space::M))
            .pivot(egui::Align2::CENTER_TOP)
            .show(ui.ctx(), |ui| {
                egui::Frame::new()
                    .fill(egui::Color32::from_rgb(34, 44, 62))
                    .stroke(egui::Stroke::new(
                        1.5,
                        egui::Color32::from_rgb(96, 150, 230),
                    ))
                    .corner_radius(radius::CONTROL)
                    .inner_margin(design::margin_same(space::M))
                    .show(ui, |ui| {
                        ui.set_max_width(360.);
                        ui.label(egui::RichText::new(&title).strong().size(text::LABEL));
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(instruction)
                                    .size(text::SMALL)
                                    .color(egui::Color32::LIGHT_GRAY),
                            )
                            .wrap(),
                        );
                        if let Some(summary) = summary {
                            ui.label(summary);
                        }
                        if full {
                            ui.label(
                                egui::RichText::new(format!("At most {MAX_CLICKS} points"))
                                    .size(text::SMALL),
                            );
                        }
                        if busy {
                            ui.horizontal(|ui| {
                                crate::app::ui::widgets::spinner(ui, 12.);
                                ui.label(egui::RichText::new("Running…").size(text::SMALL));
                            });
                        }
                        ui.horizontal(|ui| match kind {
                            Input::Points => {
                                done = ui
                                    .add_enabled(can_send, egui::Button::new("Done"))
                                    .on_hover_text("Enter")
                                    .clicked();
                                cancel = ui.button("Cancel").on_hover_text("Escape").clicked();
                            }
                            Input::Prompt => {
                                done = ui.button("Done").on_hover_text("Escape").clicked();
                            }
                            _ => {
                                cancel = ui.button("Cancel").on_hover_text("Escape").clicked();
                            }
                        });
                    });
            });
        if done {
            self.command_input_done();
        } else if cancel {
            self.end_command_input();
        }
    }
}
