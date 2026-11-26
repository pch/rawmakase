//! Lightroom's Export dialog, and the question it asks when the file exists.
use super::{Editor, Status};
use crate::app::export::naming::menu::Choice;
use crate::app::export::naming::presets::Preset;
use crate::app::extensions::FormOf;
use crate::app::ui::design::{Tone, button, radius, size, space, text};
use crate::app::ui::theme;
use crate::app::ui::widgets::{confirm_modal, form_row, modal_frame, pretty_path, primary_button};
use crate::color::OutputSpace;
use crate::export_settings::{AfterExport, Destination, Existing, Format, Include, NameContext};
use eframe::egui::{self, Color32, Sense, Stroke, Vec2};
use std::{path::Path, sync::atomic::Ordering};

const WIDTH: f32 = 700.;
const HEIGHT: f32 = 600.;
/// The name the built-in watermark is listed under.
const SIMPLE_COPYRIGHT_LABEL: &str = "Simple Copyright Watermark";

fn destination_label(d: Destination) -> &'static str {
    match d {
        Destination::SameFolder => "Same folder as original photo",
        Destination::Desktop => "Desktop",
        Destination::Pictures => "Pictures",
        Destination::Folder => "Specific folder",
    }
}
fn existing_label(e: Existing) -> &'static str {
    match e {
        Existing::Ask => "Ask what to do",
        Existing::Unique => "Choose a new name for the exported file",
        Existing::Overwrite => "Overwrite without warning",
        Existing::Skip => "Skip",
    }
}

impl Editor {
    pub(in crate::app) fn export_windows(&mut self, ctx: &egui::Context) {
        self.conflict_window(ctx);
        if self.exports.dialog {
            self.export_dialog(ctx);
        }
        self.watermark_editor(ctx);
        self.name_editor_window(ctx);
        self.export_waiting(ctx);
    }

