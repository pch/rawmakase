use super::clipping::{self, ClipSide};
use super::crop_tool::{Guide, GuideShow, Ruler};
use super::dialogs::FileDialog;
use super::state::{MixerTab, Tool};
use super::targeted_tool::{hsl_target, target_button};
use super::tone_drag::tone_drag_ui;
use super::widgets::{
    SliderEvent, adjustment_section, name_history_step, parametric_curve_ui, segmented,
    setting_slider, slider, slider_with, switched_section, tone_curve_ui, toolbar_action,
};
use super::worker::AutoKind;
use super::{Editor, bulk_import::ImportKind};
use crate::app::icons::{self, Icon};
use crate::model::panels::{Panel, PanelState};
use crate::model::params::ParameterId;
use crate::model::recipe::Recipe;
use crate::model::recipe::Treatment;
use crate::model::{operators::SharpeningSliders, white_balance::NamedWhiteBalance};
use crate::{app::theme, develop::targeted::Target};
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};

mod lens_profile;

/// A histogram corner's clipping triangle: its corner point, which way it
/// points (1 right, -1 left) and the area that takes its clicks.
#[derive(Clone, Copy)]
struct ClipTriangle {
    corner: Pos2,
    dir: f32,
    hit: Rect,
}
impl ClipTriangle {
    fn new(histogram: Rect, side: ClipSide) -> Self {
        let (corner, dir) = match side {
            ClipSide::Shadows => (histogram.left_top() + Vec2::new(6., 6.), 1.),
            ClipSide::Highlights => (histogram.right_top() + Vec2::new(-6., 6.), -1.),
        };
        Self {
            corner,
            dir,
            hit: Rect::from_center_size(corner + Vec2::new(4. * dir, 3.), Vec2::splat(16.)),
        }
    }
}

