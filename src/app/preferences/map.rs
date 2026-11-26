//! Preferences › General's Map: whether the Library's Metadata panel shows
//! where photos were taken on a map, and the tile server it asks.
use super::{group, hint};
use crate::app::Editor;
use crate::app::library::map::{Settings, tiles};
use crate::app::ui::widgets::form_row;
use eframe::egui;

impl Editor {
    pub(super) fn map_rows(&mut self, ui: &mut egui::Ui) {
        group(ui, "Map");
        let mut changed = false;
        form_row(ui, "Metadata panel", |ui| {
            changed |= ui
                .checkbox(&mut self.map.on, "Show where photos were taken on a map")
                .changed();
        });
        form_row(ui, "", |ui| {
            hint(
                ui,
                "Each map image asked for tells the tile server roughly where your photos \
                 were taken. Nothing is asked for while this is off.",
            );
        });
        let on = self.map.on;
        ui.add_enabled_ui(on, |ui| {
            changed |= committed(ui, "Tile server", &mut self.map.tiles, tiles::OPENSTREETMAP);
            form_row(ui, "", |ui| match self.map.problem() {
                Some(problem) => {
                    ui.colored_label(ui.visuals().error_fg_color, problem);
                }
                None => hint(
                    ui,
                    "An HTTPS address, with {z}, {x} and {y} where a tile's numbers go.",
                ),
            });
            changed |= committed(
                ui,
                "Credit",
                &mut self.map.credit,
                tiles::OPENSTREETMAP_CREDIT,
            );
            form_row(ui, "", |ui| {
                hint(ui, "Shown on the map, as the tile server's terms ask.")
            });
            let default = Settings::default();
            let custom = self.map.tiles != default.tiles || self.map.credit != default.credit;
            form_row(ui, "", |ui| {
                if ui
                    .add_enabled(custom, egui::Button::new("Use OpenStreetMap"))
                    .clicked()
                {
                    self.map = Settings { on, ..default };
                    changed = true;
                }
            });
        });
        if changed {
            let _ = self.save_session();
        }
    }
}

/// A text field whose value is taken when it is left or Enter is pressed, so
/// a half-typed address is never asked for tiles. Empty is `default`.
fn committed(ui: &mut egui::Ui, label: &str, value: &mut String, default: &str) -> bool {
    let id = ui.id().with(("map-field", label));
    let mut committed = false;
    form_row(ui, label, |ui| {
        let mut draft = ui
            .data(|d| d.get_temp::<String>(id))
            .unwrap_or_else(|| value.clone());
        let field = crate::app::ui::widgets::named(
            ui.add(egui::TextEdit::singleline(&mut draft).desired_width(320.)),
            label,
        );
        // A draft left behind when the page was switched away from while it was
        // typed in (the field wasn't drawn as it lost the focus) is taken too.
        let stranded = !field.has_focus() && ui.data(|d| d.get_temp::<String>(id)).is_some();
        if field.lost_focus() || stranded {
            let taken = if draft.trim().is_empty() {
                default.to_owned()
            } else {
                draft.trim().to_owned()
            };
            committed = taken != *value;
            *value = taken;
            ui.data_mut(|d| d.remove::<String>(id));
        } else if field.has_focus() {
            ui.data_mut(|d| d.insert_temp(id, draft));
        }
    });
    committed
}

#[cfg(test)]
mod tests {
    use crate::app::library::map::Settings;
    use eframe::egui::Vec2;

    /// The General page's Map group, off, on, and on with an address that
    /// can't be used, for `RAWMAKASE_UI_SHOTS` ([`crate::app::ui::shots`]).
    #[test]
    fn shots_of_the_map_preferences() {
        for name in [
            "preferences-map-off",
            "preferences-map-on",
            "preferences-map-http",
        ] {
            crate::app::ui::shots::shot(
                name,
                Vec2::new(760., 330.),
                move |e, _| {
                    e.map = match name {
                        "preferences-map-off" => Settings::default(),
                        "preferences-map-on" => Settings {
                            on: true,
                            ..Settings::default()
                        },
                        _ => Settings {
                            on: true,
                            tiles: "http://tiles.example.org/{z}/{x}/{y}.png".into(),
                            ..Settings::default()
                        },
                    };
                    e.onboarding.visible = false;
                },
                // The group alone: the window's General page is taller than a shot.
                |e, ctx| {
                    crate::app::ui::shots::panel(ctx, 700., |ui| {
                        ui.add_space(8.);
                        e.map_rows(ui);
                        super::super::gap(ui);
                    })
                },
            );
        }
    }
}
