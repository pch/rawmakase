//! Pairs of a photo's render and its embedded JPEG: which preview may be used,
//! its colour, its alignment with the render and the pixels a fit may not learn
//! from.
use super::{PROXY_EDGE, RENDER_FACTOR, Refusal, Rgb, refuse};
use crate::raster::PreviewSpace;
use rawmakase_cancel::Cancel;
use rawmakase_interop::exif::ColourFacts;
use rawmakase_model::color::srgb_encode;
use rayon::prelude::*;
use serde::Serialize;

/// The largest camera-written JPEG preview, upright, as linear ProPhoto RGB.
pub(super) struct Preview {
    pub width: u32,
    pub height: u32,
    pub pro: Vec<Rgb>,
    /// A channel at the encoding's ends (≥ 0.98 or ≤ 0.004): clipped by the camera.
    pub clipped: Vec<bool>,
    pub space: &'static str,
}

/// The colour space an embedded preview's file states for it, and whether it is a DNG
/// Adobe software wrote.
struct Stated {
    adobe_dng: bool,
    icc: Option<Vec<u8>>,
    named: Option<(PreviewSpace<'static>, &'static str)>,
}
impl Stated {
    fn space(&self) -> Option<(PreviewSpace<'_>, &'static str)> {
        match &self.icc {
            Some(icc) => Some((PreviewSpace::Icc(icc), "icc")),
            None => self.named.as_ref().map(|(space, name)| {
                let space = match space {
                    PreviewSpace::AdobeRgb => PreviewSpace::AdobeRgb,
                    _ => PreviewSpace::Srgb,
                };
                (space, *name)
            }),
        }
    }
}

fn stated_space(path: &std::path::Path, raw: &crate::raw::Raw, bytes: &[u8]) -> Stated {
    let raw_facts = rawmakase_interop::exif::colour_facts(path)
        .or_else(|| {
            // A CR3's EXIF is in boxes of its own: ColorSpace from there.
            let exif = crate::photo::camera_exif(path)?;
            let field = exif.get(rawmakase_interop::exif::tag::COLOR_SPACE)?;
            Some(ColourFacts {
                color_space: field.number().map(|v| v as u16),
                ..Default::default()
            })
        })
        .unwrap_or_default();
    let adobe_dng = raw.metadata.baseline_exposure.is_some()
        && raw_facts
            .software
            .as_deref()
            .is_some_and(|s| s.trim_start().to_ascii_lowercase().starts_with("adobe"));
    let facts = rawmakase_interop::exif::colour_facts_of_jpeg(bytes)
        .unwrap_or_default()
        .or(raw_facts);
    let icc = image::codecs::jpeg::JpegDecoder::new(std::io::Cursor::new(bytes))
        .ok()
        .and_then(|mut d| image::ImageDecoder::icc_profile(&mut d).ok().flatten());
    let named = match (facts.color_space, facts.interop_index.as_deref()) {
        (Some(1), _) => Some((PreviewSpace::Srgb, "srgb")),
        (Some(0xffff), Some(index)) if index.eq_ignore_ascii_case("R03") => {
            Some((PreviewSpace::AdobeRgb, "adobe-rgb"))
        }
        // Nikon writes no EXIF ColorSpace and Olympus "uncalibrated" for Adobe RGB:
        // their maker notes say it.
        _ => match raw.maker_preview_space() {
            Some(crate::raw::MakerPreviewSpace::Srgb) => {
                Some((PreviewSpace::Srgb, "srgb-maker-note"))
            }
            Some(crate::raw::MakerPreviewSpace::AdobeRgb) => {
                Some((PreviewSpace::AdobeRgb, "adobe-rgb-maker-note"))
            }
            None => None,
        },
    };
    Stated {
        adobe_dng,
        icc,
        named,
    }
}

/// The embedded preview of the RAW at `path`, upright, as linear ProPhoto RGB in the
/// colour space its file states, whatever a fit would make of it; `None` when none is
/// stated, so it can only be taken as it is.
pub(super) fn preview_pro(
    path: &std::path::Path,
    raw: &mut crate::raw::Raw,
    image: &image::RgbImage,
) -> Option<Vec<Rgb>> {
    let bytes = raw.thumbnail().ok()?;
    if crate::raw::is_jpeg_xl(&bytes) {
        return None;
    }
    let stated = stated_space(path, raw, &bytes);
    let (space, _) = stated.space()?;
    crate::raster::rgb8_linear_pro_photo(image, space).ok()
}

