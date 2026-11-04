//! Lightroom's Filename Template Editor: the template as text with its
//! tokens shown as chips, a menu of tokens for each of Lightroom's groups,
//! inserted at the cursor, the Preset menu (`presets`), and an Example the
//! host makes for its first photo. Done, or Return, gives the host the
//! template; Cancel drops it. A file name has no line breaks, so the
//! template has none either.
use super::presets::{Library, Preset, Presets};
use crate::app::ui::design::{self, Tone, button, inset, space, text};
use crate::app::ui::theme;
use crate::app::ui::widgets::{modal_frame, primary_button};
use crate::export_settings::{Menu, Span, TokenTemplate, menu as tokens_in};
use eframe::egui::{self, Color32, Stroke, Vec2};
use egui::text::{CCursor, CCursorRange, LayoutJob, TextFormat};

/// The editor's groups, as Lightroom lays them out: a name, and its menus.
const GROUPS: [(&str, &[Menu]); 5] = [
    ("Image Name", &[Menu::ImageName]),
    ("Numbering", &[Menu::Numbering]),
    ("Additional", &[Menu::Date]),
    ("Metadata", &[Menu::Metadata, Menu::Exif]),
    ("Custom", &[Menu::Custom]),
];
const MENUS: [Menu; 6] = [
    Menu::ImageName,
    Menu::Numbering,
    Menu::Date,
    Menu::Metadata,
    Menu::Exif,
    Menu::Custom,
];

/// The editor, open.
pub(in crate::app) struct NameEditor {
    /// The template in the editor's form.
    pub text: String,
    /// The token each menu shows, by its index in the menu.
    chosen: [usize; MENUS.len()],
    /// The text has been given the keyboard, as the editor opened.
    focused: bool,
    pub presets: Presets,
}

impl NameEditor {
    /// Editing `tokens`, Lightroom's form, which are `preset`'s.
    pub(crate) fn new(tokens: &str, preset: Preset) -> Self {
        Self {
            text: TokenTemplate::parse(tokens).to_display(),
            chosen: [0; MENUS.len()],
            focused: false,
            presets: Presets::new(preset, tokens),
        }
    }
}

/// What the editor's frame ended with.
#[derive(Debug, PartialEq)]
pub(in crate::app) enum Outcome {
    /// Still open.
    Open,
    /// Done: the template made.
    Done(TokenTemplate),
    /// Cancel, Escape or a click outside.
    Cancel,
}

pub(in crate::app) fn text_id() -> egui::Id {
    egui::Id::new("filename-template-text")
}

/// `label` as a token inserted in `text` where the cursor is, over the
/// selection; the cursor then after it.
fn insert(ctx: &egui::Context, text: &mut String, label: &str) {
    let token = format!("{{{label}}}");
    let chars = text.chars().count();
    let mut state = egui::TextEdit::load_state(ctx, text_id()).unwrap_or_default();
    let (start, end) = state.cursor.char_range().map_or((chars, chars), |r| {
        let (a, b) = (r.primary.index.0.min(chars), r.secondary.index.0.min(chars));
        (a.min(b), a.max(b))
    });
    let byte = |char_index: usize| {
        text.char_indices()
            .nth(char_index)
            .map_or(text.len(), |(i, _)| i)
    };
    let (from, to) = (byte(start), byte(end));
    text.replace_range(from..to, &token);
    let after = start + token.chars().count();
    state
        .cursor
        .set_char_range(Some(CCursorRange::one(CCursor::new(after))));
    state.store(ctx, text_id());
    ctx.memory_mut(|m| m.request_focus(text_id()));
}

impl NameEditor {
    /// The template as typed: what it is, or what is wrong with it.
    pub(crate) fn parsed(
        &self,
    ) -> Result<TokenTemplate, Vec<crate::export_settings::DisplayError>> {
        TokenTemplate::from_display(&self.text)
    }

