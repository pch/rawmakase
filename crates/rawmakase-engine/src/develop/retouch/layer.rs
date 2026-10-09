//! The retouched camera image: the highlight-recovered image with every red eye
//! correction, then every Heal and Clone operation, applied in order. Previews keep it
//! between renders and, when operations change, recompute only the 256-pixel tiles
//! those changes reach; exports build it at once.
use super::{
    RetouchOp,
    heal::{self, PixelRect},
};
use crate::model::image_frame::ImageFrame;
use crate::{
    camera_data::CameraImage,
    develop::{color_noise::ChromaDenoise, red_eye},
    model::red_eye::RedEyeOp,
};
use anyhow::{Result, ensure};
use std::{
    collections::BTreeSet,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, Ordering},
    },
};

const TILE: i32 = 256;
/// The soft edge Heal and Clone render with: Camera Raw's.
const FEATHER: heal::FeatherProfile = heal::FeatherProfile::Measured;

/// The operations that change the camera image's pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Retouching<'a> {
    pub(crate) red_eye: &'a [RedEyeOp],
    pub(crate) retouch: &'a [RetouchOp],
}
impl<'a> Retouching<'a> {
    pub(crate) fn of(r: &'a crate::model::recipe::Recipe) -> Self {
        Self {
            red_eye: &r.red_eye,
            retouch: &r.retouch,
        }
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.red_eye.is_empty() && self.retouch.is_empty()
    }
    /// Every operation placed on one image, in the order they apply: red eye first, so
    /// a heal copying from an eye copies the corrected pupil.
    fn steps(&self, frame: &ImageFrame) -> Vec<Step> {
        let eyes = self
            .red_eye
            .iter()
            .map(|op| Step::Eye(red_eye::Placed::new(op, frame)));
        let heals = self
            .retouch
            .iter()
            .map(|op| Step::Heal(heal::Placed::new(op, frame, FEATHER)));
        eyes.chain(heals).collect()
    }
    /// Destination rectangles of the operations that differ between `self` and
    /// `other`, compared position by position within each list.
    fn changed(&self, other: &Retouching, frame: &ImageFrame) -> Vec<PixelRect> {
        fn diff<T: PartialEq>(a: &[T], b: &[T], dest: impl Fn(&T) -> PixelRect) -> Vec<PixelRect> {
            (0..a.len().max(b.len()))
                .filter(|i| a.get(*i) != b.get(*i))
                .flat_map(|i| {
                    [a.get(i), b.get(i)]
                        .into_iter()
                        .flatten()
                        .map(&dest)
                        .collect::<Vec<_>>()
                })
                .collect()
        }
        let mut rects = diff(self.red_eye, other.red_eye, |op| {
            red_eye::Placed::new(op, frame).dest()
        });
        let dest = |op: &RetouchOp| heal::Placed::new(op, frame, FEATHER).dest();
        rects.extend(diff(self.retouch, other.retouch, dest));
        rects
    }
}
/// One operation placed on an image.
enum Step {
    Eye(red_eye::Placed),
    Heal(heal::Placed),
}
impl Step {
    /// Pixels the operation writes.
    fn dest(&self) -> PixelRect {
        match self {
            Step::Eye(p) => p.dest(),
            Step::Heal(p) => p.dest(),
        }
    }
    /// Pixels the operation reads.
    fn reads(&self) -> [PixelRect; 2] {
        match self {
            // Each pixel is corrected from its own value.
            Step::Eye(p) => [p.dest(); 2],
            Step::Heal(p) => p.reads(),
        }
    }
    fn apply(&self, im: &mut CameraImage) {
        match self {
            Step::Eye(p) => p.apply(im),
            Step::Heal(p) => {
                heal::apply(im, p);
            }
        }
    }
}

