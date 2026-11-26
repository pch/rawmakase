use super::*;
use crate::{
    camera_data::{CameraImage, Metadata},
    camera_profiles::CameraProfile,
    model::{
        effects::VignetteStyle,
        masks::{LocalAdjust, MaskComponent, MaskGroup, MaskShape},
        point_color::{PointColor, visualize_range},
        recipe::Recipe,
        transform::Transform,
    },
};
use std::{
    collections::VecDeque,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

/// Builds a synthetic camera image with varied tones, saturated out-of-sRGB colors,
/// sharp high-contrast edges, and smooth gradients for testing every export stage.
fn synthetic_photo(w: u32, h: u32) -> CameraImage {
    let metadata = Metadata {
        width: w,
        height: h,
        wb: [1.05, 1.0, 0.96],
        daylight_wb: [1.05, 1.0, 0.96],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        cam_xyz: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    };
    let mut pixels = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        let fy = y as f32 / h.max(1) as f32;
        for x in 0..w {
            let fx = x as f32 / w.max(1) as f32;
            // High-frequency checkerboard / edge bars for sharpening & Lanczos3 tests.
            let bar = if ((x / 11) + (y / 13)) % 2 == 0 {
                0.08
            } else {
                0.0
            };
            let r = (0.04 + 0.55 * fx + bar).min(0.95);
            let g = (0.06 + 0.65 * fy + 0.15 * (fx * 6.0).sin().abs() + bar).min(0.95);
            let b = (0.05 + 0.50 * (1.0 - fx) * (0.4 + 0.6 * fy)).min(0.95);
            pixels.push([r, g, b]);
        }
    }
    CameraImage {
        width: w,
        height: h,
        recovered: Default::default(),
        pixels: pixels.into(),
        metadata,
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    }
}

fn profiled_recipe(im: &CameraImage) -> Recipe {
    Recipe {
        profile: Some(Arc::new(
            CameraProfile::camera_matrix_default(&im.metadata).expect("default profile"),
        )),
        ..Default::default()
    }
}

// -----------------------------------------------------------------------------
// Deterministic unit & fault-injection tests (no hardware GPU required)
// -----------------------------------------------------------------------------

#[test]
fn policy_and_env_resolution_rules() {
    assert_eq!(
        resolve_policy(ExportBackendPolicy::Auto, None, false),
        (ExportBackendPolicy::Auto, None)
    );
    assert_eq!(
        resolve_policy(ExportBackendPolicy::Auto, Some("1"), false),
        (ExportBackendPolicy::Auto, None)
    );
    let (pol, reason) = resolve_policy(ExportBackendPolicy::Auto, Some("0"), false);
    assert_eq!(pol, ExportBackendPolicy::CpuOnly);
    assert_eq!(reason.unwrap().0, ExportFallbackKind::ConfiguredCpu);

    // `ExportBackend::new(CpuOnly)` never upgrades even if env is "1".
    let (pol, reason) = resolve_policy(ExportBackendPolicy::CpuOnly, Some("1"), false);
    assert_eq!(pol, ExportBackendPolicy::CpuOnly);
    assert_eq!(reason.unwrap().0, ExportFallbackKind::ConfiguredCpu);

    // `ExportBackend::from_env(CpuOnly)` honors `RAWMAKASE_EXPORT_GPU=1`.
    assert_eq!(
        resolve_policy(ExportBackendPolicy::CpuOnly, Some("1"), true),
        (ExportBackendPolicy::Auto, None)
    );
    let (pol, _) = resolve_policy(ExportBackendPolicy::CpuOnly, Some("0"), true);
    assert_eq!(pol, ExportBackendPolicy::CpuOnly);
}

#[test]
fn wgsl_shaders_parse_and_validate_for_all_output_spaces() {
    use wgpu::naga::{
        front::wgsl,
        valid::{Capabilities, ValidationFlags, Validator},
    };
    let mut validator = Validator::new(ValidationFlags::all(), Capabilities::all());

    // Validate `develop.wgsl` across all 3 OutputSpaces.
    for space in OutputSpace::ALL {
        let variant = Variant {
            output_space: space,
            ..Variant::FULL
        };
        let source = crate::develop::pipeline::pixel_params::wgsl_prelude()
            + &variant.constants()
            + include_str!("develop.wgsl");
        let module = wgsl::parse_str(&source)
            .unwrap_or_else(|e| panic!("develop.wgsl parse failed for {space:?}: {e}"));
        validator
            .validate(&module)
            .unwrap_or_else(|e| panic!("develop.wgsl validation failed for {space:?}: {e}"));
    }

    // Validate `export_finish.wgsl`.
    let finish_source = PostCropVignette::wgsl_tone() + include_str!("export_finish.wgsl");
    let finish_module = wgsl::parse_str(&finish_source).expect("export_finish.wgsl parses");
    validator
        .validate(&finish_module)
        .expect("export_finish.wgsl validates");
}

#[test]
fn cpu_only_backend_renders_all_spaces_and_formats_without_probing_gpu() -> Result<()> {
    let im = synthetic_photo(64, 48);
    let mut r = profiled_recipe(&im);
    r.sharpening = 0.5;
    r.effects.vignette = -0.3;
    let valid = r.checked()?;
    let backend = ExportBackend::cpu_only();
    let initial = backend.status();
    assert_eq!(initial.policy, ExportBackendPolicy::CpuOnly);
    assert!(!initial.gpu_active);
    assert!(!initial.uninitialized);
    assert_eq!(
        initial.fallback_kind,
        Some(ExportFallbackKind::ConfiguredCpu)
    );

    for space in OutputSpace::ALL {
        let oracle = crate::develop::render_export(&im, &valid, None, 32, space, Cancel::NEVER)?;
        for format in [ExportFormat::F32, ExportFormat::Rgb8, ExportFormat::Rgb16] {
            let out = backend.render_export(&im, &valid, None, 32, space, format, Cancel::NEVER)?;
            assert_eq!(out.space(), space);
            assert_eq!(out.format(), format);
            assert_eq!(
                (out.width(), out.height()),
                (oracle.width(), oracle.height())
            );
            match format {
                ExportFormat::F32 => assert_eq!(out.pixels(), oracle.pixels()),
                ExportFormat::Rgb8 => {
                    assert_eq!(out.as_rgb8().unwrap(), oracle.rgb8().as_slice());
                    assert!(matches!(out.rgb8_cow(), std::borrow::Cow::Borrowed(_)));
                }
                ExportFormat::Rgb16 => {
                    assert_eq!(out.as_rgb16().unwrap(), oracle.rgb16().as_slice());
                    assert!(matches!(out.rgb16_cow(), std::borrow::Cow::Borrowed(_)));
                }
            }
        }
    }
    let st = backend.status();
    assert_eq!(st.gpu_renders, 0);
    assert_eq!(st.cpu_renders, 9);
    assert_eq!(st.fallback_count(ExportFallbackKind::ConfiguredCpu), 9);
    backend.reset_stats();
    assert_eq!(backend.status().cpu_renders, 0);
    Ok(())
}