/// The preview of the RAW at `path`, or why it can't be used: not a JPEG (JPEG XL
/// previews are Lightroom's), too small, written by Adobe software into a DNG (its
/// preview is Adobe's render), or of a colour space the file does not state.
pub(super) fn preview(
    path: &std::path::Path,
    raw: &mut crate::raw::Raw,
) -> Result<Preview, Refusal> {
    let bytes = raw
        .thumbnail()
        .map_err(|e| refuse(format!("No embedded preview: {e}")))?;
    if crate::raw::is_jpeg_xl(&bytes) {
        return Err(refuse(
            "The embedded preview is JPEG XL, not the camera's JPEG",
        ));
    }
    let stated = stated_space(path, raw, &bytes);
    if stated.adobe_dng {
        return Err(refuse(
            "A DNG written by Adobe software: its preview is Adobe's render",
        ));
    }
    let (space, name) = stated
        .space()
        .ok_or_else(|| refuse("The preview's colour space is not stated"))?;
    let image = crate::raw::thumbnail(raw).map_err(|e| refuse(format!("{e:#}")))?;
    let (width, height) = image.dimensions();
    if width.max(height) < 2 * PROXY_EDGE {
        return Err(refuse(format!("The preview is only {width} × {height}")));
    }
    let pro = crate::raster::rgb8_linear_pro_photo(&image, space)
        .map_err(|e| refuse(format!("{e:#}")))?;
    let clipped = image
        .pixels()
        .map(|p| p.0.iter().any(|v| *v >= 250 || *v <= 1))
        .collect();
    Ok(Preview {
        width,
        height,
        pro,
        clipped,
        space: name,
    })
}

/// The preview cut centrally to the photo's aspect ratio (`width` × `height`) when
/// they differ: cameras that crop to an aspect ratio in camera embed either the
/// whole frame around it (Canon) or the crop letterboxed in the sensor's shape
/// (Panasonic), both centred. Whether it was cut; alignment then checks it.
pub(super) fn to_aspect(p: Preview, width: u32, height: u32) -> (Preview, bool) {
    let target = width as f32 / height as f32;
    let ratio = (p.width as f32 / p.height as f32) / target;
    if (ratio - 1.).abs() <= 0.02 {
        return (p, false);
    }
    let (w, h) = if ratio > 1. {
        (
            ((p.height as f32 * target).round() as u32).min(p.width),
            p.height,
        )
    } else {
        (
            p.width,
            ((p.width as f32 / target).round() as u32).min(p.height),
        )
    };
    let (x0, y0) = ((p.width - w) / 2, (p.height - h) / 2);
    let pick = |i: u32| ((y0 + i / w) * p.width + x0 + i % w) as usize;
    let cut = Preview {
        width: w,
        height: h,
        pro: (0..w * h).map(|i| p.pro[pick(i)]).collect(),
        clipped: (0..w * h).map(|i| p.clipped[pick(i)]).collect(),
        space: p.space,
    };
    (cut, true)
}

/// A photo's renders at the proxy size, from renders `RENDER_FACTOR` times larger
/// reduced in boxes.
pub(super) struct Proxy {
    pub width: u32,
    pub height: u32,
    /// Outside the photo, or not every sample of the box is.
    pub valid: Vec<bool>,
    /// A sample of the box reached 0.99 of the sensor's white.
    pub sensor_clipped: Vec<bool>,
    /// `Stage::CameraInput`.
    pub camera: Vec<Rgb>,
}

/// `pixels` (`width` wide) reduced by `RENDER_FACTOR` in boxes, NaN where any of a
/// box's samples is.
pub(super) fn reduce(pixels: &[Rgb], width: u32, height: u32) -> (u32, u32, Vec<Rgb>) {
    let f = RENDER_FACTOR;
    let (w, h) = (width / f, height / f);
    let mut out = vec![[0f32; 3]; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            let mut sum = [0f64; 3];
            for dy in 0..f {
                for dx in 0..f {
                    let p = pixels[((y * f + dy) * width + x * f + dx) as usize];
                    for c in 0..3 {
                        sum[c] += f64::from(p[c]);
                    }
                }
            }
            let n = f64::from(f * f);
            out[(y * w + x) as usize] = sum.map(|v| (v / n) as f32);
        }
    }
    (w, h, out)
}

