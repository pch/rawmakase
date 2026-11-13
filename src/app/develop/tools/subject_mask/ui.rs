//! The Masking drawer's side of the extensions' mask generators, Subject, Sky
//! and Background among them: their tiles, what they are waiting for, and
//! what went wrong. Everything stays in the drawer; a selection that fails or
//! is cancelled leaves the photo as it was. The bundled AI masks' tiles keep
//! the icons, order and wording Select Subject always had.
use super::{Failure, Feature, Prompt, Request, Target};
use crate::app::Editor;
use crate::app::develop::tools::retouch_tool::{control_label, hint, indented};
use crate::app::extensions::mask_generator::{Offer, Setup};
use crate::app::extensions::viewer_input::Gesture;
use crate::app::ui::design::{self, Tone, radius, space, text};
use eframe::egui;

impl Editor {
    /// The Select Subject and Select Background actions, with whatever the running or
    /// failed selection has to say. While the first use is being set up, the actions
    /// give way to a card that says what is needed, in order, and does it.
    pub(in crate::app) fn selection_actions(&mut self, ui: &mut egui::Ui) {
        if let Some(request) = self.selection.running().cloned() {
            self.selection_progress(ui, &request);
            return;
        }
        if self.selection.prompting.is_some() {
            self.prompting_card(ui);
            return;
        }
        if self.selection.prompt.is_some() {
            self.setup_card(ui);
            return;
        }
        let reason = self.selection_unavailable();
        // Three to a row, the first labelled.
        let tiles = ordered(self.extensions.mask_tiles());
        for (row, tiles) in tiles.chunks(3).enumerate() {
            control_label(ui, if row == 0 { "Select" } else { "" }, |ui| {
                let gap = 4.;
                let w = (ui.available_width() - 2. * gap) / 3.;
                for (offer, why) in tiles {
                    let tip = match offer.feature() {
                        Some(Feature::Subject) => "Mask the photo's main subject".to_string(),
                        Some(Feature::Sky) => "Mask the sky".to_string(),
                        Some(Feature::Background) => {
                            "Mask everything but the photo's main subject".to_string()
                        }
                        None => format!("Mask with {}", offer.name),
                    };
                    let disabled = reason.or(why.as_deref());
                    if self.feature_tile(ui, &offer.name, offer.feature(), w, &tip, disabled) {
                        self.request_selection(Request {
                            generator: offer.clone(),
                            target: Target::NewMask,
                        });
                    }
                }
            });
        }
        if let Some(why) = reason {
            hint(ui, why);
        }
        if let Some((request, failure)) = self.selection.failure.clone() {
            self.selection_failure(ui, request, &failure);
        }
    }
    /// One of the Select tiles: an icon over its name, as Lightroom's Add New Mask;
    /// a generator other than Subject's and Sky's (`feature` none) has a plain
    /// frame. Returns whether
    /// it was clicked; when `disabled` says why, it is dimmed and says so on hover.
    fn feature_tile(
        &self,
        ui: &mut egui::Ui,
        name: &str,
        feature: Option<Feature>,
        width: f32,
        tip: &str,
        disabled: Option<&str>,
    ) -> bool {
        let palette = crate::app::ui::theme::palette(ui.ctx());
        let sense = if disabled.is_some() {
            egui::Sense::hover()
        } else {
            egui::Sense::click()
        };
        let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 52.), sense);
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Button,
                disabled.is_none(),
                format!("Select {name}"),
            )
        });
        let hot = response.hovered() && disabled.is_none();
        ui.painter().rect_filled(
            rect,
            radius::CONTROL,
            palette.tone(if hot { Tone::OUTLINE } else { Tone::FIELD }),
        );
        let ink = palette.tone(match (disabled.is_some(), hot) {
            (true, _) => Tone::DISABLED,
            (false, true) => Tone::BRIGHT,
            (false, false) => Tone::SOFT,
        });
        let icon = rect.center_top() + egui::vec2(0., 20.);
        match feature {
            Some(feature) => paint_feature_icon(ui.painter(), icon, feature, ink),
            None => paint_generator_icon(ui.painter(), icon, ink),
        }
        ui.painter().text(
            rect.center_bottom() - egui::vec2(0., 9.),
            egui::Align2::CENTER_CENTER,
            name,
            egui::FontId::proportional(text::SMALL),
            ink,
        );
        let response = match disabled {
            Some(why) => response.on_hover_text(why),
            None => response
                .on_hover_text(tip)
                .on_hover_cursor(egui::CursorIcon::PointingHand),
        };
        response.clicked()
    }
    /// While aiming: what to do on the photo, and how to finish.
    fn prompting_card(&mut self, ui: &mut egui::Ui) {
        let Some(p) = &self.selection.prompting else {
            return;
        };
        let (name, used, applied) = (
            p.request.generator.name().to_string(),
            p.input.summary(),
            p.applied.is_some(),
        );
        let offer = &p.request.generator;
        let first = if offer.feature().is_some() {
            "Click the subject, or drag a box around it. Each click can be \
             refined with more: click to add, Alt-click to leave out."
                .to_string()
        } else {
            format!(
                "{} Each click can be refined with more: click to add, Alt-click to leave out.",
                offer
                    .hint
                    .as_deref()
                    .unwrap_or("Click it on the photo, or drag a box around it.")
            )
        };
        let accent = egui::Color32::from_rgb(96, 150, 230);
        let failure = self.selection.failure.as_ref().map(|f| f.1.message());
        egui::Frame::new()
            .fill(egui::Color32::from_rgb(34, 44, 62))
            .stroke(egui::Stroke::new(1.5, accent))
            .corner_radius(radius::CONTROL)
            .inner_margin(design::margin_same(space::M))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    egui::RichText::new(format!("Select {name}: click it on the photo"))
                        .strong()
                        .size(text::LABEL),
                );
                ui.add_space(space::XS);
                let small = |text: &str| {
                    egui::RichText::new(text)
                        .size(text::SMALL)
                        .color(egui::Color32::LIGHT_GRAY)
                };
                ui.add(
                    egui::Label::new(small(if applied {
                        "Click more of it to add to it, Alt-click something to leave it out, \
                         or drag a box to start over from the box."
                    } else {
                        &first
                    }))
                    .wrap(),
                );
                ui.add(egui::Label::new(used));
                if let Some(why) = failure {
                    ui.colored_label(egui::Color32::from_rgb(230, 170, 90), why);
                }
                ui.add_space(space::XS);
                ui.horizontal(|ui| {
                    let done = egui::Button::new(egui::RichText::new("Done").strong())
                        .fill(egui::Color32::from_rgb(52, 98, 170));
                    if ui.add(done).on_hover_text("Escape").clicked() {
                        self.selection.end_prompting();
                    }
                    if ui
                        .button("Start over")
                        .on_hover_text("Forget the clicks; the next click selects afresh")
                        .clicked()
                    {
                        self.clear_prompt();
                    }
                });
            });
    }
    /// Clicks, drags and markers on the photo while aiming. Returns that the tool owns
    /// the pointer.
    pub(in crate::app) fn prompt_overlay(
        &mut self,
        ui: &mut egui::Ui,
        response: &egui::Response,
        rect: egui::Rect,
        to_screen: &dyn Fn([f32; 2]) -> egui::Pos2,
        to_image: &dyn Fn(egui::Pos2) -> [f32; 2],
    ) -> bool {
        let busy = self.selection.running().is_some();
        let Some(p) = &mut self.selection.prompting else {
            return true;
        };
        match p
            .input
            .overlay(ui, response, rect, busy, true, to_screen, to_image)
        {
            Some(Gesture::Click { at, positive }) => self.prompt_click(at, positive),
            Some(Gesture::Drag { from, to }) => self.prompt_box(from, to),
            None => {}
        }
        true
    }
    /// What the first use needs, as a highlighted card in the place the actions were:
    /// the steps with the finished ones ticked, and the button for the next: what
    /// the generator said it needs, with Download, its progress and Cancel.
    fn setup_card(&mut self, ui: &mut egui::Ui) {
        let Some(offer) = self
            .selection
            .prompt
            .as_ref()
            .map(|p| p.request().generator.clone())
        else {
            return;
        };
        let setup = match &self.selection.prompt {
            Some(Prompt::Setup { setup, .. }) => Some(setup.clone()),
            _ => None,
        };
        let state = self
            .extensions
            .mask_setup(&offer.extension, &offer.generator)
            .cloned();
        let accent = egui::Color32::from_rgb(96, 150, 230);
        egui::Frame::new()
            .fill(egui::Color32::from_rgb(34, 44, 62))
            .stroke(egui::Stroke::new(1.5, accent))
            .corner_radius(radius::CONTROL)
            .inner_margin(design::margin_same(space::M))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    egui::RichText::new(format!("Before “Select {}” can run", offer.name))
                        .strong()
                        .size(text::LABEL),
                );
                ui.add_space(space::XS);
                let step = |ui: &mut egui::Ui, done: bool, current: bool, text: &str| {
                    ui.horizontal(|ui| {
                        let (mark, color) = if done {
                            ("✓", egui::Color32::from_rgb(110, 200, 130))
                        } else if current {
                            ("▶", accent)
                        } else {
                            ("○", egui::Color32::GRAY)
                        };
                        ui.label(egui::RichText::new(mark).color(color).strong());
                        ui.label(egui::RichText::new(text).color(if done {
                            egui::Color32::GRAY
                        } else {
                            egui::Color32::WHITE
                        }));
                    });
                };
                if let Some(setup) = &setup {
                    step(
                        ui,
                        false,
                        true,
                        &format!(
                            "Download {} ({} MB)",
                            setup.what,
                            setup.bytes.div_ceil(1_000_000)
                        ),
                    );
                }
                ui.add_space(space::S);
                if setup.is_none() {
                    return;
                }
                self.download_step(ui, &offer, state);
            });
    }
    /// The download a generator needs: its progress and Cancel while it runs,
    /// why it stopped when it failed, and Download.
    fn download_step(&mut self, ui: &mut egui::Ui, offer: &Offer, state: Option<Setup>) {
        match state {
            Some(Setup::Running { done, text }) => {
                let label = text.unwrap_or_else(|| "Downloading…".into());
                match done {
                    Some(done) => {
                        ui.add(
                            egui::ProgressBar::new(done as f32)
                                .desired_width(ui.available_width())
                                .text(label),
                        );
                    }
                    None => {
                        ui.horizontal(|ui| {
                            crate::app::ui::widgets::spinner(ui, 14.);
                            ui.label(label);
                        });
                    }
                }
                if ui.button("Cancel download").clicked() {
                    self.extensions
                        .cancel_mask_setup(&offer.extension, &offer.generator);
                }
                return;
            }
            Some(Setup::Failed(why)) => {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!("The download stopped: {why}"))
                            .size(text::SMALL)
                            .color(egui::Color32::from_rgb(230, 170, 90)),
                    )
                    .wrap(),
                );
                ui.add_space(space::XS);
            }
            None => {}
        }
        // Subject's and Sky's models run on this computer, unless a remote
        // worker answers, when nothing is asked to be downloaded.
        if offer.feature().is_some() {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(
                        "The model runs on this computer; no photo leaves it. It is a one-time \
                         download and needs about that much free disk space.",
                    )
                    .size(text::SMALL)
                    .color(egui::Color32::LIGHT_GRAY),
                )
                .wrap(),
            );
            ui.add_space(space::S);
        }
        ui.horizontal(|ui| {
            let go = egui::Button::new(egui::RichText::new("Download").strong())
                .fill(egui::Color32::from_rgb(52, 98, 170));
            if ui.add(go).clicked() {
                self.setup_generator(offer);
            }
            if ui.button("Not now").clicked() {
                self.selection.prompt = None;
            }
        });
    }
    fn selection_progress(&mut self, ui: &mut egui::Ui, request: &Request) {
        control_label(ui, "Select", |ui| {
            crate::app::ui::widgets::spinner(ui, 14.);
            // No percentage: the runtime reports none for a run. Where it
            // runs, when the program says (a remote worker).
            let where_at = self
                .extensions
                .mask_progress(&request.generator.extension)
                .map(|text| format!(" ({text})"))
                .unwrap_or_default();
            ui.label(format!(
                "Finding the {}…{where_at}",
                request.generator.name().to_lowercase()
            ))
            .on_hover_text(
                "The first selection on a photo takes a few seconds; the next ones are quick",
            );
            if ui
                .button("Cancel")
                .on_hover_text("Stop selecting; the photo is left as it was")
                .clicked()
            {
                self.cancel_selection();
            }
        });
    }
    fn selection_failure(&mut self, ui: &mut egui::Ui, request: Request, failure: &Failure) {
        hint(ui, &failure.message());
        let builtin = request.generator.feature().is_some();
        if *failure == Failure::NoSubject && builtin {
            hint(
                ui,
                "Automatic selection knows people and animals. For anything else, click it yourself.",
            );
        }
        let aim = matches!(failure, Failure::NoSubject | Failure::NothingFound)
            && request.generator.takes_prompt();
        indented(ui, |ui| {
            let label = if builtin {
                "Click the subject…"
            } else {
                "Click it on the photo…"
            };
            if aim
                && ui
                    .add(
                        egui::Button::new(egui::RichText::new(label).strong())
                            .fill(egui::Color32::from_rgb(52, 98, 170)),
                    )
                    .clicked()
            {
                self.begin_aiming(request.clone());
            }
            if ui.button("Try again").clicked() {
                self.selection.failure = None;
                // A failed click is retried with its clicks, not as an automatic selection.
                if self.selection.prompting.is_some() {
                    self.run_prompt();
                } else {
                    self.request_selection(request.clone());
                }
            }
            if ui.button("Dismiss").clicked() {
                self.selection.failure = None;
            }
        });
    }
    /// Subject and Background entries for a mask's Add, Subtract and Intersect menus.
    pub(in crate::app) fn selection_menu(
        &mut self,
        ui: &mut egui::Ui,
        mask: usize,
        op: crate::model::masks::MaskOp,
    ) {
        ui.separator();
        let reason = self.selection_unavailable();
        let tiles = ordered(self.extensions.mask_tiles())
            .into_iter()
            .filter(|(offer, _)| offer.takes_none());
        for (generator, why) in tiles {
            let why = reason.map(str::to_string).or(why);
            let response = ui.add_enabled(
                why.is_none() && self.selection.running().is_none(),
                egui::Button::new(generator.name()),
            );
            let response = match &why {
                Some(why) => response.on_disabled_hover_text(why),
                None => response,
            };
            if response.clicked() {
                self.request_selection(Request {
                    generator,
                    target: Target::Component { mask, op },
                });
                ui.close();
            }
        }
    }
    /// The selected generated component's source, Regenerate and, for generators
    /// that take clicks, aiming with them. When the generator that made it can't run
    /// (its extension disabled or missing), both show disabled with why.
    pub(in crate::app) fn regenerate_ui(
        &mut self,
        ui: &mut egui::Ui,
        mask: usize,
        component: usize,
        generator: Result<Offer, String>,
        source: &str,
    ) {
        hint(ui, source);
        let reason = self
            .selection_unavailable()
            .map(str::to_string)
            .or_else(|| generator.as_ref().err().cloned());
        let busy = self.selection.running().is_some();
        let free = reason.is_none() && !busy && self.selection.prompting.is_none();
        let (auto, prompt) = match &generator {
            Ok(g) => (g.takes_none(), g.takes_prompt()),
            // Shown, disabled, so the user learns why.
            Err(_) => (true, true),
        };
        indented(ui, |ui| {
            if auto {
                let regenerate = ui.add_enabled(free, egui::Button::new("Regenerate"));
                let regenerate = match &reason {
                    Some(why) => regenerate.on_disabled_hover_text(why),
                    None => regenerate.on_hover_text(
                        "Find it again and replace only this part of the mask; its other parts \
                         and the adjustments stay",
                    ),
                };
                if regenerate.clicked()
                    && let Ok(generator) = &generator
                {
                    self.request_selection(Request {
                        generator: generator.clone(),
                        target: Target::Regenerate { mask, component },
                    });
                }
            }
            if prompt {
                let refine = ui.add_enabled(free, egui::Button::new("Refine with clicks…"));
                let refine = match &reason {
                    Some(why) => refine.on_disabled_hover_text(why),
                    None => refine.on_hover_text(
                        "Click on the photo to add to this selection, Alt-click to leave \
                         something out",
                    ),
                };
                if refine.clicked() {
                    self.begin_refining(mask, component);
                }
            }
        });
        if let Some(why) = generator.as_ref().err() {
            hint(ui, why);
        }
        if let Some(request) = self.selection.running().cloned() {
            self.selection_progress(ui, &request);
        }
        if let Some((request, failure)) = self.selection.failure.clone() {
            self.selection_failure(ui, request, &failure);
        }
        if self.selection.prompt.is_some() {
            self.setup_card(ui);
        }
    }
}

