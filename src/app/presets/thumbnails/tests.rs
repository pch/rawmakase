//! The tiles' lifecycle, with a stand-in renderer and the UI's clock given:
//! what is asked for, what a change cancels and what stays on screen.
use super::*;
use std::time::{Duration, Instant};

/// No Lensfun database, as the tests have none installed.
static NO_LENSFUN: crate::lens::lensfun::installed::Handle =
    crate::lens::lensfun::installed::Handle::Given(None);

/// Renders a gray tile per preset, logging each one it starts. A preset whose id
/// begins with `panic` panics, `fail` fails, and `block` waits until released or
/// cancelled (then failing, as a cancelled render does).
#[derive(Clone, Default)]
struct Stub {
    started: Arc<Mutex<Vec<String>>>,
    release: Arc<AtomicBool>,
    resets: Arc<Mutex<usize>>,
}
impl Render for Stub {
    fn tile(
        &mut self,
        job: &JobView,
        preset: &crate::xmp::Preset,
        cancel: &AtomicBool,
    ) -> anyhow::Result<egui::ColorImage> {
        self.started.lock().unwrap().push(preset.id.clone());
        if preset.id.starts_with("panic") {
            panic!("boom");
        }
        anyhow::ensure!(!preset.id.starts_with("fail"), "no such profile");
        if preset.id.starts_with("block") {
            while !self.release.load(Ordering::Relaxed) {
                anyhow::ensure!(!cancel.load(Ordering::Relaxed), "Cancelled");
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        let side = job.edge as usize;
        Ok(egui::ColorImage::new(
            [side, side],
            vec![egui::Color32::GRAY; side * side],
        ))
    }
    fn reset(&mut self) {
        *self.resets.lock().unwrap() += 1;
    }
}
impl Stub {
    fn started(&self) -> Vec<String> {
        self.started.lock().unwrap().clone()
    }
}

fn preset(id: &str) -> crate::xmp::Preset {
    crate::xmp::Preset {
        id: id.into(),
        name: id.into(),
        group: "Group".into(),
        path: Default::default(),
        settings: Default::default(),
        curves: Default::default(),
        look: String::new(),
        blockers: Vec::new(),
        notes: Vec::new(),
        photo_settings: false,
        local: Default::default(),
        builtin: false,
    }
}

/// What the panel renders tiles from: a photo, an edit and a library.
struct Photo {
    image: Arc<CameraImage>,
    recipe: Recipe,
    library: Arc<crate::presets::Library>,
    path: PathBuf,
}
impl Photo {
    fn new(ids: &[&str]) -> Self {
        Self {
            image: crate::app::testing::blue_image(),
            recipe: Recipe::default(),
            library: Arc::new(crate::presets::Library {
                presets: ids.iter().map(|id| preset(id)).collect(),
                errors: Vec::new(),
            }),
            path: "a.ARW".into(),
        }
    }
    fn inputs(&self) -> Inputs<'_> {
        Inputs {
            image: &self.image,
            recipe: &self.recipe,
            metadata: &self.image.metadata,
            profiles: &[],
            lensfun: &NO_LENSFUN,
            monitor: None,
            library: &self.library,
            photo: (Some(self.path.clone()), None),
        }
    }
    fn index(&self, id: &str) -> usize {
        self.library
            .presets
            .iter()
            .position(|p| p.id == id)
            .unwrap()
    }
}

struct Panel {
    ctx: egui::Context,
    stub: Stub,
    t: Thumbnails,
    /// The first frame is as if the grid had opened a second before.
    opened_before: bool,
}
impl Panel {
    fn new() -> Self {
        let ctx = egui::Context::default();
        let stub = Stub::default();
        let t = Thumbnails::with_render(ctx.clone(), stub.clone());
        Self {
            ctx,
            stub,
            t,
            opened_before: true,
        }
    }
    /// A panel whose grid opens with its first frame.
    fn opening() -> Self {
        Self {
            opened_before: false,
            ..Self::new()
        }
    }
    /// One frame of the grid at `now` seconds showing `ids`, `edge` pixels square.
    fn frame_at(&mut self, now: f64, edge: u32, photo: &Photo, ids: &[&str], idle: bool) {
        let inputs = photo.inputs();
        if self.opened_before && self.t.seen.is_none() {
            self.t.begin(&self.ctx, now - 1., edge, Some(&inputs));
            self.t.end(&self.ctx, now - 1., idle, Some(&inputs));
            self.t.sweep();
        }
        self.t.begin(&self.ctx, now, edge, Some(&inputs));
        for id in ids {
            self.t.want(photo.index(id), id);
            let _ = self.t.tile(id);
        }
        self.t.end(&self.ctx, now, idle, Some(&inputs));
        self.t.sweep();
    }
    fn frame(&mut self, now: f64, photo: &Photo, ids: &[&str]) {
        self.frame_at(now, 64, photo, ids, true);
    }
    /// The frames from `now` until `ids` have all been reported, at most ten seconds.
    fn settle(&mut self, now: f64, photo: &Photo, ids: &[&str]) {
        let start = Instant::now();
        loop {
            self.frame(now, photo, ids);
            if ids.iter().all(|id| !self.t.due(id)) {
                return;
            }
            assert!(
                start.elapsed().as_secs() < 10,
                "tiles {ids:?} never arrived"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    /// Waits until the stub has started `n` tiles.
    fn started(&self, n: usize) -> Vec<String> {
        let start = Instant::now();
        while self.stub.started().len() < n {
            assert!(start.elapsed().as_secs() < 10, "{:?}", self.stub.started());
            std::thread::sleep(Duration::from_millis(1));
        }
        self.stub.started()
    }
    fn shows(&mut self, id: &str) -> &'static str {
        let due = self.t.due(id);
        match self.t.tile(id) {
            Tile::Missing => "missing",
            Tile::Picture(_) if due => "stale",
            Tile::Picture(_) => "picture",
            Tile::Failed(_) => "failed",
        }
    }
    fn batch_cancel(&self) -> Arc<AtomicBool> {
        self.t.batch.as_ref().expect("a batch").cancel.clone()
    }
}

/// Only the tiles shown are rendered, in the order shown, once the quiet period
/// after the photo opened has passed.
#[test]
fn only_the_tiles_shown_are_rendered() {
    let photo = Photo::new(&["a", "b", "c", "d"]);
    let mut p = Panel::opening();
    p.frame(0., &photo, &["b", "c"]);
    assert!(p.t.batch.is_none(), "within the quiet period");
    p.settle(1., &photo, &["b", "c"]);
    assert_eq!(p.stub.started(), ["b", "c"]);
    assert_eq!(p.shows("b"), "picture");
    assert_eq!(p.shows("a"), "missing");
}

/// While the main view renders, or within the quiet period of an edit, nothing
/// is submitted; the UI is woken when the period ends.
#[test]
fn no_batch_while_the_main_view_renders_or_an_edit_is_fresh() {
    let mut photo = Photo::new(&["a"]);
    let mut p = Panel::opening();
    p.frame(0., &photo, &["a"]);
    p.frame_at(0.5, 64, &photo, &["a"], false);
    assert!(p.t.batch.is_none(), "the main view renders");
    photo.recipe.exposure = 1.;
    p.frame(0.6, &photo, &["a"]);
    p.frame(0.85, &photo, &["a"]);
    assert!(p.t.batch.is_none(), "0.25 s after the edit");
    p.frame(0.91, &photo, &["a"]);
    assert!(p.t.batch.is_some(), "0.31 s after the edit");
}

/// A batch running with nothing changed is never submitted again, and each tile
/// renders once.
#[test]
fn a_running_batch_is_not_submitted_again() {
    let photo = Photo::new(&["block", "b"]);
    let mut p = Panel::new();
    p.frame(1., &photo, &["block", "b"]);
    p.started(1);
    let cancel = p.batch_cancel();
    for k in 0..20 {
        p.frame(1.1 + k as f64 * 0.1, &photo, &["block", "b"]);
    }
    assert!(Arc::ptr_eq(&cancel, &p.batch_cancel()), "the same batch");
    assert!(!cancel.load(Ordering::Relaxed));
    p.stub.release.store(true, Ordering::Relaxed);
    p.settle(5., &photo, &["block", "b"]);
    assert_eq!(p.stub.started(), ["block", "b"]);
}

/// Scrolling while a batch renders cancels it, and the tiles now shown render
/// next; the tile cancelled part way reports nothing.
#[test]
fn scrolling_cancels_the_batch_for_the_tiles_now_shown() {
    let photo = Photo::new(&["block", "b", "c", "d"]);
    let mut p = Panel::new();
    p.frame(1., &photo, &["block", "b"]);
    p.started(1);
    let first = p.batch_cancel();
    p.frame(1.1, &photo, &["c", "d"]);
    assert!(
        first.load(Ordering::Relaxed),
        "the batch scrolled away is cancelled"
    );
    p.settle(1.2, &photo, &["c", "d"]);
    assert_eq!(p.stub.started(), ["block", "c", "d"]);
    assert_eq!(p.shows("block"), "missing", "cancelled, not failed");
    assert_eq!(p.shows("b"), "missing");
}

/// Leaving the grid part way through a batch and coming back with nothing
/// changed renders the interrupted tile again; it was never recorded as failed.
#[test]
fn leaving_and_coming_back_renders_the_interrupted_tile_again() {
    let photo = Photo::new(&["a", "block"]);
    let mut p = Panel::new();
    p.frame(1., &photo, &["a", "block"]);
    p.started(2);
    let cancel = p.batch_cancel();
    // A frame without the grid: List view, say. The worker lets go of its copy.
    p.t.sweep();
    assert!(cancel.load(Ordering::Relaxed));
    assert!(p.t.batch.is_none());
    let start = Instant::now();
    while *p.stub.resets.lock().unwrap() == 0 {
        assert!(start.elapsed().as_secs() < 10);
        std::thread::sleep(Duration::from_millis(1));
    }
    p.frame(1.1, &photo, &["a", "block"]);
    assert!(
        p.t.batch.is_some(),
        "submitted again at once: nothing changed"
    );
    p.stub.release.store(true, Ordering::Relaxed);
    p.settle(1.2, &photo, &["a", "block"]);
    assert_eq!(p.shows("block"), "picture");
    assert_eq!(p.stub.started(), ["a", "block", "block"]);
    assert_eq!(*p.stub.resets.lock().unwrap(), 1);
}

/// An edit starts a new generation: the batch rendering is cancelled, the tiles
/// of the edit before stay on screen until each is replaced, and a late tile of
/// the old generation is dropped.
#[test]
fn an_edit_cancels_the_batch_and_keeps_the_tiles_until_replaced() {
    let mut photo = Photo::new(&["a", "block"]);
    let mut p = Panel::new();
    p.settle(1., &photo, &["a"]);
    p.frame(1.1, &photo, &["a", "block"]);
    p.started(2);
    let cancel = p.batch_cancel();
    let generation = p.t.generation;
    photo.recipe.exposure = 0.5;
    p.frame(1.2, &photo, &["a", "block"]);
    assert!(cancel.load(Ordering::Relaxed));
    assert_eq!(p.t.generation, generation + 1);
    assert_eq!(p.shows("a"), "stale", "shown until replaced");
    // A result of the edit before that landed late is not this edit's.
    p.t.inbox.lock().unwrap().push(Outcome {
        generation,
        edge: 64,
        id: "block".into(),
        result: Ok(egui::ColorImage::new(
            [64, 64],
            vec![egui::Color32::RED; 64 * 64],
        )),
    });
    p.frame(1.3, &photo, &["a", "block"]);
    assert_eq!(p.shows("block"), "missing");
    p.stub.release.store(true, Ordering::Relaxed);
    p.settle(1.6, &photo, &["a", "block"]);
    assert_eq!(p.shows("a"), "picture");
    assert_eq!(p.t.entries.len(), 2, "replaced in place");
}

/// What starts a new generation: a new decode, an edit, the library read again
/// (a new library), the profiles and the monitor; the same inputs again don't.
/// Another photo also lets go of its tiles; a new decode of the same photo, the
/// full-size one after its draft, keeps them shown until replaced.
#[test]
fn what_renders_the_tiles_again() {
    let mut photo = Photo::new(&["a"]);
    let mut p = Panel::new();
    p.settle(1., &photo, &["a"]);
    let mut generation = p.t.generation;
    let mut bumped = |p: &mut Panel, photo: &Photo, now: f64| {
        p.frame(now, photo, &["a"]);
        let did = p.t.generation != generation;
        generation = p.t.generation;
        did
    };
    assert!(!bumped(&mut p, &photo, 2.), "nothing changed");
    photo.recipe.contrast = 0.2;
    assert!(bumped(&mut p, &photo, 2.1), "an edit");
    photo.library = Arc::new((*photo.library).clone());
    assert!(bumped(&mut p, &photo, 2.2), "the library read again");
    // The full-size decode after the draft: the same photo, a new image.
    p.settle(3., &photo, &["a"]);
    photo.image = Arc::new((*photo.image).clone());
    assert!(bumped(&mut p, &photo, 4.), "a new decode");
    assert_eq!(p.shows("a"), "stale", "the draft's tile shows meanwhile");
    // Another photo: nothing of this one's shows.
    photo.path = "b.ARW".into();
    photo.image = crate::app::testing::blue_image();
    assert!(bumped(&mut p, &photo, 5.));
    assert_eq!(p.shows("a"), "missing");
    // The monitor profile and the profiles are inputs too.
    let profile = Arc::new(crate::camera_profiles::CameraProfile::creative_for_test(
        &photo.image.metadata,
    ));
    let inputs = |monitor: Option<PathBuf>, profiles: &[Arc<CameraProfile>]| {
        let mut seen = Seen::of(&photo.inputs());
        seen.monitor = monitor;
        let mut i = photo.inputs();
        i.profiles = profiles;
        seen.same(&i)
    };
    assert!(!inputs(Some("monitor.icc".into()), &[]));
    assert!(!inputs(None, std::slice::from_ref(&profile)));
    assert!(inputs(None, &[]));
}

/// A resized panel shows the tiles it has, scaled, and renders them at the new
/// size only once it has kept its size for the quiet period.
#[test]
fn a_resize_shows_scaled_tiles_and_waits_for_the_quiet_period() {
    let photo = Photo::new(&["a", "b"]);
    let mut p = Panel::new();
    p.settle(1., &photo, &["a", "b"]);
    let generation = p.t.generation;
    p.frame_at(2., 80, &photo, &["a", "b"], true);
    assert_eq!(p.t.generation, generation, "a resize is no new generation");
    assert_eq!(p.shows("a"), "stale");
    p.frame_at(2.1, 96, &photo, &["a", "b"], true);
    p.frame_at(2.35, 96, &photo, &["a", "b"], true);
    assert!(p.t.batch.is_none(), "still resizing");
    p.frame_at(2.41, 96, &photo, &["a", "b"], true);
    assert_eq!(p.t.batch.as_ref().unwrap().edge, 96);
    let start = Instant::now();
    while p.shows("a") != "picture" || p.shows("b") != "picture" {
        assert!(start.elapsed().as_secs() < 10);
        p.frame_at(2.5, 96, &photo, &["a", "b"], true);
        std::thread::sleep(Duration::from_millis(2));
    }
    match p.t.tile("a") {
        Tile::Picture(texture) => assert_eq!(texture.size(), [96, 96]),
        _ => unreachable!(),
    }
}

/// A tile that panics reports its failure, the rest of its batch renders, the
/// renderer is rebuilt, and later batches render. A failed tile is not asked for
/// again until the generation changes.
#[test]
fn a_panicking_or_failing_tile_leaves_the_rest_rendering() {
    let mut photo = Photo::new(&["panic", "b", "fail", "c"]);
    let mut p = Panel::new();
    p.settle(1., &photo, &["panic", "b", "fail"]);
    assert_eq!(p.shows("panic"), "failed");
    assert_eq!(p.shows("fail"), "failed");
    assert_eq!(p.shows("b"), "picture");
    assert_eq!(*p.stub.resets.lock().unwrap(), 1, "rebuilt after the panic");
    match p.t.tile("fail") {
        Tile::Failed(why) => assert_eq!(why, "no such profile"),
        _ => unreachable!(),
    }
    // A later batch renders.
    p.settle(2., &photo, &["panic", "b", "fail", "c"]);
    assert_eq!(p.shows("c"), "picture");
    for k in 0..5 {
        p.frame(2.1 + k as f64 * 0.1, &photo, &["panic", "b", "fail", "c"]);
    }
    let tries = |p: &Panel, id: &str| p.stub.started().iter().filter(|s| *s == id).count();
    assert_eq!((tries(&p, "panic"), tries(&p, "fail")), (1, 1));
    // An edit tries them again.
    photo.recipe.exposure = 0.3;
    p.frame(3., &photo, &["panic", "fail"]);
    assert_eq!(p.shows("fail"), "missing");
    p.settle(4., &photo, &["panic", "fail"]);
    assert_eq!((tries(&p, "panic"), tries(&p, "fail")), (2, 2));
}

/// The cache keeps at most its size in bytes, letting go of the tiles drawn
/// longest ago, never one shown.
#[test]
fn the_cache_lets_go_of_the_tiles_drawn_longest_ago() {
    let ids: Vec<String> = (0..12).map(|i| format!("p{i}")).collect();
    let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    let photo = Photo::new(&refs);
    let mut p = Panel::new();
    // 1024 pixels square: 4 MB a tile, 16 tiles in 64 MB.
    let start = Instant::now();
    for (k, chunk) in refs.chunks(6).enumerate() {
        loop {
            p.frame_at(1. + k as f64, 1024, &photo, chunk, true);
            if chunk.iter().all(|id| !p.t.due(id)) {
                break;
            }
            assert!(start.elapsed().as_secs() < 20);
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    assert_eq!(p.t.entries.len(), 12);
    // Six more: the first six, drawn longest ago, make room.
    let more: Vec<String> = (12..18).map(|i| format!("p{i}")).collect();
    let all: Vec<&str> = refs
        .iter()
        .copied()
        .chain(more.iter().map(String::as_str))
        .collect();
    let photo = Photo {
        library: Arc::new(crate::presets::Library {
            presets: all.iter().map(|id| preset(id)).collect(),
            errors: Vec::new(),
        }),
        ..photo
    };
    let shown = &all[12..];
    p.frame_at(9., 1024, &photo, shown, true);
    loop {
        p.frame_at(10., 1024, &photo, shown, true);
        if shown.iter().all(|id| !p.t.due(id)) {
            break;
        }
        assert!(start.elapsed().as_secs() < 30);
        std::thread::sleep(Duration::from_millis(2));
    }
    p.frame_at(10., 1024, &photo, shown, true);
    let bytes: usize = p.t.entries.values().map(Entry::bytes).sum();
    assert!(bytes <= CACHE_BYTES, "{bytes}");
    assert_eq!(p.t.entries.len(), 16);
    for id in shown {
        assert!(p.t.entries.contains_key(*id));
    }
    assert!(!p.t.entries.contains_key("p0") && p.t.entries.contains_key("p11"));
}

/// Quitting stops the worker within the deadline, a tile rendering cancelled.
#[test]
fn quitting_stops_the_worker_within_the_deadline() {
    let photo = Photo::new(&["block"]);
    let mut p = Panel::new();
    p.frame(1., &photo, &["block"]);
    p.started(1);
    let cancel = p.batch_cancel();
    let stopping = p.t.stop();
    assert!(cancel.load(Ordering::Relaxed));
    let waited = crate::app::task::wait_for(vec![stopping], Duration::from_secs(5));
    assert_eq!(waited.detached, 0);
    assert!(
        p.t.inbox.lock().unwrap().is_empty(),
        "the cancelled tile reports nothing"
    );
}

/// A tile is the middle of its render, square, at most its size.
#[test]
fn a_tile_is_the_middle_of_its_render() {
    let out = crate::rendered::Rendered {
        width: 6,
        height: 4,
        pixels: (0..24)
            .map(|i| match i % 6 {
                0 | 5 => [0.; 3],
                _ => [1.; 3],
            })
            .collect(),
    };
    let tile = square(&out, 4, None);
    assert_eq!(tile.size, [4, 4]);
    assert!(tile.pixels.iter().all(|p| *p == egui::Color32::WHITE));
    assert_eq!(square(&out, 9, None).size, [4, 4]);
}

/// While no photo is decoded (the next one opening), the grid shows placeholders,
/// never the last photo's tiles.
#[test]
fn no_tiles_show_while_the_next_photo_opens() {
    let photo = Photo::new(&["a"]);
    let mut p = Panel::new();
    p.settle(1., &photo, &["a"]);
    assert_eq!(p.shows("a"), "picture");
    p.t.begin(&p.ctx, 2., 64, None);
    p.t.want(0, "a");
    assert_eq!(p.shows("a"), "missing");
    p.t.end(&p.ctx, 2., true, None);
    assert!(p.t.batch.is_none());
}

/// A resize cancels the batch rendering at the old size.
#[test]
fn a_resize_cancels_the_batch_at_the_old_size() {
    let photo = Photo::new(&["block"]);
    let mut p = Panel::new();
    p.frame(1., &photo, &["block"]);
    p.started(1);
    let cancel = p.batch_cancel();
    p.frame_at(1.1, 80, &photo, &["block"], true);
    assert!(cancel.load(Ordering::Relaxed));
    assert!(p.t.batch.is_none(), "nothing new within the quiet period");
}