impl Editor {
    /// Lightroom-style histogram: filled channels whose overlaps mix to
    /// cyan, magenta, yellow and gray, with clipping indicators in the corners.
    /// Dragging in it moves Blacks, Shadows, Exposure, Highlights or Whites.
    pub(super) fn histogram_ui(&mut self, ui: &mut egui::Ui) {
        let palette = theme::palette(ui.ctx());
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 96.), Sense::hover());
        let painter = ui.painter().clone();
        painter.rect_filled(rect, 2., palette.gray(20));
        for i in 1..5 {
            let x = rect.left() + i as f32 / 5. * rect.width();
            painter.line_segment(
                [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
                Stroke::new(1., palette.gray(32)),
            );
        }
        let histogram = self.preview.histogram;
        let h = histogram.bins;
        // Light smoothing, and scaling that ignores the clipped end bins.
        let smooth = |c: usize, i: usize| {
            let at = |j: isize| h[c][j.clamp(0, 255) as usize] as f32;
            let i = i as isize;
            (at(i - 1) + 2. * at(i) + at(i + 1)) / 4.
        };
        let max = (2..254)
            .flat_map(|i| (0..3).map(move |c| (c, i)))
            .map(|(c, i)| smooth(c, i))
            .fold(1., f32::max);
        let plot = rect.shrink2(Vec2::new(0., 4.));
        let bar = plot.width() / 256.;
        let colors = [
            Color32::from_rgb(196, 58, 52),
            Color32::from_rgb(62, 170, 70),
            Color32::from_rgb(56, 104, 220),
        ];
        let pair = |a: usize, b: usize| match (a.min(b), a.max(b)) {
            (0, 1) => Color32::from_rgb(190, 176, 60),
            (1, 2) => Color32::from_rgb(58, 168, 186),
            _ => Color32::from_rgb(170, 70, 170),
        };
        for i in 0..256 {
            let mut v: Vec<(f32, usize)> = (0..3)
                .map(|c| ((smooth(c, i) / max).sqrt().min(1.) * plot.height(), c))
                .collect();
            v.sort_by(|a, b| a.0.total_cmp(&b.0));
            let x = plot.left() + i as f32 * bar;
            let segment = |from: f32, to: f32, color: Color32| {
                if to > from {
                    painter.rect_filled(
                        Rect::from_min_max(
                            Pos2::new(x, plot.bottom() - to),
                            Pos2::new(x + bar + 0.5, plot.bottom() - from),
                        ),
                        0.,
                        color,
                    );
                }
            };
            segment(0., v[0].0, palette.gray(150));
            segment(v[0].0, v[1].0, pair(v[1].1, v[2].1));
            segment(v[1].0, v[2].0, colors[v[2].1]);
        }
        // After the bars, so the region shows over them; before the triangles,
        // so their clicks stay theirs.
        let triangles = ClipSide::BOTH.map(|side| ClipTriangle::new(rect, side));
        let region = tone_drag_ui(
            ui,
            rect,
            &triangles.map(|t| t.hit),
            &mut self.view.tone_drag,
            self.document.edit.recipe_mut(),
        );
        if let Some(region) = region {
            let [from, to] = region.span();
            painter.rect_filled(
                Rect::from_x_y_ranges(
                    rect.left() + from * rect.width()..=rect.left() + to * rect.width(),
                    rect.y_range(),
                ),
                0.,
                Color32::from_white_alpha(14),
            );
        }
        // Clipping triangles, as Lightroom's: each lit in the colours of the
        // channels clipping at its end; a click toggles its warning, hovering
        // shows it while the pointer stays, and J toggles both.
        for (side, ClipTriangle { corner, dir, hit }) in ClipSide::BOTH.into_iter().zip(triangles) {
            let left = side == ClipSide::Shadows;
            let response = ui
                .interact(hit, ui.id().with(("clip", left)), Sense::click())
                .on_hover_text(if left {
                    "Show shadow clipping · J shows both"
                } else {
                    "Show highlight clipping · J shows both"
                });
            if response.clicked() {
                self.view.clipping.toggle(side);
            }
            if response.hovered() {
                self.view.clipping.set_hover(Some(side));
            }
            let on = self.view.clipping.is_on(side);
            let color = clipping::indicator_color(clipping::clipped_channels(&histogram, side))
                .unwrap_or_else(|| palette.gray(if on || response.hovered() { 150 } else { 80 }));
            painter.add(egui::Shape::convex_polygon(
                vec![
                    corner,
                    corner + Vec2::new(9. * dir, 0.),
                    corner + Vec2::new(0., 7.),
                ],
                color,
                if on {
                    Stroke::new(1., Color32::WHITE)
                } else {
                    Stroke::NONE
                },
            ));
        }
        // The region and its value take the EXIF line's place, as in Lightroom.
        let region_text = region.map(|region| {
            format!(
                "{}   {}",
                region.label(),
                region.display(region.value(self.document.edit.recipe()))
            )
        });
        // So does the RGB readout while the pointer is over the photo.
        let exif = region_text
            .or_else(|| self.view.readout.text())
            .unwrap_or_else(|| {
                self.document
                    .metadata
                    .as_ref()
                    .map_or_else(String::new, |m| {
                        let info = crate::metadata::PhotoInfo::from_metadata(m);
                        [
                            info.iso_text(),
                            info.focal_text(),
                            info.aperture_text(),
                            info.shutter_text(),
                        ]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join("     ")
                    })
            });
        ui.vertical_centered(|ui| {
            ui.label(egui::RichText::new(exif).size(11.).color(palette.gray(170)))
                .on_hover_text("Output histogram of the whole photo");
        });
    }
    /// Lightroom's tool strip: Crop, Remove, Red Eye and Masking, with the open tool's
    /// drawer below it.
    pub(super) fn tool_strip(&mut self, ui: &mut egui::Ui) {
        let palette = theme::palette(ui.ctx());
        ui.add_space(6.);
        // Red Eye has no shortcut, as in Lightroom.
        const TOOLS: [(Tool, &str, &str); 4] = [
            (Tool::Crop, "Crop", "Crop & Straighten · R"),
            (Tool::Remove, "Remove", "Spot Removal: Heal and Clone · Q"),
            (Tool::RedEye, "Red Eye", "Red Eye Correction"),
            (Tool::Mask, "Masking", "Masking · Shift+W"),
        ];
        let (strip, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.), Sense::hover());
        let width = (strip.width() - 12.) / 4.;
        for (k, (tool, label, tip)) in TOOLS.into_iter().enumerate() {
            let rect = Rect::from_min_size(
                strip.min + Vec2::new(k as f32 * (width + 4.), 0.),
                Vec2::new(width, strip.height()),
            );
            let response = ui
                .interact(rect, ui.id().with(("tool", label)), Sense::click())
                .on_hover_text(tip);
            let active = self.view.is(tool);
            ui.painter().rect_filled(
                rect,
                3.,
                palette.gray(if active {
                    72
                } else if response.hovered() {
                    50
                } else {
                    38
                }),
            );
            let icon = rect.left_center() + Vec2::new(16., 0.);
            match tool {
                Tool::Crop => crop_icon(ui.painter(), icon, active),
                Tool::Remove => heal_icon(ui.painter(), icon, active),
                Tool::RedEye => eye_icon(ui.painter(), icon, active),
                _ => mask_icon(ui.painter(), icon, active),
            }
            ui.painter().text(
                rect.left_center() + Vec2::new(30., 0.),
                egui::Align2::LEFT_CENTER,
                label,
                egui::FontId::proportional(12.),
                palette.gray(if active { 245 } else { 200 }),
            );
            if response
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                self.view.toggle(tool);
            }
        }
        let drawer = |ui: &mut egui::Ui, add: &mut dyn FnMut(&mut egui::Ui)| {
            ui.add_space(4.);
            egui::Frame::new()
                .fill(palette.gray(40))
                .corner_radius(3.)
                .inner_margin(egui::Margin {
                    left: 0,
                    right: 8,
                    top: 8,
                    bottom: 8,
                })
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(4., 6.);
                    add(ui)
                });
        };
        match self.view.tool {
            Tool::Remove => {
                return drawer(ui, &mut |ui| {
                    experimental(ui, UNMEASURED);
                    self.retouch_panel(ui)
                });
            }
            Tool::RedEye => {
                return drawer(ui, &mut |ui| {
                    experimental(ui, RED_EYE_NOTE);
                    self.red_eye_panel(ui)
                });
            }
            Tool::Mask => {
                return drawer(ui, &mut |ui| {
                    experimental(ui, UNMEASURED);
                    self.mask_panel(ui)
                });
            }
            Tool::Crop => {}
            _ => return,
        }
        ui.add_space(4.);
        let mut action = None;
        let mut guides = self.view.crop_guides;
        let analysing = self.document.straighten.is_running();
        let r = self.document.edit.recipe_mut();
        egui::Frame::new()
            .fill(palette.gray(40))
            .corner_radius(3.)
            .inner_margin(egui::Margin {
                left: 0,
                right: 8,
                top: 8,
                bottom: 8,
            })
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = Vec2::new(4., 6.);
                control_row(ui, "Aspect", |ui| {
                    let swap = 26.;
                    egui::ComboBox::from_id_salt("crop-aspect")
                        .width(ui.available_width() - swap - 4.)
                        .selected_text(aspect_name(self.view.aspect))
                        .show_ui(ui, |ui| {
                            for (aspect, name) in ASPECTS {
                                ui.selectable_value(&mut self.view.aspect, aspect, name);
                            }
                        });
                    if ui
                        .add_sized([swap, 20.], egui::Button::new("⇄"))
                        .on_hover_text("Swap portrait and landscape · X")
                        .clicked()
                    {
                        action = Some(CropAction::Swap);
                    }
                });
                setting_slider(ui, ParameterId::Straighten, &mut r.straighten, 0.);
                control_row(ui, "Straighten", |ui| {
                    let w = (ui.available_width() - 4.) / 2.;
                    let armed = self.view.ruler == Ruler::Armed;
                    if ui
                        .add_sized([w, 20.], egui::Button::new("Ruler").selected(armed))
                        .on_hover_text(
                            "Drag along a horizon or vertical to level it · or Cmd-drag on the photo",
                        )
                        .clicked()
                    {
                        self.view.ruler = if armed { Ruler::Off } else { Ruler::Armed };
                    }
                    if ui
                        .add_enabled(!analysing, egui::Button::new("Auto").min_size(Vec2::new(w, 20.)))
                        .on_hover_text("Level the photo as Upright's Level would, by its angle alone")
                        .clicked()
                    {
                        action = Some(CropAction::AutoStraighten);
                    }
                });
                control_row(ui, "Orientation", |ui| {
                    let w = ui.available_width();
                    let mut none = usize::MAX;
                    segmented(
                        ui,
                        &mut none,
                        &[
                            (0, "Rotate L"),
                            (1, "Rotate R"),
                            (2, "Flip H"),
                            (3, "Flip V"),
                        ],
                        w,
                    );
                    use crate::develop::{Mirror, QuarterTurn, mirror, turn};
                    match none {
                        0 => turn(r, QuarterTurn::Left),
                        1 => turn(r, QuarterTurn::Right),
                        2 => mirror(r, Mirror::Horizontal),
                        3 => mirror(r, Mirror::Vertical),
                        _ => {}
                    }
                });
                control_row(ui, "Overlay", |ui| {
                    let show = 78.;
                    egui::ComboBox::from_id_salt("crop-guide")
                        .width(ui.available_width() - show - 4.)
                        .selected_text(guides.guide.name())
                        .show_ui(ui, |ui| {
                            for guide in Guide::ALL {
                                if ui
                                    .selectable_label(guides.guide == guide, guide.name())
                                    .clicked()
                                {
                                    guides.guide = guide;
                                    guides.orientation = 0;
                                }
                            }
                        })
                        .response
                        .on_hover_text("O cycles the overlays, Shift+O turns them");
                    egui::ComboBox::from_id_salt("crop-guide-show")
                        .width(show)
                        .selected_text(guides.show.name())
                        .show_ui(ui, |ui| {
                            for when in GuideShow::ALL {
                                ui.selectable_value(&mut guides.show, when, when.name());
                            }
                        })
                        .response
                        .on_hover_text("When the overlay shows: always, with the pointer over the photo, or never");
                });
                ui.horizontal(|ui| {
                    ui.add_space(83.);
                    ui.label(
                        egui::RichText::new("Drag the frame on the photo; changes apply live.")
                            .size(10.)
                            .color(palette.gray(125)),
                    );
                });
                ui.horizontal(|ui| {
                    ui.add_space(83.);
                    let w = (ui.available_width() - 4.) / 2.;
                    if ui
                        .add_sized([w, 22.], egui::Button::new("Reset"))
                        .on_hover_text("Remove the crop, straightening, rotation and flips")
                        .clicked()
                    {
                        r.crop = [0., 0., 1., 1.];
                        r.straighten = 0.;
                        r.rotation = 0;
                        r.flip_x = false;
                        r.flip_y = false;
                    }
                    if ui
                        .add_sized(
                            [w, 22.],
                            egui::Button::new(
                                egui::RichText::new("Done").color(palette.on_accent_text(245)),
                            )
                            .fill(palette.accent()),
                        )
                        .on_hover_text("Finish cropping · Enter or R")
                        .clicked()
                    {
                        self.view.tool = Tool::None;
                    }
                });
            });
        self.set_crop_guides(guides);
        match action {
            Some(CropAction::Swap) => self.swap_crop_orientation(),
            Some(CropAction::AutoStraighten) => self.start_auto_straighten(),
            None => {}
        }
    }
    pub(super) fn controls(&mut self, ui: &mut egui::Ui) {
        let palette = theme::palette(ui.ctx());
        ui.spacing_mut().item_spacing = Vec2::new(4., 3.);
        ui.spacing_mut().button_padding = Vec2::new(6., 2.);
        ui.spacing_mut().interact_size.y = 20.;
        self.histogram_ui(ui);
        self.tool_strip(ui);
        ui.add_space(6.);
        let mut import_profiles = false;
        let mut import_lens = false;
        let mut import_folder = None;
        let mut import_adobe = false;
        // Cached per camera: this scans Adobe's profile folders.
        let adobe_key = self
            .document
            .metadata
            .as_ref()
            .map(|m| egui::Id::new(("adobe-profiles", &m.make, &m.model)));
        let adobe: Vec<std::path::PathBuf> = match (adobe_key, &self.document.metadata) {
            (Some(key), Some(m)) => ui.ctx().data_mut(|d| {
                d.get_temp_mut_or_insert_with(key, || crate::camera_profiles::adobe_installed(m))
                    .clone()
            }),
            _ => Vec::new(),
        };
        let metadata = self.document.metadata.clone();
        let profiles = self.document.profiles.clone();
        let profile_errors = self.document.profile_errors.clone();
        let histogram = self.preview.histogram.bins;
        // Auto needs the decoded photo, and runs one estimate at a time.
        let auto_ready = self.document.full().is_some() && !self.document.auto.is_running();
        let auto_in_effect = self.auto_in_effect();
        let mut auto_request = None;
        // Upright analyses the decoded photo once and keeps a correction for every mode.
        let upright_ready = self.document.full().is_some() && !self.document.upright.is_running();
        let mut upright_request = false;
        let mut guided_action = None;
        let mut treatment_request = None;
        let mut auto_mix_request = false;
        let mut profile_changed_from = None;
        // A conversion to black & white waiting for the photo to decode.
        let pending_treatment = self
            .document
            .pending_treatment
            .as_ref()
            .map(|p| p.treatment);
        let grading_document = self.document.edit.history().id();
        let saved_curves = self.saved_curves();
        let mut curve_choice = None;
        // The Targeted Adjustment Tool: the sliders a drag is moving, and a target
        // button's click.
        let targeted = self.targeted_weights();
        let mut targeted_request = None;
        let view = &mut self.view;
        let (r, photo) = self.document.recipe_and_colors();

        if adjustment_section(ui, "Basic", |ui| {
            let shortcut = if cfg!(target_os = "macos") {
                "⌘⇧U"
            } else {
                "Ctrl+Shift+U"
            };
            // Auto, right-aligned, styled as the toolbar's Before and Clipping.
            let (row, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 32.), Sense::hover());
            ui.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(row)
                    .layout(egui::Layout::right_to_left(egui::Align::Center)),
                |ui| {
                    let tip = if auto_in_effect {
                        "Auto settings are applied".to_owned()
                    } else {
                        format!("Set the tone sliders automatically · {shortcut}")
                    };
                    let auto =
                        toolbar_action(ui, "Auto", 52., false, auto_ready && !auto_in_effect, 0);
                    if auto.on_hover_text(tip).clicked() {
                        auto_request = Some(AutoKind::Settings);
                    }
                },
            );
            // Lightroom's Treatment, above the profile; V switches it.
            // A conversion waiting for the photo to decode shows as made.
            let shown = pending_treatment.unwrap_or_else(|| r.treatment());
            let mut treatment = shown;
            control_row(ui, "Treatment", |ui| {
                let w = ui.available_width();
                segmented(
                    ui,
                    &mut treatment,
                    &[
                        (Treatment::Color, "Color"),
                        (Treatment::BlackWhite, "Black & White"),
                    ],
                    w,
                );
            });
            if treatment != shown {
                treatment_request = Some(treatment);
            }
            let old_profile = r.profile.clone();
            // Without a profile the matrix path renders through the DNG default look.
            let matrix = "Default (camera matrix)";
            control_row(ui, "Profile", |ui| {
                egui::ComboBox::from_id_salt("camera-profile")
                    .width(ui.available_width())
                    .selected_text(r.profile.as_ref().map_or(matrix, |p| p.name.as_str()))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut r.profile, None, matrix);
                        for profile in &profiles {
                            ui.selectable_value(&mut r.profile, Some(profile.clone()), &profile.name)
                                .on_hover_text(&profile.copyright);
                        }
                        ui.separator();
                        if !adobe.is_empty()
                            && ui
                                .button("Import Adobe profiles for this camera")
                                .on_hover_text(format!(
                                    "Adds Adobe Standard and the Camera Matching profiles from Lightroom's installation ({} files), which Adobe Color and the other Adobe looks build on.",
                                    adobe.len()
                                ))
                                .clicked()
                        {
                            import_adobe = true;
                            ui.close();
                        }
                        if ui
                            .button("Import profiles…")
                            .on_hover_text("Import DCP camera profiles and XMP look profiles. Select the matching base DCP and XMP together.")
                            .clicked()
                        {
                            import_profiles = true;
                            ui.close();
                        }
                        if ui
                            .button("Import profiles from folder…")
                            .on_hover_text("Import every DCP profile and XMP look in a folder and its subfolders")
                            .clicked()
                        {
                            import_folder = Some(ImportKind::CameraProfiles);
                            ui.close();
                        }
                    });
            });
            // Lightroom's Profile Amount, under the profile. Profiles without one show
            // it dimmed at 100%, so the panel doesn't move when switching.
            let supports_amount = r.profile.as_ref().is_some_and(|p| p.supports_amount());
            ui.add_enabled_ui(supports_amount, |ui| {
                let mut fixed = 1.;
                let amount = if supports_amount {
                    &mut r.profile_amount
                } else {
                    &mut fixed
                };
                ui.push_id("profile-amount", |ui| {
                    slider_with(ui, "Amount", amount, 0. ..=2., 1., Some((100., 0)), None)
                });
            })
            .response
            .on_disabled_hover_text("This profile has no Amount");
            if !profile_errors.is_empty() {
                ui.horizontal(|ui| {
                    ui.add_space(88.);
                    ui.small(format!("{} profiles unavailable", profile_errors.len()))
                        .on_hover_text(profile_errors.join("\n"));
                    if !adobe.is_empty() && ui.small_button("Import Adobe base profile").clicked() {
                        import_adobe = true;
                    }
                });
            }
            if old_profile != r.profile {
                // A newly chosen profile starts at 100%, as in Lightroom.
                r.profile_amount = 1.;
            }
            if old_profile != r.profile
                && let Some(m) = &metadata
            {
                r.profile_changed(m);
                profile_changed_from = Some(old_profile);
            }
            // White balance is its own group below the profile, as in Lightroom.
            ui.add_space(12.);
            let as_shot = metadata.as_ref().map(|m| {
                let mut shot = r.clone();
                shot.reset_white_balance(m);
                (shot.temperature, shot.tint)
            });
            control_row(ui, "WB", |ui| {
                let current = (r.temperature, r.tint);
                let near =
                    |(t, n): (f32, f32)| (t - current.0).abs() < 1. && (n - current.1).abs() < 0.5;
                let selected = if r.auto_white_balance.is_some_and(|[t, n]| near((t, n))) {
                    "Auto"
                } else if as_shot.is_some_and(near) {
                    "As Shot"
                } else {
                    NamedWhiteBalance::ALL
                        .into_iter()
                        .find(|w| near((w.values().temperature, w.values().tint)))
                        .map_or("Custom", NamedWhiteBalance::name)
                };
                // The selector sits at the row's far left, so the menu lines up
                // with Profile's.
                let rect = Rect::from_center_size(
                    Pos2::new(ui.max_rect().left() - 88. + 13., ui.max_rect().center().y),
                    Vec2::new(26., 20.),
                );
                let response = ui.interact(rect, ui.id().with("wb-selector"), Sense::click());
                let picking = view.is(Tool::WhiteBalance);
                if picking || response.hovered() {
                    ui.painter()
                        .rect_filled(rect, 3., palette.gray(if picking { 72 } else { 50 }));
                }
                eyedropper_icon(ui.painter(), rect.center(), picking || response.hovered());
                if response
                    .on_hover_text("White balance selector (W): click a neutral area of the photo")
                    .clicked()
                {
                    view.toggle(Tool::WhiteBalance);
                }
                egui::ComboBox::from_id_salt("white-balance")
                    .width(ui.available_width())
                    .selected_text(selected)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(selected == "As Shot", "As Shot")
                            .clicked()
                            && let Some(m) = &metadata
                        {
                            r.wb = [1.; 3];
                            r.reset_white_balance(m);
                        }
                        if ui
                            .add_enabled(
                                auto_ready,
                                egui::Button::selectable(selected == "Auto", "Auto"),
                            )
                            .on_hover_text("Make the photo's near-neutral areas neutral")
                            .clicked()
                        {
                            auto_request = Some(AutoKind::WhiteBalance);
                        }
                        for named in NamedWhiteBalance::ALL {
                            if ui
                                .selectable_label(selected == named.name(), named.name())
                                .clicked()
                                && let Some(m) = &metadata
                            {
                                let values = named.values();
                                r.temperature = values.temperature;
                                r.tint = values.tint;
                                r.update_wb(m);
                                r.auto_white_balance = None;
                            }
                        }
                        ui.add_enabled(
                            false,
                            egui::Button::selectable(selected == "Custom", "Custom"),
                        );
                    });
            });
            let photo = metadata.as_ref();
            setting_control(ui, r, ParameterId::Temperature, 6500., photo);
            setting_control(ui, r, ParameterId::Tint, 0., photo);
            subheading(ui, "Tone");
            for id in [
                ParameterId::Exposure,
                ParameterId::Contrast,
                ParameterId::Highlights,
                ParameterId::Shadows,
                ParameterId::Whites,
                ParameterId::Blacks,
            ] {
                setting_control(ui, r, id, 0., photo);
            }
            subheading(ui, "Presence");
            for id in [
                ParameterId::Texture,
                ParameterId::Clarity,
                ParameterId::Dehaze,
                ParameterId::Vibrance,
                ParameterId::Saturation,
            ] {
                setting_control(ui, r, id, 0., photo);
            }
        }) {
            r.wb = [1.; 3];
            r.tint = 0.;
            if let Some(m) = &metadata {
                r.reset_white_balance(m);
            }
            r.effects.monochrome = false;
            r.effects.texture = 0.;
            r.effects.clarity = 0.;
            r.effects.dehaze = 0.;
            r.vibrance = 0.;
            r.saturation = 0.;
            r.exposure = Recipe::default().exposure;
            r.contrast = 0.;
            r.highlights = 0.;
            r.shadows = 0.;
            r.whites = 0.;
            r.blacks = 0.;
        }

        let mut switch = PanelSwitch::new(r, Panel::ToneCurve);
        if switched_section(ui, "Tone Curve", &mut switch.state, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.;
                let (button, _) = ui.allocate_exact_size(Vec2::new(22., 22.), Sense::hover());
                if target_button(
                    ui,
                    button,
                    view.is(Tool::Targeted(Target::ToneCurve)),
                    &format!("Targeted adjustment: drag up or down on the photo to move the region of the tone curve there · {}", targeted_shortcut("T")),
                ) {
                    targeted_request = Some(Target::ToneCurve);
                }
                segmented(
                    ui,
                    &mut view.parametric_curve,
                    &[(true, "Parametric"), (false, "Point")],
                    112.,
                );
                if !view.parametric_curve {
                    let w = ui.available_width();
                    segmented(
                        ui,
                        &mut view.selected_curve,
                        &[(0, "RGB"), (1, "R"), (2, "G"), (3, "B")],
                        w,
                    );
                }
            });
            ui.add_space(4.);
            if view.parametric_curve {
                let moving = targeted.filter(|w| w.target == Target::ToneCurve);
                parametric_curve_ui(
                    ui,
                    &mut r.effects,
                    &histogram,
                    moving.and_then(|w| (0..4).find(|i| w.shares[*i] > 0.)),
                );
                subheading(ui, "Region");
                // Lightroom lists them lightest first.
                for (i, id) in ParameterId::PARAMETRIC.into_iter().enumerate().rev() {
                    let row = ui.push_id(("parametric", i), |ui| {
                        setting_slider(ui, id, id.value_mut(r), 0.)
                    });
                    highlight_targeted(ui, row.response.rect, moving.map_or(0., |w| w.shares[i]));
                }
            } else {
                ui.push_id(view.selected_curve, |ui| {
                    tone_curve_ui(
                        ui,
                        if view.selected_curve == 0 {
                            &mut r.curve
                        } else {
                            &mut r.effects.channels[view.selected_curve - 1]
                        },
                        &histogram,
                        view.selected_curve,
                    );
                });
                // Lightroom's Point Curve menu: the built-in curves, saved ones, Save….
                use crate::presets::curves::ShownCurve;
                let chosen = ShownCurve::of(r, &saved_curves);
                let shown = chosen.name(&saved_curves).to_string();
                control_row(ui, "Point Curve", |ui| {
                    egui::ComboBox::from_id_salt("point-curve")
                        .width(ui.available_width())
                        .selected_text(&shown)
                        .show_ui(ui, |ui| {
                            use super::curve_menu::CurveChoice;
                            use crate::presets::curves::BuiltinCurve;
                            for curve in BuiltinCurve::ALL {
                                if ui
                                    .selectable_label(
                                        chosen == ShownCurve::Builtin(curve),
                                        curve.name(),
                                    )
                                    .clicked()
                                {
                                    curve_choice = Some(CurveChoice::Builtin(curve));
                                }
                            }
                            if !saved_curves.is_empty() {
                                ui.separator();
                            }
                            for (i, saved) in saved_curves.iter().enumerate() {
                                if ui
                                    .selectable_label(chosen == ShownCurve::Saved(i), &saved.name)
                                    .clicked()
                                {
                                    curve_choice = Some(CurveChoice::Saved(saved.clone()));
                                }
                            }
                            ui.separator();
                            if ui
                                .selectable_label(false, "Save…")
                                .on_hover_text(
                                    "Save the RGB, Red, Green and Blue curves under a name",
                                )
                                .clicked()
                            {
                                curve_choice = Some(CurveChoice::Save);
                            }
                        });
                });
                // Lightroom's Refine Saturation, under the RGB point curve it acts on;
                // the channel curves keep the same height so nothing below moves.
                subheading(ui, "Refine");
                // Camera Raw renders 101–200 as 100: shown there, and kept as imported
                // until the slider is moved.
                let mut shown = r.curve_saturation.min(1.);
                let before = shown;
                ui.add_enabled_ui(view.selected_curve == 0, |ui| {
                    setting_slider(ui, ParameterId::CurveSaturation, &mut shown, 1.);
                });
                if shown != before {
                    r.curve_saturation = shown;
                }
            }
            subheading(ui, "Levels");
            slider(
                ui,
                "Black point",
                &mut r.black_point,
                0. ..=r.white_point - 0.01,
                0.,
            );
            slider(
                ui,
                "White point",
                &mut r.white_point,
                r.black_point + 0.01..=1.,
                1.,
            );
            setting_slider(ui, ParameterId::Midtone, &mut r.midtone, 1.);
        }) {
            r.black_point = 0.;
            r.white_point = 1.;
            r.midtone = 1.;
            r.curve = Recipe::default().curve;
            r.effects.channels = std::array::from_fn(|_| Default::default());
            r.curve_saturation = 1.;
            r.effects.parametric = [0.; 4];
        }
        switch.finish(r);

        // The B&W panel replaces the Color Mixer whenever the photo renders black &
        // white, by its Treatment or by a black & white profile.
        let black_white = r.treatment() == Treatment::BlackWhite;
        let (mixer_title, mixer_panel) = if black_white {
            ("B&W", Panel::BlackWhiteMix)
        } else {
            ("Color Mixer", Panel::ColorMixer)
        };
        let mut switch = PanelSwitch::new(r, mixer_panel);
        if switched_section(ui, mixer_title, &mut switch.state, |ui| {
            if black_white {
                let heading = subheading(ui, "Black & White Mix");
                // Measured (once) only while this panel is open.
                let auto_mix = photo
                    .spread()
                    .zip(photo.metadata)
                    .map(|(spread, metadata)| {
                        crate::develop::AutoMix {
                            spread: &spread,
                            metadata,
                        }
                        .for_recipe(r)
                    });
                let button = Rect::from_min_size(
                    Pos2::new(heading.right() - 52., heading.top() - 4.),
                    Vec2::new(52., 24.),
                );
                // Greyed out while the mix is Auto's, as the Basic panel's Auto.
                let enabled = auto_mix.is_some_and(|mix| mix != r.effects.gray_mix);
                let auto = ui
                    .scope_builder(egui::UiBuilder::new().max_rect(button), |ui| {
                        ui.add_enabled(enabled, egui::Button::new("Auto").small())
                    })
                    .inner;
                if auto
                    .on_hover_text("Set the mix from the photo's colors")
                    .on_disabled_hover_text("The Auto mix is applied")
                    .clicked()
                {
                    auto_mix_request = true;
                }
                let target = Rect::from_min_size(
                    Pos2::new(button.left() - 28., button.top() + 1.),
                    Vec2::splat(22.),
                );
                if target_button(
                    ui,
                    target,
                    view.is(Tool::Targeted(Target::BlackWhite)),
                    &format!(
                        "Targeted adjustment: drag up or down on the photo to brighten or darken its color · {}",
                        targeted_shortcut("G")
                    ),
                ) {
                    targeted_request = Some(Target::BlackWhite);
                }
                let moving = targeted.filter(|w| w.target == Target::BlackWhite);
                for (i, name) in BANDS.iter().enumerate() {
                    let row = ui.push_id(("bw", i), |ui| {
                        slider_with(
                            ui,
                            name,
                            &mut r.effects.gray_mix[i],
                            -1. ..=1.,
                            0.,
                            None,
                            Some((palette.gray(40), band_color(i))),
                        )
                    });
                    highlight_targeted(ui, row.response.rect, moving.map_or(0., |w| w.shares[i]));
                }
                return;
            }
            // Lightroom's tabs: the HSL and Color mixer, and Point Color.
            ui.horizontal(|ui| {
                let w = ui.available_width();
                segmented(
                    ui,
                    &mut view.mixer_tab,
                    &[
                        (MixerTab::Mixer, "Mixer"),
                        (MixerTab::PointColor, "Point Color"),
                    ],
                    w,
                );
            });
            if view.mixer_tab == MixerTab::PointColor {
                super::point_color_panel::point_color_panel(ui, &mut r.point_colors, view);
                return;
            }
            control_row(ui, "Mixer", |ui| {
                let w = ui.available_width();
                segmented(
                    ui,
                    &mut view.mixer_color,
                    &[(false, "HSL"), (true, "Color")],
                    w,
                );
            });
            if view.mixer_color {
                ui.horizontal(|ui| {
                    for (i, band) in BANDS.iter().enumerate() {
                        let (rect, response) =
                            ui.allocate_exact_size(Vec2::splat(27.), Sense::click());
                        ui.painter().circle_filled(rect.center(), 7., band_color(i));
                        if view.selected_band == i {
                            ui.painter().circle_stroke(
                                rect.center(),
                                10.,
                                Stroke::new(1.5, palette.gray(215)),
                            );
                        }
                        if response.on_hover_text(*band).clicked() {
                            view.selected_band = i;
                        }
                    }
                });
                let i = view.selected_band;
                ui.push_id(("hsl", i), |ui| {
                    for (c, name) in ["Hue", "Saturation", "Luminance"].into_iter().enumerate() {
                        slider_with(
                            ui,
                            name,
                            &mut r.hsl[i][c],
                            -1. ..=1.,
                            0.,
                            None,
                            Some(hsl_gradient(&palette, i, c)),
                        );
                    }
                });
            } else {
                let all = view.mixer_adjust == 3;
                let channel_tip = |c: usize| {
                    let name = ["hue", "saturation", "luminance"][c];
                    format!(
                        "Targeted adjustment: drag up or down on the photo to change its color's {name} · {}",
                        targeted_shortcut(["H", "S", "L"][c])
                    )
                };
                control_row(ui, "Adjust", |ui| {
                    ui.spacing_mut().item_spacing.x = 6.;
                    if !all {
                        let (button, _) =
                            ui.allocate_exact_size(Vec2::new(22., 22.), Sense::hover());
                        let target = hsl_target(view.mixer_adjust);
                        if target_button(
                            ui,
                            button,
                            view.is(Tool::Targeted(target)),
                            &channel_tip(view.mixer_adjust),
                        ) {
                            targeted_request = Some(target);
                        }
                    }
                    let w = ui.available_width();
                    segmented(
                        ui,
                        &mut view.mixer_adjust,
                        &[(0, "Hue"), (1, "Sat"), (2, "Lum"), (3, "All")],
                        w,
                    );
                });
                // Again after the tabs, which may have just changed.
                let all = view.mixer_adjust == 3;
                let channels = if all {
                    0..3
                } else {
                    view.mixer_adjust..view.mixer_adjust + 1
                };
                for c in channels {
                    let target = hsl_target(c);
                    if all {
                        let heading = subheading(ui, ["Hue", "Saturation", "Luminance"][c]);
                        let button = Rect::from_min_size(
                            Pos2::new(heading.right() - 24., heading.top() - 3.),
                            Vec2::splat(22.),
                        );
                        if target_button(
                            ui,
                            button,
                            view.is(Tool::Targeted(target)),
                            &channel_tip(c),
                        ) {
                            targeted_request = Some(target);
                        }
                    }
                    let moving = targeted.filter(|w| w.target == target);
                    for (i, name) in BANDS.iter().enumerate() {
                        let row = ui.push_id(("hsl-all", c, i), |ui| {
                            slider_with(
                                ui,
                                name,
                                &mut r.hsl[i][c],
                                -1. ..=1.,
                                0.,
                                None,
                                Some(hsl_gradient(&palette, i, c)),
                            );
                        });
                        highlight_targeted(
                            ui,
                            row.response.rect,
                            moving.map_or(0., |w| w.shares[i]),
                        );
                    }
                }
            }
        }) {
            if black_white {
                r.effects.gray_mix = [0.; 8];
            } else {
                // Both tabs: the mixer and Point Color.
                r.hsl = [[0.; 3]; 8];
                r.point_colors.clear();
            }
        }
        switch.finish(r);

        let mut switch = PanelSwitch::new(r, Panel::ColorGrading);
        if switched_section(ui, "Color Grading", &mut switch.state, |ui| {
            ui.push_id(grading_document, |ui| {
                super::color_grading::color_grading_ui(ui, r, &mut view.grading);
            });
        }) {
            r.grading = [[0.; 3]; 3];
            r.effects.global_grade = [0.; 3];
            r.effects.balance = 0.;
            r.effects.blending = 0.5;
        }
        switch.finish(r);

        let mut switch = PanelSwitch::new(r, Panel::Detail);
        let sharpening = SharpeningSliders::defaults();
        if switched_section(ui, "Detail", &mut switch.state, |ui| {
            // Lightroom's Color noise reduction default for raw files.
            let color_default = 0.25;
            let mut control = |ui: &mut egui::Ui, id: ParameterId, default: f32| {
                setting_slider(ui, id, id.value_mut(r), default);
            };
            subheading(ui, "Sharpening");
            control(ui, ParameterId::SharpeningAmount, sharpening.amount);
            control(ui, ParameterId::SharpeningRadius, sharpening.radius);
            control(ui, ParameterId::SharpeningDetail, sharpening.detail);
            control(ui, ParameterId::SharpeningMasking, sharpening.masking);
            subheading(ui, "Noise Reduction");
            control(ui, ParameterId::LuminanceNoise, 0.);
            ui.push_id("luma-nr", |ui| {
                control(ui, ParameterId::LuminanceDetail, 0.5);
                control(ui, ParameterId::LuminanceContrast, 0.);
            });
            control(ui, ParameterId::ColorNoise, color_default);
            ui.push_id("color-nr", |ui| {
                control(ui, ParameterId::ColorNoiseDetail, 0.5);
                control(ui, ParameterId::ColorNoiseSmoothness, 0.5);
            });
        }) {
            let d = Recipe::default().effects;
            r.noise_luma = 0.;
            r.effects.luma_detail = d.luma_detail;
            r.effects.luma_contrast = d.luma_contrast;
            r.set_color_noise_defaults();
            r.set_sharpening_defaults();
        }
        switch.finish(r);

        let mut switch = PanelSwitch::new(r, Panel::LensCorrections);
        if switched_section(ui, "Lens Corrections", &mut switch.state, |ui| {
            let photo = metadata.as_ref();
            subheading(ui, "Profile");
            control_row(ui, "", |ui| {
                ui.checkbox(&mut r.lens_ca, "Remove Chromatic Aberration")
                    .on_hover_text("Remove red/cyan and blue/yellow fringes toward the edges of the frame, measured from the photo itself.");
            });
            control_row(ui, "", |ui| {
                let mut on = r.lens_profile;
                if ui
                    .checkbox(&mut on, "Enable Profile Corrections")
                    .on_hover_text("Correct distortion and vignetting with an Adobe lens profile, or the lens data the camera stored in the RAW.")
                    .changed()
                    && let Some(m) = &metadata
                {
                    let state = if on {
                        crate::model::recipe::ProfileCorrections::On
                    } else {
                        crate::model::recipe::ProfileCorrections::Off
                    };
                    r.set_profile_corrections(m, state);
                }
            });
            let lens = metadata
                .as_ref()
                .map(|m| m.lens_model.clone())
                .filter(|l| !l.is_empty());
            control_row(ui, "Lens", |ui| {
                ui.label(
                    egui::RichText::new(lens.as_deref().unwrap_or("Unknown"))
                        .size(11.)
                        .color(palette.gray(200)),
                );
            });
            lens_profile::profile_menus(ui, r, metadata.as_ref());
            // Said here as well as on import, so a missing profile is never silent.
            if let Some(missing) = metadata.as_ref().and_then(|m| r.missing_lens_profile(m)) {
                control_row(ui, "", |ui| {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(missing)
                                .size(11.)
                                .color(palette.gray(150)),
                        )
                        .wrap(),
                    );
                });
            }
            if r.lens_profile {
                subheading(ui, "Amount");
                ui.push_id("lens-amount", |ui| {
                    setting_control(ui, r, ParameterId::LensDistortion, 1., photo);
                    setting_control(ui, r, ParameterId::LensVignetting, 1., photo);
                });
            }
            control_row(ui, "", |ui| {
                if ui
                    .small_button("Import Lens Profiles…")
                    .on_hover_text("Import Adobe .lcp lens profiles, e.g. from /Library/Application Support/Adobe/CameraRaw/LensProfiles")
                    .clicked()
                {
                    import_lens = true;
                }
                if ui
                    .small_button("Import Folder…")
                    .on_hover_text("Import every .lcp lens profile in a folder and its subfolders")
                    .clicked()
                {
                    import_folder = Some(ImportKind::LensProfiles);
                }
            });
            // Lightroom's Manual tab: Distortion, then Defringe and Vignetting.
            subheading(ui, "Distortion");
            ui.push_id("manual-distortion", |ui| {
                setting_control(ui, r, ParameterId::ManualDistortion, 0., photo);
            });
            let row = subheading(ui, "Defringe");
            // The Fringe Color Selector, at the row's far left as White Balance's.
            let rect = Rect::from_center_size(
                Pos2::new(row.left() + 13., row.center().y),
                Vec2::new(26., 20.),
            );
            let response = ui.interact(rect, ui.id().with("fringe-selector"), Sense::click());
            let picking = view.is(Tool::Defringe);
            if picking || response.hovered() {
                ui.painter()
                    .rect_filled(rect, 3., palette.gray(if picking { 72 } else { 50 }));
            }
            eyedropper_icon(ui.painter(), rect.center(), picking || response.hovered());
            if response
                .on_hover_text("Fringe color selector: click a purple or green fringe in the photo")
                .clicked()
            {
                view.toggle(Tool::Defringe);
            }
            defringe_sliders(ui, &mut r.effects);
            subheading(ui, "Vignetting");
            ui.push_id("lens-vignette", |ui| {
                setting_control(ui, r, ParameterId::LensVignetteAmount, 0., photo);
                setting_control(ui, r, ParameterId::LensVignetteMidpoint, 0.5, photo);
            });
        }) {
            if let Some(m) = &metadata {
                r.lens_builtin = m.lens.as_ref().is_none_or(|l| l.default_on);
            }
            let defaults = Recipe::default();
            r.lens_ca = defaults.lens_ca;
            r.lens_profile = defaults.lens_profile;
            r.lens_profile_choice = defaults.lens_profile_choice;
            r.lens_distortion = defaults.lens_distortion;
            r.lens_vignetting = defaults.lens_vignetting;
            r.lens_manual_distortion = defaults.lens_manual_distortion;
            let d = Recipe::default().effects;
            r.effects.defringe = d.defringe;
            r.effects.defringe_ranges = d.defringe_ranges;
            r.effects.lens_vignette = d.lens_vignette;
            r.effects.lens_vignette_midpoint = d.lens_vignette_midpoint;
        }
        switch.finish(r);

        let mut switch = PanelSwitch::new(r, Panel::Transform);
        if switched_section(ui, "Transform", &mut switch.state, |ui| {
            use crate::model::transform::UprightMode;
            // As Lightroom: Update beside the heading, then the modes in two rows.
            control_row(ui, "Upright", |ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Guided solves its guides again; without guides it has nothing to go by.
                    let analysed = match r.upright.mode {
                        UprightMode::Off => false,
                        UprightMode::Guided => !r.upright.guides.is_empty(),
                        _ => true,
                    };
                    if ui
                        .add_enabled(upright_ready && analysed, egui::Button::new("Update"))
                        .on_hover_text(
                            "Analyse the photo again, e.g. after changing lens corrections",
                        )
                        .clicked()
                    {
                        upright_request = true;
                    }
                });
            });
            let u = &mut r.upright;
            let before = u.mode;
            for row in [
                [
                    (UprightMode::Off, "Off"),
                    (UprightMode::Auto, "Auto"),
                    (UprightMode::Guided, "Guided"),
                ],
                [
                    (UprightMode::Level, "Level"),
                    (UprightMode::Vertical, "Vertical"),
                    (UprightMode::Full, "Full"),
                ],
            ] {
                let w = ui.available_width();
                segmented(ui, &mut u.mode, &row, w);
            }
            if u.mode != before {
                if u.mode == UprightMode::Guided {
                    // As in Lightroom, choosing Guided picks up its tool.
                    guided_action = Some(GuidedAction::Choose);
                } else if u.mode != UprightMode::Off && u.corrections.len() <= u.mode.code() {
                    upright_request = true;
                }
            }
            if u.mode == UprightMode::Guided {
                let count = u.guides.len();
                control_row(ui, "Guides", |ui| {
                    let drawing = view.is(Tool::Guided);
                    if ui
                        .selectable_label(drawing, "Draw")
                        .on_hover_text(
                            "Draw up to four guides along verticals and horizontals · Shift+T",
                        )
                        .clicked()
                    {
                        guided_action = Some(GuidedAction::Toggle);
                    }
                    ui.label(
                        egui::RichText::new(format!(
                            "{count} of {}",
                            crate::model::transform::MAX_GUIDES
                        ))
                        .color(palette.gray(170)),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(count > 0, egui::Button::new("Clear"))
                            .on_hover_text("Remove every guide")
                            .clicked()
                        {
                            guided_action = Some(GuidedAction::Clear);
                        }
                    });
                });
                if view.is(Tool::Guided) {
                    control_row(ui, "", |ui| {
                        ui.checkbox(&mut view.guided.loupe, "Show Loupe")
                            .on_hover_text("Magnify the photo while placing a guide's end");
                        ui.checkbox(&mut view.guided.grid, "Grid");
                    });
                }
            } else if view.is(Tool::Guided) {
                view.tool = Tool::None;
            }
            control_row(ui, "", |ui| {
                ui.checkbox(&mut r.constrain_crop, "Constrain Crop")
                    .on_hover_text("Shrink the crop, keeping its aspect, to leave out the white areas outside the photo");
            });
            subheading(ui, "Transform");
            // Stored as Camera Raw applies them, before the photo is turned for
            // display; shown, as in Lightroom, along the displayed photo's axes.
            let turns = metadata.as_ref().map_or(0, |m| {
                crate::model::image_frame::ImageFrame::for_metadata(m).turns
            });
            let axes =
                crate::model::transform::display_axes((turns + r.rotation) % 4, r.flip_x, r.flip_y);
            let mut shown = r.transform.displayed(axes);
            let t = &mut shown;
            setting_slider(ui, ParameterId::TransformVertical, &mut t.vertical, 0.);
            setting_slider(ui, ParameterId::TransformHorizontal, &mut t.horizontal, 0.);
            setting_slider(ui, ParameterId::TransformRotate, &mut t.rotate, 0.);
            setting_slider(ui, ParameterId::TransformAspect, &mut t.aspect, 0.);
            setting_slider(ui, ParameterId::TransformScale, &mut t.scale, 1.);
            setting_slider(ui, ParameterId::TransformOffsetX, &mut t.offset_x, 0.);
            setting_slider(ui, ParameterId::TransformOffsetY, &mut t.offset_y, 0.);
            if shown != r.transform.displayed(axes) {
                r.transform = shown.recorded(axes);
            }
        }) {
            r.transform = Default::default();
            r.upright = Default::default();
            r.constrain_crop = false;
        }
        switch.finish(r);

        let mut switch = PanelSwitch::new(r, Panel::Effects);
        if switched_section(ui, "Effects", &mut switch.state, |ui| {
            let photo = metadata.as_ref();
            subheading(ui, "Post-Crop Vignetting");
            setting_control(ui, r, ParameterId::VignetteAmount, 0., photo);
            setting_control(ui, r, ParameterId::VignetteMidpoint, 0.5, photo);
            setting_control(ui, r, ParameterId::VignetteFeather, 0.5, photo);
            subheading(ui, "Grain");
            ui.push_id("grain", |ui| {
                setting_control(ui, r, ParameterId::GrainAmount, 0., photo);
                setting_control(ui, r, ParameterId::GrainSize, 0.25, photo);
                setting_control(ui, r, ParameterId::GrainRoughness, 0.5, photo);
            });
        }) {
            r.effects.reset_post_crop();
        }
        switch.finish(r);

        let mut switch = PanelSwitch::new(r, Panel::Calibration);
        if switched_section(ui, "Calibration", &mut switch.state, |ui| {
            subheading(ui, "Process");
            ui.small("Current version");
            if r.profile.is_some()
                && let Some(m) = &metadata
            {
                let baseline = crate::camera_profiles::reference::baseline_exposure(m);
                if baseline != r.camera_exposure
                    && ui.small_button("Use camera exposure baseline")
                        .on_hover_text("Apply the camera's reference exposure offset while leaving the Exposure slider unchanged.")
                        .clicked()
                {
                    r.camera_exposure = baseline;
                }
            }
            subheading(ui, "Shadows");
            ui.push_id("calibration-shadows", |ui| {
                setting_slider(ui, ParameterId::ShadowTint, &mut r.effects.shadow_tint, 0.);
            });
            for (i, name) in ["Red Primary", "Green Primary", "Blue Primary"]
                .iter()
                .enumerate()
            {
                subheading(ui, name);
                ui.push_id(("calibration", i), |ui| {
                    let primary = ParameterId::PRIMARIES[i];
                    for id in primary {
                        setting_slider(ui, id, id.value_mut(r), 0.);
                    }
                });
            }
        }) {
            r.effects.calibration = [[0.; 2]; 3];
            r.effects.shadow_tint = 0.;
        }
        switch.finish(r);
        if let Some(kind) = auto_request {
            self.start_auto(kind);
        }
        if let Some(old) = profile_changed_from {
            self.follow_profile_treatment(old.as_deref());
        }
        if let Some(treatment) = treatment_request {
            self.set_treatment(treatment);
        }
        if auto_mix_request {
            self.auto_black_white_mix();
        }
        if let Some(choice) = curve_choice {
            self.choose_point_curve(choice);
        }
        if let Some(target) = targeted_request {
            self.toggle_targeted(target);
        }
        if upright_request {
            self.start_upright();
        }
        match guided_action {
            Some(GuidedAction::Choose) => {
                if self.view.is(Tool::Guided) {
                    self.choose_guided();
                } else {
                    self.toggle_guided_tool();
                }
            }
            Some(GuidedAction::Toggle) => self.toggle_guided_tool(),
            Some(GuidedAction::Clear) => {
                self.view.guided.selected = None;
                self.set_guides(Vec::new(), "Clear Guides");
            }
            None => {}
        }
        if import_profiles {
            self.dialog(FileDialog::CameraProfile, &ui.ctx().clone());
        }
        if import_lens {
            self.dialog(FileDialog::LensProfile, &ui.ctx().clone());
        }
        if let Some(kind) = import_folder {
            self.dialog(FileDialog::ImportFolder(kind), &ui.ctx().clone());
        }
        if import_adobe && let Some(m) = self.document.metadata.clone() {
            if let Some(key) = adobe_key {
                ui.ctx()
                    .data_mut(|d| d.remove::<Vec<std::path::PathBuf>>(key));
            }
            match crate::camera_profiles::import_files(&adobe) {
                Ok(done) => {
                    let (profiles, errors) = crate::camera_profiles::installed(&m);
                    // As the loader's: the raw defaults are resolved again
                    // after this frame, so changing an unedited photo's recipe
                    // is not taken for an edit.
                    let _ = self.tx.send(super::worker::Event::Profiles {
                        id: self.load.id(),
                        profiles,
                        errors,
                    });
                    ui.ctx().request_repaint();
                    self.refresh_library_defaults();
                    self.status = format!(
                        "Imported {} Adobe profiles for {} {}. Choose one from the Profile menu.",
                        done.len(),
                        m.make,
                        m.model
                    );
                }
                Err(e) => self.status = format!("Profiles not imported: {e:#}"),
            }
        }
    }
}

