//! Screen previews: a photo rendered with its edit at the size a view shows
//! it, for the views that show more than one photo large (Compare). The
//! grid's 640 px previews stand in until they are ready.
//!
//! Renders run on two threads, newest request first, so both photos of a
//! comparison come in together; a request no longer shown is skipped. The
//! results are kept as textures, a few at a time, oldest dropped first.
use super::previews::EditSource;
use super::thumbnails;
use crate::catalog::{Photo, PhotoId};
use eframe::egui;
use rawmakase_cancel::Cancel;
use rawmakase_decode::ai::cache::AiCache;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, Sender, channel},
    },
    thread::JoinHandle,
};

/// Previews are rendered in steps of this many pixels, so resizing the
/// window a little does not render them again.
const EDGE_STEP: u32 = 512;
/// Textures kept besides the ones shown: two comparisons' worth.
const KEPT: usize = 4;
const THREADS: usize = 2;

/// A preview as asked for: photo, file (photo ids can be reused), edge and
/// the edit's stamp, so an edit saved anywhere renders it again.
type Key = (PhotoId, PathBuf, u32, u64);
/// Renders a preview, giving up once the flag is set.
type Render = fn(&Path, u32, Option<&EditSource>, Cancel<'_>) -> anyhow::Result<image::RgbImage>;

struct Job {
    key: Key,
    /// Matches the result to this request, not to an earlier one for the
    /// same key, such as one made before a retry.
    ticket: u64,
    edit: Option<EditSource>,
}
struct Done {
    key: Key,
    ticket: u64,
    outcome: Outcome,
}
enum Outcome {
    Ready(image::RgbImage),
    Failed(String),
    /// No longer shown when its turn came; asked for again if shown.
    Skipped,
}
#[derive(Default)]
struct Queue {
    jobs: Vec<Job>,
    closed: bool,
}
/// The workers' queue, and the flag that cancels the renders under way once it closes.
#[derive(Default)]
struct Shared {
    queue: Mutex<Queue>,
    ready: Condvar,
    cancel: AtomicBool,
    /// Where AI Denoise's finished results are; none until the app sets it.
    ai: Mutex<Option<AiCache>>,
}

/// What a view can draw for a photo.
pub(super) enum Shown<'a> {
    Ready(&'a egui::TextureHandle),
    Loading,
    /// Failed; why is said in the top bar (`take_failures`).
    Failed,
}

pub(super) struct ScreenPreviews {
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
    results: Receiver<Done>,
    /// Previews shown this frame and last; the workers skip the rest.
    wanted: Arc<Mutex<HashSet<Key>>>,
    seen: HashSet<Key>,
    /// Requests on their way, by the ticket of the latest.
    pending: HashMap<Key, u64>,
    next_ticket: u64,
    /// Failed previews, each with why until it is shown and that is said.
    failed: HashMap<Key, Option<String>>,
    /// Why previews shown since the Library last asked failed, for the top
    /// bar.
    to_say: Vec<String>,
    textures: HashMap<Key, egui::TextureHandle>,
    order: VecDeque<Key>,
}
impl ScreenPreviews {
    pub(super) fn new(ctx: &egui::Context) -> Self {
        Self::with(ctx, render)
    }
    /// Previews of edits with AI Denoise render from the finished results in `cache`.
    pub(super) fn set_ai_cache(&self, cache: &AiCache) {
        *self.shared.ai.lock().unwrap() = Some(cache.clone());
    }
    fn with(ctx: &egui::Context, render: Render) -> Self {
        let shared: Arc<Shared> = Default::default();
        let wanted: Arc<Mutex<HashSet<Key>>> = Default::default();
        let (tx, results) = channel();
        let threads = (0..THREADS)
            .map(|_| {
                let (shared, wanted, tx, ctx) =
                    (shared.clone(), wanted.clone(), tx.clone(), ctx.clone());
                std::thread::spawn(move || work(&shared, &wanted, &tx, &ctx, render))
            })
            .collect();
        Self {
            shared,
            threads,
            results,
            wanted,
            seen: HashSet::new(),
            pending: HashMap::new(),
            next_ticket: 0,
            failed: HashMap::new(),
            to_say: Vec::new(),
            textures: HashMap::new(),
            order: VecDeque::new(),
        }
    }
    /// `photo` at `edge` pixels with the edit `stamp` identifies (see
    /// `Catalog::edit_stamp`), asked for unless it is ready, failed or on its
    /// way; `edit` is looked up only when it is asked for.
    pub(super) fn get(
        &mut self,
        photo: &Photo,
        edge: u32,
        stamp: u64,
        edit: impl FnOnce() -> Option<EditSource>,
    ) -> Shown<'_> {
        let key = (
            photo.id,
            photo.path.clone(),
            edge.div_ceil(EDGE_STEP).max(1) * EDGE_STEP,
            stamp,
        );
        self.seen.insert(key.clone());
        if !self.textures.contains_key(&key)
            && !self.failed.contains_key(&key)
            && !self.pending.contains_key(&key)
        {
            self.next_ticket += 1;
            self.pending.insert(key.clone(), self.next_ticket);
            self.wanted.lock().unwrap().insert(key.clone());
            self.shared.queue.lock().unwrap().jobs.push(Job {
                key: key.clone(),
                ticket: self.next_ticket,
                edit: edit(),
            });
            self.shared.ready.notify_one();
        }
        if let Some(texture) = self.textures.get(&key) {
            // Kept longest: the previews in use.
            self.order.retain(|k| *k != key);
            self.order.push_back(key);
            Shown::Ready(texture)
        } else if let Some(why) = self.failed.get_mut(&key) {
            // Said when shown, so a photo no longer in view says nothing.
            self.to_say.extend(why.take());
            Shown::Failed
        } else {
            Shown::Loading
        }
    }
    /// Why the previews shown since last asked failed, each once, for the
    /// top bar.
    pub(super) fn take_failures(&mut self) -> Vec<String> {
        std::mem::take(&mut self.to_say)
    }
    pub(super) fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(Done {
            key,
            ticket,
            outcome,
        }) = self.results.try_recv()
        {
            // Asked for before its edit changed: a newer request is coming.
            if self.pending.get(&key) != Some(&ticket) {
                continue;
            }
            self.pending.remove(&key);
            match outcome {
                Outcome::Ready(image) => {
                    let size = [image.width() as usize, image.height() as usize];
                    let texture = ctx.load_texture(
                        "library-screen-preview",
                        egui::ColorImage::from_rgb(size, image.as_raw()),
                        egui::TextureOptions::LINEAR,
                    );
                    // The oldest go first, but never one still shown, so a
                    // survey of many photos is not rendered over and over.
                    let shown = self.wanted.lock().unwrap().clone();
                    while self.textures.len() >= KEPT + shown.len() {
                        let Some(at) = self
                            .order
                            .iter()
                            .position(|k| !shown.contains(k) && !self.seen.contains(k))
                        else {
                            break;
                        };
                        let old = self.order.remove(at).unwrap();
                        self.textures.remove(&old);
                    }
                    self.order.push_back(key.clone());
                    self.textures.insert(key, texture);
                }
                Outcome::Failed(error) => {
                    self.failed.insert(key, Some(error));
                }
                Outcome::Skipped => {}
            }
        }
    }
    /// Hands the workers the previews shown this frame. Call once per frame.
    pub(super) fn publish_shown(&mut self) {
        *self.wanted.lock().unwrap() = std::mem::take(&mut self.seen);
    }
    /// Lets previews that failed be asked for again, e.g. once their
    /// originals are back online.
    pub(super) fn retry_failed(&mut self) {
        self.failed.clear();
        // Renders on their way may fail as these did: asked for afresh.
        self.pending.clear();
    }
    /// Forgets every preview, e.g. as the raw defaults changed; renders on
    /// their way are dropped when they arrive.
    pub(super) fn clear(&mut self) {
        self.textures.clear();
        self.order.clear();
        self.failed.clear();
        self.pending.clear();
    }
    /// Forgets `id`'s previews, as it was removed.
    pub(super) fn forget(&mut self, id: PhotoId) {
        self.textures.retain(|key, _| key.0 != id);
        self.order.retain(|key| key.0 != id);
        self.failed.retain(|key, _| key.0 != id);
        self.pending.retain(|key, _| key.0 != id);
    }
    #[cfg(test)]
    pub(super) fn wait(&mut self, ctx: &egui::Context) {
        let started = std::time::Instant::now();
        while !self.pending.is_empty() && started.elapsed() < std::time::Duration::from_secs(20) {
            std::thread::sleep(std::time::Duration::from_millis(5));
            self.poll(ctx);
        }
    }
}
impl ScreenPreviews {
    /// Ends the workers, cancelling the renders under way, and drops their results;
    /// the workers are then waited for at exit.
    pub(super) fn close(&mut self) -> Vec<crate::app::task::Stopping> {
        self.shut();
        self.results = channel().1;
        self.threads
            .drain(..)
            .map(|thread| crate::app::task::Stopping::new(Some(thread)))
            .collect()
    }
    fn shut(&self) {
        self.shared.queue.lock().unwrap().closed = true;
        self.shared.cancel.store(true, Ordering::Relaxed);
        self.shared.ready.notify_all();
    }
}
impl Drop for ScreenPreviews {
    fn drop(&mut self) {
        self.shut();
    }
}