    /// Draws the editor over its host, with `example`, the first photo's
    /// name by the template typed and what it has to say, and `library`,
    /// the saved templates its Preset menu lists and changes; how the frame
    /// ended.
    pub(crate) fn show(
        &mut self,
        ctx: &egui::Context,
        example: &(String, Vec<String>),
        library: &mut Library,
    ) -> Outcome {
        let palette = theme::palette(ctx);
        let editor = self;
        // Typing goes to the template as it opens, not to a field of the
        // window under it (which would also keep Escape and Return).
        if !editor.focused {
            ctx.memory_mut(|m| m.request_focus(text_id()));
            editor.focused = true;
        }
        // Return is Done, as in Lightroom, also from the text; it is taken
        // before the editor draws so the text does not get a line break. An
        // open menu, or a question of the Preset menu, keeps it.
        let asking = editor.presets.asking();
        let returned = !asking
            && !egui::Popup::is_any_open(ctx)
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
        // A file name has no line breaks: pasted, typed or composed, they are
        // dropped before the text takes them, so its cursor stays where it was.
        let line_break = |c: &char| matches!(c, '\n' | '\r');
        ctx.input_mut(|i| {
            for event in &mut i.events {
                match event {
                    egui::Event::Text(text)
                    | egui::Event::Paste(text)
                    | egui::Event::Ime(egui::ImeEvent::Commit(text)) => {
                        text.retain(|c| !line_break(&c));
                    }
                    egui::Event::Ime(egui::ImeEvent::Preedit {
                        text,
                        active_range_chars,
                    }) => {
                        let kept = |n| text.chars().take(n).filter(|c| !line_break(c)).count();
                        if let Some(range) = active_range_chars {
                            *range = kept(range.start)..kept(range.end);
                        }
                        text.retain(|c| !line_break(&c));
                    }
                    _ => {}
                }
            }
        });
        let mut done = false;
        let mut cancel = false;
        let response = egui::Modal::new(egui::Id::new("filename-template-editor"))
            .backdrop_color(Color32::from_black_alpha(100))
            .frame(modal_frame(&palette).inner_margin(design::margin_same(inset::DIALOG)))
            .show(ctx, |ui| {
                ui.set_width(560.);
                ui.spacing_mut().item_spacing = Vec2::new(space::M, space::M);
                ui.label(
                    egui::RichText::new("Filename Template Editor")
                        .size(text::TITLE)
                        .color(palette.tone(Tone::STRONG)),
                );
                let tokens = TokenTemplate::from_display(&editor.text)
                    .ok()
                    .map(|t| t.to_lightroom());
                if let Some(tokens) = editor.presets.row(ui, &palette, tokens.as_deref(), library) {
                    editor.text = TokenTemplate::parse(&tokens).to_display();
                }
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Example:").color(palette.tone(Tone::SECONDARY)));
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&example.0).color(palette.tone(Tone::TEXT)),
                        )
                        .truncate(),
                    );
                });
                let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap: f32| {
                    let job = layout(buf.as_str(), wrap, &palette);
                    ui.fonts_mut(|f| f.layout_job(job))
                };
                ui.add(
                    egui::TextEdit::multiline(&mut editor.text)
                        .id(text_id())
                        .desired_rows(3)
                        .desired_width(f32::INFINITY)
                        .return_key(None)
                        .layouter(&mut layouter),
                );
                // What is wrong, or what to know.
                match TokenTemplate::from_display(&editor.text) {
                    Err(errors) => {
                        for e in errors.iter().take(3) {
                            ui.label(
                                egui::RichText::new(&e.message)
                                    .size(text::LABEL)
                                    .color(Color32::from_rgb(235, 110, 100)),
                            );
                        }
                    }
                    Ok(template) => {
                        for note in &example.1 {
                            ui.label(
                                egui::RichText::new(note)
                                    .size(text::LABEL)
                                    .color(palette.tone(Tone::MUTED)),
                            );
                        }
                        if !template.names_each_photo() {
                            ui.label(
                                egui::RichText::new(
                                    "Without Filename or a Sequence #, photos may share a name: \
                                     those after the first get “-2”, “-3”…",
                                )
                                .size(text::LABEL)
                                .color(palette.tone(Tone::MUTED)),
                            );
                        }
                    }
                }
                ui.add_space(space::XS);
                let mut chosen = editor.chosen;
                let mut inserted = None;
                for (group, menus) in GROUPS {
                    ui.label(
                        egui::RichText::new(group)
                            .size(text::LABEL)
                            .color(palette.tone(Tone::SOFT)),
                    );
                    for &m in menus {
                        let slot = MENUS.iter().position(|x| *x == m).unwrap_or(0);
                        let defs: Vec<_> = tokens_in(m).collect();
                        let index = chosen[slot].min(defs.len().saturating_sub(1));
                        ui.horizontal(|ui| {
                            ui.add_space(space::XL);
                            egui::ComboBox::from_id_salt(("filename-token-menu", slot))
                                .width(260.)
                                .selected_text(defs.get(index).map_or("", |d| d.label))
                                .show_ui(ui, |ui| {
                                    for (i, def) in defs.iter().enumerate() {
                                        ui.selectable_value(&mut chosen[slot], i, def.label);
                                    }
                                });
                            if ui.button("Insert").clicked()
                                && let Some(def) = defs.get(chosen[slot].min(defs.len() - 1))
                            {
                                inserted = Some(def.label);
                            }
                        });
                    }
                }
                editor.chosen = chosen;
                if let Some(label) = inserted {
                    insert(ui.ctx(), &mut editor.text, label);
                }
                ui.add_space(space::M);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().button_padding = button::DIALOG;
                        let valid = TokenTemplate::from_display(&editor.text).is_ok();
                        done = ui
                            .add_enabled_ui(valid, |ui| primary_button(ui, "Done"))
                            .inner
                            .clicked()
                            || (returned && valid);
                        cancel = ui
                            .add(egui::Button::new("Cancel").min_size(Vec2::new(84., 30.)))
                            .clicked();
                    });
                });
            });
        let tokens = TokenTemplate::from_display(&editor.text)
            .ok()
            .map(|t| t.to_lightroom());
        // Escape closes one window at a time: the question first.
        let close = !asking && !editor.presets.asking() && response.should_close();
        editor
            .presets
            .prompt_window(ctx, tokens.as_deref(), library);
        if done && let Ok(template) = TokenTemplate::from_display(&editor.text) {
            Outcome::Done(template)
        } else if cancel || close {
            Outcome::Cancel
        } else {
            Outcome::Open
        }
    }
}

