//! Send To: an export's delivery to an export destination, driven by the
//! export. It begins when the export is queued, sends each file as the batch
//! puts it in place, and ends once the batch is over and every file sent was
//! received; `export.end`'s answer says how each file went. The requests go
//! through the extension host (`extensions/destinations.rs`), and their
//! outcomes come back here ([`Editor::delivery_answered`]).
//!
//! At most [`WINDOW`] files are in flight at a time. Deliveries to one
//! extension go one after another: its program serves requests in turn, so a
//! second one's `export.begin` would wait, and time out, behind the first's
//! long `export.end`. While a delivery is under way, a later export into a
//! folder it writes to doesn't overwrite anything there
//! ([`Editor::delivery_holds`]).
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
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

/// How many of a delivery's files may be sent and not yet received.
const WINDOW: usize = 8;

/// A delivery from its beginning to its end's answer.
pub(super) struct Delivery {
    to: Begun,
    /// Its `export.begin`, until it is sent: it waits while another delivery
    /// to the same extension is under way.
    begin: Option<ExportBegin>,
    /// The files it sends or will: where its batch planned them, and where
    /// they were written.
    paths: HashSet<PathBuf>,
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
    /// Begins a delivery of `batch`'s files, planned at `targets`, to Send
    /// To's destination, as `settings` choose it, with the options chosen for
    /// it read now. Returns its id, or `None` when there is no Send To or it
    /// couldn't begin, which the top bar says.
    pub(super) fn begin_delivery(
        &mut self,
        batch: &[BatchPhoto],
        targets: &[PathBuf],
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
        let (to, begin) = match self.extensions.prepare_delivery(key, begin, chosen) {
            Ok(prepared) => prepared,
            Err(why) => {
                self.messages.fail(format!("Not sent: {why}"));
                return None;
            }
        };
        let delivery = Delivery {
            to,
            begin: Some(begin),
            paths: targets.iter().map(|t| same_file(t)).collect(),
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
        let extension = delivery.to.extension.clone();
        self.exports.deliveries.insert(id.clone(), delivery);
        self.start_delivery(&extension);
        // Begun, or waiting its turn; gone only if it couldn't begin.
        self.exports.deliveries.contains_key(&id).then_some(id)
    }

    /// Sends the `export.begin` of the next delivery to `extension` waiting
    /// its turn, once none to it is under way.
    fn start_delivery(&mut self, extension: &str) {
        let to_it = |d: &&Delivery| d.to.extension == extension;
        if self
            .exports
            .deliveries
            .values()
            .filter(to_it)
            .any(|d| d.begin.is_none())
        {
            return;
        }
        let Some(id) = self
            .exports
            .deliveries
            .iter()
            .filter(|(_, d)| to_it(d))
            .map(|(id, _)| id.clone())
            .min_by_key(|id| order(id))
        else {
            return;
        };
        let d = self.exports.deliveries.get_mut(&id).expect("found above");
        let begin = d.begin.take().expect("waiting");
        let to = d.to.clone();
        if let Err(why) = self.extensions.begin_delivery(&to, begin) {
            self.delivery_failed(&id, &why);
        }
    }

    /// Whether a delivery under way writes to the folder of `path`, and so
    /// may hold a file there a later export would replace: its own files,
    /// and any name it takes while its export runs (a `-2`). Returns the
    /// destination's name.
    pub(super) fn delivery_holds(&self, path: &Path) -> Option<&str> {
        let folder = same_file(path).parent()?.to_path_buf();
        self.exports
            .deliveries
            .values()
            .find(|d| d.paths.iter().any(|p| p.parent() == Some(folder.as_path())))
            .map(|d| d.to.name.as_str())
    }

    /// Stop on extension `extension`'s page: its deliveries, under way and
    /// waiting their turn, end here too, and none starts in their place.
    pub(in crate::app) fn stop_deliveries_to(&mut self, extension: &str) {
        let ids: Vec<String> = self
            .exports
            .deliveries
            .iter()
            .filter(|(_, d)| d.to.extension == extension)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            if let Some(d) = self.exports.deliveries.remove(&id) {
                self.messages
                    .note(format!("Sending to {} stopped", d.to.name));
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
        d.paths.insert(same_file(path));
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

    /// Delivery `id`'s export was removed from the queue before it ran. One
    /// still waiting its turn is forgotten, since its program never heard
    /// of it; one begun ends with no files, as cancelled.
    pub(super) fn delivery_removed(&mut self, id: &str) {
        match self.exports.deliveries.get(id) {
            Some(d) if d.begin.is_some() => {
                self.exports.deliveries.remove(id);
            }
            Some(_) => self.delivery_exported(id, "Export removed".into(), true),
            None => {}
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
                let ended = ended.unwrap_or_default();
                // A result for a file it wasn't sent is off the contract.
                if let Some(other) = ended.files.iter().find(|r| !d.sent.contains_key(&r.index)) {
                    let why = format!("it answered for file {}, which it wasn't sent", other.index);
                    let to = d.to.clone();
                    let told = self.extensions.refuse_end(&to, id);
                    self.extension_outcomes(told);
                    return self
                        .delivery_failed(id, &RemoteError::new(ErrorCode::InvalidRequest, why));
                }
                let d = self.exports.deliveries.remove(id).expect("looked up above");
                self.delivered(&d, &ended);
                self.start_delivery(&d.to.extension);
            }
        }
    }

    /// Whether Send To has a delivery not over: sending, or waiting for its
    /// export or its turn.
    pub(in crate::app) fn delivering(&self) -> bool {
        !self.exports.deliveries.is_empty()
    }

    /// A delivery to `name`, begun and sending after its export, for tests
    /// that draw what it shows.
    #[cfg(test)]
    pub(in crate::app) fn open_delivery_for_tests(&mut self, name: &str) {
        let id = self.exports.next_delivery();
        self.exports.deliveries.insert(
            id,
            Delivery {
                to: Begun {
                    extension: "dev.jane.gallery".into(),
                    destination: "web".into(),
                    name: name.into(),
                },
                begin: None,
                paths: HashSet::new(),
                facts: Vec::new(),
                begun: true,
                waiting: Vec::new(),
                in_flight: 0,
                sent: BTreeMap::new(),
                exported: Some(("Exported 12 of 12".into(), false)),
                ending: true,
            },
        );
    }

    /// Stops every delivery, as closing anyway does. All are gone first, so
    /// none waiting its turn begins in a stopped one's place; each program
    /// that heard of one is told.
    pub(in crate::app) fn stop_all_deliveries(&mut self) {
        for (id, d) in std::mem::take(&mut self.exports.deliveries) {
            if d.begin.is_none() {
                let outcomes = self.extensions.stop_delivery(&d.to, &id);
                self.extension_outcomes(outcomes);
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
        // Its own requests settle as cancelled and find it gone; another's,
        // if its program had to be stopped, are said.
        let outcomes = self.extensions.stop_delivery(&d.to, id);
        self.extension_outcomes(outcomes);
        self.messages
            .note(format!("Sending to {} stopped", d.to.name));
        self.start_delivery(&d.to.extension);
    }

    /// Sends what delivery `id` can send now: the files written once it has
    /// begun, then its end once the batch is over and every file sent was
    /// received.
    fn advance_delivery(&mut self, id: &str) {
        let Some(mut d) = self.exports.deliveries.get_mut(id) else {
            return;
        };
        if !d.begun || d.ending {
            return;
        }
        let to = d.to.clone();
        while d.in_flight < WINDOW && !d.waiting.is_empty() {
            let file = d.waiting.remove(0);
            let (index, name) = (file.index, file.name.clone());
            if let Err(why) = self.extensions.send_file(&to, file) {
                return self.cut_off(id, &why);
            }
            d = self.exports.deliveries.get_mut(id).expect("still open");
            d.in_flight += 1;
            d.sent.insert(index, name);
        }
        let Some((_, cancelled)) = d.exported else {
            return;
        };
        if d.in_flight > 0 || !d.waiting.is_empty() {
            return;
        }
        d.ending = true;
        let files = u32::try_from(d.sent.len()).unwrap_or(u32::MAX);
        if let Err(why) = self.extensions.end_delivery(&to, id, files, cancelled) {
            self.cut_off(id, &why);
        }
    }

    /// Delivery `id` couldn't send a request because the host no longer has
    /// it open. When the host ended it (a request of it failed, or its
    /// program ended), an outcome that says why follows in the same batch of
    /// outcomes or the next, and ends it with that reason: it waits for that
    /// outcome rather than being called stopped. Any other reason ends it now.
    fn cut_off(&mut self, id: &str, why: &RemoteError) {
        if why.code != ErrorCode::Cancelled {
            self.delivery_failed(id, why);
        }
    }

    /// Delivery `id` can't go on: the top bar says why, and nothing more is
    /// sent.
    fn delivery_failed(&mut self, id: &str, why: &RemoteError) {
        let Some(d) = self.exports.deliveries.remove(id) else {
            return;
        };
        // Its own requests settle as cancelled and find it gone; another's,
        // if its program had to be stopped, are said.
        let outcomes = self.extensions.stop_delivery(&d.to, id);
        self.extension_outcomes(outcomes);
        self.start_delivery(&d.to.extension);
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
        let Some(id) = sending(&self.exports.deliveries).cloned() else {
            return;
        };
        let d = &self.exports.deliveries[&id];
        let progress = self.extensions.delivery_progress(&d.to, &id);
        let palette = theme::palette(ui.ctx());
        let (rect, _) = ui.allocate_exact_size(Vec2::new(210., 28.), Sense::hover());
        let mut label = format!("Sending to {}", d.to.name);
        if let Some(said) = progress.as_ref().and_then(|(_, t)| t.as_deref()) {
            label = format!("{label} · {said}");
        }
        let painter = ui.painter();
        // Cut short before the Stop button: a destination's line can be long.
        let mut job = egui::text::LayoutJob::simple_singleline(
            label,
            egui::FontId::proportional(text::SMALL),
            palette.tone(Tone::SOFT),
        );
        job.wrap = egui::text::TextWrapping::truncate_at_width(rect.width() - 26.);
        let galley = painter.layout_job(job);
        let at = egui::pos2(rect.left(), rect.top() + 7. - galley.size().y / 2.);
        painter.galley(at, galley, palette.tone(Tone::SOFT));
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

/// Where delivery `id` comes in the order deliveries began: ids are
/// numbers, and "10" comes after "9".
fn order(id: &str) -> u64 {
    id.parse().unwrap_or(u64::MAX)
}

/// The delivery the top bar shows: the earliest one still sending after its
/// export is over. One waiting its turn behind another isn't sending yet.
fn sending(deliveries: &BTreeMap<String, Delivery>) -> Option<&String> {
    deliveries
        .iter()
        .filter(|(_, d)| d.exported.is_some() && d.begin.is_none())
        .map(|(id, _)| id)
        .min_by_key(|id| order(id))
}

/// `path` with its folders' links resolved, so one file reached through a
/// linked folder is the same file. A folder the export hasn't made yet is
/// resolved from the deepest one that exists, so the path stays the same
/// once the export makes it (on Windows, a resolved path also gains a
/// `\\?\` prefix the unresolved one lacks). As it is when no folder of it
/// can be read.
fn same_file(path: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut at = path;
    while let (Some(parent), Some(name)) = (at.parent(), at.file_name()) {
        rest.push(name);
        // A relative path's last folder is the working one.
        let parent = if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        };
        if let Ok(resolved) = parent.canonicalize() {
            return rest.iter().rev().fold(resolved, |p, n| p.join(n));
        }
        at = parent;
    }
    path.to_path_buf()
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Level {
    Done,
    Failure,
}

/// The top bar's line for delivery `d`'s end, after what was exported:
/// "Exported 12 of 12 · Sent 12 files to Web Gallery", or "Sent 10 of 12"
/// with why the others weren't, and the destination's message, where to see
/// the result, why a file was skipped and each file's URL or remote id in
/// the detail. A file without a result was sent. None when no file was sent.
fn delivered_line(d: &Delivery, ended: &ExportEnded) -> (Level, Option<String>, Option<String>) {
    let name = &d.to.name;
    let total = d.sent.len();
    if total == 0 {
        return (Level::Done, None, None);
    }
    let mut sent = 0;
    let mut problems = Vec::new();
    let mut said = Vec::new();
    // By index, once: an answer may hold a result for each of thousands of
    // files.
    let results: std::collections::HashMap<u32, _> =
        ended.files.iter().map(|r| (r.index, r)).collect();
    for (index, file) in &d.sent {
        let Some(r) = results.get(index) else {
            sent += 1;
            continue;
        };
        let message = r.message.as_deref();
        match r.result {
            Sent::Failed => problems.push(format!("{file}: {}", message.unwrap_or("not sent"))),
            Sent::Skipped => said.push(format!(
                "{file}: skipped, {}",
                message.unwrap_or("not sent")
            )),
            Sent::Sent => {
                sent += 1;
                if let Some(at) = r
                    .url
                    .as_ref()
                    .or(r.remote_id.as_ref())
                    .map(String::as_str)
                    .or(message)
                {
                    said.push(format!("{file}: {at}"));
                }
            }
        }
    }
    let files = plural(total, "file", "files");
    let what = if sent == total {
        format!("Sent {files} to {name}")
    } else {
        format!("Sent {sent} of {files} to {name}")
    };
    let line = match &d.exported {
        Some((exported, _)) => format!("{exported} · {what}"),
        None => what,
    };
    let detail: Vec<String> = [&ended.message, &ended.location]
        .into_iter()
        .flatten()
        .cloned()
        .chain(problems.iter().cloned())
        .chain(said)
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
            begin: None,
            paths: HashSet::new(),
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

    /// The top bar shows the delivery sending, not one waiting its turn
    /// behind it, though "10" sorts before "9" as text.
    #[test]
    fn the_top_bar_shows_the_delivery_sending_not_one_waiting_its_turn() {
        let mut waiting = delivery(&[]);
        waiting.begin = Some(ExportBegin {
            delivery: "10".into(),
            destination: "web".into(),
            folder: None,
            count: 1,
            options: serde_json::Map::new(),
        });
        waiting.begun = false;
        waiting.ending = false;
        let mut deliveries: BTreeMap<String, Delivery> = [
            ("9".to_string(), delivery(&[(0, "a.jpg")])),
            ("10".to_string(), waiting),
            ("11".to_string(), delivery(&[(0, "b.jpg")])),
        ]
        .into();
        assert_eq!(sending(&deliveries).map(String::as_str), Some("9"));
        // One waiting its turn isn't shown, even when it began first.
        let mut waiting = deliveries.remove("10").expect("put in above");
        waiting.begin.as_mut().expect("waiting").delivery = "8".into();
        let only: BTreeMap<String, Delivery> = [("8".to_string(), waiting)].into();
        assert_eq!(sending(&only), None);
        deliveries.extend(only);
        assert_eq!(sending(&deliveries).map(String::as_str), Some("9"));
    }

    /// A folder the export makes later is the same folder before and after:
    /// on Windows, a resolved path gains a `\\?\` prefix.
    #[test]
    fn a_folder_not_made_yet_is_the_same_folder_once_it_is() {
        let dir = tempfile::tempdir().unwrap();
        let planned = dir.path().join("Web").join("Day 1").join("a.jpg");
        let before = same_file(&planned);
        std::fs::create_dir_all(planned.parent().unwrap()).unwrap();
        assert_eq!(before, same_file(&planned));
    }

    /// A folder the export makes later is the same folder before and after,
    /// reached through a link or not: the hold finds it either way.
    #[cfg(unix)]
    #[test]
    fn a_file_in_a_folder_not_made_yet_is_the_same_file_once_it_is() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let planned = link.join("Web").join("a.jpg");
        let before = same_file(&planned);
        std::fs::create_dir(real.join("Web")).unwrap();
        assert_eq!(before, same_file(&planned));
        assert_eq!(before, same_file(&real.join("Web").join("a.jpg")));
    }

    #[test]
    fn a_deliverys_line_says_how_each_file_went() {
        let d = delivery(&[(0, "a.jpg"), (1, "b.jpg"), (2, "c.jpg")]);
        // A file without a result was sent; a skipped one says why.
        let all = ExportEnded {
            files: vec![
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
                Some(
                    "Published Wedding\nhttps://example.com\na.jpg: https://example.com/a\nb.jpg: skipped, already there"
                        .into()
                )
            )
        );
        // A failure is named with why.
        let some = ExportEnded {
            files: vec![result(1, Sent::Failed, Some("too large"), None)],
            ..Default::default()
        };
        assert_eq!(
            delivered_line(&d, &some),
            (
                Level::Failure,
                Some("Exported 3 of 3 · Sent 2 of 3 files to Web Gallery".into()),
                Some("b.jpg: too large".into())
            )
        );
        // All sent, nothing more to say.
        let one = delivery(&[(0, "a.jpg")]);
        assert_eq!(
            delivered_line(&one, &ExportEnded::default()),
            (
                Level::Done,
                Some("Exported 3 of 3 · Sent 1 file to Web Gallery".into()),
                None
            )
        );
        // Nothing sent, nothing said.
        assert_eq!(
            delivered_line(&delivery(&[]), &ExportEnded::default()).1,
            None
        );
    }
}