/// A worker thread: renders the newest job still wanted, until closed.
fn work(
    shared: &Shared,
    wanted: &Mutex<HashSet<Key>>,
    results: &Sender<Done>,
    ctx: &egui::Context,
    render: Render,
) {
    loop {
        let job = {
            let mut queue = shared.queue.lock().unwrap();
            while queue.jobs.is_empty() && !queue.closed {
                queue = shared.ready.wait(queue).unwrap();
            }
            if queue.closed {
                return;
            }
            queue.jobs.pop().unwrap()
        };
        let Job { key, ticket, edit } = job;
        let outcome = if !wanted.lock().unwrap().contains(&key) {
            Outcome::Skipped
        } else {
            let ai = shared.ai.lock().unwrap().clone();
            let rendered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                match (&edit, &ai) {
                    // As the Library's edited previews: from a finished result, else plain.
                    (Some(source), Some(_)) if source.renders_ai() => {
                        super::previews::develop_for_preview(
                            &key.1,
                            source,
                            key.2,
                            ai.as_ref(),
                            Cancel::new(&shared.cancel),
                        )
                        .map(|(image, _)| image)
                    }
                    _ => render(&key.1, key.2, edit.as_ref(), Cancel::new(&shared.cancel)),
                }
            }));
            match rendered {
                Ok(Ok(image)) => Outcome::Ready(image),
                Ok(Err(e)) => Outcome::Failed(format!("{e:#}")),
                Err(_) => Outcome::Failed("The preview could not be rendered".into()),
            }
        };
        let done = Done {
            key,
            ticket,
            outcome,
        };
        if results.send(done).is_err() {
            return;
        }
        ctx.request_repaint();
    }
}

