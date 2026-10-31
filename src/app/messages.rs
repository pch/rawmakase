//! What the app says to the user, by how much it matters. [`Messages::note`] stays
//! in the status line. [`Messages::done`], a result, shows in the top bar briefly,
//! where its progress was; [`Messages::warn`], for a refusal or a partial result,
//! for longer; [`Messages::fail`], for what
//! the user asked for and did not happen, stays there until dismissed. Running work
//! ([`Editor::running_work`]) shows beside them, in the same slot as an export's
//! progress.
//!
//! Each thing is said once and nothing is drawn over the photo. A problem tints
//! once as it arrives, then rests as a lamp, one line and a cross; the whole message
//! opens on a click. It takes no keyboard input.
use super::design::{Tone, radius, space, text};
use super::theme;
use eframe::egui::{self, Color32, Rect, Stroke, Vec2};

/// How much a message matters, least first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Level {
    /// Progress and small confirmations: the status line only.
    #[default]
    Note,
    /// A result: the top bar, briefly and without a tint.
    Done,
    /// Refused, or only partly done: the top bar, for a while.
    Warning,
    /// What was asked for did not happen: the top bar, until it is dismissed.
    Failure,
}

/// What a failure is about, when a later event ends it: a save that went
/// through on retry leaves no entry saying it did not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Topic {
    Save,
    Load,
    Render,
}

/// The status line's text.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Line {
    text: String,
    level: Level,
    detail: Option<String>,
}

#[derive(Clone, Debug)]
struct Entry {
    id: u64,
    level: Level,
    text: String,
    detail: Option<String>,
    topic: Option<Topic>,
    /// Seconds still to show; `None` until dismissed.
    left: Option<f64>,
    /// The egui time it first showed, which its tint fades from.
    shown: Option<f64>,
    /// Its time has started: said anew, it starts on the next frame.
    counting: bool,
}

/// Something the user started that is still running. It is asked for afresh
/// each frame from the state that owns the work ([`Editor::running_work`]), so a
/// row can't outlive it: there is nothing to finish, whichever way it ends.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Work {
    /// Which work this is, whatever its text says now.
    pub(super) key: &'static str,
    pub(super) text: String,
}

/// Work done sooner than this never shows: a flicker tells nothing.
const WORK_DELAY: f64 = 0.35;
/// The running line's share of its track, and its seconds per sweep.
const LIGHT_SHARE: f32 = 0.3;
const LIGHT_SWEEP: f64 = 1.1;
/// The running work's size in the bar, as an export's progress.
const WORK_SIZE: Vec2 = Vec2::new(190., 28.);

/// Messages kept at most: the oldest that can go by itself goes first.
const MOST_ENTRIES: usize = 32;
/// The bar's line at most, and what it leaves for the bar's own controls.
const LINE_WIDTH: f32 = 460.;
const BAR_RESERVE: f32 = 230.;
/// The seconds a problem's tint fades over as it arrives.
const TINT: f64 = 1.6;
/// The whole messages' window: its width and its list's height at most.
const OPEN_WIDTH: f32 = 440.;
/// From the bar's line down to the bar's edge.
const BAR_BELOW_LINE: f32 = 9.;
const OPEN_HEIGHT: f32 = 360.;
/// The cross's box.
const CLOSE: f32 = 16.;
/// The seconds a message fades out over, at the end of its time.
const FADE: f64 = 0.3;

/// See the module documentation. Other than the status line and entries, it
/// holds no state of what it reports.
#[derive(Debug, Default)]
pub(super) struct Messages {
    line: Line,
    /// What the status line shows: the last that belongs there. A problem is
    /// said in the bar instead.
    quiet: Line,
    /// `line` is not yet taken by [`Messages::absorb`].
    unread: bool,
    entries: Vec<Entry>,
    next_id: u64,
    /// Failures dismissed while their topic is still open: a retry failing the
    /// same way says so in the line, but does not bring the entry back.
    dismissed: Vec<(Topic, String)>,
    /// The window or pointer paused the timers on the last frame.
    was_paused: bool,
    /// The egui time of the last frame shown, for the entries' timers.
    last_time: Option<f64>,
    /// Running work with the egui time it was first seen.
    working: Vec<(&'static str, f64)>,
    /// The work to show this frame.
    work: Vec<Work>,
    /// The whole messages are open, under the line.
    open: bool,
    /// The bar was drawn this frame.
    bar_drawn: bool,
    /// Where this frame's line is, if the bar drew it, and the open window's.
    line_rect: Option<Rect>,
    open_rect: Option<Rect>,
}

impl Messages {
    /// Messages whose status line says `text`, as the app starts.
    pub(super) fn starting_with(text: &str) -> Self {
        let mut messages = Self::default();
        messages.note(text);
        messages
    }
    /// Says `text` at a level decided elsewhere.
    pub(super) fn say(&mut self, level: Level, text: impl Into<String>) {
        self.post(level, text.into(), None, None);
    }
    /// Progress or a small confirmation, in the status line alone.
    pub(super) fn note(&mut self, text: impl Into<String>) {
        self.post(Level::Note, text.into(), None, None);
    }
    /// A result: the top bar, briefly.
    pub(super) fn done(&mut self, text: impl Into<String>) {
        self.post(Level::Done, text.into(), None, None);
    }
    /// A result with more to say, one item per line, when it is opened.
    pub(super) fn done_with_detail(&mut self, text: impl Into<String>, detail: String) {
        self.post(Level::Done, text.into(), Some(detail), None);
    }
    /// A refusal or a result only partly as asked: the top bar, for a while.
    pub(super) fn warn(&mut self, text: impl Into<String>) {
        self.post(Level::Warning, text.into(), None, None);
    }
    /// A warning with more to say, one item per line, when it is opened.
    pub(super) fn warn_with_detail(&mut self, text: impl Into<String>, detail: String) {
        self.post(Level::Warning, text.into(), Some(detail), None);
    }
    /// What the user asked for did not happen: the top bar, until dismissed.
    pub(super) fn fail(&mut self, text: impl Into<String>) {
        self.post(Level::Failure, text.into(), None, None);
    }
    /// A failure with more to say, one item per line, when it is opened.
    pub(super) fn fail_with_detail(&mut self, text: impl Into<String>, detail: String) {
        self.post(Level::Failure, text.into(), Some(detail), None);
    }
    /// A failure [`Messages::settle`] ends once `topic` has moved on.
    pub(super) fn fail_on(&mut self, topic: Topic, text: impl Into<String>) {
        self.post(Level::Failure, text.into(), None, Some(topic));
    }
    /// `topic` is over: its failures are no longer true.
    pub(super) fn settle(&mut self, topic: Topic) {
        self.entries.retain(|c| c.topic != Some(topic));
        self.dismissed.retain(|(t, _)| *t != topic);
    }