/// The proxy of the camera input stage: valid pixels, and those a sample of which
/// touched a pixel the sensor clipped (`clipped`: the stage of the clip indicator,
/// above 0 there).
pub(super) fn proxy(
    camera: &rawmakase_model::rendered::Rendered,
    clipped: &rawmakase_model::rendered::Rendered,
) -> Proxy {
    let f = RENDER_FACTOR;
    let (width, height, reduced) = reduce(&camera.pixels, camera.width, camera.height);
    let mut sensor_clipped = vec![false; reduced.len()];
    for y in 0..height {
        for x in 0..width {
            sensor_clipped[(y * width + x) as usize] = (0..f * f).any(|i| {
                let p = clipped.pixels[((y * f + i / f) * clipped.width + x * f + i % f) as usize];
                p[0] > 0.
            });
        }
    }
    Proxy {
        width,
        height,
        valid: reduced
            .iter()
            .map(|p| p.iter().all(|v| v.is_finite()))
            .collect(),
        sensor_clipped,
        camera: reduced,
    }
}

/// Where the preview lies on the render: its scale about the centre, radial
/// distortion and shift in proxy pixels, and the correlation of the two's detail in
/// the centre and the four quadrants there.
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct Alignment {
    pub scale: f32,
    pub radial: [f32; 2],
    pub shift: [f32; 2],
    pub regions: [f32; 5],
}

/// The preview's box averages over the proxy's pixels.
pub(super) struct Sampled {
    pub pro: Vec<Rgb>,
    pub clipped: Vec<bool>,
}

/// Summed-area tables of the preview, for box averages in constant time.
struct Integral {
    width: usize,
    sums: Vec<[f64; 4]>,
}
impl Integral {
    fn new(p: &Preview) -> Self {
        let (w, h) = (p.width as usize, p.height as usize);
        let mut sums = vec![[0f64; 4]; (w + 1) * (h + 1)];
        for y in 0..h {
            let mut row = [0f64; 4];
            for x in 0..w {
                let i = y * w + x;
                let v = p.pro[i];
                row[0] += f64::from(v[0]);
                row[1] += f64::from(v[1]);
                row[2] += f64::from(v[2]);
                row[3] += f64::from(u8::from(p.clipped[i]));
                let above = sums[y * (w + 1) + x + 1];
                sums[(y + 1) * (w + 1) + x + 1] = std::array::from_fn(|c| above[c] + row[c]);
            }
        }
        Self { width: w, sums }
    }
    /// The mean over pixel columns `x0..x1` and rows `y0..y1`, and whether any is clipped.
    fn mean(&self, [x0, y0, x1, y1]: [usize; 4]) -> (Rgb, bool) {
        let at = |x: usize, y: usize| self.sums[y * (self.width + 1) + x];
        let (a, b, c, d) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
        let n = ((x1 - x0) * (y1 - y0)).max(1) as f64;
        let v: [f64; 4] = std::array::from_fn(|k| d[k] - b[k] - c[k] + a[k]);
        (std::array::from_fn(|k| (v[k] / n) as f32), v[3] > 0.)
    }
}

/// The preview sampled on a `width` × `height` grid covering it, scaled by `scale`
/// and bent by `radial` (1 + k₀r² + k₁r⁴ of the half-diagonal: a camera that corrects
/// its lens in its JPEG, where the file has no correction for the render) about the
/// centre, and shifted by `shift` proxy pixels.
fn sample(
    p: &Preview,
    integral: &Integral,
    [width, height]: [u32; 2],
    scale: f32,
    radial: [f32; 2],
    shift: [f32; 2],
) -> Sampled {
    let (sx, sy) = (
        p.width as f32 / width as f32,
        p.height as f32 / height as f32,
    );
    let n = (width * height) as usize;
    let mut pro = vec![[f32::NAN; 3]; n];
    let mut clipped = vec![true; n];
    for y in 0..height {
        for x in 0..width {
            let at = |v: u32, len: u32, d: f32, s: f32| {
                let c = len as f32 / 2.;
                ((v as f32 - c - d) / scale + c) * s
            };
            let (cx, cy) = (width as f32 / 2., height as f32 / 2.);
            let (dx, dy) = (
                x as f32 + 0.5 - cx - shift[0],
                y as f32 + 0.5 - cy - shift[1],
            );
            let r2 = (dx * dx + dy * dy) / (cx * cx + cy * cy);
            let bend = r2 * (radial[0] + r2 * radial[1]) / scale;
            let (bx, by) = (dx * bend * sx, dy * bend * sy);
            let (u0, u1) = (
                at(x, width, shift[0], sx) + bx,
                at(x + 1, width, shift[0], sx) + bx,
            );
            let (v0, v1) = (
                at(y, height, shift[1], sy) + by,
                at(y + 1, height, shift[1], sy) + by,
            );
            if u0 < 0. || v0 < 0. || u1 > p.width as f32 || v1 > p.height as f32 {
                continue;
            }
            let x0 = u0.round() as usize;
            let y0 = v0.round() as usize;
            let x1 = (u1.round() as usize).max(x0 + 1).min(p.width as usize);
            let y1 = (v1.round() as usize).max(y0 + 1).min(p.height as usize);
            let i = (y * width + x) as usize;
            (pro[i], clipped[i]) = integral.mean([x0, y0, x1, y1]);
        }
    }
    Sampled { pro, clipped }
}

