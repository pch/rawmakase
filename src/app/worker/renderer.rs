use super::{
    Event, Latest, Pane, Preview, RenderJob, RenderStage, RetiredTextures, TaskKind, send,
};
use crate::model::recipe::Recipe;
use crate::{
    develop::{self, gpu, quality::Output},
    raw,
};
use eframe::{egui, egui_wgpu};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, atomic::Ordering, mpsc::Sender},
    time::Instant,
};
#[derive(Clone)]
pub(in crate::app) enum RenderBackend {
    /// The CPU, for headless tests.
    #[cfg(test)]
    Cpu,
    /// The GPU; with the UI's render state, previews are presented into textures
    /// the viewport draws, and otherwise read back from a device of their own.
    Gpu(Option<egui_wgpu::RenderState>),
}
/// Long edges of the Navigator's and the library thumbnail's copies.
const NAVIGATOR: u32 = 360;
const THUMBNAIL: u32 = 640;
/// A full 100% region presented within this needs no reduced preview first.
const QUICK_REGION: std::time::Duration = std::time::Duration::from_millis(40);

/// A finished render and the view it shows.
struct Shown {
    image: Arc<crate::camera_data::CameraImage>,
    recipe: Recipe,
    max_edge: u32,
    region: Option<[u32; 4]>,
    clipping: crate::rendered::ClipOverlay,
    monitor: Option<PathBuf>,
    navigator: bool,
    overlay: super::Overlay,
    out: Shot,
}
enum Shot {
    Pixels(crate::rendered::Rendered),
    /// A presented frame, valid while its texture still holds `generation`.
    Frame {
        texture: wgpu::Texture,
        generation: u64,
        preview: Presented,
        histogram: Box<crate::rendered::Histogram>,
    },
}
impl Shown {
    fn new(job: &RenderJob, out: Shot) -> Self {
        Self {
            image: job.image.clone(),
            recipe: job.recipe.clone(),
            max_edge: job.max_edge,
            region: job.region,
            clipping: job.clipping,
            monitor: job.monitor.clone(),
            navigator: job.navigator,
            overlay: job.overlay,
            out,
        }
    }
    /// The same photo, view and edit, with pixels that are still there.
    fn matches(&self, job: &RenderJob, gpu: Option<&gpu::Processor>) -> bool {
        let same = Arc::ptr_eq(&self.image, &job.image)
            && self.max_edge == job.max_edge
            && self.region == job.region
            && self.recipe == job.recipe
            && self.overlay == job.overlay;
        match &self.out {
            Shot::Pixels(_) => same,
            Shot::Frame {
                texture,
                generation,
                ..
            } => {
                // A cached frame kept no loupe samples.
                same && !job.samples
                    && self.clipping == job.clipping
                    && self.monitor == job.monitor
                    && self.navigator == job.navigator
                    && gpu.and_then(|g| g.generation(texture)) == Some(*generation)
            }
        }
    }
}
/// A presented frame's textures as the UI names them.
#[derive(Clone, Copy)]
struct Presented {
    id: egui::TextureId,
    size: [usize; 2],
    navigator: Option<(egui::TextureId, [usize; 2])>,
}
impl Presented {
    fn preview(self) -> Preview {
        Preview::Texture {
            id: self.id,
            size: self.size,
            navigator: self.navigator,
        }
    }
}
/// egui's names for the textures frames are presented into.
struct Textures {
    state: egui_wgpu::RenderState,
    ids: HashMap<wgpu::Texture, egui::TextureId>,
}
impl Textures {
    fn id(&mut self, texture: &wgpu::Texture) -> (egui::TextureId, [usize; 2]) {
        let size = [texture.width() as usize, texture.height() as usize];
        let id = *self.ids.entry(texture.clone()).or_insert_with(|| {
            self.state.renderer.write().register_native_texture(
                &self.state.device,
                &texture.create_view(&Default::default()),
                wgpu::FilterMode::Linear,
            )
        });
        (id, size)
    }
    /// Unregisters textures the GPU dropped; the UI shows newer frames by then.
    fn release(&mut self, textures: Vec<wgpu::Texture>) {
        for texture in textures {
            if let Some(id) = self.ids.remove(&texture) {
                self.state.renderer.write().free_texture(&id);
            }
        }
    }
}