    fn export_dialog(&mut self, ctx: &egui::Context) {
        let palette = theme::palette(ctx);
        if let Some(folder) = self.exports.picked.lock().ok().and_then(|mut p| p.take()) {
            self.exports.draft.folder = Some(folder);
            self.exports.draft.destination = Destination::Folder;
        }
        // Names and folders are shown for the first photo.
        let (source, fields, place) = self
            .exports
            .scope
            .photos
            .first()
            .map(|p| (p.source.clone(), p.fields.clone(), p.place))
            .unwrap_or_default();
        let count = self.exports.scope.photos.len();
        let chosen = self.exports.scope.chosen.max(count);
        // As the export names it.
        let at = NameContext {
            index: place,
            total: chosen,
            fields: &fields,
        };
        // What the names need from the files is read once they need it
        // (here, or once the Filename Template Editor adds it); Export waits
        // for it, and says when it could not be read.
        let template = self.exports.draft.name_template();
        self.names_wait(&template);
        let read = self.names_read_status(&template);
        let reading = matches!(read, Some(Status::Reading { .. }));
        let left_out = self.exports.scope.left_out.clone();
        // Return is the Export button, as in Lightroom, also from a text field;
        // it is taken before the dialog draws so a focused control does not also
        // act on it. An open menu, or the Edit Watermark window on top, keeps it.
        let mut confirmed = (!egui::Popup::is_any_open(ctx)
            && self.exports.watermark_editor.is_none()
            && self.exports.name_editor.is_none()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)))
        .then_some(true);
        let response = egui::Modal::new(egui::Id::new("export-dialog"))
            .backdrop_color(Color32::from_black_alpha(140))
            .frame(modal_frame(&palette))
            .show(ctx, |ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::new(WIDTH, HEIGHT), Sense::hover());
                let title = if count == 1 {
                    "Export One File".to_string()
                } else {
                    format!("Export {count} Files")
                };
                crate::app::ui::widgets::name_dialog(ui, &title);
                ui.painter().text(
                    rect.left_top() + Vec2::new(24., 26.),
                    egui::Align2::LEFT_CENTER,
                    title,
                    egui::FontId::proportional(text::TITLE),
                    palette.tone(Tone::STRONG),
                );
                // Said before the export starts: what will not be exported, and why.
                if !left_out.is_empty() {
                    let mut reasons: Vec<(String, usize)> = Vec::new();
                    for (_, why) in &left_out {
                        match reasons.iter_mut().find(|(r, _)| r == why) {
                            Some((_, n)) => *n += 1,
                            None => reasons.push((why.clone(), 1)),
                        }
                    }
                    let reasons = reasons
                        .iter()
                        .map(|(why, n)| format!("{why} ({n})"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    let line = format!(
                        "{} won't be exported: {reasons}",
                        crate::app::ui::widgets::plural(left_out.len(), "photo", "photos")
                    );
                    let at = egui::Rect::from_min_size(
                        rect.left_top() + Vec2::new(200., 16.),
                        Vec2::new(WIDTH - 224., 20.),
                    );
                    ui.put(
                        at,
                        egui::Label::new(
                            egui::RichText::new(line)
                                .size(text::LABEL)
                                .color(Color32::from_rgb(230, 170, 100)),
                        )
                        .truncate(),
                    )
                    .on_hover_text(
                        left_out
                            .iter()
                            .map(|(name, why)| format!("{name}: {why}"))
                            .collect::<Vec<_>>()
                            .join("\n"),
                    );
                }
                let body = egui::Rect::from_min_max(
                    rect.left_top() + Vec2::new(16., 52.),
                    rect.right_bottom() - Vec2::new(16., 64.),
                );
                let mut content = ui.new_child(egui::UiBuilder::new().max_rect(body));
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(&mut content, |ui| {
                        self.export_sections(ui, &source, &at, read.as_ref());
                    });
                ui.painter().hline(
                    rect.x_range(),
                    rect.bottom() - 64.,
                    Stroke::new(1., palette.tone(Tone::RAISED)),
                );
                let footer = egui::Rect::from_min_max(
                    egui::pos2(rect.left() + 24., rect.bottom() - 56.),
                    egui::pos2(rect.right() - 24., rect.bottom() - 8.),
                );
                let mut bar = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(footer)
                        .layout(egui::Layout::right_to_left(egui::Align::Center)),
                );
                bar.spacing_mut().button_padding = button::DIALOG;
                let export = match &read {
                    Some(Status::Reading { done, total }) => {
                        format!("Reading photo info… {done} of {total}")
                    }
                    Some(Status::Failed(_)) => "Export without them".into(),
                    _ => "Export".into(),
                };
                if bar
                    .add_enabled_ui(!reading, |ui| primary_button(ui, &export))
                    .inner
                    .clicked()
                {
                    confirmed = Some(true);
                }
                if bar
                    .add(egui::Button::new("Cancel").min_size(Vec2::new(84., 30.)))
                    .clicked()
                {
                    confirmed = Some(false);
                }
                if let Some((target, _)) = self.exports.draft.target_for(&source, &at) {
                    bar.add_space(space::L);
                    bar.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(format!("Saves to {}", pretty_path(&target)))
                                    .size(text::LABEL)
                                    .color(palette.tone(Tone::MUTED)),
                            )
                            .truncate(),
                        )
                        .on_hover_text(target.display().to_string());
                    });
                }
            });
        // Escape closes one window at a time: a window opened from the
        // dialog keeps it, also on the frame it opens, while the dialog is
        // still the top one.
        let close = self.exports.watermark_editor.is_none()
            && self.exports.name_editor.is_none()
            && response.should_close();
        match confirmed.or(close.then_some(false)) {
            // Return too waits for the names' files.
            Some(true) if reading => {}
            Some(true) if self.exports.draft.target(&source).is_none() => {
                self.messages.warn("Choose a folder to export to");
            }
            Some(true) if let Err(e) = self.exports.draft.check_output() => {
                self.messages.warn(e.to_string());
            }
            Some(true) => {
                if let Some(key) = self.exports.draft.send_to.clone() {
                    self.take_send_to_drafts(&key, true);
                }
                self.exports.option_form = Default::default();
                self.exports.dialog = false;
                self.export(self.exports.draft.clone());
            }
            Some(false) => {
                // Nothing typed outlives a cancelled dialog.
                self.exports.option_form = Default::default();
                self.exports.dialog = false;
                self.exports.read.cancel();
            }
            None => {}
        }
    }

    /// The dialog's sections, in its scroll area; a shot draws them whole.
    pub(super) fn export_sections(
        &mut self,
        ui: &mut egui::Ui,
        source: &Path,
        example: &NameContext,
        read: Option<&Status>,
    ) {
        ui.spacing_mut().item_spacing = Vec2::new(space::M, space::M);
        ui.spacing_mut().interact_size.y = size::FIELD;
        let palette = theme::palette(ui.ctx());
        let picking = self.exports.picking.load(Ordering::Relaxed);
        let s = &mut self.exports.draft;
        let mut choose = false;
        section(ui, "Export Location");
        form_row(ui, "Export To", |ui| {
            crate::app::ui::widgets::named_menu(
                crate::app::ui::widgets::dropdown(ui, "export-to")
                    .width(300.)
                    .selected_text(destination_label(s.destination))
                    .show_ui(ui, |ui| {
                        for d in Destination::ALL {
                            ui.selectable_value(&mut s.destination, d, destination_label(d));
                        }
                    }),
                "Export To",
            );
        });
        let folder = s.base_folder(source);
        form_row(ui, "Folder", |ui| {
            choose = ui
                .add_enabled(!picking, egui::Button::new("Choose…"))
                .clicked();
            let text = folder
                .as_deref()
                .map_or("No folder chosen".into(), pretty_path);
            ui.add(
                egui::Label::new(egui::RichText::new(text).color(palette.tone(Tone::SECONDARY)))
                    .truncate(),
            );
        });
        form_row(ui, "", |ui| {
            ui.checkbox(&mut s.subfolder, "Put in Subfolder:");
            crate::app::ui::widgets::named(
                ui.add_enabled(
                    s.subfolder,
                    egui::TextEdit::singleline(&mut s.subfolder_name).desired_width(240.),
                ),
                "Subfolder",
            );
        });
        form_row(ui, "Existing Files", |ui| {
            crate::app::ui::widgets::named_menu(
                crate::app::ui::widgets::dropdown(ui, "export-existing")
                    .width(300.)
                    .selected_text(existing_label(s.existing))
                    .show_ui(ui, |ui| {
                        for e in Existing::ALL {
                            ui.selectable_value(&mut s.existing, e, existing_label(e));
                        }
                    }),
                "Existing Files",
            );
        });

        section(ui, "File Naming");
        let naming = s.naming();
        let custom = s.custom_tokens().map(str::to_string);
        let mut edit_names = false;
        form_row(ui, "Rename To", |ui| {
            let combo = crate::app::ui::widgets::dropdown(ui, "export-naming").width(220.);
            match crate::app::export::naming::menu::combo(
                ui,
                combo,
                "Rename To",
                naming,
                custom.as_deref(),
                &self.exports.saved,
                false,
            ) {
                Some(Choice::Preset(n)) => s.choose_naming(n),
                Some(Choice::Tokens(tokens)) => s.choose_tokens(&tokens),
                Some(Choice::Edit) => edit_names = true,
                None => {}
            }
        });
        let template = s.name_template();
        form_row(ui, "Custom Text", |ui| {
            crate::app::ui::widgets::named(
                ui.add_enabled(
                    template.uses_text(),
                    egui::TextEdit::singleline(&mut s.custom_text).desired_width(220.),
                ),
                "Custom Text",
            );
        });
        form_row(ui, "Start Number", |ui| {
            crate::app::ui::widgets::named(
                ui.add_enabled(
                    template.uses_sequence(),
                    egui::DragValue::new(&mut s.start_number).range(0..=999_999),
                ),
                "Start Number",
            );
        });
        form_row(ui, "Example", |ui| {
            let (name, notes) = s.file_name_for(source, example);
            match read {
                Some(Status::Reading { .. }) => ui.label(
                    egui::RichText::new("Reading photo info…").color(palette.tone(Tone::SECONDARY)),
                ),
                _ => ui.label(egui::RichText::new(name).color(palette.tone(Tone::TEXT))),
            };
            if let Some(Status::Failed(why)) = read {
                ui.label(
                    egui::RichText::new(format!("Photo info could not be read: {why}"))
                        .size(text::LABEL)
                        .color(Color32::from_rgb(230, 170, 100)),
                );
            } else if !notes.is_empty() && !matches!(read, Some(Status::Reading { .. })) {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(notes.join(" · "))
                            .size(text::LABEL)
                            .color(palette.tone(Tone::MUTED)),
                    )
                    .truncate(),
                );
            }
        });
        if edit_names {
            let tokens = custom.as_deref().unwrap_or(naming.tokens()).to_string();
            let preset = Preset::of(naming, custom.as_deref(), &self.exports.saved);
            self.exports.name_editor = Some(super::NameEditor::new(&tokens, preset));
            return;
        }
        let s = &mut self.exports.draft;
        form_row(ui, "Extensions", |ui| {
            crate::app::ui::widgets::named_menu(
                crate::app::ui::widgets::dropdown(ui, "export-case")
                    .width(140.)
                    .selected_text(if s.uppercase {
                        "Uppercase"
                    } else {
                        "Lowercase"
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut s.uppercase, false, "Lowercase");
                        ui.selectable_value(&mut s.uppercase, true, "Uppercase");
                    }),
                "Extensions",
            );
        });

        section(ui, "File Settings");
        form_row(ui, "Image Format", |ui| {
            crate::app::ui::widgets::named_menu(
                crate::app::ui::widgets::dropdown(ui, "export-format")
                    .width(140.)
                    .selected_text(s.format.raster().name())
                    .show_ui(ui, |ui| {
                        for format in Format::ALL {
                            if ui
                                .selectable_label(s.format == format, format.raster().name())
                                .clicked()
                            {
                                s.choose_format(format);
                            }
                        }
                    }),
                "Image Format",
            );
        });
        // One row for either format, and a JPEG's size limit under it, as
        // Lightroom's: the limit picks the quality, so the slider is off with it.
        match s.format {
            Format::Jpeg => {
                form_row(ui, "Quality", |ui| {
                    ui.spacing_mut().slider_width = 220.;
                    crate::app::ui::widgets::named(
                        ui.add_enabled(!s.limit_size, egui::Slider::new(&mut s.quality, 1..=100)),
                        "Quality",
                    );
                });
                form_row(ui, "", |ui| {
                    ui.checkbox(&mut s.limit_size, "Limit File Size To:");
                    crate::app::ui::widgets::named(
                        ui.add_enabled(
                            s.limit_size,
                            egui::DragValue::new(&mut s.size_limit)
                                .range(1..=crate::export_settings::SIZE_LIMIT_MAX)
                                .suffix(" K"),
                        ),
                        "File Size Limit",
                    );
                });
            }
            Format::Tiff => form_row(ui, "Bit Depth", |ui| {
                let linear = s
                    .hdr
                    .is_some_and(|h| h.transfer == crate::color::hdr_output::HdrTransfer::Linear);
                let depth = if linear {
                    "32-bit float for HDR edits, 16 bits/component otherwise"
                } else {
                    "16 bits/component, uncompressed"
                };
                ui.label(egui::RichText::new(depth).color(palette.tone(Tone::TEXT)));
            }),
            Format::Png => form_row(ui, "Bit Depth", |ui| {
                ui.label(egui::RichText::new("8 bits/component").color(palette.tone(Tone::TEXT)));
            }),
            // Lossy at the quality, as JPEG is, or lossless; 8 bits/component, 16 for HDR.
            Format::Jxl => form_row(ui, "Quality", |ui| {
                ui.spacing_mut().slider_width = 160.;
                crate::app::ui::widgets::named(
                    ui.add_enabled(s.jxl_lossy, egui::Slider::new(&mut s.quality, 1..=100)),
                    "Quality",
                );
                let mut lossless = !s.jxl_lossy;
                if ui
                    .checkbox(&mut lossless, "Lossless")
                    .on_hover_text("8 bits/component, 16 for HDR; much larger files")
                    .changed()
                {
                    s.jxl_lossy = !lossless;
                }
            }),
        }
        form_row(ui, "Color Space", |ui| {
            let later = s.later_color_space().is_some();
            let shown = if later {
                "Not available in this version"
            } else {
                s.color_space.label()
            };
            crate::app::ui::widgets::named_menu(
                crate::app::ui::widgets::dropdown(ui, "export-color-space")
                    .width(220.)
                    .selected_text(shown)
                    .show_ui(ui, |ui| {
                        for space in OutputSpace::ALL {
                            let supported = s.format.supports(space);
                            let item = ui
                                .add_enabled(
                                    supported.is_ok(),
                                    egui::Button::selectable(
                                        !later && s.color_space == space,
                                        space.label(),
                                    ),
                                )
                                .on_disabled_hover_text(
                                    supported.err().map(|e| e.to_string()).unwrap_or_default(),
                                );
                            if item.clicked() {
                                s.choose_color_space(space);
                            }
                        }
                    }),
                "Color Space",
            );
        });
        if s.format.holds_hdr() {
            hdr_rows(ui, s);
        }
        // Why the choice can't be exported as it is: ProPhoto RGB in JPEG or PNG,
        // or a space from a later release.
        if let Err(e) = s.check_output() {
            form_row(ui, "", |ui| {
                ui.label(
                    egui::RichText::new(e.to_string())
                        .size(text::LABEL)
                        .color(Color32::from_rgb(230, 170, 100)),
                );
            });
        }

        section(ui, "Image Sizing");
        form_row(ui, "", |ui| {
            ui.checkbox(&mut s.resize, "Resize to Fit: Long Edge");
            crate::app::ui::widgets::named(
                ui.add_enabled(
                    s.resize,
                    egui::DragValue::new(&mut s.long_edge)
                        .range(100..=30_000)
                        .suffix(" px"),
                ),
                "Long Edge",
            );
        });
        form_row(ui, "Resolution", |ui| {
            crate::app::ui::widgets::named(
                ui.add(
                    egui::DragValue::new(&mut s.ppi)
                        .range(1..=10_000)
                        .suffix(" pixels per inch"),
                ),
                "Resolution",
            );
        });

        section(ui, "Watermarking");
        let presets = self.exports.watermarks.clone();
        let marks = crate::environment::watermarks(&self.core);
        let simple = crate::watermark::SIMPLE_COPYRIGHT;
        let mut edit = None;
        form_row(ui, "", |ui| {
            ui.checkbox(&mut s.watermark, "Watermark:");
            let label = if s.watermark_name == simple {
                SIMPLE_COPYRIGHT_LABEL.to_string()
            } else {
                s.watermark_name.clone()
            };
            ui.add_enabled_ui(s.watermark, |ui| {
                crate::app::ui::widgets::named_menu(
                    crate::app::ui::widgets::dropdown(ui, "export-watermark")
                        .width(260.)
                        .selected_text(label)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut s.watermark_name,
                                simple.to_string(),
                                SIMPLE_COPYRIGHT_LABEL,
                            );
                            for w in &presets {
                                ui.selectable_value(&mut s.watermark_name, w.name.clone(), &w.name);
                            }
                            ui.separator();
                            if ui.selectable_label(false, "Edit Watermarks…").clicked() {
                                edit = Some(
                                    presets
                                        .iter()
                                        .find(|w| w.name == s.watermark_name)
                                        .cloned()
                                        .unwrap_or_default(),
                                );
                            }
                        }),
                    "Watermark",
                );
            });
            // A preset whose image went missing is flagged here, and the
            // export would stop before rendering.
            let missing = s.watermark
                && presets
                    .iter()
                    .find(|w| w.name == s.watermark_name)
                    .is_some_and(|w| w.image_missing(&marks));
            if missing {
                ui.label(
                    egui::RichText::new("Watermark image not found")
                        .size(text::LABEL)
                        .color(Color32::from_rgb(230, 120, 100)),
                );
            }
        });
        if let Some(w) = edit {
            self.exports.watermark_editor = Some(super::watermark_editor::WatermarkEditor::new(
                w,
                self.watermarks(),
                ui.ctx(),
            ));
            return;
        }
        let s = &mut self.exports.draft;
        section(ui, "Metadata");
        form_row(ui, "Include", |ui| {
            crate::app::ui::widgets::named_menu(
                crate::app::ui::widgets::dropdown(ui, "export-include")
                    .width(260.)
                    .selected_text(s.include.label())
                    .show_ui(ui, |ui| {
                        for include in Include::ALL {
                            if include == Include::Custom {
                                ui.separator();
                            }
                            ui.selectable_value(&mut s.include, include, include.label());
                        }
                    }),
                "Include",
            );
        });
        // Four rows in every mode, so nothing below moves.
        if s.include == Include::Custom {
            form_row(ui, "", |ui| {
                ui.checkbox(&mut s.capture, "Camera and capture info (EXIF)");
            });
            form_row(ui, "", |ui| {
                ui.add_space(space::XXL);
                ui.add_enabled(
                    s.capture,
                    egui::Checkbox::new(&mut s.location, "Include location info"),
                );
            });
            form_row(ui, "", |ui| {
                ui.checkbox(&mut s.develop, "Develop settings (Camera Raw XMP)");
            });
            form_row(ui, "", |ui| {
                ui.checkbox(
                    &mut s.descriptive,
                    "Title, caption, creator, copyright, rating, label and keywords",
                );
            });
        } else {
            let forced = s.include.removes_location();
            let mut remove = s.remove_location || forced;
            form_row(ui, "", |ui| {
                ui.add_enabled(
                    !forced,
                    egui::Checkbox::new(&mut remove, "Remove Location Info"),
                );
            });
            if !forced {
                s.remove_location = remove;
            }
            for _ in 0..3 {
                form_row(ui, "", |_| {});
            }
        }
        let destinations = self.extensions.export_destinations();
        let s = &mut self.exports.draft;
        section(ui, "Post-Processing");
        form_row(ui, "After Export", |ui| {
            crate::app::ui::widgets::named_menu(
                crate::app::ui::widgets::dropdown(ui, "export-after")
                    .width(220.)
                    .selected_text(s.after_export.label())
                    .show_ui(ui, |ui| {
                        for after in AfterExport::ALL {
                            ui.selectable_value(&mut s.after_export, after, after.label());
                        }
                    }),
                "After Export",
            );
        });
        // Only with an extension's export destination to offer, or one chosen.
        let before = s.send_to.clone();
        if !destinations.is_empty() || s.send_to.is_some() {
            send_to_row(ui, &mut s.send_to, &destinations);
        }
        let left = before.filter(|b| s.send_to.as_ref() != Some(b));
        let chosen = s.send_to.as_ref().and_then(|key| {
            destinations
                .into_iter()
                .find(|d| d.key == *key && d.unavailable.is_none())
        });
        // What was typed for the destination left keeps, as a field losing
        // focus would: its field is no longer drawn to lose it.
        if let Some(left) = left {
            self.take_send_to_drafts(&left, true);
        }
        if let Some(destination) = chosen {
            self.send_to_options(ui, &destination);
        }
        ui.add_space(space::M);
        if choose {
            self.choose_export_folder(ui.ctx());
        }
    }

    /// The chosen export destination's options, under Send To, drawn as its
    /// extension's settings are; a value changed is kept in the draft by
    /// destination, and left out once it is the option's default again.
    pub(super) fn send_to_options(
        &mut self,
        ui: &mut egui::Ui,
        destination: &crate::app::extensions::Destination,
    ) {
        let key = &destination.key;
        let values = crate::app::extensions::option_values(
            &destination.options,
            self.exports.draft.send_to_options.get(key),
        );
        for option in &destination.options {
            if !crate::app::extensions::setting_shown(&destination.options, &values, option) {
                continue;
            }
            let value = values.get(&option.id).cloned().unwrap_or_default();
            let label = option.name.clone().unwrap_or_else(|| option.id.clone());
            let mut set = None;
            form_row(ui, &label, |ui| {
                set = self.setting_control(ui, FormOf::Export, key, option, &value);
            });
            if let Some(help) = &option.help {
                form_row(ui, "", |ui| crate::app::extensions::setting_help(ui, help));
            }
            if let Some(value) = set {
                self.exports.option_form.forget_draft(key, &option.id);
                choose_option(&mut self.exports.draft, key, option, value);
            }
        }
    }

    /// The text being typed into the chosen destination's options, taken
    /// into the draft before it exports, as a field losing focus would; or
    /// dropped when the dialog is cancelled.
    fn take_send_to_drafts(&mut self, key: &str, keep: bool) {
        let drafts = self.exports.option_form.take_drafts(key);
        if !keep {
            return;
        }
        let Some(destination) = self
            .extensions
            .export_destinations()
            .into_iter()
            .find(|d| d.key == key)
        else {
            return;
        };
        for (id, text) in drafts {
            if let Some(option) = destination.options.iter().find(|o| o.id == id) {
                choose_option(&mut self.exports.draft, key, option, text.into());
            }
        }
    }

    /// The system folder chooser, on its own thread so the window keeps drawing.
    fn choose_export_folder(&mut self, ctx: &egui::Context) {
        self.exports.picking.store(true, Ordering::Relaxed);
        let picked = self.exports.picked.clone();
        let picking = self.exports.picking.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            if let Some(folder) = rfd::FileDialog::new().pick_folder()
                && let Ok(mut slot) = picked.lock()
            {
                *slot = Some(folder);
            }
            picking.store(false, Ordering::Relaxed);
            ctx.request_repaint();
        });
    }

    /// Ask's question for a batch, once for all its files that exist, in
    /// Lightroom's words; the answer applies to every one of them.
    fn conflict_window(&mut self, ctx: &egui::Context) {
        let Some(pending) = &self.exports.pending else {
            return;
        };
        let shown = 12;
        let mut files: Vec<String> = pending
            .conflicts
            .iter()
            .take(shown)
            .map(|p| pretty_path(p))
            .collect();
        if pending.conflicts.len() > shown {
            files.push(format!("and {} more", pending.conflicts.len() - shown));
        }
        let detail = format!(
            "{}\n\nDo you wish to overwrite the existing files, skip the existing files, or rename the exported files to avoid collision?",
            files.join("\n")
        );
        let Some(choice) = confirm_modal(
            ctx,
            "export-conflict",
            "The following files already exist.",
            &detail,
            false,
            &[
                ("Cancel", None),
                ("Skip", Some(Existing::Skip)),
                ("Use Unique Names", Some(Existing::Unique)),
                ("Overwrite", Some(Existing::Overwrite)),
            ],
            None,
        ) else {
            return;
        };
        let Some(pending) = self.exports.pending.take() else {
            return;
        };
        match choice {
            Some(answer) => self.plan_export(pending, Some(answer)),
            None => self.messages.note("Export cancelled"),
        }
    }
}