/// Log luminance less its 5 × 5 mean: the detail two renders of one scene share
/// whatever their tone curves.
fn detail(lum: &[f32], width: u32, height: u32) -> Vec<f32> {
    let (w, h) = (width as i32, height as i32);
    let log: Vec<f32> = lum.iter().map(|v| (v.max(0.) + 1e-3).ln()).collect();
    let mut out = vec![f32::NAN; log.len()];
    for y in 2..h - 2 {
        for x in 2..w - 2 {
            let mut sum = 0.;
            for dy in -2..=2 {
                for dx in -2..=2 {
                    sum += log[((y + dy) * w + x + dx) as usize];
                }
            }
            let i = (y * w + x) as usize;
            out[i] = log[i] - sum / 25.;
        }
    }
    out
}

/// The centre half and the four quadrants of a `width` × `height` image.
fn regions(width: u32, height: u32) -> [[u32; 4]; 5] {
    let (w, h) = (width, height);
    [
        [w / 4, h / 4, 3 * w / 4, 3 * h / 4],
        [0, 0, w / 2, h / 2],
        [w / 2, 0, w, h / 2],
        [0, h / 2, w / 2, h],
        [w / 2, h / 2, w, h],
    ]
}

fn correlation(a: &[f32], b: &[f32], width: u32, [x0, y0, x1, y1]: [u32; 4]) -> f32 {
    let mut n = 0f64;
    let (mut sa, mut sb, mut saa, mut sbb, mut sab) = (0f64, 0f64, 0f64, 0f64, 0f64);
    for y in y0..y1 {
        for x in x0..x1 {
            let i = (y * width + x) as usize;
            let (p, q) = (f64::from(a[i]), f64::from(b[i]));
            if !p.is_finite() || !q.is_finite() {
                continue;
            }
            n += 1.;
            sa += p;
            sb += q;
            saa += p * p;
            sbb += q * q;
            sab += p * q;
        }
    }
    if n < 16. {
        return 0.;
    }
    let cov = sab / n - sa * sb / (n * n);
    let va = saa / n - sa * sa / (n * n);
    let vb = sbb / n - sb * sb / (n * n);
    if va <= 0. || vb <= 0. {
        return 0.;
    }
    (cov / (va * vb).sqrt()) as f32
}

/// The least correlation for a photo to line up: strictly, the centre and every
/// quadrant at `MIN_CORRELATION`, which also ends the search; else the centre at
/// `MIN_CENTRE` and each quadrant at `MIN_QUADRANT`: a quadrant of sky
/// or blurred background has little detail to correlate, and a few pixels off in it
/// matter little to a colour fit, which the held-out tiles judge anyway. A fit that
/// also fits the matrix (`strict`) could bend it to wrong pairs: its quadrants must
/// reach the centre's.
const MIN_CORRELATION: f32 = 0.85;
const MIN_CENTRE: f32 = 0.8;
const MIN_QUADRANT: f32 = 0.45;