    /// The open photo is gone: nothing that failed for it is still true.
    pub(super) fn document_closed(&mut self) {
        self.settle(Topic::Save);
        self.settle(Topic::Load);
        self.settle(Topic::Render);
    }

    fn post(&mut self, level: Level, text: String, detail: Option<String>, topic: Option<Topic>) {
        let detail = detail.filter(|d| !d.is_empty());
        self.line = Line {
            text: text.clone(),
            level,
            detail: detail.clone(),
        };
        self.unread = true;
        if level == Level::Note {
            self.quiet = self.line.clone();
            return;
        }
        if topic.is_some_and(|t| self.dismissed.iter().any(|d| d.0 == t && d.1 == text)) {
            return;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.add_entry(Entry {
            id,
            level,
            left: lifetime(level, &text),
            text,
            detail,
            topic,
            shown: None,
            counting: false,
        });
    }

    /// Newest first. The same message again, as a retry failing or a key held,
    /// is the one entry, shown afresh, not another.
    fn add_entry(&mut self, mut entry: Entry) {
        let same = self.entries.iter().position(|c| {
            c.text == entry.text && c.level == entry.level && c.detail == entry.detail
        });
        if let Some(same) = same {
            // Its time starts afresh, but it tints only as it first arrives.
            entry.shown = self.entries.remove(same).shown;
        }
        self.entries.insert(0, entry);
        if self.entries.len() > MOST_ENTRIES {
            let least = (0..self.entries.len())
                .rev()
                .min_by_key(|&i| self.entries[i].level)
                .expect("entries were just added");
            self.entries.remove(least);
        }
    }

    /// Takes what is running, for this frame: what has run for a moment shows.
    /// Work that stopped forgets when it started.
    pub(super) fn set_work(&mut self, ctx: &egui::Context, work: Vec<Work>) {
        let now = ctx.input(|i| i.time);
        self.working
            .retain(|(key, _)| work.iter().any(|w| w.key == *key));
        for w in &work {
            if !self.working.iter().any(|(key, _)| *key == w.key) {
                self.working.push((w.key, now));
            }
        }
        // Until each has had its moment, asked again every frame: a repaint can
        // land just short of it.
        let mut settled = work;
        let working = &self.working;
        settled.retain(|w| {
            let since = working.iter().find(|(key, _)| *key == w.key).map(|w| w.1);
            let left = WORK_DELAY - since.map_or(0., |since| now - since);
            if left > 0. {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(left));
            }
            left <= 0.
        });
        self.work = settled;
    }

    /// Takes on what `other`, the Library's own messages, has said since last
    /// asked. Its line stays readable there.
    pub(super) fn absorb(&mut self, other: &mut Messages) {
        if other.unread {
            self.line = other.line.clone();
            if other.line.level == Level::Note {
                self.quiet = other.line.clone();
            }
            self.unread = true;
            other.unread = false;
        }
        // Oldest first, so the newest ends on top.
        for mut entry in other.entries.drain(..).rev() {
            entry.id = self.next_id;
            self.next_id += 1;
            self.add_entry(entry);
        }
    }

    /// The last thing said, whatever its level: for what reads it back, as the
    /// control socket and a failed save's caller.
    pub(super) fn line(&self) -> &str {
        &self.line.text
    }
    #[cfg(test)]
    pub(super) fn level(&self) -> Level {
        self.line.level
    }
    /// What the status line shows.
    pub(super) fn quiet_line(&self) -> &str {
        &self.quiet.text
    }
    /// What the status line sums up, one item per line.
    pub(super) fn quiet_detail(&self) -> Option<&str> {
        self.quiet.detail.as_deref()
    }
    /// Whether the last thing said has the detail, for tests.
    #[cfg(test)]
    pub(super) fn detail(&self) -> Option<&str> {
        self.line.detail.as_deref()
    }
    /// Whether the bar is saying something about `topic`.
    pub(super) fn says(&self, topic: Topic) -> bool {
        self.entries.iter().any(|c| c.topic == Some(topic))
    }
    /// Whether a message says `text` in the bar.
    #[cfg(test)]
    pub(super) fn has_entry(&self, text: &str) -> bool {
        self.entries.iter().any(|c| c.text == text)
    }
    /// What the message saying `text` in the bar has to say in full.
    #[cfg(test)]
    pub(super) fn entry_detail(&self, text: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|c| c.text == text)?
            .detail
            .as_deref()
    }
    /// Whether a message in the bar says something containing `text`.
    #[cfg(test)]
    pub(super) fn has_entry_with(&self, text: &str) -> bool {
        self.entries.iter().any(|c| c.text.contains(text))
    }
    /// Empties the status line. The bar's messages stay.
    pub(super) fn clear_line(&mut self) {
        self.line = Line::default();
        self.quiet = Line::default();
    }
    /// The lines as they are, for [`Messages::restore_line`] after work whose
    /// progress would replace them.
    pub(super) fn saved_line(&self) -> (Line, Line) {
        (self.line.clone(), self.quiet.clone())
    }
    pub(super) fn restore_line(&mut self, (line, quiet): (Line, Line)) {
        self.line = line;
        self.quiet = quiet;
    }

    /// Counts down the messages' time since the last frame, from when each
    /// first showed. Only the one the bar shows: those behind it wait their turn.
    /// Not while the window is in the background or `paused` (the pointer is on
    /// it, or the messages are open), so nothing expires unread.
    fn tick(&mut self, now: f64, paused: bool) {
        let mut elapsed = self.last_time.map_or(0., |last| (now - last).max(0.));
        // A span with the pause at either end counts as paused: it was, for all
        // that can be told, as a window coming back from sleep shows.
        if paused || self.was_paused {
            elapsed = 0.;
        }
        self.last_time = Some(now);
        self.was_paused = paused;
        let shown = self.shown_entry().map(|e| e.id);
        for entry in &mut self.entries {
            // Those behind it wait their turn, but for a result: it is news only
            // now, and would be stale by the time it could show.
            if Some(entry.id) != shown && entry.level != Level::Done {
                continue;
            }
            entry.shown.get_or_insert(now);
            // Its time runs from the frame after it was said.
            if std::mem::replace(&mut entry.counting, true)
                && let Some(left) = &mut entry.left
            {
                *left -= elapsed;
            }
        }
        self.entries.retain(|c| c.left.is_none_or(|left| left > 0.));
    }

    /// The message the bar shows: the worst, the newest of those.
    fn shown_entry(&self) -> Option<&Entry> {
        let worst = self.entries.iter().map(|c| c.level).max()?;
        self.entries.iter().find(|c| c.level == worst)
    }

    /// The top bar's left: running work, beside the other progress.
    pub(super) fn work_ui(&mut self, ui: &mut egui::Ui) {
        let palette = theme::palette(ui.ctx());
        let now = ui.input(|i| i.time);
        for work in &self.work {
            work_ui(ui, work, now, &palette);
            ui.add_space(space::XL);
        }
    }