/// The preview renderer: one thread, with a lane for the edit and one for Before, so
/// each keeps its latest job and the edit's goes first.
pub(crate) struct Renderer(Latest<RenderJob>);
impl Renderer {
    /// Stops rendering once the current job is done.
    pub(in crate::app) fn stop(&mut self) -> crate::app::task::Stopping {
        self.0.stop()
    }
    pub(crate) fn submit(&self, job: RenderJob) {
        let lane = match job.pane {
            Pane::After => 0,
            Pane::Before => 1,
        };
        self.0.submit_to(lane, job);
    }
}

/// A CPU-only renderer, for headless tests.
#[cfg(test)]
pub(crate) fn renderer(tx: Sender<Event>, ctx: egui::Context) -> Renderer {
    renderer_with_backend(tx, ctx, RenderBackend::Cpu)
}
pub(in crate::app) fn renderer_with_backend(
    tx: Sender<Event>,
    ctx: egui::Context,
    backend: RenderBackend,
) -> Renderer {
    let mut state = RendererState::default();
    Renderer(Latest::with_lanes(2, move |job: RenderJob| {
        let pane = job.pane;
        let id = job.id;
        let rendered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            render(&mut state, &backend, &tx, &ctx, job)
        }));
        if let Err(panic) = rendered {
            // Whatever the panic interrupted is suspect: start over from nothing.
            // The UI frees the presented textures once it stops drawing them.
            if let Some(Textures { state: gpu, ids }) = state.textures.take()
                && !ids.is_empty()
            {
                let ids = ids.into_values().collect();
                send(
                    &tx,
                    &ctx,
                    Event::RendererReset(RetiredTextures { state: gpu, ids }),
                );
            }
            state = RendererState::default();
            send(
                &tx,
                &ctx,
                Event::Failed {
                    id,
                    task: TaskKind::Render(pane),
                    error: format!("Rendering failed: {}", super::panic_message(&*panic)),
                },
            );
        }
    }))
}

/// Everything the renderer keeps between jobs, replaced as a whole after a panic.
#[derive(Default)]
struct RendererState {
    processor: Option<develop::PreviewRenderer>,
    textures: Option<Textures>,
    /// The monitor profile as the GPU applies it, per profile path.
    lut: Option<(PathBuf, Result<Arc<gpu::MonitorLut>, String>)>,
    /// What each pane last showed: the edit's, and Before's.
    after: PaneState,
    before: PaneState,
}
/// What the renderer keeps between one pane's jobs.
#[derive(Default)]
struct PaneState {
    /// The last finished Fit and 100% region, so switching back to a view with the same
    /// edit shows its sharp image at once instead of rendering it again.
    fit: Option<Shown>,
    zoomed: Option<Shown>,
    /// Whether the last finished image was a 100% region: then the region is being
    /// edited or panned, and a reduced preview comes first.
    showing_region: bool,
    /// Whether the last full 100% region was presented on the GPU quickly enough that a
    /// reduced preview before it would only add work and a blurry frame.
    quick_region: bool,
    /// The whole photo's histogram for the last edit shown at 100%, so panning
    /// there does not render the whole photo again.
    whole_shown: Option<(Arc<crate::camera_data::CameraImage>, Recipe, Histogram)>,
}