#[test]
fn band_planning_covers_output_with_exact_halos() {
    // Unresized with sharpening halo = 9.
    let bands = plan_bands(600, 700, 700, 9, None, DEFAULT_BAND_PIXELS, Some(64));
    assert_eq!(bands.len(), 700u32.div_ceil(64) as usize);
    let mut next_y = 0;
    for b in &bands {
        assert_eq!(b.oy0, next_y);
        assert_eq!(b.sy_min, b.oy0);
        assert_eq!(b.sy_max, b.oy1);
        assert_eq!(b.y_comp0, b.oy0.saturating_sub(9));
        assert_eq!(b.y_comp1, (b.oy1 + 9).min(700));
        next_y = b.oy1;
    }
    assert_eq!(next_y, 700);

    // Resized (1000 -> 333) with sharpening halo = 9.
    let (y_stride, y_weights) = weights::lanczos_axis(1000, 333);
    let bands = plan_bands(
        1500,
        1000,
        333,
        9,
        Some((y_stride, &y_weights)),
        DEFAULT_BAND_PIXELS,
        Some(64),
    );
    assert_eq!(bands.len(), 333u32.div_ceil(64) as usize);
    let mut next_oy = 0;
    for b in &bands {
        assert_eq!(b.oy0, next_oy);
        for oy in b.oy0..b.oy1 {
            let off = (oy * y_stride) as usize;
            let start = y_weights[off] as u32;
            let count = y_weights[off + 1] as u32;
            assert!(start >= b.sy_min);
            assert!(start + count <= b.sy_max);
            assert!(b.y_comp0 <= b.sy_min);
            assert!(b.y_comp1 >= b.sy_max);
        }
        next_oy = b.oy1;
    }
    assert_eq!(next_oy, 333);
}