/// The preview aligned with the proxy (scales 0.97–1.03, then 0.88–1.12 with radial distortion, shifts of
/// up to 2 proxy pixels), or why it can't be: another picture, cropped or distorted unlike the
/// render, or an aspect ratio more than 2% from the render's.
pub(super) fn align(
    p: &Preview,
    proxy: &Proxy,
    strict: bool,
    cancel: Cancel<'_>,
) -> Result<(Sampled, Alignment), Refusal> {
    let (w, h) = (proxy.width, proxy.height);
    let ratio = (p.width as f32 / p.height as f32) / (w as f32 / h as f32);
    if (ratio - 1.).abs() > 0.02 {
        return Err(refuse(format!(
            "The preview's aspect ratio differs from the photo's by {:.1}%",
            (ratio - 1.).abs() * 100.
        )));
    }
    let integral = Integral::new(p);
    let green: Vec<f32> = proxy
        .camera
        .iter()
        .zip(&proxy.valid)
        .map(|(c, v)| if *v { c[1] } else { f32::NAN })
        .collect();
    let ours = detail(&green, w, h);
    let regions = regions(w, h);
    // The best of `tries` (scale, radial, shift), the first of equals, and `best`.
    let search = |tries: Vec<(f32, [f32; 2], [f32; 2])>, best: Option<(f32, Alignment)>| {
        let scored = tries
            .into_par_iter()
            .map(|(scale, radial, shift)| {
                super::stopped(cancel)?;
                let sampled = sample(p, &integral, [w, h], scale, radial, shift);
                let lum: Vec<f32> = sampled.pro.iter().map(|q| luminance(*q)).collect();
                let theirs = detail(&lum, w, h);
                let regions = regions.map(|region| correlation(&ours, &theirs, w, region));
                let alignment = Alignment {
                    scale,
                    radial,
                    shift,
                    regions,
                };
                Ok((regions.iter().sum::<f32>() / 5., alignment))
            })
            .collect::<Result<Vec<_>, Refusal>>()?;
        let found = scored.into_iter().fold(best, |best, tried| match best {
            Some((mean, _)) if mean >= tried.0 => best,
            _ => Some(tried),
        });
        Ok::<_, Refusal>(found)
    };
    let scale_of = |s: i32| 1. + s as f32 * 0.01;
    let shifts = || (-2..=2).flat_map(|dy| (-2..=2).map(move |dx| [dx as f32, dy as f32]));
    // Scale and shift; then, only where that isn't enough, the lens: scale and the
    // r² term, the r⁴ term with it, and the shift again. The whole product would take
    // many thousands of samples.
    let tries = (-3..=3)
        .flat_map(|s| shifts().map(move |shift| (scale_of(s), [0., 0.], shift)))
        .collect();
    let mut best = search(tries, None)?;
    let lined_up = |best: &Option<(f32, Alignment)>| {
        best.is_some_and(|(_, a)| a.regions.iter().all(|r| *r >= MIN_CORRELATION))
    };
    if !lined_up(&best) {
        let at = best.expect("searched").1;
        let tries = (-12..=12)
            .flat_map(|s| (-16..=16).map(move |k| (scale_of(s), [k as f32 * 0.01, 0.], at.shift)))
            .collect();
        best = search(tries, best)?;
        let at = best.expect("searched").1;
        let tries = (-5..=5)
            .flat_map(|k1| {
                (-2..=2).map(move |k0| {
                    let radial = [at.radial[0] + k0 as f32 * 0.01, k1 as f32 * 0.02];
                    (at.scale, radial, at.shift)
                })
            })
            .collect();
        best = search(tries, best)?;
        let at = best.expect("searched").1;
        let tries = shifts().map(|shift| (at.scale, at.radial, shift)).collect();
        best = search(tries, best)?;
        // The coarse steps can leave the frame's edges a pixel out: finer around them.
        let at = best.expect("searched").1;
        let tries = (-2..=2)
            .flat_map(|s| {
                (-2..=2).flat_map(move |k0| {
                    (-2..=2).map(move |k1| {
                        let radial = [
                            at.radial[0] + k0 as f32 * 0.004,
                            at.radial[1] + k1 as f32 * 0.008,
                        ];
                        (at.scale + s as f32 * 0.004, radial, at.shift)
                    })
                })
            })
            .collect();
        best = search(tries, best)?;
    }
    let (_, alignment) = best.expect("searched");
    let worst = alignment.regions[1..]
        .iter()
        .copied()
        .fold(f32::INFINITY, f32::min);
    let (centre, quadrant) = match strict {
        true => (MIN_CORRELATION, MIN_CORRELATION),
        false => (MIN_CENTRE, MIN_QUADRANT),
    };
    if alignment.regions[0] < centre || worst < quadrant {
        let worst = worst.min(alignment.regions[0]);
        return Err(Refusal {
            reason: format!(
                "The preview does not line up with the photo (detail correlation {worst:.2})"
            ),
            report: None,
            alignment: Some(alignment),
        });
    }
    let sampled = sample(
        p,
        &integral,
        [w, h],
        alignment.scale,
        alignment.radial,
        alignment.shift,
    );
    Ok((sampled, alignment))
}