    /// The message that matters most, after the progress in the top bar: past a
    /// hairline when `after_progress`, so the two aren't taken for one.
    pub(super) fn line_ui(&mut self, ui: &mut egui::Ui, after_progress: bool) {
        self.bar_drawn = true;
        let palette = theme::palette(ui.ctx());
        let now = ui.input(|i| i.time);
        let others = self.entries.len().saturating_sub(1);
        let Some(entry) = self.shown_entry() else {
            self.line_rect = None;
            return;
        };
        if after_progress {
            ui.add_space(space::L);
            let (rule, _) = ui.allocate_exact_size(Vec2::new(1., 20.), egui::Sense::hover());
            ui.painter()
                .rect_filled(rule, 0., palette.tone(Tone::OUTLINE));
            ui.add_space(space::L);
        }
        let room = (ui.available_width() - BAR_RESERVE).clamp(120., LINE_WIDTH);
        let (rect, clicked, closed) = line_ui(ui, entry, others, room, now, &palette);
        self.line_rect = Some(rect);
        if clicked {
            self.open = !self.open;
        }
        if closed {
            self.dismiss(|_| true);
        }
    }

    /// Shift+Esc dismisses every message, as the bar's cross does. Only taken when
    /// there is one, and before anything else reads Esc: some of it ignores modifiers, and
    /// a tool should not close with it. Free in Lightroom, unlike Cmd+.
    pub(super) fn dismiss_key(&mut self, ctx: &egui::Context) {
        if !self.entries.is_empty()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::Escape))
        {
            self.dismiss(|_| true);
        }
    }

    fn dismiss(&mut self, which: impl Fn(&Entry) -> bool) {
        for entry in self.entries.iter().filter(|c| which(c)) {
            if let Some(topic) = entry.topic {
                self.dismissed.push((topic, entry.text.clone()));
            }
        }
        self.entries.retain(|c| !which(c));
    }

    /// Ends the frame's messages: their time, the whole messages when open, and
    /// the bar's part in a corner of its own when no bar was drawn (the chrome
    /// hidden, so a problem still shows). Call once a frame, after the bar.
    pub(super) fn show(&mut self, ctx: &egui::Context) {
        let (now, focused, pointer) = ctx.input(|i| (i.time, i.focused, i.pointer.hover_pos()));
        let rects = [self.line_rect, self.open_rect];
        let over = pointer.is_some_and(|at| rects.iter().flatten().any(|r| r.contains(at)));
        self.tick(now, over || self.open || !focused);
        if !self.bar_drawn {
            self.floating_ui(ctx);
        }
        if self.entries.is_empty() {
            self.open = false;
        }
        if self.open {
            self.open_ui(ctx);
        } else {
            self.open_rect = None;
        }
        // The soonest something changes without any input.
        let soonest = self.shown_entry().and_then(|entry| {
            let tint = entry
                .shown
                .filter(|shown| now - shown < TINT)
                .map(|_| 1. / 30.);
            let left = entry
                .left
                .map(|left| if left <= FADE { 1. / 30. } else { left - FADE });
            tint.or(left)
        });
        if let Some(wait) = soonest {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait));
        }
        self.line_rect = None;
        self.bar_drawn = false;
    }

    /// The bar's part where the bar is not: small, at the window's top.
    fn floating_ui(&mut self, ctx: &egui::Context) {
        if self.work.is_empty() && self.entries.is_empty() {
            return;
        }
        let palette = theme::palette(ctx);
        egui::Area::new(egui::Id::new("messages-no-bar"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_TOP, Vec2::new(0., space::M))
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(palette.tone(Tone::BAR))
                    .corner_radius(radius::CARD)
                    .inner_margin(egui::Margin::symmetric(space::L as i8, space::XS as i8))
                    .show(ui, |ui| {
                        ui.set_max_width(LINE_WIDTH + BAR_RESERVE);
                        ui.horizontal(|ui| {
                            self.work_ui(ui);
                            self.line_ui(ui, !self.work.is_empty());
                        });
                    });
            });
    }

    /// Every message in full, under the line, until clicked away.
    fn open_ui(&mut self, ctx: &egui::Context) {
        let palette = theme::palette(ctx);
        // Its lamp and words under the line's, its top clear of the bar.
        let at = self.line_rect.map_or(egui::pos2(space::XL, 48.), |r| {
            r.left_bottom() + Vec2::new(-(space::L + 1.), BAR_BELOW_LINE + space::XS)
        });
        let mut dismissed = None;
        let mut all = false;
        let area = egui::Area::new(egui::Id::new("messages-open"))
            .order(egui::Order::Foreground)
            .fixed_pos(at)
            .constrain(true)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(palette.tone(Tone::CARD))
                    .stroke(Stroke::new(1., palette.tone(Tone::OUTLINE)))
                    .corner_radius(radius::CARD)
                    .inner_margin(space::L as i8)
                    .show(ui, |ui| {
                        ui.set_width(OPEN_WIDTH);
                        // Its own state per list, none from a longer or shorter one.
                        let listed: Vec<u64> = self.entries.iter().map(|c| c.id).collect();
                        egui::ScrollArea::vertical()
                            .id_salt(("messages-open", listed))
                            .max_height(OPEN_HEIGHT)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                for (i, entry) in self.entries.iter().enumerate() {
                                    if i > 0 {
                                        ui.add_space(space::L);
                                    }
                                    if message_ui(ui, entry, &palette) {
                                        dismissed = Some(entry.id);
                                    }
                                }
                            });
                        if self.entries.len() > 1 {
                            ui.add_space(space::M);
                            // At the far end, under the crosses.
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                                all = link_ui(
                                    ui,
                                    "Dismiss all",
                                    egui::Id::new("messages-all"),
                                    &palette,
                                );
                            });
                        }
                    });
            });
        let rect = area.response.rect;
        // An idle window draws only what is asked for: until its size settles.
        if self.open_rect != Some(rect) {
            ctx.request_repaint();
        }
        self.open_rect = Some(rect);
        let away = ctx.input(|i| {
            i.pointer.any_pressed()
                && i.pointer.interact_pos().is_some_and(|at| {
                    !rect.contains(at) && !self.line_rect.is_some_and(|r| r.contains(at))
                })
        });
        if away {
            self.open = false;
        }
        if all {
            self.dismiss(|_| true);
        } else if let Some(id) = dismissed {
            self.dismiss(|c| c.id == id);
        }
    }
}

/// The id of a message's dismiss cross in the bar.
fn close_id(id: u64) -> egui::Id {
    egui::Id::new("message-close").with(id)
}
/// The id of the bar's line, which opens the messages.
fn line_id(id: u64) -> egui::Id {
    egui::Id::new("message-line").with(id)
}