/// A Crop panel button that acts once the panel is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CropAction {
    Swap,
    AutoStraighten,
}

/// The aspect menu's name for `aspect`: a preset either way round (X swaps them),
/// Original or Free, or Custom.
fn aspect_name(aspect: f32) -> &'static str {
    let near = |a: f32, b: f32| (a - b).abs() < 1e-4;
    ASPECTS
        .iter()
        .find(|(a, _)| near(*a, aspect) || (*a > 0. && near(1. / *a, aspect)))
        .map_or("Custom", |(_, name)| *name)
}

/// The crop's aspect presets, long side over short.
pub(super) const ASPECTS: [(f32, &str); 9] = [
    (-1., "Original"),
    (0., "Free"),
    (1., "1 x 1"),
    (1.25, "4 x 5"),
    (1.4, "5 x 7"),
    (1.5, "2 x 3"),
    (4. / 3., "3 x 4"),
    (16. / 9., "16 x 9"),
    (65. / 24., "65 x 24 (XPan)"),
];
pub(super) const BANDS: [&str; 8] = [
    "Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple", "Magenta",
];
fn band_color(i: usize) -> Color32 {
    [
        Color32::from_rgb(208, 69, 61),
        Color32::from_rgb(225, 137, 55),
        Color32::from_rgb(213, 193, 61),
        Color32::from_rgb(90, 160, 92),
        Color32::from_rgb(72, 178, 178),
        Color32::from_rgb(78, 125, 207),
        Color32::from_rgb(141, 103, 191),
        Color32::from_rgb(193, 92, 165),
    ][i]
}
/// Rail colors for a mixer slider: neighbouring hues for Hue, gray to color
/// for Saturation and dark to light for Luminance.
fn hsl_gradient(palette: &theme::Palette, band: usize, channel: usize) -> (Color32, Color32) {
    let color = band_color(band);
    match channel {
        0 => (band_color((band + 7) % 8), band_color((band + 1) % 8)),
        1 => (palette.gray(110), color),
        _ => (palette.gray(25), color.lerp_to_gamma(Color32::WHITE, 0.45)),
    }
}
/// A panel's switch while its section is drawn; a click on it is stored after.
pub(super) struct PanelSwitch {
    panel: Panel,
    pub(super) state: PanelState,
}
impl PanelSwitch {
    pub(super) fn new(r: &Recipe, panel: Panel) -> Self {
        Self {
            panel,
            state: r.panels.state(panel),
        }
    }
    pub(super) fn finish(self, r: &mut Recipe) {
        r.panels.set(self.panel, self.state);
    }
}
/// Marks a slider row a Targeted Adjustment Tool drag is moving, more strongly the
/// larger its `share` of the drag.
fn highlight_targeted(ui: &egui::Ui, row: Rect, share: f32) {
    if share <= 0. {
        return;
    }
    let painter = ui.painter();
    painter.rect_filled(row, 3., Color32::from_white_alpha((6. + 16. * share) as u8));
    painter.rect_filled(
        Rect::from_min_size(row.left_top(), Vec2::new(3., row.height())),
        1.5,
        theme::palette(ui.ctx())
            .gray(235)
            .gamma_multiply(0.4 + 0.6 * share),
    );
}
/// The Targeted Adjustment Tool's shortcut ending in `key`, as the tooltips show it.
fn targeted_shortcut(key: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("⌘⌥⇧{key}")
    } else {
        format!("Ctrl+Alt+Shift+{key}")
    }
}
/// Group caption (Tone, Presence…) starting where the slider rails start.
fn subheading(ui: &mut egui::Ui, text: &str) -> Rect {
    super::widgets::set_edit_context(ui, text);
    ui.add_space(8.);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 16.), Sense::hover());
    ui.painter().text(
        rect.left_center() + Vec2::new(88., 0.),
        egui::Align2::LEFT_CENTER,
        text,
        egui::FontId::proportional(11.),
        theme::palette(ui.ctx()).gray(165),
    );
    rect
}
/// A labelled control row on the slider grid: caption right-aligned in the
/// 83 px label column, controls from the rail start (88 px) to the right edge,
/// and the same height as a slider row. Text too long for the row, such as a lens
/// or profile name, is cut short: wider rows push the panel past the window.
fn control_row<R>(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let (row, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 26.), Sense::hover());
    ui.painter().text(
        Pos2::new(row.left() + 78., row.center().y),
        egui::Align2::RIGHT_CENTER,
        label,
        egui::FontId::proportional(11.),
        theme::palette(ui.ctx()).gray(190),
    );
    let rect = Rect::from_min_max(Pos2::new(row.left() + 88., row.top()), row.max);
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
        |ui| {
            ui.spacing_mut().item_spacing.x = 4.;
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
            add(ui)
        },
    )
    .inner
}
/// Defringe's Amount and hue range for Purple, then Green. Double-clicking a hue
/// resets it to that color's own default range.
pub(super) fn defringe_sliders(ui: &mut egui::Ui, e: &mut crate::model::effects::Effects) {
    for (i, name, hue) in [(0, "Purple", [0.55, 0.9]), (1, "Green", [0.2, 0.5])] {
        ui.push_id(("defringe", i), |ui| {
            slider_with(
                ui,
                &format!("{name} Amount"),
                &mut e.defringe[i],
                0. ..=1.,
                0.,
                Some((20., 0)),
                None,
            );
            let defaults = crate::model::effects::DEFRINGE_RANGES[i];
            let [lo, hi] = &mut e.defringe_ranges[i];
            let colors = hue.map(|h| {
                let c = crate::develop::color::hue_rgb(h).map(|v| (v * 180.) as u8);
                Color32::from_rgb(c[0], c[1], c[2])
            });
            let low = slider_with(
                ui,
                &format!("{name} Hue"),
                lo,
                0. ..=(*hi - 0.1).max(0.),
                defaults[0],
                None,
                Some((colors[0], colors[1])),
            );
            let high = ui
                .push_id("hi", |ui| {
                    slider_with(
                        ui,
                        "",
                        hi,
                        (*lo + 0.1).min(1.)..=1.,
                        defaults[1],
                        None,
                        Some((colors[0], colors[1])),
                    )
                })
                .inner;
            // One range, as in Lightroom: resetting either end resets both, so the
            // other end never keeps the default out of reach.
            if [low, high].contains(&SliderEvent::Reset) {
                [*lo, *hi] = defaults;
                // Named after the whole range, not the end the slider clamped.
                name_history_step(
                    ui,
                    format!("Defringe {name} Hue"),
                    format!("{:.0} / {:.0}", defaults[0] * 100., defaults[1] * 100.),
                );
            }
        });
    }
}
fn eyedropper_icon(painter: &egui::Painter, c: Pos2, strong: bool) {
    let color = theme::palette(painter.ctx()).gray(if strong { 235 } else { 170 });
    icons::paint_at(painter, Icon::Eyedropper, c, 14., color);
}
/// The line that opens the Remove and Masking drawers: both tools are new.
/// What the experimental label's tooltip says about a tool.
const UNMEASURED: &str = "Not yet measured against Lightroom. Spots and masks are saved apart \
     from the rest of the edit, so older RAWmakase releases open the photo without them.";