/// The tiles as the drawer shows them: Subject, Sky and Background first, in
/// that order, then the others as their extensions list them.
pub(super) fn ordered(mut tiles: Vec<(Offer, Option<String>)>) -> Vec<(Offer, Option<String>)> {
    tiles.sort_by_key(|(offer, _)| match offer.feature() {
        Some(Feature::Subject) => 0,
        Some(Feature::Sky) => 1,
        Some(Feature::Background) => 2,
        None => 3,
    });
    tiles
}

/// A generator's icon: the dashed frame, with a spark in it.
fn paint_generator_icon(painter: &egui::Painter, c: egui::Pos2, ink: egui::Color32) {
    let stroke = egui::Stroke::new(1.2, ink);
    let (w, h) = (11., 9.5);
    let corners = [
        c + egui::vec2(-w, -h),
        c + egui::vec2(w, -h),
        c + egui::vec2(w, h),
        c + egui::vec2(-w, h),
        c + egui::vec2(-w, -h),
    ];
    painter.add(egui::Shape::dashed_line(&corners, stroke, 2.2, 1.8));
    painter.circle_filled(c, 2.4, ink);
    for i in 0..4 {
        let t = std::f32::consts::FRAC_PI_2 * i as f32;
        let d = egui::vec2(t.cos(), t.sin());
        painter.line_segment([c + d * 4., c + d * 6.5], stroke);
    }
}

