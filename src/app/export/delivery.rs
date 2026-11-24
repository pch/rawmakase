//! Send To: an export's delivery to an export destination, driven by the
//! export. It begins when the export is queued, sends each file as the batch
//! puts it in place, and ends once the batch is over and every file sent was
//! received; `export.end`'s answer says how each file went. The requests go
//! through the extension host (`extensions/destinations.rs`), and their
//! outcomes come back here ([`Editor::delivery_answered`]).
use super::Exports;
use crate::app::Editor;
use crate::app::extensions::destinations::{Begun, FileFacts, Step};
use crate::app::ui::design::{Tone, radius, text};
use crate::app::ui::theme;
use crate::app::ui::widgets::plural;
use crate::export::batch::{BatchPhoto, Outcome};
use crate::export_settings::ExportSettings;
use eframe::egui::{self, Color32, Sense, Vec2};
use rawmakase_extensions::protocol::method::{ExportBegin, ExportEnded, ExportFile, Sent};
use rawmakase_extensions::protocol::{ErrorCode, RemoteError};
use std::collections::BTreeMap;

/// A delivery from its beginning to its end's answer.
pub(super) struct Delivery {
    to: Begun,
    /// What each photo's `export.file` says besides its file, by its place.
    facts: Vec<FileFacts>,
    /// `export.begin` was answered: files can go.
    begun: bool,
    /// Files written before that.
    waiting: Vec<ExportFile>,
    /// Files sent and not yet received.
    in_flight: usize,
    /// The files sent, by place, with their names.
    sent: BTreeMap<u32, String>,
    /// Once the batch is over: what the export's report said, and whether it
    /// was cancelled.
    exported: Option<(String, bool)>,
    /// `export.end` was sent.
    ending: bool,
}

impl Exports {
    /// A new delivery id, unique while the app runs.
    fn next_delivery(&mut self) -> String {
        self.deliveries_begun += 1;
        self.deliveries_begun.to_string()
    }
}

impl Editor {
    /// Begins a delivery of `batch`'s files to Send To's destination, as
    /// `settings` choose it, with the options chosen for it read now.
    /// Returns its id, or `None` when there is no Send To or it couldn't
    /// begin, which the top bar says.
    pub(super) fn begin_delivery(
        &mut self,
        batch: &[BatchPhoto],
        settings: &ExportSettings,
    ) -> Option<String> {
        let key = settings.send_to.as_ref()?;
        let descriptive = crate::export::assemble::Policy::of(settings).descriptive;
        let id = self.exports.next_delivery();
        let begin = ExportBegin {
            delivery: id.clone(),
            destination: String::new(),
            folder: settings.export_folder(batch.iter().map(|p| p.source.as_path())),
            count: u32::try_from(batch.len()).unwrap_or(u32::MAX),
            options: serde_json::Map::new(),
        };
        let chosen = settings.send_to_options.get(key);
        match self.extensions.begin_delivery(key, begin, chosen) {
            Ok(to) => {
                let delivery = Delivery {
                    to,
                    facts: batch
                        .iter()
                        .map(|p| FileFacts::of(p, descriptive))
                        .collect(),
                    begun: false,
                    waiting: Vec::new(),
                    in_flight: 0,
                    sent: BTreeMap::new(),
                    exported: None,
                    ending: false,
                };
                self.exports.deliveries.insert(id.clone(), delivery);
                Some(id)
            }
            Err(why) => {
                self.messages.fail(format!("Not sent: {why}"));
                None
            }
        }
    }

    /// A photo of delivery `id`'s batch has its outcome: an exported file
    /// goes to the destination.
    pub(super) fn delivery_photo(&mut self, id: &str, index: usize, outcome: &Outcome) {
        let Outcome::Exported { path, size, .. } = outcome else {
            return;
        };
        let Some(d) = self.exports.deliveries.get_mut(id) else {
            return;
        };
        let Some(facts) = d.facts.get(index) else {
            return;
        };
        d.waiting.push(facts.file(id, index, path, *size));
        self.advance_delivery(id);
    }

    /// Delivery `id`'s batch is over: `exported` is what its report said.
    pub(super) fn delivery_exported(&mut self, id: &str, exported: String, cancelled: bool) {
        if let Some(d) = self.exports.deliveries.get_mut(id) {
            d.exported = Some((exported, cancelled));
            self.advance_delivery(id);
        }
    }

    /// One of a delivery's requests answered.
    pub(in crate::app) fn delivery_answered(
        &mut self,
        id: &str,
        step: Step,
        result: Result<Option<ExportEnded>, RemoteError>,
    ) {
        let Some(d) = self.exports.deliveries.get_mut(id) else {
            return;
        };
        match (step, result) {
            (_, Err(e)) => self.delivery_failed(id, &e),
            (Step::Begin, Ok(_)) => {
                d.begun = true;
                self.advance_delivery(id);
            }
            (Step::File(_), Ok(_)) => {
                d.in_flight = d.in_flight.saturating_sub(1);
                self.advance_delivery(id);
            }
            (Step::End, Ok(ended)) => {
                let d = self.exports.deliveries.remove(id).expect("looked up above");
                self.delivered(&d, &ended.unwrap_or_default());
            }
        }
    }