fn render(
    state: &mut RendererState,
    backend: &RenderBackend,
    tx: &Sender<Event>,
    ctx: &egui::Context,
    job: RenderJob,
) {
    let RendererState {
        processor,
        textures,
        lut,
        after,
        before,
    } = state;
    let PaneState {
        fit,
        zoomed,
        showing_region,
        quick_region,
        whole_shown,
    } = match job.pane {
        Pane::After => after,
        Pane::Before => before,
    };
    let (whole_slot, region_slot) = match job.pane {
        Pane::After => (gpu::Slot::Whole, gpu::Slot::Region),
        Pane::Before => (gpu::Slot::BeforeWhole, gpu::Slot::BeforeRegion),
    };
    {
        let processor = processor.get_or_insert_with(|| match backend {
            #[cfg(test)]
            RenderBackend::Cpu => develop::PreviewRenderer::default(),
            RenderBackend::Gpu(None) => develop::PreviewRenderer::with_gpu(),
            RenderBackend::Gpu(Some(state)) => {
                *textures = Some(Textures {
                    state: state.clone(),
                    ids: HashMap::new(),
                });
                develop::PreviewRenderer::with_processor(gpu::Processor::with_device(
                    state.device.clone(),
                    state.queue.clone(),
                    &state.adapter.get_info(),
                ))
            }
        });
        let t = Instant::now();
        let mut warning = String::new();
        let monitor = match (&job.monitor, textures.is_some()) {
            (Some(path), true) => {
                if lut.as_ref().is_none_or(|(p, _)| p != path) {
                    let built = gpu::MonitorLut::sample(|rgb| raw::display_transform(path, rgb))
                        .map(Arc::new)
                        .map_err(|e| e.to_string());
                    *lut = Some((path.clone(), built));
                }
                match &lut.as_ref().unwrap().1 {
                    Ok(lut) => Some(lut.clone()),
                    Err(e) => {
                        warning = format!(" • ICC failed: {e}");
                        None
                    }
                }
            }
            _ => None,
        };
        // Overlays are drawn into CPU pixels.
        let overlay = job.overlay != super::Overlay::None;
        let drawn: Vec<wgpu::Texture> = textures.as_ref().map_or_else(Vec::new, |t| {
            t.ids
                .iter()
                .filter(|(_, id)| job.drawn.contains(id))
                .map(|(texture, _)| texture.clone())
                .collect()
        });
        let display = |slot, navigator: bool, thumbnail: bool| {
            (textures.is_some() && !overlay).then(|| gpu::Display {
                slot,
                clipping: job.clipping,
                monitor: monitor.clone(),
                navigator: navigator.then_some(NAVIGATOR),
                thumbnail: thumbnail.then_some(THUMBNAIL),
                samples: job.samples,
                drawn: drawn.clone(),
            })
        };
        let whole = display(whole_slot, job.navigator, job.thumbnail);
        let zoomed_display = display(region_slot, false, false);
        let status = |stage: RenderStage, backend: &str, warning: &str| {
            format!(
                "{} • {backend} • {:.0} ms{warning}",
                stage.label(),
                t.elapsed().as_secs_f64() * 1000.
            )
        };
        // CPU pixels: display bytes, overlays and reduced copies here, off the UI thread.
        let publish_pixels = |out: crate::rendered::Rendered, stage: RenderStage, gpu: bool| {
            if job.cancel.load(Ordering::Relaxed) {
                return;
            }
            let mut rgb = out.rgb8();
            let samples = job
                .samples
                .then(|| image::RgbImage::from_raw(out.width, out.height, rgb.clone()))
                .flatten();
            let reduce = |rgb: &[u8], edge: u32| {
                let full = image::RgbImage::from_raw(out.width, out.height, rgb.to_vec())?;
                let k = (edge as f32 / out.width.max(out.height) as f32).min(1.);
                Some(image::imageops::thumbnail(
                    &full,
                    ((out.width as f32 * k) as u32).max(1),
                    ((out.height as f32 * k) as u32).max(1),
                ))
            };
            let thumbnail = (job.thumbnail && stage == RenderStage::Fit)
                .then(|| reduce(&rgb, THUMBNAIL))
                .flatten();
            let mut warning = String::new();
            if let Some(p) = &job.monitor
                && let Err(e) = raw::display_transform(p, &mut rgb)
            {
                warning = format!(" • ICC failed: {e}");
            }
            match job.overlay {
                super::Overlay::None => {}
                super::Overlay::Spots(threshold) => {
                    rgb = develop::retouch::visualize_spots(&out, threshold);
                }
                super::Overlay::Mask {
                    index,
                    color,
                    opacity,
                } => {
                    let weights = develop::masks::overlay_weights(
                        &job.image,
                        &job.recipe,
                        index,
                        &out,
                        job.region,
                    );
                    for (p, w) in rgb
                        .as_chunks_mut::<3>()
                        .0
                        .iter_mut()
                        .zip(weights.iter().flatten())
                    {
                        let a = w * opacity;
                        for c in 0..3 {
                            p[c] = (p[c] as f32 * (1. - a) + color[c] as f32 * a) as u8;
                        }
                    }
                }
            }
            job.clipping.paint(&mut rgb, &out.pixels);
            let navigator = (job.navigator && job.region.is_none())
                .then(|| reduce(&rgb, NAVIGATOR))
                .flatten();
            send(
                tx,
                ctx,
                Event::Rendered {
                    id: job.id,
                    pane: job.pane,
                    histogram: Box::new(out.histogram()),
                    preview: Preview::Pixels {
                        image: out,
                        display_rgb: rgb,
                        navigator,
                    },
                    thumbnail,
                    samples,
                    stage,
                    status: status(stage, if gpu { "GPU finish" } else { "CPU" }, &warning),
                },
            );
        };
        let result = (|| -> anyhow::Result<()> {
            if job.cancel.load(Ordering::Relaxed) {
                return Ok(());
            }
            let stage = if job.region.is_some() {
                RenderStage::Region
            } else {
                RenderStage::Fit
            };
            let mut publish = |out: Output, stage: RenderStage, cache: bool, gpu: bool| match out {
                Output::Pixels(out) => {
                    publish_pixels(out.clone(), stage, gpu);
                    Some(Shot::Pixels(out))
                }
                Output::Frame(frame) => {
                    let textures = textures.as_mut()?;
                    textures.release(frame.released);
                    let (id, size) = textures.id(&frame.texture);
                    let presented = Presented {
                        id,
                        size,
                        navigator: frame.navigator.as_ref().map(|t| textures.id(t)),
                    };
                    if !job.cancel.load(Ordering::Relaxed) {
                        send(
                            tx,
                            ctx,
                            Event::Rendered {
                                id: job.id,
                                pane: job.pane,
                                preview: presented.preview(),
                                histogram: frame.histogram.clone(),
                                thumbnail: frame
                                    .thumbnail
                                    .and_then(|(w, h, rgb)| image::RgbImage::from_raw(w, h, rgb)),
                                samples: frame
                                    .samples
                                    .and_then(|(w, h, rgb)| image::RgbImage::from_raw(w, h, rgb)),
                                stage,
                                status: status(stage, "GPU", &warning),
                            },
                        );
                    }
                    cache.then_some(Shot::Frame {
                        texture: frame.texture,
                        generation: frame.generation,
                        preview: presented,
                        histogram: frame.histogram,
                    })
                }
            };
            let cached = if job.region.is_some() {
                &*zoomed
            } else {
                &*fit
            };
            if let Some(shown) = cached.as_ref().filter(|s| s.matches(&job, processor.gpu())) {
                match &shown.out {
                    Shot::Pixels(out) => publish_pixels(out.clone(), stage, false),
                    Shot::Frame {
                        preview, histogram, ..
                    } => send(
                        tx,
                        ctx,
                        Event::Rendered {
                            id: job.id,
                            pane: job.pane,
                            preview: preview.preview(),
                            histogram: histogram.clone(),
                            thumbnail: None,
                            samples: None,
                            stage,
                            status: status(stage, "GPU", &warning),
                        },
                    ),
                }
                *showing_region = job.region.is_some();
                if job.region.is_some() && job.pane == Pane::After {
                    whole_histogram(&job, processor, fit, whole_shown, tx, ctx)?;
                }
                return Ok(());
            }
            if let Some(region) = job.region {
                let out = {
                    // While editing at 100%, a reduced preview keeps sliders responsive;
                    // a newer job cancels the full-resolution render that follows.
                    // Zooming in goes straight to the full region, over the enlarged Fit.
                    if *showing_region && !*quick_region {
                        let out = processor.render_region_preview_to(
                            &job.image,
                            &job.recipe,
                            region,
                            &job.cancel,
                            zoomed_display.as_ref(),
                        )?;
                        let gpu = processor.used_gpu();
                        publish(out, RenderStage::Draft, false, gpu);
                    }
                    let started = Instant::now();
                    let out = processor.render_to(
                        &job.image,
                        &job.recipe,
                        0,
                        Some(region),
                        &job.cancel,
                        zoomed_display.as_ref(),
                    )?;
                    *quick_region =
                        matches!(out, Output::Frame(_)) && started.elapsed() < QUICK_REGION;
                    out
                };
                let gpu = processor.used_gpu();
                *zoomed =
                    publish(out, RenderStage::Region, true, gpu).map(|out| Shown::new(&job, out));
                *showing_region = true;
                // Only the edit's histogram is shown.
                if job.pane == Pane::After {
                    whole_histogram(&job, processor, fit, whole_shown, tx, ctx)?;
                }
                return Ok(());
            }
            let out = processor.render_to(
                &job.image,
                &job.recipe,
                job.max_edge,
                None,
                &job.cancel,
                whole.as_ref(),
            )?;
            let gpu = processor.used_gpu();
            *fit = publish(out, RenderStage::Fit, true, gpu).map(|out| Shown::new(&job, out));
            *showing_region = false;
            Ok(())
        })();
        if let Err(e) = result
            && !job.cancel.load(Ordering::Relaxed)
        {
            send(
                tx,
                ctx,
                Event::Failed {
                    id: job.id,
                    task: TaskKind::Render(job.pane),
                    error: e.to_string(),
                },
            );
        }
    }
}

