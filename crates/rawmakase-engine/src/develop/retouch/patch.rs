//! AI Remove patches as the renderer reads them: resolved from the asset store
//! (`storage::mask_assets::resolve_patch`, which checks they are patches),
//! checked against the size their spot describes, and decoded from half
//! precision once while something renders them.
use crate::model::retouch::{RetouchOp, RetouchPatch};
use anyhow::Result;
use rawmakase_model::storage::mask_assets::{self, AssetError};
use std::sync::{Arc, Mutex, OnceLock};

/// A patch's pixels in linear camera RGB, upright over `rect` of the image frame.
pub(crate) struct PatchPixels {
    width: u32,
    height: u32,
    rect: [f32; 4],
    pixels: Vec<[f32; 3]>,
}
impl PatchPixels {
    /// The patch at image-frame position `p`, interpolated between pixel
    /// centres (`(i + 0.5) / width` across `rect`), held at its edge outside.
    pub(crate) fn sample(&self, p: [f32; 2]) -> [f32; 3] {
        let [x0, y0, x1, y1] = self.rect;
        let (w, h) = (self.width as usize, self.height as usize);
        let x = ((p[0] - x0) / (x1 - x0) * w as f32 - 0.5).clamp(0., (w - 1) as f32);
        let y = ((p[1] - y0) / (y1 - y0) * h as f32 - 0.5).clamp(0., (h - 1) as f32);
        let (ix, iy) = (x as usize, y as usize);
        let (jx, jy) = ((ix + 1).min(w - 1), (iy + 1).min(h - 1));
        let (fx, fy) = (x - ix as f32, y - iy as f32);
        let at = |x: usize, y: usize| self.pixels[y * w + x];
        let (a, b, c, d) = (at(ix, iy), at(jx, iy), at(ix, jy), at(jx, jy));
        std::array::from_fn(|i| {
            (a[i] * (1. - fx) + b[i] * fx) * (1. - fy) + (c[i] * (1. - fx) + d[i] * fx) * fy
        })
    }
}

/// How many decoded patches are kept for the next render: a few spots' worth
/// (at most 48 MiB each), so dragging a slider doesn't decode them again.
const KEPT: usize = 4;

/// Decoded patches by the reference that placed them (content id, size and
/// rect), the most recently used first.
type Kept = Mutex<Vec<(RetouchPatch, Arc<PatchPixels>)>>;

fn kept() -> &'static Kept {
    static KEPT_PATCHES: OnceLock<Kept> = OnceLock::new();
    KEPT_PATCHES.get_or_init(Default::default)
}

/// The pixels of `patch`, or why they can't be provided: missing, damaged, not
/// a patch, or not the size the spot describes. Never an empty patch.
pub(crate) fn resolve(patch: &RetouchPatch) -> Result<Arc<PatchPixels>, AssetError> {
    let kept = kept();
    {
        let list = kept.lock().unwrap_or_else(|e| e.into_inner());
        // The whole reference: the same pixels laid elsewhere, or described at
        // another size, are another placement (or a damaged reference).
        if let Some((_, p)) = list.iter().find(|(kept, _)| placed_alike(kept, patch)) {
            return Ok(p.clone());
        }
    }
    mask_assets::ensure_patches([(patch.id.as_str(), patch.width, patch.height)])?;
    let bitmap = mask_assets::resolve_patch(&patch.id)?;
    let pixels = rawmakase_model::storage::patches::decode(&bitmap)
        .map_err(|e| AssetError::PatchCorrupt(patch.id.clone(), e.to_string()))?;
    let decoded = Arc::new(PatchPixels {
        width: patch.width,
        height: patch.height,
        rect: patch.rect,
        pixels,
    });
    let mut list = kept.lock().unwrap_or_else(|e| e.into_inner());
    list.retain(|(kept, _)| !placed_alike(kept, patch));
    list.insert(0, (patch.clone(), decoded.clone()));
    list.truncate(KEPT);
    Ok(decoded)
}

/// Whether `a` and `b` lay the same pixels the same way.
fn placed_alike(a: &RetouchPatch, b: &RetouchPatch) -> bool {
    a.id == b.id
        && (a.width, a.height) == (b.width, b.height)
        && a.rect.map(f32::to_bits) == b.rect.map(f32::to_bits)
}

/// Checks that every patch of `ops` can be provided, as rendering will need:
/// what the preview, export and Library previews check before they render.
pub fn ensure(ops: &[RetouchOp]) -> Result<(), AssetError> {
    for patch in ops.iter().filter_map(|op| op.patch.as_ref()) {
        resolve(patch)?;
    }
    Ok(())
}