/// Send To: Nowhere, or an export destination of an enabled extension. One
/// that can't take the files now is listed, disabled, with why; chosen, it
/// is named with why beside it, and the export runs without it.
pub(super) fn send_to_row(
    ui: &mut egui::Ui,
    send_to: &mut Option<String>,
    destinations: &[crate::app::extensions::Destination],
) {
    const NOWHERE: &str = "Nowhere";
    let chosen = send_to
        .as_ref()
        .map(|key| destinations.iter().find(|d| d.key == *key).ok_or(key));
    let (shown, why) = match chosen {
        None => (NOWHERE.to_string(), None),
        Some(Ok(d)) => (d.name.clone(), d.unavailable.clone()),
        Some(Err(key)) => (
            key.clone(),
            Some("its extension isn't installed or is disabled".to_string()),
        ),
    };
    form_row(ui, "Send To", |ui| {
        crate::app::ui::widgets::named_menu(
            crate::app::ui::widgets::dropdown(ui, "export-send-to")
                .width(220.)
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    ui.selectable_value(send_to, None, NOWHERE);
                    for d in destinations {
                        let item = ui
                            .add_enabled(
                                d.unavailable.is_none(),
                                egui::Button::selectable(send_to.as_ref() == Some(&d.key), &d.name),
                            )
                            .on_disabled_hover_text(d.unavailable.clone().unwrap_or_default());
                        if item.clicked() {
                            *send_to = Some(d.key.clone());
                        }
                    }
                }),
            "Send To",
        );
        if let Some(why) = why {
            ui.label(
                egui::RichText::new(format!("Not sent: {why}"))
                    .size(text::SMALL)
                    .color(theme::palette(ui.ctx()).tone(Tone::SECONDARY)),
            );
        }
    });
}