type Histogram = Box<crate::rendered::Histogram>;

/// At 100% the view shows a region, but the histogram describes the whole
/// photo, as Lightroom's does: from the Fit render of the same edit when there
/// is one, and otherwise from a render at the Fit's size, which reuses the Fit
/// view's cached stages.
fn whole_histogram(
    job: &RenderJob,
    processor: &mut develop::PreviewRenderer,
    fit: &Option<Shown>,
    whole: &mut Option<(Arc<crate::camera_data::CameraImage>, Recipe, Histogram)>,
    tx: &Sender<Event>,
    ctx: &egui::Context,
) -> anyhow::Result<()> {
    let same = |image: &Arc<crate::camera_data::CameraImage>, recipe: &Recipe| {
        Arc::ptr_eq(image, &job.image) && *recipe == job.recipe
    };
    let histogram = if let Some((.., histogram)) = whole.as_ref().filter(|(i, r, _)| same(i, r)) {
        histogram.clone()
    } else if let Some(shown) = fit.as_ref().filter(|s| same(&s.image, &s.recipe)) {
        match &shown.out {
            Shot::Pixels(out) => Box::new(out.histogram()),
            Shot::Frame { histogram, .. } => histogram.clone(),
        }
    } else {
        let out = processor.render_to(
            &job.image,
            &job.recipe,
            job.max_edge,
            None,
            &job.cancel,
            None,
        )?;
        Box::new(out.pixels().histogram())
    };
    *whole = Some((job.image.clone(), job.recipe.clone(), histogram.clone()));
    if !job.cancel.load(Ordering::Relaxed) {
        send(
            tx,
            ctx,
            Event::Histogram {
                id: job.id,
                histogram,
            },
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera_data::{CameraImage, Metadata};
    use std::sync::{Arc, atomic::AtomicBool};
    fn image() -> Arc<CameraImage> {
        let (w, h) = (240, 160);
        Arc::new(CameraImage {
            recovered: Default::default(),
            width: w,
            height: h,
            pixels: (0..w * h)
                .map(|i| {
                    let v = 0.2 + 0.1 * ((i % w) as f32 * 0.2).sin();
                    [v * 1.1, v, v * 0.8]
                })
                .collect(),
            metadata: Metadata {
                width: w,
                height: h,
                wb: [1.; 3],
                matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
                ..Default::default()
            },
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
        })
    }
    /// Renders one job and returns its published stages and final pixels.
    fn run(
        worker: &Renderer,
        rx: &std::sync::mpsc::Receiver<Event>,
        id: u64,
        image: &Arc<CameraImage>,
        recipe: &Recipe,
        region: Option<[u32; 4]>,
    ) -> Vec<(RenderStage, Vec<[f32; 3]>)> {
        run_in(worker, rx, Pane::After, id, image, recipe, region)
    }
    /// `run`, for `pane`.
    fn run_in(
        worker: &Renderer,
        rx: &std::sync::mpsc::Receiver<Event>,
        pane: Pane,
        id: u64,
        image: &Arc<CameraImage>,
        recipe: &Recipe,
        region: Option<[u32; 4]>,
    ) -> Vec<(RenderStage, Vec<[f32; 3]>)> {
        worker.submit(RenderJob {
            id,
            pane,
            image: image.clone(),
            max_edge: 60,
            cancel: Arc::new(AtomicBool::new(false)),
            recipe: recipe.clone(),
            region,
            monitor: None,
            clipping: crate::rendered::ClipOverlay::NONE,
            navigator: region.is_none(),
            thumbnail: false,
            samples: false,
            overlay: Default::default(),
            drawn: Vec::new(),
        });
        let mut stages = Vec::new();
        loop {
            match rx.recv_timeout(std::time::Duration::from_secs(20)).unwrap() {
                Event::Rendered {
                    preview: Preview::Pixels { image, .. },
                    stage,
                    id: i,
                    pane: p,
                    ..
                } if i == id && p == pane => {
                    stages.push((stage, image.pixels));
                    if stage != RenderStage::Draft {
                        return stages;
                    }
                }
                Event::Failed { error, .. } => panic!("{error}"),
                _ => {}
            }
        }
    }
    #[test]
    fn switching_views_shows_the_previous_images_without_drafts() {
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = renderer(tx, egui::Context::default());
        let image = image();
        let mut recipe = Recipe::default();
        let region = Some([10, 10, 80, 60]);
        let fit = run(&worker, &rx, 1, &image, &recipe, None);
        assert_eq!(fit.len(), 1);
        // Zooming in renders the full region directly.
        let zoomed = run(&worker, &rx, 2, &image, &recipe, region);
        assert_eq!(zoomed.len(), 1);
        // Back to Fit and 100% again: the same images, no drafts.
        assert_eq!(run(&worker, &rx, 3, &image, &recipe, None), fit);
        assert_eq!(run(&worker, &rx, 4, &image, &recipe, region), zoomed);
        // Editing at 100% shows a reduced preview first.
        recipe.exposure = 0.5;
        let edited = run(&worker, &rx, 5, &image, &recipe, region);
        assert_eq!(edited.len(), 2);
        assert_eq!(edited[0].0, RenderStage::Draft);
        // Back to Fit after the edit: the new Fit, without drafts; the viewport keeps
        // showing its previous Fit under the region until then.
        let back = run(&worker, &rx, 6, &image, &recipe, None);
        assert_eq!(back.len(), 1);
        assert_ne!(back[0].1, fit[0].1);
    }
    #[test]
    fn before_renders_beside_the_edit_keep_the_edits_views() {
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = renderer(tx, egui::Context::default());
        let image = image();
        let mut recipe = Recipe::default();
        let before = Recipe {
            exposure: -1.,
            ..Default::default()
        };
        let region = Some([10, 10, 80, 60]);
        run(&worker, &rx, 1, &image, &recipe, region);
        // Before at 100%, then at Fit, between the edit's renders.
        run_in(&worker, &rx, Pane::Before, 1, &image, &before, region);
        let shown = run_in(&worker, &rx, Pane::Before, 2, &image, &before, None);
        // The edit is still being edited at 100%: a reduced preview first, as alone.
        recipe.exposure = 0.5;
        let edited = run(&worker, &rx, 2, &image, &recipe, region);
        assert_eq!(edited[0].0, RenderStage::Draft);
        // And Before's Fit is still its own, without drafts.
        assert_eq!(
            run_in(&worker, &rx, Pane::Before, 3, &image, &before, None),
            shown
        );
    }
    #[test]
    fn a_panicking_render_fails_and_the_next_one_succeeds() {
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = renderer(tx, egui::Context::default());
        let recipe = Recipe::default();
        // Fewer pixels than its size says: indexing it panics.
        let mut broken = (*image()).clone();
        broken.pixels.truncate(10);
        worker.submit(RenderJob {
            id: 1,
            pane: Pane::After,
            image: Arc::new(broken),
            max_edge: 60,
            cancel: Arc::new(AtomicBool::new(false)),
            recipe: recipe.clone(),
            region: None,
            monitor: None,
            clipping: crate::rendered::ClipOverlay::NONE,
            navigator: true,
            thumbnail: false,
            samples: false,
            overlay: Default::default(),
            drawn: Vec::new(),
        });
        loop {
            match rx.recv_timeout(std::time::Duration::from_secs(20)).unwrap() {
                Event::Failed { id: 1, task, .. } => {
                    assert_eq!(task, TaskKind::Render(Pane::After));
                    break;
                }
                Event::Rendered { id: 1, .. } => panic!("the broken image rendered"),
                _ => {}
            }
        }
        assert_eq!(run(&worker, &rx, 2, &image(), &recipe, None).len(), 1);
    }
    #[test]
    fn the_histogram_describes_the_whole_photo_at_100_percent() {
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = renderer(tx, egui::Context::default());
        let image = image();
        let mut recipe = Recipe::default();
        let region = Some([10, 10, 80, 60]);
        // The histogram each job ends with, as the app keeps it.
        let histogram = |id: u64, recipe: &Recipe, region: Option<[u32; 4]>| {
            worker.submit(RenderJob {
                id,
                pane: Pane::After,
                image: image.clone(),
                max_edge: 60,
                cancel: Arc::new(AtomicBool::new(false)),
                recipe: recipe.clone(),
                region,
                monitor: None,
                clipping: crate::rendered::ClipOverlay::NONE,
                navigator: region.is_none(),
                thumbnail: false,
                samples: false,
                overlay: Default::default(),
                drawn: Vec::new(),
            });
            loop {
                match rx.recv_timeout(std::time::Duration::from_secs(20)).unwrap() {
                    Event::Rendered {
                        id: i,
                        stage,
                        histogram,
                        ..
                    } if i == id && region.is_none() && stage != RenderStage::Draft => {
                        return histogram;
                    }
                    Event::Histogram { id: i, histogram } if i == id => return histogram,
                    Event::Failed { error, .. } => panic!("{error}"),
                    _ => {}
                }
            }
        };
        let fit = histogram(1, &recipe, None);
        assert_eq!(histogram(2, &recipe, region), fit);
        // Edited at 100%: the whole photo's new histogram, as Fit then shows it.
        recipe.exposure = 0.5;
        let edited = histogram(3, &recipe, region);
        assert_ne!(edited, fit);
        assert_eq!(histogram(4, &recipe, None), edited);
    }
}