/// Scriptable mock GPU driver for deterministic fault-injection, OOM retry, cancel, and panic tests.
enum MockAction {
    /// Succeed and return a sentinel solid-colored ExportImage (distinguishable from CPU fallback).
    Succeed,
    /// Fail with a Tier-1 per-photo failure.
    PerPhoto(ExportFallbackKind, &'static str),
    /// Fail with retryable OOM.
    Oom(&'static str),
    /// Fail with a Tier-2 session failure.
    Session(ExportFallbackKind, &'static str),
    /// Cancel mid-band: flip `flag` after band 1 and simulate checking `cancel` before band 2.
    CancelMidBand(Arc<AtomicBool>),
    /// Panic during GPU execution.
    Panic(&'static str),
}

struct ScriptedMock {
    actions: VecDeque<MockAction>,
    calls: Arc<AtomicUsize>,
    releases: Arc<AtomicUsize>,
    seen_band_caps: Arc<Mutex<Vec<Option<u32>>>>,
}

impl ScriptedMock {
    fn new(actions: impl Into<Vec<MockAction>>) -> (Self, MockHandle) {
        let calls = Arc::new(AtomicUsize::new(0));
        let releases = Arc::new(AtomicUsize::new(0));
        let seen_band_caps = Arc::new(Mutex::new(Vec::new()));
        let handle = MockHandle {
            calls: calls.clone(),
            releases: releases.clone(),
            seen_band_caps: seen_band_caps.clone(),
        };
        (
            Self {
                actions: VecDeque::from(actions.into()),
                calls,
                releases,
                seen_band_caps,
            },
            handle,
        )
    }
}

struct MockHandle {
    calls: Arc<AtomicUsize>,
    releases: Arc<AtomicUsize>,
    seen_band_caps: Arc<Mutex<Vec<Option<u32>>>>,
}

impl MockGpuDriver for ScriptedMock {
    fn render_banded(
        &mut self,
        job: &mut PreparedExport<'_>,
        band_cap: Option<u32>,
        cancel: Cancel<'_>,
    ) -> std::result::Result<ExportImage, GpuFailure> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.seen_band_caps.lock().unwrap().push(band_cap);
        let action = self.actions.pop_front().unwrap_or(MockAction::Succeed);
        match action {
            MockAction::Succeed => {
                let n = job.out_w as usize * job.out_h as usize;
                let pixels = match job.format {
                    ExportFormat::F32 => ExportPixels::F32(vec![[0.25, 0.5, 0.75]; n]),
                    ExportFormat::Rgb8 => ExportPixels::Rgb8(vec![64; n * 3]),
                    ExportFormat::Rgb16 => ExportPixels::Rgb16(vec![16384; n * 3]),
                };
                Ok(ExportImage::from_pixels(
                    job.out_w, job.out_h, pixels, job.space,
                ))
            }
            MockAction::PerPhoto(kind, msg) => Err(GpuFailure::PerPhoto(kind, msg.into())),
            MockAction::Oom(msg) => Err(GpuFailure::OutOfMemory(msg.into())),
            MockAction::Session(kind, msg) => Err(GpuFailure::Session(kind, msg.into())),
            MockAction::CancelMidBand(flag) => {
                // Simulate 4 bands: complete band 0, set cancel flag, check before band 1.
                let bands = plan_bands(
                    job.geometry.width,
                    job.geometry.height,
                    job.out_h,
                    0,
                    None,
                    DEFAULT_BAND_PIXELS,
                    band_cap,
                );
                assert!(bands.len() >= 2, "mid-band cancel test needs >= 2 bands");
                for (i, _) in bands.iter().enumerate() {
                    if cancel.is_cancelled() {
                        return Err(GpuFailure::Cancelled);
                    }
                    if i == 0 {
                        flag.store(true, Ordering::SeqCst);
                    }
                }
                Err(GpuFailure::Cancelled)
            }
            MockAction::Panic(msg) => panic!("{msg}"),
        }
    }

    fn release_buffers(&mut self) {
        self.releases.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn tier1_per_photo_fallbacks_keep_gpu_active() -> Result<()> {
    let im = synthetic_photo(64, 48);
    let base_recipe_raw = profiled_recipe(&im);
    let base_recipe = base_recipe_raw.checked()?;
    let (mock, handle) = ScriptedMock::new(vec![
        MockAction::PerPhoto(ExportFallbackKind::ExceedsLimits, "oversized"),
        MockAction::Succeed,
        MockAction::Succeed,
    ]);
    let backend = ExportBackend::with_mock("MockGPU", Box::new(mock));

    // Photo #1: oversized -> Tier-1 CPU fallback, matches CPU oracle, GPU stays active.
    let oracle = crate::develop::render_export(
        &im,
        &base_recipe,
        None,
        0,
        OutputSpace::Srgb,
        Cancel::NEVER,
    )?;
    let out1 = backend.render_export(
        &im,
        &base_recipe,
        None,
        0,
        OutputSpace::Srgb,
        ExportFormat::F32,
        Cancel::NEVER,
    )?;
    assert_eq!(out1.pixels(), oracle.pixels());
    assert!(backend.status().gpu_active);
    assert_eq!(
        backend
            .status()
            .fallback_count(ExportFallbackKind::ExceedsLimits),
        1
    );

    // Photo #2: normal recipe -> runs on GPU!
    let _out2 = backend.render_export(
        &im,
        &base_recipe,
        None,
        0,
        OutputSpace::Srgb,
        ExportFormat::Rgb8,
        Cancel::NEVER,
    )?;
    assert_eq!(backend.status().gpu_renders, 1);
    assert_eq!(handle.calls.load(Ordering::SeqCst), 2);

    // Photo #3a..#3f: unsupported recipes (range mask, local Clarity/Texture/Sharpness/Noise,
    // unresolved non-profile recipe) -> Tier-1 CPU fallback without invoking GPU driver.
    let mut range_mask = profiled_recipe(&im);
    range_mask.masks.push(MaskGroup {
        components: vec![MaskComponent::new(MaskShape::LuminanceRange {
            low: 0.2,
            high: 0.8,
            falloff: [0.1, 0.1],
        })],
        adjust: LocalAdjust {
            exposure: 0.5,
            ..Default::default()
        },
        ..Default::default()
    });
    let mut local_clarity = profiled_recipe(&im);
    local_clarity.masks.push(MaskGroup {
        components: vec![MaskComponent::new(MaskShape::Linear {
            from: [0.1, 0.5],
            to: [0.9, 0.5],
        })],
        adjust: LocalAdjust {
            clarity: 0.5,
            ..Default::default()
        },
        ..Default::default()
    });
    let mut local_texture = profiled_recipe(&im);
    local_texture.masks.push(MaskGroup {
        components: vec![MaskComponent::new(MaskShape::Linear {
            from: [0.1, 0.5],
            to: [0.9, 0.5],
        })],
        adjust: LocalAdjust {
            texture: 0.4,
            ..Default::default()
        },
        ..Default::default()
    });
    let mut local_sharpness = profiled_recipe(&im);
    local_sharpness.masks.push(MaskGroup {
        components: vec![MaskComponent::new(MaskShape::Linear {
            from: [0.1, 0.5],
            to: [0.9, 0.5],
        })],
        adjust: LocalAdjust {
            sharpness: 0.6,
            ..Default::default()
        },
        ..Default::default()
    });
    let mut local_noise = profiled_recipe(&im);
    local_noise.masks.push(MaskGroup {
        components: vec![MaskComponent::new(MaskShape::Linear {
            from: [0.1, 0.5],
            to: [0.9, 0.5],
        })],
        adjust: LocalAdjust {
            noise: 0.6,
            ..Default::default()
        },
        ..Default::default()
    });
    // Unresolved non-profile recipe (metadata without cam_xyz has no default matrix profile).
    let mut no_matrix_im = synthetic_photo(32, 24);
    no_matrix_im.metadata.cam_xyz = [[0.; 3]; 3];
    let no_profile = Recipe::default();

    for (img, r) in [
        (&im, range_mask),
        (&im, local_clarity),
        (&im, local_texture),
        (&im, local_sharpness),
        (&im, local_noise),
        (&no_matrix_im, no_profile),
    ] {
        let v = r.checked()?;
        let _ = backend.render_export(
            img,
            &v,
            None,
            0,
            OutputSpace::Srgb,
            ExportFormat::F32,
            Cancel::NEVER,
        )?;
    }
    assert_eq!(
        handle.calls.load(Ordering::SeqCst),
        2,
        "unsupported recipes skip GPU call"
    );
    assert_eq!(
        backend
            .status()
            .fallback_count(ExportFallbackKind::UnsupportedRecipe),
        6
    );
    assert!(backend.status().gpu_active);

    // Photo #4: normal recipe -> GPU is still active and renders on GPU.
    let _out4 = backend.render_export(
        &im,
        &base_recipe,
        None,
        0,
        OutputSpace::Srgb,
        ExportFormat::Rgb8,
        Cancel::NEVER,
    )?;
    assert_eq!(backend.status().gpu_renders, 2);
    assert_eq!(handle.calls.load(Ordering::SeqCst), 3);
    Ok(())
}

#[test]
fn tier1_oom_releases_buffers_and_retries_with_halved_band_height() -> Result<()> {
    let im = synthetic_photo(64, 64);
    let recipe_raw = profiled_recipe(&im);
    let valid = recipe_raw.checked()?;

    // Case A: transient OOM succeeds on retry with halved band height.
    let (mock, handle) = ScriptedMock::new(vec![MockAction::Oom("vram full"), MockAction::Succeed]);
    let backend = ExportBackend::with_mock("MockGPU", Box::new(mock));
    backend.set_band_height_override(Some(32));
    let _ = backend.render_export(
        &im,
        &valid,
        None,
        0,
        OutputSpace::Srgb,
        ExportFormat::Rgb8,
        Cancel::NEVER,
    )?;
    assert_eq!(handle.calls.load(Ordering::SeqCst), 2);
    assert_eq!(handle.releases.load(Ordering::SeqCst), 1);
    assert_eq!(
        *handle.seen_band_caps.lock().unwrap(),
        vec![Some(32), Some(16)]
    );
    assert_eq!(backend.status().gpu_renders, 1);
    assert_eq!(backend.status().cpu_renders, 0);
    assert!(backend.status().gpu_active);

    // Case B: persistent OOM falls back to CPU for this photo, keeping GPU active for the next.
    let (mock, handle) = ScriptedMock::new(vec![
        MockAction::Oom("vram full 1"),
        MockAction::Oom("vram full 2"),
        MockAction::Succeed,
    ]);
    let backend = ExportBackend::with_mock("MockGPU", Box::new(mock));
    backend.set_band_height_override(Some(32));
    let oracle =
        crate::develop::render_export(&im, &valid, None, 0, OutputSpace::Srgb, Cancel::NEVER)?;
    let out = backend.render_export(
        &im,
        &valid,
        None,
        0,
        OutputSpace::Srgb,
        ExportFormat::F32,
        Cancel::NEVER,
    )?;
    assert_eq!(out.pixels(), oracle.pixels());
    assert_eq!(handle.releases.load(Ordering::SeqCst), 2);
    assert_eq!(backend.status().cpu_renders, 1);
    assert_eq!(
        backend
            .status()
            .fallback_count(ExportFallbackKind::OutOfMemory),
        1
    );
    assert!(backend.status().gpu_active);

    // Next photo succeeds on GPU.
    let _ = backend.render_export(
        &im,
        &valid,
        None,
        0,
        OutputSpace::Srgb,
        ExportFormat::Rgb8,
        Cancel::NEVER,
    )?;
    assert_eq!(backend.status().gpu_renders, 1);
    Ok(())
}

#[test]
fn tier2_session_fallbacks_disable_gpu_for_remaining_batch() -> Result<()> {
    let im = synthetic_photo(64, 48);
    let recipe_raw = profiled_recipe(&im);
    let valid = recipe_raw.checked()?;
    let oracle =
        crate::develop::render_export(&im, &valid, None, 0, OutputSpace::Srgb, Cancel::NEVER)?;

    for (kind, msg) in [
        (ExportFallbackKind::DriverError, "wgpu validation error"),
        (ExportFallbackKind::DeviceLostOrTimeout, "device lost"),
    ] {
        let (mock, handle) =
            ScriptedMock::new(vec![MockAction::Session(kind, msg), MockAction::Succeed]);
        let backend = ExportBackend::with_mock("MockGPU", Box::new(mock));

        // Photo #1 triggers Tier-2 fallback and returns valid CPU output.
        let out1 = backend.render_export(
            &im,
            &valid,
            None,
            0,
            OutputSpace::Srgb,
            ExportFormat::F32,
            Cancel::NEVER,
        )?;
        assert_eq!(out1.pixels(), oracle.pixels());
        let st = backend.status();
        assert!(!st.gpu_active);
        assert_eq!(st.fallback_kind, Some(kind));
        assert_eq!(st.fallback_reason.as_deref(), Some(msg));
        assert_eq!(st.cpu_renders, 1);

        // Photo #2 runs directly on CPU without calling the mock GPU again.
        let out2 = backend.render_export(
            &im,
            &valid,
            None,
            0,
            OutputSpace::Srgb,
            ExportFormat::F32,
            Cancel::NEVER,
        )?;
        assert_eq!(out2.pixels(), oracle.pixels());
        assert_eq!(handle.calls.load(Ordering::SeqCst), 1);
        assert_eq!(backend.status().cpu_renders, 2);
        assert_eq!(backend.status().fallback_count(kind), 2);
    }
    Ok(())
}

#[test]
fn mid_band_cancel_aborts_without_cpu_fallback_or_disabling_gpu() -> Result<()> {
    let im = synthetic_photo(64, 64);
    let recipe_raw = profiled_recipe(&im);
    let valid = recipe_raw.checked()?;
    let cancel_flag = Arc::new(AtomicBool::new(false));
    let (mock, _handle) = ScriptedMock::new(vec![
        MockAction::CancelMidBand(cancel_flag.clone()),
        MockAction::Succeed,
    ]);
    let backend = ExportBackend::with_mock("MockGPU", Box::new(mock));
    backend.set_band_height_override(Some(16));

    let err = backend
        .render_export(
            &im,
            &valid,
            None,
            0,
            OutputSpace::Srgb,
            ExportFormat::Rgb8,
            Cancel::new(&cancel_flag),
        )
        .err()
        .expect("cancelled render must return Err");
    assert!(err.to_string().contains("Render superseded"), "{err}");
    let st = backend.status();
    assert!(st.gpu_active, "cancel must not disable GPU");
    assert_eq!(st.cpu_renders, 0, "cancel must not trigger CPU fallback");
    assert_eq!(st.gpu_renders, 0);

    // Subsequent render with fresh cancel token succeeds on GPU.
    let _ = backend.render_export(
        &im,
        &valid,
        None,
        0,
        OutputSpace::Srgb,
        ExportFormat::Rgb8,
        Cancel::NEVER,
    )?;
    assert_eq!(backend.status().gpu_renders, 1);
    Ok(())
}

#[test]
fn panic_inside_gpu_render_does_not_poison_lock() -> Result<()> {
    let im = synthetic_photo(48, 32);
    let recipe_raw = profiled_recipe(&im);
    let valid = recipe_raw.checked()?;
    let oracle =
        crate::develop::render_export(&im, &valid, None, 0, OutputSpace::Srgb, Cancel::NEVER)?;

    let (mock, _handle) = ScriptedMock::new(vec![MockAction::Panic("simulated driver panic")]);
    let backend = ExportBackend::with_mock("MockGPU", Box::new(mock));

    let caught = catch_unwind(AssertUnwindSafe(|| {
        backend.render_export(
            &im,
            &valid,
            None,
            0,
            OutputSpace::Srgb,
            ExportFormat::F32,
            Cancel::NEVER,
        )
    }));
    assert!(caught.is_err(), "panic should unwind to caller");

    // Lock is not poisoned; status, release_buffers, and subsequent renders work cleanly.
    let st = backend.status();
    assert!(!st.gpu_active);
    assert_eq!(st.fallback_kind, Some(ExportFallbackKind::DriverError));
    backend.release_buffers();

    let out = backend.render_export(
        &im,
        &valid,
        None,
        0,
        OutputSpace::Srgb,
        ExportFormat::F32,
        Cancel::NEVER,
    )?;
    assert_eq!(out.pixels(), oracle.pixels());
    assert_eq!(backend.status().cpu_renders, 1);
    Ok(())
}

#[test]
fn scope_error_classification_prioritizes_oom_over_cascade_validation() {
    let oom = || wgpu::Error::OutOfMemory {
        source: Box::new(std::io::Error::other("simulated vram exhaustion")),
    };
    let validation = || wgpu::Error::Validation {
        source: Box::new(std::io::Error::other("invalid buffer handle")),
        description: "Buffer is invalid".into(),
    };
    let internal = || wgpu::Error::Internal {
        source: Box::new(std::io::Error::other("internal driver fault")),
        description: "Internal error".into(),
    };

    // OOM with cascade Validation/Internal errors must stay Tier-1 OutOfMemory so the
    // halved-band retry path runs instead of disabling GPU export for the session.
    assert!(matches!(
        classify_scope_errors(Some(oom()), Some(internal()), Some(validation())),
        Some(GpuFailure::OutOfMemory(_))
    ));
    assert!(matches!(
        classify_scope_errors(Some(oom()), None, Some(validation())),
        Some(GpuFailure::OutOfMemory(_))
    ));

    // Validation or Internal without OOM disables GPU export for the session.
    assert!(matches!(
        classify_scope_errors(None, None, Some(validation())),
        Some(GpuFailure::Session(ExportFallbackKind::DriverError, _))
    ));
    assert!(matches!(
        classify_scope_errors(None, Some(internal()), None),
        Some(GpuFailure::Session(ExportFallbackKind::DriverError, _))
    ));
    assert!(classify_scope_errors(None, None, None).is_none());
}

#[test]
fn status_does_not_block_while_gpu_export_is_in_flight() -> Result<()> {
    use std::sync::mpsc;

    struct BlockingMock {
        entered: Option<mpsc::Sender<()>>,
        release: mpsc::Receiver<()>,
    }

    impl MockGpuDriver for BlockingMock {
        fn render_banded(
            &mut self,
            job: &mut PreparedExport<'_>,
            _band_cap: Option<u32>,
            _cancel: Cancel<'_>,
        ) -> std::result::Result<ExportImage, GpuFailure> {
            if let Some(tx) = self.entered.take() {
                let _ = tx.send(());
            }
            let _ = self.release.recv_timeout(Duration::from_secs(5));
            let n = job.out_w as usize * job.out_h as usize;
            Ok(ExportImage::from_pixels(
                job.out_w,
                job.out_h,
                ExportPixels::Rgb8(vec![128; n * 3]),
                job.space,
            ))
        }

        fn release_buffers(&mut self) {}
    }

    let im = synthetic_photo(48, 32);
    let recipe_raw = profiled_recipe(&im);
    let valid = recipe_raw.checked()?;
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let backend = ExportBackend::with_mock(
        "MockGPU",
        Box::new(BlockingMock {
            entered: Some(entered_tx),
            release: release_rx,
        }),
    );

    std::thread::scope(|s| -> Result<()> {
        let worker = s.spawn(|| {
            backend.render_export(
                &im,
                &valid,
                None,
                0,
                OutputSpace::Srgb,
                ExportFormat::Rgb8,
                Cancel::NEVER,
            )
        });

        entered_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("worker entered GPU render");

        let start = Instant::now();
        let st = backend.status();
        let _dbg = format!("{backend:?}");
        let elapsed = start.elapsed();
        assert!(st.gpu_active);
        assert!(
            elapsed < Duration::from_secs(1),
            "status() blocked while GPU render was in flight ({elapsed:?})"
        );

        release_tx.send(()).expect("release worker");
        worker.join().expect("worker joined")?;
        Ok(())
    })?;
    assert_eq!(backend.status().gpu_renders, 1);
    Ok(())
}

#[test]
fn visualize_range_is_stripped_on_export_backend() -> Result<()> {
    let im = synthetic_photo(48, 32);
    let mut r = profiled_recipe(&im);
    r.point_colors = vec![PointColor::sampled([0.2, 0.8, 0.1])];
    let mut shown = r.clone();
    shown.point_colors = visualize_range(&r.point_colors, 0).unwrap();

    let backend = ExportBackend::cpu_only();
    let expected = backend.render_export(
        &im,
        &r.checked()?,
        None,
        0,
        OutputSpace::Srgb,
        ExportFormat::F32,
        Cancel::NEVER,
    )?;
    let actual = backend.render_export(
        &im,
        &shown.checked()?,
        None,
        0,
        OutputSpace::Srgb,
        ExportFormat::F32,
        Cancel::NEVER,
    )?;
    assert_eq!(actual.pixels(), expected.pixels());
    Ok(())
}

// -----------------------------------------------------------------------------
// Hardware GPU parity, band-seam exactness, and Develop 100% consistency tests
// -----------------------------------------------------------------------------

fn max_u8_diff(a: &[u8], b: &[u8]) -> (u8, usize) {
    assert_eq!(a.len(), b.len());
    let mut max = 0u8;
    let mut diff_pixels = 0usize;
    for (pa, pb) in a.as_chunks::<3>().0.iter().zip(b.as_chunks::<3>().0) {
        let mut p_diff = false;
        for c in 0..3 {
            let d = pa[c].abs_diff(pb[c]);
            max = max.max(d);
            if d > 0 {
                p_diff = true;
            }
        }
        if p_diff {
            diff_pixels += 1;
        }
    }
    (max, diff_pixels)
}

fn max_u16_diff(a: &[u16], b: &[u16]) -> u16 {
    assert_eq!(a.len(), b.len());
    a.iter()
        .zip(b)
        .map(|(&x, &y)| x.abs_diff(y))
        .max()
        .unwrap_or(0)
}

fn rms_f32_diff(a: &[[f32; 3]], b: &[[f32; 3]]) -> f64 {
    assert_eq!(a.len(), b.len());
    let sum: f64 = a
        .iter()
        .flatten()
        .zip(b.iter().flatten())
        .map(|(&x, &y)| {
            let d = f64::from(x) - f64::from(y);
            d * d
        })
        .sum();
    (sum / (a.len() * 3) as f64).sqrt()
}

/// Mandatory band-seam bit-exactness test: renders with `band_height = 64` (forcing 10+
/// horizontal seams) with sharpening (`radius = 3.0`), Lanczos3 downscale, Post-Crop
/// Vignette, and Film Grain all active simultaneously, and asserts 0 LSB / 0 ULP
/// bit-exact equality against single-band full-frame GPU output across all 3 OutputSpaces
/// and all 3 pixel formats (including odd non-multiple-of-4 dimensions).
#[test]
#[ignore = "Requires a hardware compute adapter; run explicitly on supported machines"]
fn gpu_export_band_seam_bit_exactness() -> Result<()> {
    // Odd width & height exercise non-multiple-of-4 Rgb8/Rgb16 packing boundaries too.
    let im = synthetic_photo(765, 701);
    let mut r = profiled_recipe(&im);
    r.exposure = 0.25;
    r.shadows = 0.35;
    r.highlights = -0.3;
    r.sharpening = 0.8;
    r.sharpening_radius = 3.0;
    r.effects.clarity = 0.3;
    r.effects.dehaze = 0.2;
    r.effects.vignette = -0.6;
    r.effects.vignette_highlights = 0.5;
    r.effects.vignette_style = VignetteStyle::HighlightPriority;
    r.effects.grain = 0.4;
    r.effects.grain_size = 0.3;
    r.effects.grain_roughness = 0.5;
    r.masks.push(MaskGroup {
        components: vec![MaskComponent::new(MaskShape::Linear {
            from: [0.1, 0.2],
            to: [0.9, 0.8],
        })],
        adjust: LocalAdjust {
            exposure: 0.4,
            saturation: 0.2,
            ..Default::default()
        },
        ..Default::default()
    });
    let valid = r.checked()?;

    let backend = ExportBackend::new(ExportBackendPolicy::Auto);
    for space in OutputSpace::ALL {
        for max_edge in [0u32, 383u32] {
            for format in [ExportFormat::F32, ExportFormat::Rgb8, ExportFormat::Rgb16] {
                backend.set_band_height_override(Some(u32::MAX));
                let single = backend.render_export(
                    &im,
                    &valid,
                    None,
                    max_edge,
                    space,
                    format,
                    Cancel::NEVER,
                )?;
                assert!(
                    backend.status().gpu_active && backend.status().cpu_renders == 0,
                    "expected real GPU execution, status: {:?}",
                    backend.status()
                );

                backend.set_band_height_override(Some(64));
                let banded = backend.render_export(
                    &im,
                    &valid,
                    None,
                    max_edge,
                    space,
                    format,
                    Cancel::NEVER,
                )?;
                assert_eq!(
                    backend.status().cpu_renders,
                    0,
                    "banded render fell back to CPU"
                );
                assert_eq!(
                    (single.width(), single.height()),
                    (banded.width(), banded.height())
                );
                match (single.storage(), banded.storage()) {
                    (ExportPixels::F32(a), ExportPixels::F32(b)) => {
                        let a_bits: Vec<[u32; 3]> = a.iter().map(|p| p.map(f32::to_bits)).collect();
                        let b_bits: Vec<[u32; 3]> = b.iter().map(|p| p.map(f32::to_bits)).collect();
                        assert_eq!(
                            a_bits, b_bits,
                            "F32 band seam mismatch for {space:?} edge={max_edge}"
                        );
                    }
                    (ExportPixels::Rgb8(a), ExportPixels::Rgb8(b)) => {
                        assert_eq!(
                            a, b,
                            "Rgb8 band seam mismatch for {space:?} edge={max_edge}"
                        );
                    }
                    (ExportPixels::Rgb16(a), ExportPixels::Rgb16(b)) => {
                        assert_eq!(
                            a, b,
                            "Rgb16 band seam mismatch for {space:?} edge={max_edge}"
                        );
                    }
                    _ => panic!("format mismatch"),
                }
            }
        }
    }
    Ok(())
}

/// GPU Export vs. CPU Export parity across Srgb, AdobeRgb, and ProPhoto (full & resized,
/// F32 / Rgb8 / Rgb16) covering geometry, tone, color, B&W, sharpening, all 3 vignette
/// styles, grain, and standard masks.
#[test]
#[ignore = "Requires a hardware compute adapter; run explicitly on supported machines"]
fn gpu_export_matches_cpu_export_across_all_output_spaces_and_features() -> Result<()> {
    let im = synthetic_photo(256, 192);
    let backend = ExportBackend::new(ExportBackendPolicy::Auto);
    // Force multi-band even on 256x192 so parity also exercises seams.
    backend.set_band_height_override(Some(64));

    type Case = (&'static str, Box<dyn Fn(&mut Recipe)>);
    let cases: Vec<Case> = vec![
        ("default", Box::new(|_| {})),
        (
            "tone_color_sharp_grain",
            Box::new(|r| {
                r.exposure = 0.3;
                r.contrast = 0.25;
                r.shadows = 0.4;
                r.highlights = -0.35;
                r.whites = 0.2;
                r.blacks = -0.15;
                r.vibrance = 0.3;
                r.saturation = 0.15;
                r.sharpening = 0.7;
                r.sharpening_radius = 2.0;
                r.effects.clarity = 0.35;
                r.effects.dehaze = 0.25;
                r.effects.grain = 0.35;
                r.effects.channels[0].insert([0.4, 0.52]);
                r.point_colors = vec![PointColor::sampled([0.3, 0.6, 0.2])];
            }),
        ),
        (
            "geometry_transform",
            Box::new(|r| {
                r.crop = [0.05, 0.08, 0.92, 0.9];
                r.straighten = 3.5;
                r.transform = Transform {
                    vertical: 0.15,
                    horizontal: -0.1,
                    ..Default::default()
                };
                r.sharpening = 0.5;
            }),
        ),
        (
            "vignette_highlight_darken",
            Box::new(|r| {
                r.effects.vignette = -0.7;
                r.effects.vignette_highlights = 0.6;
                r.effects.vignette_style = VignetteStyle::HighlightPriority;
            }),
        ),
        (
            "vignette_highlight_lighten",
            Box::new(|r| {
                r.effects.vignette = 0.55;
                r.effects.vignette_style = VignetteStyle::HighlightPriority;
            }),
        ),
        (
            "vignette_color_darken",
            Box::new(|r| {
                r.effects.vignette = -0.65;
                r.effects.vignette_highlights = 0.5;
                r.effects.vignette_style = VignetteStyle::ColorPriority;
            }),
        ),
        (
            "vignette_color_lighten",
            Box::new(|r| {
                r.effects.vignette = 0.5;
                r.effects.vignette_style = VignetteStyle::ColorPriority;
            }),
        ),
        (
            "vignette_paint_darken",
            Box::new(|r| {
                r.effects.vignette = -0.6;
                r.effects.vignette_style = VignetteStyle::PaintOverlay;
            }),
        ),
        (
            "vignette_paint_lighten",
            Box::new(|r| {
                r.effects.vignette = 0.45;
                r.effects.vignette_style = VignetteStyle::PaintOverlay;
            }),
        ),
        (
            "black_and_white_graded",
            Box::new(|r| {
                r.effects.monochrome = true;
                r.grading[0] = [0.6, 0.25, -0.1];
                r.grading[2] = [0.1, 0.2, 0.1];
            }),
        ),
        (
            "standard_masks",
            Box::new(|r| {
                r.masks.push(MaskGroup {
                    components: vec![
                        MaskComponent::new(MaskShape::Linear {
                            from: [0.1, 0.2],
                            to: [0.8, 0.7],
                        }),
                        MaskComponent::new(MaskShape::Radial {
                            center: [0.5, 0.5],
                            radii: [0.3, 0.25],
                            angle: 0.2,
                            feather: 0.4,
                        }),
                    ],
                    adjust: LocalAdjust {
                        exposure: 0.5,
                        shadows: 0.3,
                        saturation: -0.2,
                        ..Default::default()
                    },
                    ..Default::default()
                });
            }),
        ),
    ];

    for (name, edit) in &cases {
        let mut r = profiled_recipe(&im);
        edit(&mut r);
        let valid = r.checked()?;
        for space in OutputSpace::ALL {
            for max_edge in [0u32, 160u32] {
                let cpu = crate::develop::render_export(
                    &im,
                    &valid,
                    None,
                    max_edge,
                    space,
                    Cancel::NEVER,
                )?;
                let gpu_f32 = backend.render_export(
                    &im,
                    &valid,
                    None,
                    max_edge,
                    space,
                    ExportFormat::F32,
                    Cancel::NEVER,
                )?;
                let gpu_8 = backend.render_export(
                    &im,
                    &valid,
                    None,
                    max_edge,
                    space,
                    ExportFormat::Rgb8,
                    Cancel::NEVER,
                )?;
                let gpu_16 = backend.render_export(
                    &im,
                    &valid,
                    None,
                    max_edge,
                    space,
                    ExportFormat::Rgb16,
                    Cancel::NEVER,
                )?;
                assert_eq!(
                    backend.status().cpu_renders,
                    0,
                    "{name} {space:?} edge={max_edge} fell back to CPU: {:?}",
                    backend.status()
                );

                let rms = rms_f32_diff(gpu_f32.pixels(), cpu.pixels());
                assert!(
                    rms <= 1e-4,
                    "{name} {space:?} edge={max_edge}: f32 RMS {rms} > 1e-4"
                );

                let cpu_8 = cpu.rgb8();
                let (max8, diff_px) = max_u8_diff(gpu_8.as_rgb8().unwrap(), &cpu_8);
                let total_px = (cpu.width() * cpu.height()) as usize;
                let exact_ratio = 1.0 - (diff_px as f64 / total_px as f64);
                assert!(
                    max8 <= 1,
                    "{name} {space:?} edge={max_edge}: 8-bit max diff {max8} > 1"
                );
                assert!(
                    exact_ratio >= 0.999,
                    "{name} {space:?} edge={max_edge}: 8-bit exact ratio {exact_ratio:.5} < 0.999"
                );

                let cpu_16 = cpu.rgb16();
                let gpu_16_slice = gpu_16.as_rgb16().unwrap();
                let max16 = max_u16_diff(gpu_16_slice, &cpu_16);
                let within_3 = gpu_16_slice
                    .iter()
                    .zip(&cpu_16)
                    .filter(|&(&g, &c)| g.abs_diff(c) <= 3)
                    .count();
                let ratio_within_3 = within_3 as f64 / cpu_16.len() as f64;
                assert!(
                    ratio_within_3 >= 0.999,
                    "{name} {space:?} edge={max_edge}: 16-bit <=3 LSB ratio {ratio_within_3:.5} < 0.999 (max16={max16}, rms={rms:.2e})"
                );
                // Require >=99.9% of 16-bit channels within 3 LSB. On sharp step edges
                // (where GPU fast-reciprocal division in `local.wgsl` differs from IEEE
                // CPU division by ~1 ULP before bilinear sampling, clarity, and
                // sharpening) or near-zero Adobe RGB pure-power (`v^(256/563)`) pixels,
                // isolated single channels can reach ~10–30 LSB while staying <= 1/255
                // in 8-bit and ~1e-6 RMS across the frame.
                assert!(
                    max16 <= 32,
                    "{name} {space:?} edge={max_edge}: 16-bit max diff {max16} > 32 (rms={rms:.2e})"
                );
            }
        }
    }
    Ok(())
}

/// A file's own always-on lens correction (a DNG's warp, vignette and gain map)
/// exports alike on the GPU and the CPU with its switch on and off
/// (`Recipe::lens_file_correction`), measures included (#1289).
#[test]
#[ignore = "Requires a hardware compute adapter; run explicitly on supported machines"]
fn gpu_export_matches_cpu_with_the_files_lens_correction_on_and_off() -> Result<()> {
    use crate::optics::{GainGrid, LensCorrection, Radial};
    let mut im = synthetic_photo(256, 192);
    im.metadata.lens = Some(LensCorrection {
        source: LensCorrection::DNG_OPCODES.into(),
        default_on: true,
        vignetting: Radial::new(vec![0., 1.], vec![1., 1.8]),
        vignetting_measured: true,
        distortion: Radial::new(vec![0., 1.], vec![1., 0.96]),
        gain_map: Some(GainGrid {
            points: [2, 2],
            spacing: [1., 1.],
            planes: 3,
            gains: vec![1., 1.2, 1.1, 1.3, 1., 1., 1.4, 1.1, 1., 1.2, 1., 1.5],
            ..Default::default()
        }),
        ..Default::default()
    });
    let backend = ExportBackend::new(ExportBackendPolicy::Auto);
    for on in [true, false] {
        let edit = Recipe {
            lens_file_correction: on,
            shadows: 0.4,
            effects: crate::model::effects::Effects {
                clarity: 0.3,
                ..Default::default()
            },
            ..profiled_recipe(&im)
        };
        let r = edit.checked()?;
        let space = OutputSpace::ALL[0];
        let cpu = crate::develop::render_export(&im, &r, None, 0, space, Cancel::NEVER)?;
        let gpu =
            backend.render_export(&im, &r, None, 0, space, ExportFormat::F32, Cancel::NEVER)?;
        assert_eq!(backend.status().cpu_renders, 0, "fell back to the CPU");
        let rms = rms_f32_diff(gpu.pixels(), cpu.pixels());
        assert!(rms <= 1e-4, "switch {on}: f32 RMS {rms} > 1e-4");
    }
    Ok(())
}

/// GPU Export vs. Develop 100% Preview parity (sRGB, monitor profile = identity):
/// guarantees what the user sees at 100% in Develop matches what exports (`<= 1/255`).
#[test]
#[ignore = "Requires a hardware compute adapter; run explicitly on supported machines"]
fn gpu_export_matches_develop_100_percent_preview() -> Result<()> {
    use crate::develop::{
        PreviewRenderer,
        gpu::{Display, Slot},
        quality::Output,
    };
    use crate::rendered::ClipOverlay;

    let im = synthetic_photo(256, 192);
    let mut r = profiled_recipe(&im);
    r.exposure = 0.2;
    r.shadows = 0.35;
    r.highlights = -0.3;
    r.sharpening = 0.6;
    r.sharpening_radius = 1.5;
    r.effects.clarity = 0.25;
    r.effects.dehaze = 0.15;
    r.effects.vignette = -0.45;
    r.effects.vignette_highlights = 0.5;
    r.effects.grain = 0.3;
    r.masks.push(MaskGroup {
        components: vec![MaskComponent::new(MaskShape::Linear {
            from: [0.2, 0.2],
            to: [0.8, 0.8],
        })],
        adjust: LocalAdjust {
            exposure: 0.35,
            ..Default::default()
        },
        ..Default::default()
    });
    let valid = r.checked()?;

    let mut preview = PreviewRenderer::with_gpu();
    assert!(
        preview.adapter_name().is_some(),
        "PreviewRenderer needs a hardware GPU: {:?}",
        preview.fallback_reason()
    );
    let display = Display {
        slot: Slot::Whole,
        clipping: ClipOverlay::NONE,
        monitor: None,
        navigator: None,
        thumbnail: None,
        samples: true,
        drawn: Vec::new(),
    };
    let Output::Frame(frame) =
        preview.render_to(&im, &r, 0, None, Cancel::NEVER, Some(&display))?
    else {
        panic!("expected GPU-presented frame from PreviewRenderer");
    };
    let (pw, ph, preview_rgb8) = frame.samples.expect("Display::samples requested");

    let backend = ExportBackend::new(ExportBackendPolicy::Auto);
    backend.set_band_height_override(Some(64));
    let exported = backend.render_export(
        &im,
        &valid,
        None,
        0,
        OutputSpace::Srgb,
        ExportFormat::Rgb8,
        Cancel::NEVER,
    )?;
    assert!(backend.status().gpu_active && backend.status().cpu_renders == 0);
    assert_eq!((exported.width(), exported.height()), (pw, ph));

    let (max_diff, _) = max_u8_diff(exported.as_rgb8().unwrap(), &preview_rgb8);
    assert!(
        max_diff <= 1,
        "GPU Export vs Develop 100% preview max diff {max_diff} > 1"
    );
    Ok(())
}

/// Real-GPU buffer reuse and `release_buffers()` across multiple photos.
#[test]
#[ignore = "Requires a hardware compute adapter; run explicitly on supported machines"]
fn gpu_export_release_buffers_and_reuse() -> Result<()> {
    let im1 = synthetic_photo(192, 128);
    let im2 = synthetic_photo(256, 192);
    let r1_raw = profiled_recipe(&im1);
    let r2_raw = profiled_recipe(&im2);
    let r1 = r1_raw.checked()?;
    let r2 = r2_raw.checked()?;

    let backend = ExportBackend::new(ExportBackendPolicy::Auto);
    let out1 = backend.render_export(
        &im1,
        &r1,
        None,
        0,
        OutputSpace::Srgb,
        ExportFormat::Rgb8,
        Cancel::NEVER,
    )?;
    backend.release_buffers();
    let out2 = backend.render_export(
        &im2,
        &r2,
        None,
        128,
        OutputSpace::AdobeRgb,
        ExportFormat::Rgb16,
        Cancel::NEVER,
    )?;

    let st = backend.status();
    assert!(st.gpu_active);
    assert!(st.adapter_name.is_some());
    assert_eq!(st.gpu_renders, 2);
    assert_eq!(st.cpu_renders, 0);
    assert_eq!((out1.width(), out1.height()), (192, 128));
    assert_eq!((out2.width(), out2.height()), (128, 96));
    Ok(())
}