    /// Stop on the top bar's delivery.
    fn stop_delivery(&mut self, id: &str) {
        let Some(d) = self.exports.deliveries.remove(id) else {
            return;
        };
        // Its requests in flight settle as cancelled; the delivery is gone,
        // so they say nothing more.
        drop(self.extensions.stop_delivery(&d.to, id));
        self.messages
            .note(format!("Sending to {} stopped", d.to.name));
    }

    /// Sends what delivery `id` can send now: the files written once it has
    /// begun, then its end once the batch is over and every file sent was
    /// received.
    fn advance_delivery(&mut self, id: &str) {
        let Some(d) = self.exports.deliveries.get_mut(id) else {
            return;
        };
        if !d.begun || d.ending {
            return;
        }
        let to = d.to.clone();
        let files = std::mem::take(&mut d.waiting);
        for file in files {
            let (index, name) = (file.index, file.name.clone());
            if let Err(why) = self.extensions.send_file(&to, file) {
                return self.delivery_failed(id, &why);
            }
            let d = self.exports.deliveries.get_mut(id).expect("still open");
            d.in_flight += 1;
            d.sent.insert(index, name);
        }
        let d = self.exports.deliveries.get_mut(id).expect("still open");
        let Some((_, cancelled)) = d.exported else {
            return;
        };
        if d.in_flight > 0 {
            return;
        }
        d.ending = true;
        let files = u32::try_from(d.sent.len()).unwrap_or(u32::MAX);
        if let Err(why) = self.extensions.end_delivery(&to, id, files, cancelled) {
            self.delivery_failed(id, &why);
        }
    }

    /// Delivery `id` can't go on: the top bar says why, and nothing more is
    /// sent.
    fn delivery_failed(&mut self, id: &str, why: &RemoteError) {
        let Some(d) = self.exports.deliveries.remove(id) else {
            return;
        };
        drop(self.extensions.stop_delivery(&d.to, id));
        let name = &d.to.name;
        let exported = d.exported.as_ref().map(|(said, _)| said.as_str());
        let line = |what: String| match exported {
            Some(said) => format!("{said} · {what}"),
            None => what,
        };
        if matches!(why.code, ErrorCode::Cancelled | ErrorCode::Disabled) {
            self.messages
                .note(line(format!("Sending to {name} stopped")));
        } else {
            self.messages
                .fail(line(format!("Not sent to {name}: {}", why.message)));
        }
    }

    /// The top bar's line for a delivery's end, after what was exported.
    fn delivered(&mut self, d: &Delivery, ended: &ExportEnded) {
        let (level, line, detail) = delivered_line(d, ended);
        let Some(line) = line else { return };
        match (level, detail) {
            (Level::Done, Some(detail)) => self.messages.done_with_detail(line, detail),
            (Level::Done, None) => self.messages.done(line),
            (Level::Failure, Some(detail)) => self.messages.fail_with_detail(line, detail),
            (Level::Failure, None) => self.messages.fail(line),
        }
    }