const RED_EYE_NOTE: &str = "Fitted to Camera Raw on synthetic eyes. Corrections are saved apart \
     from the rest of the edit, so older RAWmakase releases open the photo without them.";
fn experimental(ui: &mut egui::Ui, note: &str) {
    ui.horizontal(|ui| {
        ui.add_space(83.);
        ui.label(
            egui::RichText::new("Experimental · early version")
                .size(10.)
                .color(theme::palette(ui.ctx()).warning()),
        )
        .on_hover_text(note);
    });
}
/// A circle with an arrow leaving it: the Remove tool.
fn heal_icon(painter: &egui::Painter, c: Pos2, strong: bool) {
    let stroke = Stroke::new(
        1.4,
        theme::palette(painter.ctx()).gray(if strong { 240 } else { 170 }),
    );
    painter.circle_stroke(c + Vec2::new(-2., 2.), 4.5, stroke);
    painter.line_segment([c + Vec2::new(1.5, -1.5), c + Vec2::new(6., -6.)], stroke);
    painter.line_segment([c + Vec2::new(6., -6.), c + Vec2::new(2.5, -6.)], stroke);
    painter.line_segment([c + Vec2::new(6., -6.), c + Vec2::new(6., -2.5)], stroke);
}
/// An eye: the Red Eye tool.
fn eye_icon(painter: &egui::Painter, c: Pos2, strong: bool) {
    let color = theme::palette(painter.ctx()).gray(if strong { 240 } else { 170 });
    let stroke = Stroke::new(1.4, color);
    let lid = |sign: f32| -> Vec<Pos2> {
        (0..=12)
            .map(|i| {
                let t = i as f32 / 12. * std::f32::consts::PI;
                c + Vec2::new(-6.5 * t.cos(), sign * 4. * t.sin())
            })
            .collect()
    };
    painter.add(egui::Shape::line(lid(1.), stroke));
    painter.add(egui::Shape::line(lid(-1.), stroke));
    painter.circle_filled(c, 2.2, color);
}
/// A dashed circle over a square: the Masking tool.
fn mask_icon(painter: &egui::Painter, c: Pos2, strong: bool) {
    let color = theme::palette(painter.ctx()).gray(if strong { 240 } else { 170 });
    let stroke = Stroke::new(1.4, color);
    painter.rect_stroke(
        Rect::from_center_size(c, Vec2::splat(12.)),
        1.,
        stroke,
        egui::StrokeKind::Inside,
    );
    painter.circle_filled(c + Vec2::new(1., 1.), 3.5, color);
}
fn crop_icon(painter: &egui::Painter, c: Pos2, strong: bool) {
    let color = theme::palette(painter.ctx()).gray(if strong { 240 } else { 170 });
    icons::paint_at(painter, Icon::Crop, c, 15., color);
}

/// What the Transform panel asks of the Guided Upright tool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GuidedAction {
    /// Guided was chosen among the modes.
    Choose,
    /// Draw: open or close the tool.
    Toggle,
    Clear,
}

/// The slider for develop setting `id`, and what changing it implies for the rest
/// of the recipe (`model::edit`).
fn setting_control(
    ui: &mut egui::Ui,
    r: &mut Recipe,
    id: ParameterId,
    default: f32,
    photo: Option<&crate::camera_data::Metadata>,
) {
    crate::model::edit::change_setting(r, id, photo, |r| {
        setting_slider(ui, id, id.value_mut(r), default)
    });
}