/// The editor's text laid out: tokens as chips, tokens RAWmakase doesn't
/// fill greyed, mistakes underlined.
fn layout(text: &str, wrap: f32, palette: &theme::Palette) -> LayoutJob {
    let font = egui::FontId::proportional(text::LARGE);
    let format = |span: Span| match span {
        Span::Text => TextFormat::simple(font.clone(), palette.tone(Tone::TEXT)),
        Span::Token => TextFormat {
            background: palette.accent(),
            ..TextFormat::simple(font.clone(), palette.on_accent())
        },
        Span::Inert => TextFormat {
            background: palette.tone(Tone::PRESSED),
            ..TextFormat::simple(font.clone(), palette.tone(Tone::SOFT))
        },
        Span::Error => TextFormat {
            underline: Stroke::new(1.5, Color32::from_rgb(235, 110, 100)),
            ..TextFormat::simple(font.clone(), Color32::from_rgb(240, 160, 150))
        },
    };
    let mut job = LayoutJob::default();
    let mut at = 0;
    for (range, span) in TokenTemplate::spans(text) {
        // Every byte once, in order, whatever the spans say.
        let start = range.start.max(at);
        if start >= range.end {
            continue;
        }
        if start > at {
            job.append(&text[at..start], 0., format(Span::Text));
        }
        job.append(&text[start..range.end], 0., format(span));
        at = range.end;
    }
    if at < text.len() {
        job.append(&text[at..], 0., format(Span::Text));
    }
    job.wrap.max_width = wrap;
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The layout shows every character of the text, once and in order, so
    /// the cursor and the text agree.
    #[test]
    fn the_layout_shows_the_whole_text() {
        let palette = theme::palette(&egui::Context::default());
        for text in [
            "",
            "{Filename}-{Date (YYYY)}",
            "a{Nope}b}c{",
            "{?x\\}y} \\{日本\\} {Flash}",
            "{Filename",
        ] {
            let job = layout(text, 400., &palette);
            assert_eq!(job.text, text);
        }
    }

    #[test]
    fn tokens_are_inserted_at_the_cursor_over_the_selection() {
        let ctx = egui::Context::default();
        let mut text = "日本-x".to_string();
        let mut state = egui::TextEdit::load_state(&ctx, text_id()).unwrap_or_default();
        state
            .cursor
            .set_char_range(Some(CCursorRange::two(CCursor::new(3), CCursor::new(2))));
        state.store(&ctx, text_id());
        insert(&ctx, &mut text, "Model");
        assert_eq!(text, "日本{Model}x");
        insert(&ctx, &mut text, "Hour");
        assert_eq!(text, "日本{Model}{Hour}x");
    }
}