/// `base` with the operations applied, built from scratch (exports).
pub(crate) fn apply(base: &CameraImage, ops: Retouching) -> CameraImage {
    let frame = ImageFrame::new(base);
    let mut out = base.clone();
    out.recovered = Default::default();
    for step in ops.steps(&frame) {
        step.apply(&mut out);
    }
    out
}
/// Tiles, as (column, row), covering a pixel rectangle.
fn tiles(rect: &PixelRect) -> impl Iterator<Item = (i32, i32)> + use<> {
    let (x0, y0) = (rect[0].max(0) / TILE, rect[1].max(0) / TILE);
    let (x1, y1) = ((rect[2] - 1).max(0) / TILE, (rect[3] - 1).max(0) / TILE);
    (y0..=y1).flat_map(move |y| (x0..=x1).map(move |x| (x, y)))
}
fn touches(dirty: &BTreeSet<(i32, i32)>, rect: &PixelRect) -> bool {
    rect[2] > rect[0] && rect[3] > rect[1] && tiles(rect).any(|t| dirty.contains(&t))
}
/// Tiles that must be recomputed when `before` becomes `after`: those of changed
/// operations, then, until nothing changes, those written by any operation that reads
/// or writes a dirty tile. Recomputing exactly these tiles, and rerunning the
/// operations that touch them in order, gives the same image as starting over.
pub(crate) fn dirty_tiles(
    frame: &ImageFrame,
    before: Retouching,
    after: Retouching,
) -> BTreeSet<(i32, i32)> {
    let mut dirty = BTreeSet::new();
    for rect in before.changed(&after, frame) {
        dirty.extend(tiles(&rect));
    }
    let placed = after.steps(frame);
    loop {
        let size = dirty.len();
        for p in &placed {
            if p.reads().iter().any(|r| touches(&dirty, r)) {
                dirty.extend(tiles(&p.dest()));
            }
        }
        if dirty.len() == size {
            return dirty;
        }
    }
}
/// Pixel rectangles of `tiles`, clipped to a `width` × `height` image.
fn tile_rects(tiles: &BTreeSet<(i32, i32)>, width: u32, height: u32) -> Vec<PixelRect> {
    tiles
        .iter()
        .map(|(x, y)| {
            [
                x * TILE,
                y * TILE,
                ((x + 1) * TILE).min(width as i32),
                ((y + 1) * TILE).min(height as i32),
            ]
        })
        .filter(|r| r[2] > r[0] && r[3] > r[1])
        .collect()
}
/// The preview's retouched image, updated incrementally.
#[derive(Default)]
pub(crate) struct RetouchCache {
    base: Option<Arc<CameraImage>>,
    ops: Vec<RetouchOp>,
    red_eye: Vec<RedEyeOp>,
    image: Option<Arc<CameraImage>>,
    /// The previous image (weakly, so its pixels are freed) and the rectangles where
    /// the current one differs from it.
    change: Option<(Weak<CameraImage>, Vec<PixelRect>)>,
    /// The last Color noise reduction: its source, settings and result.
    denoised: Option<(Arc<CameraImage>, ChromaDenoise, Arc<CameraImage>)>,
}
impl RetouchCache {
    /// `base` with `ops` applied. The previous result is reused where no change
    /// reaches it.
    pub(crate) fn get(
        &mut self,
        base: &Arc<CameraImage>,
        ops: Retouching,
        cancel: &AtomicBool,
    ) -> Result<Arc<CameraImage>> {
        let same_base = self.base.as_ref().is_some_and(|b| Arc::ptr_eq(b, base));
        let previous = self.image.clone().unwrap_or_else(|| base.clone());
        let previous_ops = if same_base {
            Retouching {
                red_eye: &self.red_eye,
                retouch: &self.ops,
            }
        } else {
            Retouching::default()
        };
        if same_base && previous_ops == ops {
            return Ok(previous);
        }
        let frame = ImageFrame::new(base);
        let dirty = dirty_tiles(&frame, previous_ops, ops);
        let rects = tile_rects(&dirty, base.width, base.height);
        let image = if ops.is_empty() {
            base.clone()
        } else {
            let start = if same_base { &previous } else { base };
            let mut out = CameraImage::clone(start);
            out.recovered = Default::default();
            // Back to the recovered pixels in dirty tiles, then rerun what reaches them.
            let width = base.width as usize;
            for r in &rects {
                for y in r[1]..r[3] {
                    let row = y as usize * width;
                    let (a, b) = (row + r[0] as usize, row + r[2] as usize);
                    out.pixels[a..b].copy_from_slice(&base.pixels[a..b]);
                }
            }
            for step in ops.steps(&frame) {
                ensure!(!cancel.load(Ordering::Relaxed), "Render superseded");
                if step.reads().iter().any(|r| touches(&dirty, r)) {
                    step.apply(&mut out);
                }
            }
            Arc::new(out)
        };
        // Another photo's image shares nothing with this one.
        self.change = same_base.then(|| (Arc::downgrade(&previous), rects));
        self.base = Some(base.clone());
        self.ops = ops.retouch.to_vec();
        self.red_eye = ops.red_eye.to_vec();
        self.image = Some(image.clone());
        Ok(image)
    }
    /// `source` with Color noise reduction `d`, reused while neither changes; without
    /// one, `source` itself, and the last result is let go.
    pub(crate) fn denoised(
        &mut self,
        source: &Arc<CameraImage>,
        d: Option<ChromaDenoise>,
        cancel: &AtomicBool,
    ) -> Result<Arc<CameraImage>> {
        let Some(d) = d else {
            self.denoised = None;
            return Ok(source.clone());
        };
        if let Some((s, last, out)) = &self.denoised
            && Arc::ptr_eq(s, source)
            && *last == d
        {
            return Ok(out.clone());
        }
        // Free the previous result, and the retouched image built on it, before making
        // the next; spot removal is then rebuilt on the new one.
        self.denoised = None;
        self.base = None;
        self.image = None;
        self.change = None;
        let out = Arc::new(d.apply(source, cancel)?);
        self.denoised = Some((source.clone(), d, out.clone()));
        Ok(out)
    }
    /// What the last change replaced, and where the images differ.
    /// Whether `image` is what the last change replaced; then the current image
    /// differs from it only in the returned rectangles.
    pub(crate) fn changed_from(&self, image: &Arc<CameraImage>) -> Option<&[PixelRect]> {
        let (previous, rects) = self.change.as_ref()?;
        std::ptr::eq(previous.as_ptr(), Arc::as_ptr(image)).then_some(rects.as_slice())
    }
}