/// Luminance (Y) of linear ProPhoto RGB.
pub(super) fn luminance(p: Rgb) -> f32 {
    0.288_04 * p[0] + 0.711_874 * p[1] + 0.000_086 * p[2]
}

/// A strong edge: this step of encoded luminance across 3 × 3 pixels. A ratio would
/// refuse the shadows' small steps, and with them the curve's blacks.
const EDGE_STEP: f32 = 0.3;

/// Which pixels a fit may learn from: inside the photo, unclipped by the sensor and
/// in the preview, not on a strong edge ([`EDGE_STEP`]), not in the corners vignetting
/// darkens (beyond 0.85 of the half-diagonal).
pub(super) fn usable(proxy: &Proxy, sampled: &Sampled) -> Vec<bool> {
    let (w, h) = (proxy.width as i32, proxy.height as i32);
    let lum: Vec<f32> = sampled.pro.iter().map(|p| luminance(*p)).collect();
    let half_diagonal = ((w * w + h * h) as f32).sqrt() / 2.;
    (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let i = i as usize;
            if !proxy.valid[i]
                || proxy.sensor_clipped[i]
                || sampled.clipped[i]
                || !sampled.pro[i].iter().all(|v| v.is_finite())
            {
                return false;
            }
            let (dx, dy) = (
                x as f32 + 0.5 - w as f32 / 2.,
                y as f32 + 0.5 - h as f32 / 2.,
            );
            if (dx * dx + dy * dy).sqrt() > 0.85 * half_diagonal {
                return false;
            }
            let (mut lo, mut hi) = (f32::INFINITY, 0f32);
            for ny in (y - 1).max(0)..=(y + 1).min(h - 1) {
                for nx in (x - 1).max(0)..=(x + 1).min(w - 1) {
                    let v = lum[(ny * w + nx) as usize];
                    if !v.is_finite() {
                        return false;
                    }
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
            }
            srgb_encode(hi.clamp(0., 1.)) - srgb_encode(lo.clamp(0., 1.)) <= EDGE_STEP
        })
        .collect()
}

/// The held-out third: whole tiles of an 8 × 8 grid (every third along each row,
/// staggered by row), so a held-out pixel's neighbours are mostly held out too.
pub(super) fn holdout(width: u32, height: u32) -> Vec<bool> {
    (0..width * height)
        .map(|i| {
            let (tx, ty) = (i % width * 8 / width, i / width * 8 / height);
            (tx + 2 * ty) % 3 == 0
        })
        .collect()
}