/// The seconds a message stays, `None` until dismissed. Long enough to read: a
/// long warning gets longer.
fn lifetime(level: Level, text: &str) -> Option<f64> {
    match level {
        Level::Failure => None,
        Level::Note | Level::Done => Some(6.),
        Level::Warning => Some((text.chars().count() as f64 / 12.).max(10.)),
    }
}

/// A message's headline and the rest, split at its first ": " when what comes
/// before it reads as a headline ("No camera look fitted: the preview does
/// not line up").
fn headline(text: &str) -> (&str, Option<&str>) {
    match text.split_once(": ") {
        Some((head, rest)) if head.chars().count() <= 48 && !rest.trim().is_empty() => {
            (head, Some(rest))
        }
        _ => (text, None),
    }
}

fn lamp_colour(level: Level, palette: &theme::Palette) -> Color32 {
    match level {
        Level::Failure => palette.danger(),
        Level::Warning => palette.warning(),
        Level::Note | Level::Done => palette.tone(Tone::SECONDARY),
    }
}

/// The running line's stretch of `track` at time `now`, swept back and forth.
/// Goes by the clock, not by frames.
fn running_light(track: Rect, now: f64) -> Rect {
    let phase = (now / LIGHT_SWEEP * std::f64::consts::PI).sin() as f32;
    let width = track.width() * LIGHT_SHARE;
    let left = track.left() + (phase * 0.5 + 0.5) * (track.width() - width);
    Rect::from_min_size(
        egui::pos2(left, track.top()),
        Vec2::new(width, track.height()),
    )
}

/// Running work as an export's progress is drawn: its label over a line, here
/// one that sweeps, as how far it has come isn't known.
fn work_ui(ui: &mut egui::Ui, work: &Work, now: f64, palette: &theme::Palette) {
    let (rect, _) = ui.allocate_exact_size(WORK_SIZE, egui::Sense::hover());
    let label = work.text.split_whitespace().collect::<Vec<_>>().join(" ");
    let galley = ui.painter().layout(
        label,
        egui::FontId::proportional(text::SMALL),
        palette.tone(Tone::SOFT),
        rect.width(),
    );
    // One line: the top row, whatever the text holds.
    let clip = Rect::from_min_size(rect.min, Vec2::new(rect.width(), 14.));
    ui.painter()
        .with_clip_rect(clip)
        .galley(rect.min, galley, palette.tone(Tone::SOFT));
    let track = Rect::from_min_size(
        egui::pos2(rect.left(), rect.top() + 17.),
        Vec2::new(rect.width(), 4.),
    );
    ui.painter()
        .rect_filled(track, radius::SMALL, palette.tone(Tone::OUTLINE));
    ui.painter().rect_filled(
        running_light(track, now),
        radius::SMALL,
        Color32::from_rgb(110, 150, 190),
    );
    ui.ctx().request_repaint();
}

/// The bar's line: its lamp and words, how many more there are, and the cross.
/// Returns its rectangle, and whether it was clicked and whether it was closed.
fn line_ui(
    ui: &mut egui::Ui,
    entry: &Entry,
    others: usize,
    width: f32,
    now: f64,
    palette: &theme::Palette,
) -> (Rect, bool, bool) {
    let flat = entry.text.split_whitespace().collect::<Vec<_>>().join(" ");
    let font = egui::FontId::proportional(text::SMALL);
    let more = (others > 0).then(|| format!("+{others}"));
    let extra = more.as_ref().map_or(0., |m| {
        ui.painter()
            .layout_no_wrap(m.clone(), font.clone(), Color32::WHITE)
            .size()
            .x
            + space::S
    });
    let lamp = 12.;
    let words_room = width - lamp - extra - CLOSE - space::S;
    let mut job =
        egui::text::LayoutJob::simple_singleline(flat, font.clone(), palette.tone(Tone::TEXT));
    job.wrap = egui::text::TextWrapping::truncate_at_width(words_room);
    let galley = ui.painter().layout_job(job);
    let used = lamp + galley.size().x + extra + space::S + CLOSE;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(used, 26.), egui::Sense::hover());
    let colour = lamp_colour(entry.level, palette);
    // Tints once as it arrives, then rests.
    if let Some(shown) = entry.shown.filter(|_| entry.level >= Level::Warning) {
        let fade = 1. - (now - shown) / TINT;
        if fade > 0. {
            ui.painter().rect_filled(
                rect.expand2(Vec2::new(space::S, 0.)),
                radius::CONTROL,
                colour.gamma_multiply(0.2 * fade as f32),
            );
        }
    }
    let opacity = entry
        .left
        .map_or(1., |left| ((left / FADE) as f32).clamp(0., 1.));
    let painter = ui.painter().clone();
    let painter = {
        let mut p = painter;
        p.set_opacity(opacity);
        p
    };
    painter.circle_filled(egui::pos2(rect.left() + 3., rect.center().y), 3., colour);
    let y = rect.center().y - galley.size().y / 2.;
    let words_at = egui::pos2(rect.left() + lamp, y);
    let words_width = galley.size().x;
    painter.galley(words_at, galley, palette.tone(Tone::TEXT));
    let mut x = rect.left() + lamp + words_width + space::S;
    if let Some(more) = more {
        let m = painter.layout_no_wrap(more, font, palette.tone(Tone::MUTED));
        let w = m.size().x;
        painter.galley(
            egui::pos2(x, rect.center().y - m.size().y / 2.),
            m,
            palette.tone(Tone::MUTED),
        );
        x += w + space::S;
    }
    let words = Rect::from_min_max(rect.min, egui::pos2(x, rect.max.y));
    let opens = ui
        .interact(words, line_id(entry.id), egui::Sense::CLICK)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Show the whole message · Shift+Esc dismisses");
    let cross = Rect::from_center_size(
        egui::pos2(x + CLOSE / 2., rect.center().y),
        Vec2::splat(CLOSE),
    );
    // The cross only with the pointer on the line: at rest, no second cross
    // beside the progress's Stop.
    let on_line = ui.rect_contains_pointer(rect);
    let closes = cross_ui(
        ui,
        cross,
        close_id(entry.id),
        on_line,
        if others > 0 {
            "Dismiss all · Shift+Esc"
        } else {
            "Dismiss · Shift+Esc"
        },
        palette,
    );
    (rect, opens.clicked(), closes)
}