/// Keeps `value` as option `option`'s of destination `key` in `settings`,
/// or forgets it when it is the default, so the settings hold only what was
/// changed.
pub(super) fn choose_option(
    settings: &mut crate::export_settings::ExportSettings,
    key: &str,
    option: &rawmakase_extensions::protocol::manifest::Setting,
    value: serde_json::Value,
) {
    let chosen = settings.send_to_options.entry(key.to_string()).or_default();
    if crate::app::extensions::same_value(&value, &option.default) {
        chosen.remove(&option.id);
    } else {
        chosen.insert(option.id.clone(), value);
    }
    if chosen.is_empty() {
        settings.send_to_options.remove(key);
    }
}

/// A Lightroom section band.
fn section(ui: &mut egui::Ui, title: &str) {
    let palette = theme::palette(ui.ctx());
    ui.add_space(space::S);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.), Sense::hover());
    ui.painter()
        .rect_filled(rect, radius::CONTROL, palette.tone(Tone::RAISED));
    ui.painter().text(
        rect.left_center() + Vec2::new(12., 0.),
        egui::Align2::LEFT_CENTER,
        title,
        egui::FontId::proportional(text::BODY),
        palette.tone(Tone::STRONG),
    );
}

/// HDR export: off (SDR, in the color space above), or the edit's HDR master, as HDR
/// pixels in a gamut and transfer (TIFF, JPEG XL) or as a gain map over the SDR
/// rendition (JPEG, JPEG XL). Edits with HDR off are refused when exported
/// (`export::job::prepare`), so the choice is the user's to make per export.
fn hdr_rows(ui: &mut egui::Ui, s: &mut crate::export_settings::ExportSettings) {
    use crate::color::hdr_output::{HdrGamut, HdrOutput, HdrTransfer};
    // A JPEG holds a gain map over its sRGB base, TIFF and JPEG XL HDR pixels.
    let jpeg = s.format == Format::Jpeg;
    form_row(ui, "HDR", |ui| {
        let mut on = s.hdr.is_some();
        let (label, tip) = if jpeg {
            (
                "Export HDR edits with a gain map",
                "Writes the SDR rendition with an ISO 21496-1 gain map to the HDR master: \
                 HDR displays show the master, others the SDR rendition. \
                 A watermark is drawn in both.",
            )
        } else {
            (
                "Export HDR edits in HDR",
                "Writes the HDR master of edits with High Dynamic Range on. \
                 Graphics white is 203 cd/m², and a watermark's white is graphics white.",
            )
        };
        if ui.checkbox(&mut on, label).on_hover_text(tip).changed() {
            s.choose_hdr(on.then(|| {
                if jpeg {
                    HdrOutput {
                        gamut: HdrGamut::Rec709,
                        gain_map: true,
                        ..Default::default()
                    }
                } else {
                    HdrOutput::default()
                }
            }));
        }
    });
    let Some(mut output) = s.hdr else {
        return;
    };
    if jpeg {
        // The gain map's base is sRGB; nothing else to choose.
        if !output.gain_map || output.gamut != HdrGamut::Rec709 {
            output.gain_map = true;
            output.gamut = HdrGamut::Rec709;
            s.choose_hdr(Some(output));
        }
        return;
    }
    if s.format == Format::Jxl {
        form_row(ui, "", |ui| {
            ui.checkbox(&mut output.gain_map, "As a gain map over the SDR rendition")
                .on_hover_text(
                    "An SDR JPEG XL that HDR displays show as the HDR master (ISO 21496-1), \
                     in place of HDR pixels",
                );
        });
    } else {
        output.gain_map = false;
    }
    if output.gain_map {
        output.gamut = HdrGamut::Rec709;
        if s.hdr != Some(output) {
            s.choose_hdr(Some(output));
        }
        return;
    }
    form_row(ui, "HDR Gamut", |ui| {
        crate::app::ui::widgets::named_menu(
            crate::app::ui::widgets::dropdown(ui, "export-hdr-gamut")
                .width(220.)
                .selected_text(output.gamut.label())
                .show_ui(ui, |ui| {
                    for gamut in HdrGamut::ALL {
                        ui.selectable_value(&mut output.gamut, gamut, gamut.label());
                    }
                }),
            "HDR Gamut",
        );
    });
    form_row(ui, "HDR Encoding", |ui| {
        crate::app::ui::widgets::named_menu(
            crate::app::ui::widgets::dropdown(ui, "export-hdr-transfer")
                .width(220.)
                .selected_text(output.transfer.label())
                .show_ui(ui, |ui| {
                    for transfer in HdrTransfer::ALL {
                        ui.selectable_value(&mut output.transfer, transfer, transfer.label());
                    }
                }),
            "HDR Encoding",
        );
    });
    if s.hdr != Some(output) {
        s.choose_hdr(Some(output));
    }
}