/// Whether the preview is black and white: at most one usable pixel in a thousand
/// has channels further apart than 0.002 (linear).
pub(super) fn monochrome(target: &[Rgb], usable: &[bool]) -> bool {
    let srgb = target
        .iter()
        .map(|p| rawmakase_model::color::mul(super::PRO_TO_RGB, *p));
    let (mut n, mut coloured) = (0usize, 0usize);
    for (p, u) in srgb.zip(usable) {
        if !*u {
            continue;
        }
        n += 1;
        let spread = p.iter().fold(f32::NEG_INFINITY, |a, v| a.max(*v))
            - p.iter().fold(f32::INFINITY, |a, v| a.min(*v));
        coloured += usize::from(spread > 0.002);
    }
    n > 0 && coloured * 1000 <= n
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn preview(width: u32, height: u32, f: impl Fn(u32, u32) -> f32) -> Preview {
        Preview {
            width,
            height,
            pro: (0..width * height)
                .map(|i| [f(i % width, i / width); 3])
                .collect(),
            clipped: vec![false; (width * height) as usize],
            space: "srgb",
        }
    }
    /// A scene of blobs, the same seen at any size.
    fn scene(x: f32, y: f32) -> f32 {
        0.2 + 0.15 * (x * 0.21).sin() * (y * 0.17).cos() + 0.1 * (x * 0.05 + y * 0.09).sin()
    }

    fn proxy_of(p: &Preview, w: u32, h: u32, scale: f32, shift: [f32; 2]) -> Proxy {
        let integral = Integral::new(p);
        let s = sample(p, &integral, [w, h], scale, [0., 0.], shift);
        Proxy {
            width: w,
            height: h,
            valid: vec![true; (w * h) as usize],
            sensor_clipped: vec![false; (w * h) as usize],
            camera: s.pro,
        }
    }

    #[test]
    fn a_shifted_and_scaled_preview_is_found_where_it_is() {
        let p = preview(960, 640, |x, y| scene(x as f32 / 5., y as f32 / 5.));
        let proxy = proxy_of(&p, 192, 128, 1.02, [1., -2.]);
        let (_, a) =
            align(&p, &proxy, false, Cancel::NEVER).unwrap_or_else(|e| panic!("{}", e.reason));
        assert_eq!((a.scale, a.shift), (1.02, [1., -2.]), "{a:?}");
        assert!(a.regions.iter().all(|r| *r > 0.99), "{a:?}");
    }

    #[test]
    fn a_lens_the_camera_corrected_in_its_jpeg_still_lines_up() {
        let p = preview(960, 640, |x, y| scene(x as f32 / 5., y as f32 / 5.));
        let integral = Integral::new(&p);
        let (w, h) = (192, 128);
        let bent = sample(&p, &integral, [w, h], 1.01, [0.08, 0.03], [0., 0.]);
        let proxy = Proxy {
            width: w,
            height: h,
            valid: bent.pro.iter().map(|q| q[0].is_finite()).collect(),
            sensor_clipped: vec![false; (w * h) as usize],
            camera: bent.pro,
        };
        let (_, a) =
            align(&p, &proxy, false, Cancel::NEVER).unwrap_or_else(|e| panic!("{}", e.reason));
        assert_ne!(a.radial, [0., 0.], "{a:?}");
        assert!(a.regions.iter().all(|r| *r >= MIN_CORRELATION), "{a:?}");
    }

    #[test]
    fn a_quadrant_of_little_shared_detail_does_not_refuse_a_photo() {
        let p = preview(960, 640, |x, y| scene(x as f32 / 5., y as f32 / 5.));
        let mut proxy = proxy_of(&p, 192, 128, 1., [0., 0.]);
        // Noise in the top left, outside the centre, as under a smooth sky.
        let mut noise = 12345u32;
        for y in 0..64 {
            for x in 0..48 {
                noise = noise.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let gain = 1. + 0.5 * ((noise >> 16) as f32 / 32768. - 1.);
                let q = &mut proxy.camera[y * 192 + x];
                *q = q.map(|v| v * gain);
            }
        }
        let (_, a) =
            align(&p, &proxy, false, Cancel::NEVER).unwrap_or_else(|e| panic!("{}", e.reason));
        assert!(a.regions[0] >= MIN_CORRELATION, "{a:?}");
        assert!(
            a.regions[1] < MIN_CORRELATION && a.regions[1] > MIN_QUADRANT,
            "{a:?}"
        );
        // Not where the matrix is fitted to these pairs too.
        assert!(align(&p, &proxy, true, Cancel::NEVER).is_err());
    }

    #[test]
    fn a_cancelled_search_stops() {
        let p = preview(960, 640, |x, y| scene(x as f32 / 5., y as f32 / 5.));
        let proxy = proxy_of(&p, 192, 128, 1., [0., 0.]);
        let refused = align(&p, &proxy, false, Cancel::new(&AtomicBool::new(true)))
            .err()
            .expect("cancelled");
        assert!(refused.reason.contains("cancelled"), "{}", refused.reason);
    }

    /// Without distortion a box is the one the formula below gives, to the last bit:
    /// a photo that lines up without the lens search fits as it always did.
    #[test]
    fn no_distortion_keeps_the_boxes() {
        let (pw, w, scale, shift) = (6000u32, 192u32, 1.02f32, -2f32);
        let p = preview(pw, 4, |x, _| x as f32);
        let integral = Integral::new(&p);
        let s = sample(&p, &integral, [w, 4], scale, [0., 0.], [shift, 0.]);
        let sx = pw as f32 / w as f32;
        let edge = |x: u32| ((x as f32 - w as f32 / 2. - shift) / scale + w as f32 / 2.) * sx;
        assert_eq!(edge(145).round(), 4563., "the case that rounds differently");
        for x in 0..w {
            let (u0, u1) = (edge(x), edge(x + 1));
            let i = (w + x) as usize;
            if u0 < 0. || u1 > pw as f32 {
                assert!(s.pro[i][0].is_nan());
                continue;
            }
            let (x0, x1) = (u0.round() as u32, (u1.round() as u32).min(pw));
            let mean = (x0 + x1 - 1) as f32 / 2.;
            assert!(
                (s.pro[i][0] - mean).abs() < 1e-2,
                "{x}: {} {mean}",
                s.pro[i][0]
            );
        }
    }

    #[test]
    fn another_picture_or_aspect_is_refused() {
        let p = preview(960, 640, |x, y| scene(x as f32 / 5., y as f32 / 5.));
        let other = preview(960, 640, |x, y| scene(y as f32 / 3. + 40., x as f32 / 7.));
        let proxy = proxy_of(&other, 192, 128, 1., [0., 0.]);
        assert!(align(&p, &proxy, false, Cancel::NEVER).is_err());
        let cropped = preview(900, 640, |x, y| scene(x as f32 / 5., y as f32 / 5.));
        let proxy = proxy_of(&p, 192, 128, 1., [0., 0.]);
        let refused = align(&cropped, &proxy, false, Cancel::NEVER)
            .err()
            .expect("refused");
        assert!(refused.reason.contains("aspect"), "{}", refused.reason);
    }

    #[test]
    fn a_letterboxed_or_uncropped_preview_is_cut_to_the_photo() {
        // A square photo letterboxed in a 4:3 preview, black at the sides.
        let p = preview(960, 720, |x, y| {
            if (120..840).contains(&x) {
                scene(x as f32 / 5., y as f32 / 5.)
            } else {
                0.
            }
        });
        let (cut, was_cut) = to_aspect(p, 128, 128);
        assert!(was_cut);
        assert_eq!([cut.width, cut.height], [720, 720]);
        let proxy = proxy_of(&cut, 128, 128, 1., [0., 0.]);
        assert!(align(&cut, &proxy, false, Cancel::NEVER).is_ok());
        // The same aspect ratio is left alone.
        let p = preview(960, 640, |x, y| scene(x as f32, y as f32));
        assert!(!to_aspect(p, 192, 128).1);
    }

    #[test]
    fn clipped_edges_and_corners_are_not_learned_from() {
        let p = preview(96, 64, |x, _| if x < 48 { 0.1 } else { 0.5 });
        let mut proxy = proxy_of(&p, 24, 16, 1., [0., 0.]);
        proxy.sensor_clipped[16 * 24 / 2 + 3] = true;
        let integral = Integral::new(&p);
        let s = sample(&p, &integral, [24, 16], 1., [0., 0.], [0., 0.]);
        let u = usable(&proxy, &s);
        let at = |x: usize, y: usize| u[y * 24 + x];
        assert!(at(6, 8) && at(18, 8), "flat areas");
        assert!(!at(3, 8), "sensor clipped");
        assert!(!at(11, 8) && !at(12, 8), "the edge");
        assert!(!at(0, 0) && !at(23, 15), "the corners");
    }

    /// Dark plaid: a threefold step, but a small one to the eye, and the only say the
    /// curve's blacks get.
    #[test]
    fn the_small_steps_of_shadows_are_learned_from() {
        let p = preview(96, 64, |x, _| if x < 48 { 0.005 } else { 0.02 });
        let proxy = proxy_of(&p, 24, 16, 1., [0., 0.]);
        let integral = Integral::new(&p);
        let s = sample(&p, &integral, [24, 16], 1., [0., 0.], [0., 0.]);
        let u = usable(&proxy, &s);
        assert!(u[8 * 24 + 11] && u[8 * 24 + 12]);
    }

    #[test]
    fn a_third_is_held_out_in_tiles() {
        let h = holdout(192, 128);
        let share = h.iter().filter(|v| **v).count() as f32 / h.len() as f32;
        assert!((share - 1. / 3.).abs() < 0.05, "{share}");
        // Whole tiles: a pixel and its neighbour inside a tile agree.
        assert_eq!(h[0], h[1]);
        assert_eq!(h[0], h[192]);
    }
}