/// One message in full; whether its cross was clicked.
fn message_ui(ui: &mut egui::Ui, entry: &Entry, palette: &theme::Palette) -> bool {
    let (head, rest) = headline(&entry.text);
    let words = |text: &str, tone| {
        egui::Label::new(
            egui::RichText::new(text)
                .size(text::LABEL)
                .color(palette.tone(tone)),
        )
        .wrap()
        .selectable(false)
    };
    let mut closed = false;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.;
        let (lamp, _) = ui.allocate_exact_size(Vec2::new(12., 16.), egui::Sense::hover());
        ui.painter().circle_filled(
            egui::pos2(lamp.left() + 3., lamp.center().y),
            3.,
            lamp_colour(entry.level, palette),
        );
        ui.allocate_ui_with_layout(
            Vec2::new((ui.available_width() - CLOSE - space::S).max(40.), 0.),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                // All its width, short words or not: the cross stays at the far end.
                ui.set_min_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = space::XXS;
                ui.add(words(head, Tone::TEXT));
                if let Some(rest) = rest {
                    ui.add(words(rest, Tone::SECONDARY));
                }
                if let Some(detail) = &entry.detail {
                    ui.add(words(detail, Tone::SECONDARY));
                }
                if entry.level == Level::Failure
                    && link_ui(ui, "Copy", egui::Id::new((entry.id, "copy")), palette)
                {
                    ui.ctx().copy_text(entry.text.clone());
                }
            },
        );
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(CLOSE), egui::Sense::hover());
        closed = cross_ui(
            ui,
            rect,
            close_id(entry.id).with("open"),
            true,
            "Dismiss",
            palette,
        );
    });
    closed
}

/// A quiet word that acts when clicked. Pointer only.
fn link_ui(ui: &mut egui::Ui, label: &str, id: egui::Id, palette: &theme::Palette) -> bool {
    let galley = ui.painter().layout_no_wrap(
        label.into(),
        egui::FontId::proportional(text::SMALL),
        Color32::PLACEHOLDER,
    );
    let (rect, _) = ui.allocate_exact_size(galley.size(), egui::Sense::hover());
    let response = ui.interact(rect, id, egui::Sense::CLICK);
    let tone = if response.hovered() {
        Tone::STRONG
    } else {
        Tone::MUTED
    };
    ui.painter().galley(rect.min, galley, palette.tone(tone));
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
}

/// The dismiss cross, as the bar's others are drawn. Pointer only.
fn cross_ui(
    ui: &mut egui::Ui,
    rect: Rect,
    id: egui::Id,
    visible: bool,
    hint: &str,
    palette: &theme::Palette,
) -> bool {
    let response = ui
        .interact(rect, id, egui::Sense::CLICK)
        .on_hover_text(hint);
    let tone = if response.hovered() {
        Tone::STRONG
    } else {
        Tone::SECONDARY
    };
    if !visible {
        return response.clicked();
    }
    super::icons::paint_at(
        ui.painter(),
        super::icons::Icon::Close,
        rect.center(),
        13.,
        palette.tone(tone),
    );
    response.clicked()
}