    /// While a delivery's export is over and it is still sending: "Sending to
    /// Web Gallery", with what it last said of how far it is, over a bar, and
    /// a button that stops it.
    pub(in crate::app) fn delivery_progress(&mut self, ui: &mut egui::Ui) {
        let Some((id, d)) = self
            .exports
            .deliveries
            .iter()
            .find(|(_, d)| d.exported.is_some())
        else {
            return;
        };
        let id = id.clone();
        let progress = self.extensions.delivery_progress(&d.to, &id);
        let palette = theme::palette(ui.ctx());
        let (rect, _) = ui.allocate_exact_size(Vec2::new(210., 28.), Sense::hover());
        let mut label = format!("Sending to {}", d.to.name);
        if let Some(said) = progress.as_ref().and_then(|(_, t)| t.as_deref()) {
            label = format!("{label} · {said}");
        }
        let painter = ui.painter();
        painter.text(
            egui::pos2(rect.left(), rect.top() + 7.),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(text::SMALL),
            palette.tone(Tone::SOFT),
        );
        let fraction = progress.and_then(|(done, _)| done).unwrap_or(0.) as f32;
        let bar = egui::Rect::from_min_size(
            egui::pos2(rect.left(), rect.top() + 17.),
            Vec2::new(rect.width() - 26., 4.),
        );
        painter.rect_filled(bar, radius::SMALL, palette.tone(Tone::OUTLINE));
        painter.rect_filled(
            egui::Rect::from_min_size(bar.min, Vec2::new(bar.width() * fraction.clamp(0., 1.), 4.)),
            2.,
            Color32::from_rgb(110, 150, 190),
        );
        let close = egui::Rect::from_center_size(
            egui::pos2(rect.right() - 9., rect.center().y),
            Vec2::splat(18.),
        );
        let stop = ui
            .interact(close, ui.id().with("stop-delivery"), Sense::click())
            .on_hover_text("Stop sending");
        let color = palette.tone(if stop.hovered() {
            Tone::STRONG
        } else {
            Tone::SECONDARY
        });
        crate::app::ui::icons::paint_at(
            ui.painter(),
            crate::app::ui::icons::Icon::Close,
            close.center(),
            13.,
            color,
        );
        if stop.clicked() {
            self.stop_delivery(&id);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Level {
    Done,
    Failure,
}

/// The top bar's line for delivery `d`'s end, after what was exported:
/// "Exported 12 of 12 · Sent 12 files to Web Gallery", or "Sent 10 of 12"
/// with why the others weren't, and the destination's message, where to see
/// the result, and each file's URL or remote id in the detail. A file the
/// answer left out counts as not sent. None when no file was sent.
fn delivered_line(d: &Delivery, ended: &ExportEnded) -> (Level, Option<String>, Option<String>) {
    let name = &d.to.name;
    let total = d.sent.len();
    if total == 0 {
        return (Level::Done, None, None);
    }
    let mut sent = 0;
    let mut problems = Vec::new();
    let mut shown = Vec::new();
    for (index, file) in &d.sent {
        let result = ended.files.iter().find(|r| r.index == *index);
        match result {
            Some(r) if r.result == Sent::Failed => problems.push(format!(
                "{file}: {}",
                r.message.as_deref().unwrap_or("not sent")
            )),
            Some(r) => {
                sent += usize::from(r.result == Sent::Sent);
                if let Some(at) = r.url.as_ref().or(r.remote_id.as_ref()) {
                    shown.push(format!("{file}: {at}"));
                }
            }
            None => problems.push(format!("{file}: no result")),
        }
    }
    let files = plural(total, "file", "files");
    let what = if sent == total {
        format!("Sent {files} to {name}")
    } else {
        format!("Sent {sent} of {files} to {name}")
    };
    let line = match &d.exported {
        Some((said, _)) => format!("{said} · {what}"),
        None => what,
    };
    let detail: Vec<String> = [&ended.message, &ended.location]
        .into_iter()
        .flatten()
        .cloned()
        .chain(problems.iter().cloned())
        .chain(shown)
        .collect();
    let level = if problems.is_empty() {
        Level::Done
    } else {
        Level::Failure
    };
    (
        level,
        Some(line),
        (!detail.is_empty()).then(|| detail.join("\n")),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rawmakase_extensions::protocol::method::FileResult;

    fn delivery(sent: &[(u32, &str)]) -> Delivery {
        Delivery {
            to: Begun {
                extension: "dev.jane.gallery".into(),
                destination: "web".into(),
                name: "Web Gallery".into(),
            },
            facts: Vec::new(),
            begun: true,
            waiting: Vec::new(),
            in_flight: 0,
            sent: sent.iter().map(|(i, n)| (*i, n.to_string())).collect(),
            exported: Some(("Exported 3 of 3".into(), false)),
            ending: true,
        }
    }

    fn result(index: u32, result: Sent, message: Option<&str>, url: Option<&str>) -> FileResult {
        FileResult {
            index,
            result,
            message: message.map(str::to_string),
            remote_id: None,
            url: url.map(str::to_string),
        }
    }

    #[test]
    fn a_deliverys_line_says_how_each_file_went() {
        let d = delivery(&[(0, "a.jpg"), (1, "b.jpg"), (2, "c.jpg")]);
        let all = ExportEnded {
            files: vec![
                result(2, Sent::Sent, None, None),
                result(0, Sent::Sent, None, Some("https://example.com/a")),
                result(1, Sent::Skipped, Some("already there"), None),
            ],
            message: Some("Published Wedding".into()),
            location: Some("https://example.com".into()),
        };
        assert_eq!(
            delivered_line(&d, &all),
            (
                Level::Done,
                Some("Exported 3 of 3 · Sent 2 of 3 files to Web Gallery".into()),
                Some("Published Wedding\nhttps://example.com\na.jpg: https://example.com/a".into())
            )
        );
        // A failure is named with why, and a file the answer left out counts
        // as not sent.
        let some = ExportEnded {
            files: vec![
                result(0, Sent::Sent, None, None),
                result(1, Sent::Failed, Some("too large"), None),
            ],
            ..Default::default()
        };
        assert_eq!(
            delivered_line(&d, &some),
            (
                Level::Failure,
                Some("Exported 3 of 3 · Sent 1 of 3 files to Web Gallery".into()),
                Some("b.jpg: too large\nc.jpg: no result".into())
            )
        );
        // All sent, nothing more to say.
        let one = delivery(&[(0, "a.jpg")]);
        let sent = ExportEnded {
            files: vec![result(0, Sent::Sent, None, None)],
            ..Default::default()
        };
        assert_eq!(
            delivered_line(&one, &sent),
            (
                Level::Done,
                Some("Exported 3 of 3 · Sent 1 file to Web Gallery".into()),
                None
            )
        );
        // Nothing sent, nothing said.
        assert_eq!(delivered_line(&delivery(&[]), &sent).1, None);
    }
}