/// An icon for each selection, in a dashed frame as Lightroom draws them: a head and
/// shoulders, a sky over a hill, and the frame with the head and shoulders cut out.
pub(in crate::app) fn paint_feature_icon(
    painter: &egui::Painter,
    c: egui::Pos2,
    feature: Feature,
    ink: egui::Color32,
) {
    let stroke = egui::Stroke::new(1.2, ink);
    let (w, h) = (11., 9.5);
    let corners = [
        c + egui::vec2(-w, -h),
        c + egui::vec2(w, -h),
        c + egui::vec2(w, h),
        c + egui::vec2(-w, h),
        c + egui::vec2(-w, -h),
    ];
    painter.add(egui::Shape::dashed_line(&corners, stroke, 2.2, 1.8));
    // A head and shoulders, centred a little low.
    let person = |color: egui::Color32| {
        painter.circle_filled(c + egui::vec2(0., -2.6), 2.9, color);
        painter.add(egui::Shape::convex_polygon(
            (0..=12)
                .map(|i| {
                    let t = std::f32::consts::PI * i as f32 / 12.;
                    c + egui::vec2(-t.cos() * 5.8, 8.4 - t.sin() * 5.4)
                })
                .collect(),
            color,
            egui::Stroke::NONE,
        ));
    };
    match feature {
        Feature::Subject => person(ink),
        Feature::Sky => {
            painter.circle_filled(c + egui::vec2(5., -3.8), 1.9, ink);
            painter.add(egui::Shape::convex_polygon(
                vec![
                    c + egui::vec2(-8., 7.),
                    c + egui::vec2(-3., -1.),
                    c + egui::vec2(0.5, 3.),
                    c + egui::vec2(3., 0.),
                    c + egui::vec2(8., 7.),
                ],
                ink,
                egui::Stroke::NONE,
            ));
        }
        Feature::Background => {
            painter.rect_filled(
                egui::Rect::from_center_size(c, egui::vec2(2. * w - 3., 2. * h - 3.)),
                1.,
                ink.gamma_multiply(0.55),
            );
            // The subject cut out of it: drawn in the tile's own colour.
            person(egui::Color32::from_gray(38));
        }
    }
}