impl super::Editor {
    /// What the user started that is still running and has no place of its own
    /// that says so, read from the state that owns each: a `Task`, cleared by its
    /// result (a panic included, see `task::spawn`) or by the document it belongs
    /// to going. Add long work here, in a line. An export, a file operation and a
    /// scan have their own progress in the bar, a batch its window, a selection
    /// its tile and opening a photo its badge: they are not said again.
    pub(super) fn running_work(&self) -> Vec<Work> {
        let mut work = Vec::new();
        let mut add = |running: bool, key, text: &str| {
            if running {
                work.push(Work {
                    key,
                    text: text.to_string(),
                });
            }
        };
        let document = &self.document;
        add(
            document.auto.is_running(),
            "auto",
            "Working out Auto settings…",
        );
        add(
            document.camera_look.is_running(),
            "camera look",
            "Fitting the camera's look to its JPEG preview…",
        );
        add(
            document.upright.is_running(),
            "upright",
            "Analysing for Upright…",
        );
        add(
            document.straighten.is_running(),
            "straighten",
            "Looking for a horizon…",
        );
        if let Some(job) = self.lensfun.running() {
            add(true, "lensfun", job.running_text());
        }
        add(
            self.library.as_ref().is_some_and(|l| l.rereading()),
            "metadata",
            "Reading metadata from files…",
        );
        if let Some(text) = &self.catalog_work {
            add(true, "catalog", text);
        }
        if let Some(progress) = &self.importing {
            add(true, "import", &progress.text());
        }
        // A library operation: shown wherever the user is, not only in its offer.
        if let Some(name) = self.profile_download.downloading() {
            add(true, "profile download", &format!("Downloading {name}…"));
        }
        work
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHY: &str = "No camera look fitted: The preview does not line up with the photo (detail correlation 0.40)";

    fn frame(ctx: &egui::Context, m: &mut Messages, time: f64, events: Vec<egui::Event>) {
        let input = egui::RawInput {
            time: Some(time),
            screen_rect: Some(Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(1200., 800.),
            )),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            ui.horizontal(|ui| {
                m.dismiss_key(ui.ctx());
                m.work_ui(ui);
                m.line_ui(ui, !m.work.is_empty());
            });
            m.show(ui.ctx());
        });
        output.textures_delta.clear();
    }
    fn click(at: egui::Pos2) -> Vec<egui::Event> {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        vec![egui::Event::PointerMoved(at), button(true), button(false)]
    }
    fn click_id(ctx: &egui::Context, m: &mut Messages, id: egui::Id, time: f64) {
        let at = ctx.read_response(id).expect("drawn").rect.center();
        frame(ctx, m, time, vec![egui::Event::PointerMoved(at)]);
        frame(ctx, m, time + 0.05, click(at));
        frame(ctx, m, time + 0.1, vec![]);
    }

    #[test]
    fn a_failure_stays_until_dismissed_and_later_progress_does_not_hide_it() {
        let ctx = egui::Context::default();
        let mut m = Messages::default();
        m.fail(WHY);
        frame(&ctx, &mut m, 0., vec![]);
        m.note("Reading RAW…");
        assert_eq!(m.quiet_line(), "Reading RAW…");
        frame(&ctx, &mut m, 3600., vec![]);
        frame(&ctx, &mut m, 7200., vec![]);
        assert!(m.has_entry(WHY), "a failure does not expire");
        let id = m.entries[0].id;
        click_id(&ctx, &mut m, close_id(id), 7201.);
        assert!(m.entries.is_empty(), "the cross takes it away");
    }

    #[test]
    fn each_thing_is_said_in_one_place() {
        let mut m = Messages::default();
        m.note("Developed in 0.20s");
        m.fail(WHY);
        assert_eq!(
            m.quiet_line(),
            "Developed in 0.20s",
            "not in the status line too"
        );
        assert!(m.has_entry(WHY));
        m.done("Preset created");
        assert_eq!(
            m.quiet_line(),
            "Developed in 0.20s",
            "a result is in the bar, not here"
        );
        assert!(m.has_entry("Preset created"));
        assert_eq!(
            m.line(),
            "Preset created",
            "reading back gets the last said"
        );
    }

    #[test]
    fn a_warning_goes_by_itself_and_not_while_the_pointer_or_window_is_away() {
        let mut m = Messages::default();
        m.warn("Finish what is running first");
        m.tick(0., false);
        m.tick(60., true);
        assert_eq!(m.entries.len(), 1, "paused");
        m.tick(64., false);
        assert_eq!(m.entries.len(), 1, "only the time unpaused counts");
        m.tick(75., false);
        assert!(m.entries.is_empty());
    }

    #[test]
    fn the_bar_shows_the_worst_and_a_warning_behind_it_waits_its_turn() {
        let mut m = Messages::default();
        m.warn("A warning");
        m.fail("A failure");
        assert_eq!(m.shown_entry().unwrap().text, "A failure");
        m.tick(0., false);
        m.tick(3600., false);
        assert!(m.has_entry("A warning"), "not seen yet");
    }

    #[test]
    fn a_result_goes_soon_even_behind_a_failure() {
        let mut m = Messages::default();
        m.done("Exported a.jpg");
        m.tick(0., false);
        m.tick(5., false);
        assert!(m.has_entry("Exported a.jpg"));
        m.tick(7., false);
        assert!(!m.has_entry("Exported a.jpg"), "gone by itself");
        m.done("Exported b.jpg");
        m.fail("A failure");
        m.tick(8., false);
        m.tick(9., false);
        m.tick(16., false);
        assert!(
            !m.has_entry("Exported b.jpg"),
            "stale by the time it could show"
        );
        assert!(m.has_entry("A failure"));
    }

    #[test]
    fn a_new_message_gets_its_whole_time_after_an_idle_window() {
        let mut m = Messages::default();
        m.tick(0., false);
        m.warn("Refused");
        m.tick(60., false);
        assert!(m.has_entry("Refused"));
        m.tick(69., false);
        assert!(m.has_entry("Refused"));
        m.tick(71., false);
        assert!(!m.has_entry("Refused"));
    }

    #[test]
    fn the_same_failure_again_is_one_message() {
        let mut m = Messages::default();
        for _ in 0..50 {
            m.fail("Edits not saved: disk full");
        }
        m.fail("Another");
        assert_eq!(m.entries.len(), 2);
    }

    #[test]
    fn the_same_words_about_different_photos_are_two_messages() {
        let mut m = Messages::default();
        m.fail_with_detail("Exported 0 of 1 · 1 not exported", "a.dng: offline".into());
        m.fail_with_detail("Exported 0 of 1 · 1 not exported", "b.dng: offline".into());
        assert_eq!(m.entries.len(), 2);
    }

    #[test]
    fn a_dismissed_save_failure_stays_away_through_retries_until_settled() {
        let ctx = egui::Context::default();
        let mut m = Messages::default();
        let failed = "Edits not saved: disk full";
        m.fail_on(Topic::Save, failed);
        frame(&ctx, &mut m, 0., vec![]);
        frame(&ctx, &mut m, 0.1, vec![]);
        let id = m.entries[0].id;
        click_id(&ctx, &mut m, close_id(id), 0.2);
        assert!(m.entries.is_empty());
        m.fail_on(Topic::Save, failed);
        assert!(m.entries.is_empty(), "the retry failed the same way");
        m.fail_on(Topic::Save, "Edits not saved: catalog locked");
        assert_eq!(m.entries.len(), 1, "a different failure is news");
        m.settle(Topic::Save);
        m.fail_on(Topic::Save, failed);
        assert_eq!(m.entries.len(), 1, "failing again after a save is news too");
    }

    #[test]
    fn closing_the_document_ends_its_failures_only() {
        let mut m = Messages::default();
        m.fail_on(Topic::Save, "Edits not saved: disk full");
        m.fail_on(Topic::Load, "Could not open");
        m.fail_on(Topic::Render, "Render failed");
        m.fail("Preset not created: no room");
        m.document_closed();
        assert_eq!(m.entries.len(), 1);
        assert!(m.has_entry("Preset not created: no room"));
    }

    #[test]
    fn many_messages_keep_failures_over_warnings() {
        let mut m = Messages::default();
        m.fail("The failure");
        for i in 0..100 {
            m.warn(format!("Warning {i}"));
        }
        assert_eq!(m.entries.len(), MOST_ENTRIES);
        assert!(m.has_entry("The failure"));
    }

    #[test]
    fn the_librarys_messages_are_taken_on_once() {
        let mut library = Messages::default();
        let mut m = Messages::default();
        library.fail("Metadata could not be saved: locked");
        m.note("Before");
        m.absorb(&mut library);
        assert_eq!(m.line(), "Metadata could not be saved: locked");
        assert_eq!(m.quiet_line(), "Before");
        assert_eq!(m.entries.len(), 1);
        m.note("After");
        m.absorb(&mut library);
        assert_eq!(m.line(), "After", "already taken");
        assert_eq!(library.line(), "Metadata could not be saved: locked");
    }

    #[test]
    fn a_long_message_splits_into_headline_and_body() {
        assert_eq!(
            headline(WHY),
            (
                "No camera look fitted",
                Some("The preview does not line up with the photo (detail correlation 0.40)")
            )
        );
        assert_eq!(headline("Preset saved"), ("Preset saved", None));
    }

    #[test]
    fn a_long_message_stays_one_line_until_opened_and_then_is_bounded() {
        let ctx = egui::Context::default();
        let mut m = Messages::default();
        m.fail(format!(
            "Export stopped: {}",
            "very\nlong reason ".repeat(400)
        ));
        frame(&ctx, &mut m, 0., vec![]);
        frame(&ctx, &mut m, 0.1, vec![]);
        let id = m.entries[0].id;
        let line = ctx.read_response(line_id(id)).expect("drawn").rect;
        assert!(
            line.height() <= 26. && line.width() <= LINE_WIDTH,
            "{line:?}"
        );
        click_id(&ctx, &mut m, line_id(id), 0.2);
        assert!(m.open);
        let open = ctx
            .memory(|mem| mem.area_rect(egui::Id::new("messages-open")))
            .unwrap();
        assert!(open.height() <= OPEN_HEIGHT + 60., "{open:?}");
        assert!(open.width() <= OPEN_WIDTH + 40., "{open:?}");
        // A click elsewhere puts it away; the message stays.
        frame(&ctx, &mut m, 0.5, click(egui::pos2(1100., 700.)));
        assert!(!m.open);
        assert_eq!(m.entries.len(), 1);
    }

    fn shift_esc() -> Vec<egui::Event> {
        vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::SHIFT,
        }]
    }

    #[test]
    fn shift_esc_dismisses_every_message() {
        let ctx = egui::Context::default();
        let mut m = Messages::default();
        m.warn("A warning");
        m.fail("A failure");
        frame(&ctx, &mut m, 0., vec![]);
        frame(&ctx, &mut m, 0.1, shift_esc());
        assert!(m.entries.is_empty());
    }

    #[test]
    fn the_bars_cross_dismisses_every_message() {
        let ctx = egui::Context::default();
        let mut m = Messages::default();
        m.warn("A warning");
        m.fail("A failure");
        frame(&ctx, &mut m, 0., vec![]);
        frame(&ctx, &mut m, 0.1, vec![]);
        let id = m.shown_entry().unwrap().id;
        click_id(&ctx, &mut m, close_id(id), 0.2);
        assert!(m.entries.is_empty());
    }

    #[test]
    fn shift_esc_is_left_alone_with_nothing_to_dismiss() {
        let ctx = egui::Context::default();
        let mut m = Messages::default();
        let input = egui::RawInput {
            events: shift_esc(),
            ..Default::default()
        };
        let mut left = false;
        let mut output = ctx.run_ui(input, |ui| {
            m.dismiss_key(ui.ctx());
            left = ui.input(|i| i.key_pressed(egui::Key::Escape));
        });
        output.textures_delta.clear();
        assert!(left);
    }

    /// Other code reads Esc whatever the modifiers, as a tool closing does.
    #[test]
    fn shift_esc_does_not_also_reach_what_reads_esc() {
        let ctx = egui::Context::default();
        let mut m = Messages::default();
        m.fail(WHY);
        let input = egui::RawInput {
            events: shift_esc(),
            ..Default::default()
        };
        let mut reached = false;
        let mut output = ctx.run_ui(input, |ui| {
            m.dismiss_key(ui.ctx());
            reached = ui.input(|i| i.key_pressed(egui::Key::Escape));
        });
        output.textures_delta.clear();
        assert!(!reached);
        assert!(m.entries.is_empty());
    }

    #[test]
    fn without_a_bar_a_failure_still_shows() {
        let ctx = egui::Context::default();
        let mut m = Messages::default();
        m.fail(WHY);
        for time in [0., 0.1] {
            let input = egui::RawInput {
                time: Some(time),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| m.show(ui.ctx()));
            output.textures_delta.clear();
        }
        assert!(ctx.read_response(close_id(m.entries[0].id)).is_some());
    }

    #[test]
    fn an_open_messages_cross_is_at_the_far_end_however_short_its_words() {
        let ctx = egui::Context::default();
        let mut m = Messages::default();
        m.warn("Short");
        frame(&ctx, &mut m, 0., vec![]);
        frame(&ctx, &mut m, 0.1, vec![]);
        let id = m.entries[0].id;
        click_id(&ctx, &mut m, line_id(id), 0.2);
        frame(&ctx, &mut m, 0.5, vec![]);
        let open = ctx
            .memory(|mem| mem.area_rect(egui::Id::new("messages-open")))
            .unwrap();
        let cross = ctx
            .read_response(close_id(id).with("open"))
            .expect("drawn")
            .rect;
        assert!(
            open.right() - cross.right() <= 2. * space::L,
            "{cross:?} in {open:?}"
        );
    }

    /// Reopened for a longer message, it shows it all from the first frame.
    #[test]
    fn a_message_opened_after_a_shorter_one_shows_whole_at_once() {
        let ctx = egui::Context::default();
        let mut m = Messages::default();
        m.warn("Short");
        frame(&ctx, &mut m, 0., vec![]);
        m.open = true;
        frame(&ctx, &mut m, 0.1, vec![]);
        frame(&ctx, &mut m, 0.2, vec![]);
        m.open = false;
        m.dismiss(|_| true);
        frame(&ctx, &mut m, 0.3, vec![]);
        m.warn_with_detail("Exported", "one\ntwo\nthree\nfour\nfive\nsix".to_string());
        m.open = true;
        frame(&ctx, &mut m, 0.4, vec![]);
        let height = || {
            ctx.memory(|mem| mem.area_rect(egui::Id::new("messages-open")))
                .unwrap()
                .height()
        };
        let first = height();
        frame(&ctx, &mut m, 0.5, vec![]);
        frame(&ctx, &mut m, 0.6, vec![]);
        assert_eq!(first, height(), "the first frame already shows it all");
    }

    #[test]
    fn dismiss_all_ends_at_the_panels_far_end() {
        let ctx = egui::Context::default();
        let mut m = Messages::default();
        m.warn("One");
        m.fail("Two");
        frame(&ctx, &mut m, 0., vec![]);
        m.open = true;
        frame(&ctx, &mut m, 0.1, vec![]);
        frame(&ctx, &mut m, 0.2, vec![]);
        let open = m.open_rect.unwrap();
        let all = ctx
            .read_response(egui::Id::new("messages-all"))
            .expect("drawn")
            .rect;
        assert!(
            open.right() - all.right() <= 2. * space::L,
            "{all:?} in {open:?}"
        );
        // And it does what it says.
        click_id(&ctx, &mut m, egui::Id::new("messages-all"), 0.3);
        assert!(m.entries.is_empty());
    }

    #[test]
    fn time_paused_is_not_charged_on_resuming() {
        let mut m = Messages::default();
        m.warn("Refused");
        m.tick(0., false);
        m.tick(1., true);
        m.tick(20., false);
        assert!(m.has_entry("Refused"), "almost all of it was paused");
    }

    #[test]
    fn a_new_result_behind_a_failure_gets_its_whole_time() {
        let mut m = Messages::default();
        m.fail("A failure");
        m.tick(2., false);
        m.done("Exported a.jpg");
        m.tick(20., false);
        assert!(m.has_entry("Exported a.jpg"), "posted just now");
        m.tick(25., false);
        assert!(m.has_entry("Exported a.jpg"));
        m.tick(27., false);
        assert!(!m.has_entry("Exported a.jpg"));
    }

    #[test]
    fn the_same_failure_again_keeps_its_place_in_time() {
        let mut m = Messages::default();
        m.fail_on(Topic::Save, "Edits not saved: disk full");
        m.tick(0., false);
        let shown = m.entries[0].shown;
        assert!(shown.is_some());
        m.tick(0.6, false);
        m.fail_on(Topic::Save, "Edits not saved: disk full");
        assert_eq!(m.entries[0].shown, shown, "no new tint on every retry");
    }

    #[test]
    fn a_message_said_again_after_waiting_gets_its_whole_time() {
        let mut m = Messages::default();
        m.warn("A");
        m.tick(0., false);
        m.fail_on(Topic::Save, "A failure");
        m.tick(1., false);
        m.tick(3., false);
        m.settle(Topic::Save);
        m.warn("A");
        m.tick(30., false);
        assert!(m.has_entry("A"), "said just now");
    }

    #[test]
    fn the_bar_part_never_takes_the_keyboard() {
        let ctx = egui::Context::default();
        let mut m = Messages::default();
        m.fail(WHY);
        frame(&ctx, &mut m, 0., vec![]);
        let key = |key, pressed| egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        for time in [0.1, 0.2, 0.3] {
            frame(
                &ctx,
                &mut m,
                time,
                vec![key(egui::Key::Tab, true), key(egui::Key::Tab, false)],
            );
            assert_eq!(ctx.memory(|mem| mem.focused()), None);
        }
        frame(
            &ctx,
            &mut m,
            0.4,
            vec![key(egui::Key::Enter, true), key(egui::Key::Enter, false)],
        );
        assert_eq!(m.entries.len(), 1);
    }

    fn working(text: &str) -> Vec<Work> {
        vec![Work {
            key: "fit",
            text: text.into(),
        }]
    }

    #[test]
    fn work_shows_after_a_moment_and_goes_with_the_work() {
        let ctx = egui::Context::default();
        let mut m = Messages::default();
        m.set_work(&ctx, working("Long"));
        assert!(m.work.is_empty(), "a moment first");
        let later = egui::RawInput {
            time: Some(1.),
            ..Default::default()
        };
        let mut output = ctx.run_ui(later, |ui| m.set_work(ui.ctx(), working("Long")));
        output.textures_delta.clear();
        assert_eq!(m.work.len(), 1);
        let gone = egui::RawInput {
            time: Some(2.),
            ..Default::default()
        };
        let mut output = ctx.run_ui(gone, |ui| m.set_work(ui.ctx(), vec![]));
        output.textures_delta.clear();
        assert!(m.work.is_empty() && m.working.is_empty());
    }

    /// egui takes its predicted frame time off a repaint's delay, so one wake
    /// can land just short of the moment: it must ask again.
    #[test]
    fn work_waiting_its_moment_keeps_asking_for_a_repaint() {
        let ctx = egui::Context::default();
        let mut m = Messages::default();
        // Past the frames egui repaints by itself at the start.
        for time in [0., 0.05, 0.1, 0.15, 0.2, 0.25, 0.3, 0.333] {
            let input = egui::RawInput {
                time: Some(time),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| m.set_work(ui.ctx(), working("Long")));
            output.textures_delta.clear();
            let delay = output.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
            assert!(
                delay <= std::time::Duration::from_secs(1),
                "{delay:?} at {time}"
            );
        }
    }

    #[test]
    fn the_running_line_moves_and_stays_on_its_track() {
        let track = Rect::from_min_size(egui::pos2(100., 40.), Vec2::new(190., 4.));
        assert_ne!(
            running_light(track, 0.).left(),
            running_light(track, 0.3).left()
        );
        for i in 0..200 {
            let light = running_light(track, f64::from(i) * 0.05);
            assert!(track.expand(0.01).contains_rect(light), "{light:?}");
        }
    }

    #[test]
    fn work_with_a_place_of_its_own_is_not_said_again() {
        let ctx = egui::Context::default();
        let e = crate::app::Editor::with_context(
            &ctx,
            None,
            crate::app::session::Session::default(),
            None,
        );
        let keys: Vec<_> = e.running_work().iter().map(|w| w.key).collect();
        assert!(keys.is_empty(), "{keys:?}");
    }

    #[test]
    fn a_task_shows_as_running_until_it_ends_whichever_way() {
        let ctx = egui::Context::default();
        let mut e = crate::app::Editor::with_context(
            &ctx,
            None,
            crate::app::session::Session::default(),
            None,
        );
        let shows = |e: &crate::app::Editor| e.running_work().iter().any(|w| w.key == "auto");
        assert!(!shows(&e));
        // Finished, as success and failure both report.
        let (generation, _) = e.document.auto.start();
        assert!(shows(&e));
        e.document.auto.finish(generation);
        assert!(!shows(&e));
        // Cancelled.
        e.document.auto.start();
        e.document.auto.invalidate();
        assert!(!shows(&e));
        // A panic in the worker: `failed` reports it, and the event ends the task.
        e.document.auto.start();
        let id = e.load.id();
        crate::app::task::spawn(
            e.tx.clone(),
            ctx.clone(),
            |_| panic!("the worker"),
            move |tx, message| {
                let _ = tx.send(crate::app::worker::Event::Auto {
                    id,
                    kind: crate::app::worker::AutoKind::Settings,
                    result: Err(message),
                });
            },
        );
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while shows(&e) && std::time::Instant::now() < until {
            e.events(&ctx);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!shows(&e), "the panic ended it");
        assert!(
            e.messages.line().contains("the worker"),
            "{}",
            e.messages.line()
        );
        // The photo going takes it along.
        e.document.auto.start();
        e.document.reset(None);
        assert!(!shows(&e));
    }

    /// Renders the messages in Develop's real layout for looking at them:
    /// `cargo test messages_look -- --ignored`, then `target/messages-*.png`.
    #[test]
    #[ignore = "writes images to look at"]
    fn messages_look() {
        type Say = fn(&mut Messages);
        let say: [(&str, Say); 5] = [
            ("failure", |m| m.fail(WHY)),
            ("warning", |m| m.warn("Finish what is running first")),
            ("working", |m| {
                m.warn("DSC07576.ARW can't be opened: The photo is offline. Right-click it and choose Find on Disk…")
            }),
            ("open", |m| {
                m.warn_with_detail(
                    "Exported 1 of 1 · 1 with notes",
                    "Exported, with notes\nRAWmakase Sous Chef.png: no capture date\nRAWmakase Sous Chef.png: no capture time\nRAWmakase Sous Chef.png: no Model"
                        .to_string(),
                );
                m.warn("1 folder isn't found on this computer");
                m.open = true;
            }),
            ("several", |m| {
                m.warn("Finish what is running first");
                m.fail(WHY);
            }),
        ];
        for (name, say) in say {
            let ctx = egui::Context::default();
            crate::app::install_fonts(&ctx, &fastframe_text::TextRendering::platform_default());
            crate::app::icons::install(&ctx);
            let mut e = crate::app::Editor::with_context(
                &ctx,
                None,
                crate::app::session::Session::default(),
                None,
            );
            e.module = crate::app::Module::Develop;
            e.onboarding.visible = false;
            e.messages = Messages::default();
            say(&mut e.messages);
            // At rest, past the tint on arrival.
            for entry in &mut e.messages.entries {
                entry.shown = Some(-10.);
            }
            if name == "working" {
                e.document.auto.start();
                e.messages.working.push(("auto", -10.));
            }
            let mut textures = Default::default();
            let image = crate::app::ui_snapshot::render(
                &ctx,
                &mut textures,
                Vec2::new(1280., 720.),
                2.,
                5,
                |ui| e.draw(ui),
            );
            let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(format!("target/messages-{name}.png"));
            image.save(&out).unwrap();
        }
    }
}