/// `path` within `edge` pixels: developed with its edit (or, for a RAW, the
/// defaults Develop opens it with), as `previews::develop_quickly` does; a JPEG,
/// TIFF or PNG without an edit as it is, which its defaults reproduce. An edit with
/// AI Denoise renders from a finished result instead (see `work`).
fn render(
    path: &Path,
    edge: u32,
    edit: Option<&EditSource>,
    cancel: Cancel<'_>,
) -> anyhow::Result<image::RgbImage> {
    let edited = matches!(edit, Some(EditSource::Recipe(_) | EditSource::Lightroom(_)));
    if !edited && !crate::storage::is_raw(path) {
        return Ok(thumbnails::downscale(&crate::raster::decode(path)?, edge));
    }
    super::previews::develop_quickly(path, edit, edge, cancel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_jpeg_shows_with_its_edit_and_as_it_is_without() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("photo.jpg");
        let edit = super::super::previews::tests::edited_jpeg(&path)?;
        let cancel = AtomicBool::new(false);
        let edited = render(&path, 64, Some(&edit), Cancel::new(&cancel))?;
        assert!(
            edited.pixels().all(|p| p.0[1] > 75),
            "{:?}",
            edited.get_pixel(0, 0)
        );
        let unedited = render(&path, 64, None, Cancel::new(&cancel))?;
        assert!(unedited.pixels().all(|p| p.0[1].abs_diff(60) <= 2));
        Ok(())
    }

    fn photo(id: PhotoId, path: &Path) -> Photo {
        Photo {
            id,
            path: path.into(),
            ..Default::default()
        }
    }

    #[test]
    fn previews_render_at_the_size_shown_and_are_kept() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("a.png");
        image::RgbImage::new(3000, 2000).save(&path)?;
        let ctx = egui::Context::default();
        let mut screen = ScreenPreviews::new(&ctx);
        let a = photo(PhotoId(1), &path);
        assert!(matches!(screen.get(&a, 900, 0, || None), Shown::Loading));
        screen.wait(&ctx);
        // Rendered at the next step up from the size asked for.
        match screen.get(&a, 900, 0, || None) {
            Shown::Ready(texture) => assert_eq!(texture.size(), [1024, 683]),
            _ => panic!("not rendered"),
        }
        // Forgetting the photo renders it again.
        screen.forget(PhotoId(1));
        assert!(matches!(screen.get(&a, 900, 0, || None), Shown::Loading));
        Ok(())
    }

    #[test]
    fn a_failed_or_panicking_render_is_reported_and_the_rest_go_on() {
        fn render(
            path: &Path,
            _: u32,
            _: Option<&EditSource>,
            _: Cancel<'_>,
        ) -> anyhow::Result<image::RgbImage> {
            match path.to_str() {
                Some("panics") => panic!("render panics"),
                Some("fails") => anyhow::bail!("no such photo"),
                _ => Ok(image::RgbImage::new(4, 4)),
            }
        }
        let ctx = egui::Context::default();
        let mut screen = ScreenPreviews::with(&ctx, render);
        let photos = [
            (PhotoId(1), "panics"),
            (PhotoId(2), "fails"),
            (PhotoId(3), "works"),
        ]
        .map(|(id, path)| photo(id, Path::new(path)));
        for p in &photos {
            let _ = screen.get(p, 100, 0, || None);
        }
        screen.wait(&ctx);
        assert!(matches!(
            screen.get(&photos[0], 100, 0, || None),
            Shown::Failed
        ));
        assert!(matches!(
            screen.get(&photos[1], 100, 0, || None),
            Shown::Failed
        ));
        let mut failures = screen.take_failures();
        failures.sort();
        assert_eq!(
            failures,
            ["The preview could not be rendered", "no such photo"]
        );
        assert!(matches!(
            screen.get(&photos[2], 100, 0, || None),
            Shown::Ready(_)
        ));
    }

    #[test]
    fn a_preview_no_longer_shown_is_skipped_and_asked_for_again() {
        let ctx = egui::Context::default();
        let mut screen = ScreenPreviews::with(&ctx, |_, _, _, _| Ok(image::RgbImage::new(4, 4)));
        let a = photo(PhotoId(1), Path::new("a"));
        // Queued, then not shown for a frame before a worker takes it.
        let key = (PhotoId(1), PathBuf::from("a"), EDGE_STEP, 0);
        screen.pending.insert(key.clone(), 0);
        screen.shared.queue.lock().unwrap().jobs.push(Job {
            key,
            ticket: 0,
            edit: None,
        });
        screen.shared.ready.notify_one();
        screen.wait(&ctx);
        assert!(screen.textures.is_empty());
        assert!(matches!(screen.get(&a, 100, 0, || None), Shown::Loading));
        screen.wait(&ctx);
        assert!(matches!(screen.get(&a, 100, 0, || None), Shown::Ready(_)));
    }

    #[test]
    fn a_render_asked_for_before_an_edit_changed_is_dropped() {
        let ctx = egui::Context::default();
        let mut screen = ScreenPreviews::with(&ctx, |_, _, _, _| Ok(image::RgbImage::new(4, 4)));
        let key = (PhotoId(1), PathBuf::from("a"), EDGE_STEP, 0);
        // The photo was asked for again (ticket 2) after the edit changed,
        // and the render asked for before it (ticket 1) comes in first.
        screen.pending.insert(key.clone(), 2);
        screen.wanted.lock().unwrap().insert(key.clone());
        screen.shared.queue.lock().unwrap().jobs.push(Job {
            key: key.clone(),
            ticket: 1,
            edit: None,
        });
        screen.shared.ready.notify_one();
        let started = std::time::Instant::now();
        while started.elapsed() < std::time::Duration::from_millis(300) {
            std::thread::sleep(std::time::Duration::from_millis(5));
            screen.poll(&ctx);
        }
        assert!(screen.textures.is_empty());
        assert_eq!(screen.pending.get(&key), Some(&2));
    }

    #[test]
    fn a_failed_preview_is_asked_for_again_once_retried() {
        let ctx = egui::Context::default();
        let mut screen = ScreenPreviews::with(&ctx, |_, _, _, _| anyhow::bail!("offline"));
        let a = photo(PhotoId(1), Path::new("a"));
        let _ = screen.get(&a, 100, 0, || None);
        screen.wait(&ctx);
        assert!(matches!(screen.get(&a, 100, 0, || None), Shown::Failed));
        // Its failure is for the top bar once, however often it is shown.
        assert_eq!(screen.take_failures(), ["offline"]);
        let _ = screen.get(&a, 100, 0, || None);
        screen.poll(&ctx);
        assert!(screen.take_failures().is_empty());
        screen.retry_failed();
        assert!(matches!(screen.get(&a, 100, 0, || None), Shown::Loading));
    }

    /// A preview that fails once its photo is out of view says nothing:
    /// its failure is said when it is shown again.
    #[test]
    fn a_preview_failing_out_of_view_is_said_only_when_shown() {
        let ctx = egui::Context::default();
        let mut screen = ScreenPreviews::with(&ctx, |_, _, _, _| anyhow::bail!("offline"));
        let a = photo(PhotoId(1), Path::new("a"));
        let _ = screen.get(&a, 100, 0, || None);
        screen.wait(&ctx);
        assert!(screen.take_failures().is_empty());
        assert!(matches!(screen.get(&a, 100, 0, || None), Shown::Failed));
        assert_eq!(screen.take_failures(), ["offline"]);
    }

    #[test]
    fn previews_shown_together_are_all_kept() {
        let ctx = egui::Context::default();
        let mut screen = ScreenPreviews::with(&ctx, |_, _, _, _| Ok(image::RgbImage::new(4, 4)));
        let photos: Vec<Photo> = (1..=KEPT as i64 + 3)
            .map(|id| photo(PhotoId(id), Path::new("a")))
            .collect();
        // One frame shows them all, as a survey of seven does.
        for p in &photos {
            let _ = screen.get(p, 100, 0, || None);
        }
        screen.wait(&ctx);
        assert!(
            photos
                .iter()
                .all(|p| matches!(screen.get(p, 100, 0, || None), Shown::Ready(_)))
        );
    }

    #[test]
    fn closing_cancels_the_render_under_way_and_ends_the_workers() {
        static STARTED: AtomicBool = AtomicBool::new(false);
        fn render(
            _: &Path,
            _: u32,
            _: Option<&EditSource>,
            cancel: Cancel<'_>,
        ) -> anyhow::Result<image::RgbImage> {
            STARTED.store(true, Ordering::Relaxed);
            // A render that only a cancel ends.
            while !cancel.is_cancelled() {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            anyhow::bail!("Cancelled")
        }
        let ctx = egui::Context::default();
        let mut screen = ScreenPreviews::with(&ctx, render);
        let _ = screen.get(&photo(PhotoId(1), Path::new("a")), 100, 0, || None);
        let started = std::time::Instant::now();
        while !STARTED.load(Ordering::Relaxed) {
            assert!(started.elapsed() < std::time::Duration::from_secs(20));
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let stopping = screen.close();
        let waited = crate::app::task::wait_for(stopping, std::time::Duration::from_secs(20));
        assert_eq!(waited.detached, 0);
    }
}
