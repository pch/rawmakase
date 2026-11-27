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

    // Validate `develop.wgsl` across all 3 OutputSpaces and the HDR variant.
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
    let hdr_source = crate::develop::pipeline::pixel_params::wgsl_prelude()
        + &Variant::HDR.constants()
        + include_str!("develop.wgsl");
    let hdr_module = wgsl::parse_str(&hdr_source).expect("develop.wgsl (HDR) parses");
    validator
        .validate(&hdr_module)
        .expect("develop.wgsl (HDR) validates");

    // Validate `local.wgsl` with the updated `S_PHOTO_Y0` prelude.
    let local_source = sample_slot::wgsl_prelude() + include_str!("local.wgsl");
    let local_module = wgsl::parse_str(&local_source).expect("local.wgsl parses");
    validator
        .validate(&local_module)
        .expect("local.wgsl validates");

    // Validate `export_finish.wgsl` (including HDR master entry points).
    let finish_source = PostCropVignette::wgsl_tone() + include_str!("export_finish.wgsl");
    let finish_module = wgsl::parse_str(&finish_source).expect("export_finish.wgsl parses");
    validator
        .validate(&finish_module)
        .expect("export_finish.wgsl validates");

    // Validate `pyramid.wgsl`.
    let pyramid_module =
        wgsl::parse_str(super::super::pyramid::wgsl_source()).expect("pyramid.wgsl parses");
    validator
        .validate(&pyramid_module)
        .expect("pyramid.wgsl validates");
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
        job: &PreparedExport<'_>,
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
            job: &PreparedExport<'_>,
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

/// Exit with an export rendering and a preview build waiting for the GPU behind it:
/// once both are cancelled, the export's render stops at its next check, and the
/// build, given the GPU, stops before rendering anything, on the GPU or the CPU.
#[test]
fn a_render_waiting_for_the_gpu_stops_once_cancelled_without_rendering() -> Result<()> {
    use std::sync::mpsc;

    /// Renders until cancelled, checking as the real backend's poll slices do.
    struct UntilCancelled {
        entered: Option<mpsc::Sender<()>>,
        calls: Arc<AtomicUsize>,
    }
    impl MockGpuDriver for UntilCancelled {
        fn render_banded(
            &mut self,
            _job: &PreparedExport<'_>,
            _band_cap: Option<u32>,
            cancel: Cancel<'_>,
        ) -> std::result::Result<ExportImage, GpuFailure> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(tx) = self.entered.take() {
                let _ = tx.send(());
            }
            let until = Instant::now() + Duration::from_secs(5);
            while !cancel.is_cancelled() && Instant::now() < until {
                std::thread::sleep(BAND_POLL_SLICE);
            }
            Err(GpuFailure::Cancelled)
        }
        fn release_buffers(&mut self) {}
    }

    let im = synthetic_photo(48, 32);
    let recipe = profiled_recipe(&im);
    let valid = recipe.checked()?;
    let calls = Arc::new(AtomicUsize::new(0));
    let (entered_tx, entered_rx) = mpsc::channel();
    let backend = ExportBackend::with_mock(
        "MockGPU",
        Box::new(UntilCancelled {
            entered: Some(entered_tx),
            calls: calls.clone(),
        }),
    );
    let (export_cancel, build_cancel) = (AtomicBool::new(false), AtomicBool::new(false));
    let render = |cancel: &AtomicBool| {
        backend.render_export(
            &im,
            &valid,
            None,
            0,
            OutputSpace::Srgb,
            ExportFormat::Rgb8,
            Cancel::new(cancel),
        )
    };

    std::thread::scope(|s| {
        let export = s.spawn(|| render(&export_cancel));
        entered_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the export reached the GPU");
        let build = s.spawn(|| render(&build_cancel));
        // Time for the build to finish its CPU preparation and wait for the GPU.
        // Were it slower, it would see the cancel before waiting, with the same result.
        std::thread::sleep(Duration::from_millis(200));
        let quit = Instant::now();
        build_cancel.store(true, Ordering::SeqCst);
        export_cancel.store(true, Ordering::SeqCst);
        for render in [export, build] {
            let err = render.join().expect("joined").err().expect("cancelled");
            assert!(err.to_string().contains("Render superseded"), "{err}");
        }
        assert!(
            quit.elapsed() < Duration::from_secs(1),
            "stopping took {:?}",
            quit.elapsed()
        );
    });
    assert_eq!(calls.load(Ordering::SeqCst), 1, "the build never rendered");
    let st = backend.status();
    assert_eq!((st.gpu_renders, st.cpu_renders), (0, 0));
    assert!(st.gpu_active, "a cancel never turns the GPU off");
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
    r.noise_chroma = 0.45;
    r.noise_luma = 0.5;
    r.effects.texture = 0.45;
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
    r.noise_chroma = 0.35;
    r.noise_luma = 0.4;
    r.effects.texture = 0.4;
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

/// Deterministic noisy synthetic photo with sharp color edges, clipped highlights, and
/// near-black shadow patches for verifying Color NR, Luminance NR, and Texture parity.
fn noisy_synthetic_photo(w: u32, h: u32, full_w: u32, full_h: u32, fast: bool) -> CameraImage {
    fn hash(x: u32, y: u32, c: u32) -> f32 {
        let mut v =
            x.wrapping_mul(0x9e3779b9) ^ y.wrapping_mul(0x85ebca6b) ^ c.wrapping_mul(0xc2b2ae35);
        v ^= v >> 15;
        v = v.wrapping_mul(0x2c1b3c6d);
        v ^= v >> 12;
        (v as f32 / u32::MAX as f32) * 2. - 1.
    }
    let mut im = synthetic_photo(w, h);
    im.metadata.width = full_w;
    im.metadata.height = full_h;
    im.fast = fast;
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) as usize;
            let p = if x < w / 6 && y < h / 4 {
                // Clipped highlight region
                [1.1, 1.05, 1.02]
            } else if x >= w * 5 / 6 && y < h / 4 {
                // Black region
                [0.0, 0.0, 0.0]
            } else {
                let mut base = im.pixels[i];
                for (c, v) in base.iter_mut().enumerate() {
                    let n = 0.025 * v.max(0.01).sqrt() * hash(x, y, c as u32 + 1);
                    *v = (*v + n).max(0.0);
                }
                base
            };
            im.pixels[i] = p;
        }
    }
    im
}

/// Direct operator-level GPU vs CPU parity for `GpuPyramid`: Color NR, Luminance NR,
/// Texture (positive & negative), and crop-clamped box reduction across normal, odd,
/// and half-scale decodes.
#[test]
#[ignore = "Requires a hardware compute adapter; run explicitly on supported machines"]
fn gpu_pyramid_matches_cpu_for_chroma_luma_texture_and_box_reduce() -> Result<()> {
    use crate::develop::{
        color_noise::ChromaDenoise, luminance_noise::LuminanceDenoise, texture::TextureDetail,
    };

    let mut ctx = GpuExportContext::new().expect("hardware GPU required");
    let mut signed = noisy_synthetic_photo(129, 97, 129, 97, false);
    for (i, p) in signed.pixels.iter_mut().enumerate() {
        if i % 3 == 0 {
            *p = [p[0] - 0.02, p[1] - 0.005, p[2] - 0.015];
        }
    }
    let cases = [
        ("full_odd", noisy_synthetic_photo(257, 193, 257, 193, false)),
        ("signed_shadows", signed),
        // Odd-sided half-size decode gives a fractional photo_scale (193 / 385 != 0.5).
        (
            "half_decode_fractional",
            noisy_synthetic_photo(193, 129, 385, 257, true),
        ),
    ];

    for (label, im) in &cases {
        let upload_and_run = |ctx: &mut GpuExportContext,
                              run: &dyn Fn(
            &GpuExportContext,
            &wgpu::Buffer,
        ) -> std::result::Result<(), GpuFailure>|
         -> Vec<[f32; 3]> {
            ctx.scoped(|ctx| {
                let bytes = im.pixels.len() as u64 * 12;
                let buf = GpuExportContext::ensure_buffer(
                    &ctx.device,
                    &mut ctx.buffers.photo,
                    "Test pyramid photo",
                    bytes,
                    wgpu::BufferUsages::STORAGE
                        | wgpu::BufferUsages::COPY_DST
                        | wgpu::BufferUsages::COPY_SRC,
                );
                ctx.queue
                    .write_buffer(&buf, 0, bytemuck::cast_slice(&im.pixels));
                run(ctx, &buf)?;
                GpuPyramid::readback_photo(ctx, &buf, im.width, im.height, Cancel::NEVER)
            })
            .unwrap_or_else(|_| panic!("GPU pyramid failed for {label}"))
        };

        // 1. Color NR (default 25 and strong 80 with non-default detail/smoothness)
        for (a, d, s) in [(0.25, 0.5, 0.5), (0.8, 0.75, 0.25)] {
            let dn = ChromaDenoise::new(a, d, s).unwrap();
            let cpu = dn.apply(im, Cancel::NEVER)?;
            let gpu = upload_and_run(&mut ctx, &|ctx, buf| {
                ctx.pyramid.apply_chroma_denoise(
                    ctx,
                    buf,
                    im.width,
                    im.height,
                    im.metadata.wb,
                    im.photo_scale(),
                    dn,
                    Cancel::NEVER,
                )
            });
            let rms = rms_f32_diff(&gpu, &cpu.pixels);
            assert!(rms <= 5e-5, "{label} chroma({a},{d},{s}): RMS {rms} > 5e-5");
        }

        // 2. Luminance NR
        for (a, d, c) in [(0.4, 0.5, 0.2), (0.9, 0.25, 0.7)] {
            let dn = LuminanceDenoise::new(a, d, c).unwrap();
            let cpu = dn.apply(im, Cancel::NEVER)?;
            let gpu = upload_and_run(&mut ctx, &|ctx, buf| {
                ctx.pyramid.apply_luminance_denoise(
                    ctx,
                    buf,
                    im.width,
                    im.height,
                    im.metadata.wb,
                    im.photo_scale(),
                    dn,
                    Cancel::NEVER,
                )
            });
            let rms = rms_f32_diff(&gpu, &cpu.pixels);
            assert!(rms <= 5e-5, "{label} luma({a},{d},{c}): RMS {rms} > 5e-5");
        }

        // 3. Texture (positive and negative)
        for amount in [0.65f32, -0.5f32] {
            let scale = im.photo_scale();
            let cpu = TextureDetail::of(im, scale, Cancel::NEVER)?.apply(im, amount);
            let offset = texture::level_offset(im, scale);
            let gpu = upload_and_run(&mut ctx, &|ctx, buf| {
                ctx.pyramid.apply_texture(
                    ctx,
                    buf,
                    im.width,
                    im.height,
                    offset,
                    amount,
                    Cancel::NEVER,
                )
            });
            let rms = rms_f32_diff(&gpu, &cpu.pixels);
            assert!(rms <= 5e-5, "{label} texture({amount}): RMS {rms} > 5e-5");
        }

        // 4. Crop-clamped box reduction (both full frame and explicit crop inset)
        let mut im_crop = im.clone();
        im_crop.metadata.crop_left = 12;
        im_crop.metadata.crop_top = 10;
        im_crop.metadata.crop_width = im_crop.metadata.width.saturating_sub(24);
        im_crop.metadata.crop_height = im_crop.metadata.height.saturating_sub(20);
        let cpu_meas = pipeline::measurement_source(Source::new(&im_crop), 96, true);
        let (mw, mh) = pipeline::measurement_size(im_crop.width, im_crop.height, 96);
        let crop = local_tone::inner_crop_rect(&im_crop)
            .map(|[x0, y0, x1, y1]| [x0 as u32, y0 as u32, x1 as u32, y1 as u32])
            .unwrap_or([0, 0, im_crop.width, im_crop.height]);
        let gpu_meas = ctx
            .scoped(|ctx| {
                let bytes = im_crop.pixels.len() as u64 * 12;
                let buf = GpuExportContext::ensure_buffer(
                    &ctx.device,
                    &mut ctx.buffers.photo,
                    "Test box_reduce photo",
                    bytes,
                    wgpu::BufferUsages::STORAGE
                        | wgpu::BufferUsages::COPY_DST
                        | wgpu::BufferUsages::COPY_SRC,
                );
                ctx.queue
                    .write_buffer(&buf, 0, bytemuck::cast_slice(&im_crop.pixels));
                ctx.pyramid.box_reduce(
                    ctx,
                    &buf,
                    im_crop.width,
                    im_crop.height,
                    crop,
                    mw,
                    mh,
                    Cancel::NEVER,
                )
            })
            .unwrap_or_else(|_| panic!("GPU box_reduce failed for {label}"));
        let gpu_meas = pipeline::finish_measurement_source(
            reduced_camera_image(&im_crop, mw, mh, gpu_meas),
            true,
        );
        let rms = rms_f32_diff(&gpu_meas.pixels, &cpu_meas.pixels);
        assert!(rms <= 1e-6, "{label} box_reduce: RMS {rms} > 1e-6");
    }
    Ok(())
}

/// End-to-end GPU Export vs CPU Export parity with Color NR, Luminance NR, Texture,
/// retouching, and Auto Mask brush enabled across all OutputSpaces and pixel formats.
#[test]
#[ignore = "Requires a hardware compute adapter; run explicitly on supported machines"]
fn gpu_export_matches_cpu_with_nr_texture_retouch_and_automask() -> Result<()> {
    use crate::model::{
        masks::BrushStroke,
        retouch::{RetouchMode, RetouchOp, RetouchShape},
    };

    let im = noisy_synthetic_photo(257, 193, 257, 193, false);
    let backend = ExportBackend::new(ExportBackendPolicy::Auto);
    backend.set_band_height_override(Some(64));

    type Case = (&'static str, Box<dyn Fn(&mut Recipe)>);
    let cases: Vec<Case> = vec![
        (
            "heavy_nr_texture_clarity_sharp_resize",
            Box::new(|r| {
                r.exposure = 0.2;
                r.shadows = 0.35;
                r.highlights = -0.3;
                r.noise_chroma = 0.4;
                r.effects.chroma_detail = 0.6;
                r.effects.chroma_smoothness = 0.4;
                r.noise_luma = 0.55;
                r.effects.luma_detail = 0.45;
                r.effects.luma_contrast = 0.3;
                r.effects.texture = 0.65;
                r.effects.clarity = 0.3;
                r.sharpening = 0.7;
                r.effects.vignette = -0.35;
                r.effects.grain = 0.25;
            }),
        ),
        (
            "negative_texture_and_retouch",
            Box::new(|r| {
                r.noise_chroma = 0.35;
                r.noise_luma = 0.3;
                r.effects.texture = -0.5;
                r.retouch.push(RetouchOp {
                    mode: RetouchMode::Clone,
                    shape: RetouchShape::Spot {
                        center: [0.4, 0.5],
                        radius: 0.05,
                    },
                    feather: 0.5,
                    opacity: 0.9,
                    offset: [0.2, 0.0],
                    patch: None,
                });
            }),
        ),
        (
            "nr_retouch_automask_without_texture",
            Box::new(|r| {
                r.noise_chroma = 0.4;
                r.noise_luma = 0.35;
                r.effects.texture = 0.0;
                r.retouch.push(RetouchOp {
                    mode: RetouchMode::Clone,
                    shape: RetouchShape::Spot {
                        center: [0.45, 0.5],
                        radius: 0.06,
                    },
                    feather: 0.5,
                    opacity: 0.9,
                    offset: [0.15, 0.0],
                    patch: None,
                });
                r.masks.push(MaskGroup {
                    components: vec![MaskComponent::new(MaskShape::Brush {
                        strokes: vec![BrushStroke {
                            points: vec![[0.35, 0.45], [0.5, 0.5], [0.65, 0.55]].into(),
                            radius: 0.08,
                            feather: 0.4,
                            flow: 1.0,
                            density: 1.0,
                            erase: false,
                            auto_mask: true,
                        }],
                    })],
                    adjust: LocalAdjust {
                        exposure: 0.4,
                        ..Default::default()
                    },
                    ..Default::default()
                });
            }),
        ),
        (
            "nr_texture_with_automask_brush",
            Box::new(|r| {
                r.noise_chroma = 0.4;
                r.noise_luma = 0.35;
                r.effects.texture = 0.45;
                r.masks.push(MaskGroup {
                    components: vec![MaskComponent::new(MaskShape::Brush {
                        strokes: vec![BrushStroke {
                            points: vec![[0.25, 0.4], [0.55, 0.5], [0.75, 0.6]].into(),
                            radius: 0.08,
                            feather: 0.4,
                            flow: 1.0,
                            density: 1.0,
                            erase: false,
                            auto_mask: true,
                        }],
                    })],
                    adjust: LocalAdjust {
                        exposure: 0.45,
                        saturation: 0.2,
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
                assert_eq!(
                    backend.status().cpu_renders,
                    0,
                    "{name} {space:?} edge={max_edge} fell back to CPU"
                );
                let rms = rms_f32_diff(gpu_f32.pixels(), cpu.pixels());
                assert!(
                    rms <= 2e-4,
                    "{name} {space:?} edge={max_edge}: f32 RMS {rms} > 2e-4"
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
                    exact_ratio >= 0.995,
                    "{name} {space:?} edge={max_edge}: 8-bit exact ratio {exact_ratio:.5} < 0.995"
                );
            }
        }
    }
    Ok(())
}

// -----------------------------------------------------------------------------
// M4: HDR master, single-pass gain-map, double-buffered staging & source-strip tests
// -----------------------------------------------------------------------------

/// Builds a synthetic high-dynamic-range camera image with scene highlights well above
/// white (up to ~6.0) alongside midtones, dark shadows, and sharp edges.
fn synthetic_hdr_photo(w: u32, h: u32) -> CameraImage {
    let mut im = synthetic_photo(w, h);
    let mut pixels = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        let fy = y as f32 / h.max(1) as f32;
        for x in 0..w {
            let fx = x as f32 / w.max(1) as f32;
            let bar = if ((x / 13) + (y / 11)) % 2 == 0 {
                0.15
            } else {
                0.0
            };
            // Bright specular band near top-right reaching ~6x white for HDR headroom tests.
            let specular = if fx > 0.55 && fy < 0.45 {
                4.5 * (fx - 0.55) / 0.45 * (0.45 - fy) / 0.45
            } else {
                0.0
            };
            let r = 0.03 + 1.4 * fx + bar + specular;
            let g = 0.04 + 1.2 * fy + 0.25 * (fx * 5.0).sin().abs() + bar + specular * 0.9;
            let b = 0.03 + 1.1 * (1.0 - fx) * (0.3 + 0.7 * fy) + specular * 0.75;
            pixels.push([r, g, b]);
        }
    }
    im.pixels = pixels.into();
    im
}

fn synthetic_hdr_file(w: u32, h: u32) -> CameraImage {
    use crate::camera_data::{HdrLight, Source};
    let mut pixels = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        let fy = y as f32 / h.max(1) as f32;
        for x in 0..w {
            let fx = x as f32 / w.max(1) as f32;
            let edge = if (x / 12 + y / 12) % 2 == 0 { 0.4 } else { 0.0 };
            pixels.push([
                0.05 + 3.2 * fx + edge,
                0.06 + 2.6 * fy + edge,
                0.08 + 1.8 * (1. - fx),
            ]);
        }
    }
    let light = HdrLight::new(
        w,
        h,
        pixels.clone(),
        crate::color::hdr_output::HdrTransfer::Pq,
    )
    .expect("valid HDR light");
    let metadata = Metadata {
        width: w,
        height: h,
        raw_width: w,
        raw_height: h,
        wb: [1.; 3],
        daylight_wb: [1.; 3],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        source: Source::Rendered,
        ..Default::default()
    };
    CameraImage {
        width: w,
        height: h,
        recovered: Default::default(),
        pixels: pixels.iter().map(|p| p.map(|v| v.clamp(0., 1.))).collect(),
        metadata,
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    }
    .with_hdr(Arc::new(light))
    .expect("attach HDR light")
}

fn hdr_profiled_recipe(im: &CameraImage) -> Recipe {
    let mut r = profiled_recipe(im);
    r.hdr.enabled = true;
    r.hdr.limit = 2.3;
    r
}

/// Asserts that `got` matches `want` on an HDR linear ProPhoto master using the same
/// three-tier relative tolerance as `gpu_develop_matches_cpu_hdr_master` (`p999 <= 2e-3`,
/// `max <= 1e-2`, `mean <= 2e-4`).
fn assert_hdr_matches(
    got: &crate::rendered::HdrImage,
    want: &crate::rendered::HdrImage,
    label: &str,
) {
    assert_eq!(
        (got.width, got.height),
        (want.width, want.height),
        "{label} size"
    );
    assert_eq!(got.peak, want.peak, "{label} peak");
    assert_eq!(got.pixels.len(), want.pixels.len(), "{label} len");
    let mut diffs = Vec::with_capacity(got.pixels.len() * 3);
    let mut sum_rel = 0.0f64;
    for (i, (a, b)) in got.pixels.iter().zip(&want.pixels).enumerate() {
        for c in 0..3 {
            assert!(
                a[c].is_finite() && a[c] >= 0.0,
                "{label} px {i}[{c}]={}",
                a[c]
            );
            let denom = b[c].abs().max(0.05);
            let rel = (a[c] - b[c]).abs() / denom;
            diffs.push(rel);
            sum_rel += f64::from(rel);
        }
    }
    diffs.sort_by(f32::total_cmp);
    let max_rel = diffs.last().copied().unwrap_or(0.0);
    let p999 = diffs
        .get((diffs.len() * 999) / 1000)
        .copied()
        .unwrap_or(0.0);
    let mean_rel = sum_rel / diffs.len().max(1) as f64;
    assert!(
        p999 <= 2e-3 && max_rel <= 1e-2 && mean_rel <= 2e-4,
        "{label}: p999={p999:.6}, max_rel={max_rel:.6}, mean_rel={mean_rel:.6}"
    );
}

#[test]
fn source_strip_bounds_covers_geometry_lens_and_xtrans() {
    let mut im = synthetic_photo(240, 180);
    // Attach a non-trivial radial distortion + lateral CA lens correction.
    let distortion =
        rawmakase_model::optics::Radial::new(vec![0.0, 0.5, 1.0, 1.5], vec![1.0, 1.01, 1.04, 1.08])
            .unwrap();
    let ca_r =
        rawmakase_model::optics::Radial::new(vec![0.0, 1.0, 1.5], vec![1.0, 1.003, 1.006]).unwrap();
    let ca_b =
        rawmakase_model::optics::Radial::new(vec![0.0, 1.0, 1.5], vec![1.0, 0.997, 0.994]).unwrap();
    im.metadata.lens = Some(rawmakase_model::optics::LensCorrection {
        distortion: Some(distortion),
        chromatic: Some([ca_r, ca_b]),
        default_on: true,
        ..Default::default()
    });

    let check_case = |label: &str, im: &CameraImage, r: &Recipe, y0: u32, y1: u32| {
        let g = Geometry::new(im, r, 0);
        let (sy0, sy1) = source_strip_bounds(im, r, &g, y0, y1);
        assert!(
            sy0 < sy1 && sy1 <= im.height,
            "{label}: invalid ({sy0}, {sy1})"
        );
        let lens = crate::develop::image_space::LensMap::new(im, r);
        let fw = g.width as f32;
        let fh = g.height as f32;
        let extra = if im.metadata.xtrans && lens.is_some() {
            15.0
        } else {
            1.0
        };
        for oy in y0..y1 {
            for ox in (0..g.width).step_by(3) {
                let [ax, ay] = g.source((ox as f32 + 0.5) / fw, (oy as f32 + 0.5) / fh);
                if let Some(lens) = &lens {
                    let ([_, dy], scales) = lens.scales(ax, ay);
                    for s in scales {
                        let sy = lens.center[1] + dy * s - 0.5;
                        if sy >= 0.0 && sy <= (im.height - 1) as f32 {
                            assert!(
                                (sy - extra).max(0.0) >= sy0 as f32
                                    && (sy + extra).min((im.height - 1) as f32) < sy1 as f32,
                                "{label}: ({ox},{oy}) mapped sy={sy} outside [{sy0},{sy1})"
                            );
                        }
                    }
                } else if ay >= 0.0 && ay <= (im.height - 1) as f32 {
                    assert!(
                        (ay - 1.0).max(0.0) >= sy0 as f32
                            && (ay + 1.0).min((im.height - 1) as f32) < sy1 as f32,
                        "{label}: ({ox},{oy}) mapped ay={ay} outside [{sy0},{sy1})"
                    );
                }
            }
        }
        (sy0, sy1)
    };

    // 1. Identity band is tightly bounded around [40..80) (not full height).
    let base = profiled_recipe(&im);
    let (sy0, sy1) = check_case("identity", &im, &base, 40, 80);
    assert!(sy1 - sy0 < 60, "expected tight strip, got [{sy0}, {sy1})");

    // 2. Crop + straighten + lens distortion.
    let mut tilted = base.clone();
    tilted.crop = [0.1, 0.1, 0.9, 0.9];
    tilted.straighten = 6.0;
    tilted.lens_builtin = true;
    tilted.lens_ca = true;
    let g_tilted = Geometry::new(&im, &tilted, 0);
    check_case("tilted_lens", &im, &tilted, 20, g_tilted.height.min(60));

    // 3. 90-degree rotation spans almost the full source height even for a narrow band.
    let mut rotated = base.clone();
    rotated.rotation = 1;
    let (ry0, ry1) = check_case("rot90", &im, &rotated, 10, 20);
    assert!(
        ry1 - ry0 > im.height / 2,
        "rot90 strip should span source height: [{ry0},{ry1})"
    );

    // 4. X-Trans with lens CA adds the wider vertical coherence margin.
    im.metadata.xtrans = true;
    let mut xtrans_r = base;
    xtrans_r.lens_builtin = true;
    xtrans_r.lens_ca = true;
    let (xy0, xy1) = check_case("xtrans_ca", &im, &xtrans_r, 60, 80);
    assert!(
        xy0 <= 42 && xy1 >= 98,
        "expected xtrans margin in [{xy0}, {xy1})"
    );
}

#[test]
fn hdr_and_gain_map_cpu_only_and_mock_fallbacks() -> Result<()> {
    let im = synthetic_hdr_photo(64, 48);
    let mut r = hdr_profiled_recipe(&im);
    r.sharpening = 0.4;
    let valid = r.checked()?;

    // 1. CpuOnly backend matches CPU oracle bit-for-bit for both `render_hdr` and `render_hdr_and_sdr`.
    let cpu_backend = ExportBackend::cpu_only();
    let oracle_hdr = crate::develop::render_hdr(&im, &valid, None, 32, Cancel::NEVER)?;
    let oracle_sdr =
        crate::develop::render_export(&im, &valid, None, 32, OutputSpace::Srgb, Cancel::NEVER)?;
    let got_hdr = cpu_backend.render_hdr(&im, &valid, None, 32, Cancel::NEVER)?;
    assert_eq!(got_hdr.pixels, oracle_hdr.pixels);
    let (pair_hdr, pair_sdr) =
        cpu_backend.render_hdr_and_sdr(&im, &valid, None, 32, Cancel::NEVER)?;
    assert_eq!(pair_hdr.pixels, oracle_hdr.pixels);
    assert_eq!(pair_sdr.pixels(), oracle_sdr.pixels());
    let st = cpu_backend.status();
    assert_eq!(st.gpu_renders, 0);
    assert_eq!(st.cpu_renders, 3);
    assert_eq!(st.fallback_count(ExportFallbackKind::ConfiguredCpu), 3);

    // 2. Unsupported HDR feature (vignette != 0) falls back Tier-1 without touching GPU.
    let (mock, handle) = ScriptedMock::new([MockAction::Succeed]);
    let backend = ExportBackend::with_mock("Mock GPU", Box::new(mock));
    let mut vig_r = r.clone();
    vig_r.effects.vignette = -0.3;
    let vig_valid = vig_r.checked()?;
    let vig_out = backend.render_hdr(&im, &vig_valid, None, 0, Cancel::NEVER)?;
    assert_eq!(
        vig_out.pixels,
        crate::develop::render_hdr(&im, &vig_valid, None, 0, Cancel::NEVER)?.pixels
    );
    assert_eq!(handle.calls.load(Ordering::SeqCst), 0);
    assert!(backend.status().gpu_active);
    assert_eq!(
        backend
            .status()
            .fallback_count(ExportFallbackKind::UnsupportedRecipe),
        1
    );

    // 3. Single-pass `render_hdr_and_sdr` invokes GPU once (one `gpu_renders` increment),
    //    whereas an edit with SDR rendition sliders falls back to separate calls (HDR on GPU,
    //    SDR on CPU).
    let (mock, handle) = ScriptedMock::new([MockAction::Succeed, MockAction::Succeed]);
    let backend = ExportBackend::with_mock("Mock GPU", Box::new(mock));
    let _ = backend.render_hdr_and_sdr(&im, &valid, None, 0, Cancel::NEVER)?;
    assert_eq!(backend.status().gpu_renders, 1);
    assert_eq!(backend.status().cpu_renders, 0);
    // With SDR rendition slider active, `can_share` is false -> HDR runs on GPU, SDR falls back to CPU.
    let mut sdr_slider = r.clone();
    sdr_slider.hdr.sdr.brightness = 0.25;
    let sdr_valid = sdr_slider.checked()?;
    let _ = backend.render_hdr_and_sdr(&im, &sdr_valid, None, 0, Cancel::NEVER)?;
    assert_eq!(backend.status().gpu_renders, 2);
    assert_eq!(backend.status().cpu_renders, 1);
    assert_eq!(
        backend
            .status()
            .fallback_count(ExportFallbackKind::UnsupportedRecipe),
        1
    );
    let _ = handle;

    // 4. HDR OOM retry halves band height and succeeds; cancellation aborts without CPU fallback.
    struct HdrOomCancelMock {
        attempt: usize,
        seen_caps: Arc<Mutex<Vec<Option<u32>>>>,
    }
    impl MockGpuDriver for HdrOomCancelMock {
        fn render_banded(
            &mut self,
            _: &PreparedExport<'_>,
            _: Option<u32>,
            _: Cancel<'_>,
        ) -> std::result::Result<ExportImage, GpuFailure> {
            unreachable!()
        }
        fn render_hdr_banded(
            &mut self,
            job: &PreparedHdrExport<'_>,
            band_cap: Option<u32>,
            cancel: Cancel<'_>,
        ) -> std::result::Result<crate::rendered::HdrImage, GpuFailure> {
            self.seen_caps.lock().unwrap().push(band_cap);
            self.attempt += 1;
            if cancel.is_cancelled() {
                return Err(GpuFailure::Cancelled);
            }
            if self.attempt == 1 {
                return Err(GpuFailure::OutOfMemory("simulated HDR OOM".into()));
            }
            let n = job.out_w as usize * job.out_h as usize;
            Ok(crate::rendered::HdrImage {
                width: job.out_w,
                height: job.out_h,
                pixels: vec![[1.25; 3]; n],
                peak: job.peak,
            })
        }
        fn release_buffers(&mut self) {}
    }
    let seen = Arc::new(Mutex::new(Vec::new()));
    let backend = ExportBackend::with_mock(
        "HDR OOM Mock",
        Box::new(HdrOomCancelMock {
            attempt: 0,
            seen_caps: seen.clone(),
        }),
    );
    let out = backend.render_hdr(&im, &valid, None, 0, Cancel::NEVER)?;
    assert_eq!(out.pixels[0], [1.25; 3]);
    let caps = seen.lock().unwrap();
    assert_eq!(caps.len(), 2);
    assert!(caps[0].is_none() && caps[1].is_some());
    drop(caps);

    let flag = AtomicBool::new(true);
    let err = backend.render_hdr(&im, &valid, None, 0, Cancel::new(&flag));
    assert!(err.is_err());
    assert!(backend.status().gpu_active);
    assert_eq!(backend.status().cpu_renders, 0);
    Ok(())
}

/// Hardware GPU test: verifies HDR master export (`render_hdr`) and single-pass gain-map
/// export (`render_hdr_and_sdr`) match the CPU reference across default, heavy (NR + Texture +
/// Clarity + Sharpening + Color Mixer + curves), resized, and HDR-file inputs.
#[test]
#[ignore = "requires a hardware GPU compute adapter"]
fn gpu_hdr_export_matches_cpu_and_gain_map_single_pass() -> Result<()> {
    let im = synthetic_hdr_photo(256, 192);
    let backend = ExportBackend::auto();

    type RecipeEdit = fn(&mut Recipe);
    let cases: [(&str, RecipeEdit); 3] = [
        ("default_hdr", |_| {}),
        ("heavy_hdr_nr_texture_sharpen", |r| {
            r.exposure = 0.35;
            r.contrast = 0.25;
            r.whites = 0.2;
            r.blacks = -0.15;
            r.effects.clarity = 0.2;
            r.noise_chroma = 0.35;
            r.noise_luma = 0.3;
            r.effects.texture = 0.35;
            r.sharpening = 0.65;
            r.hdr.limit = 4.0;
            r.vibrance = 0.2;
            r.saturation = 0.1;
            r.hsl[0][1] = 0.25;
        }),
        ("hdr_cropped_straightened", |r| {
            r.crop = [0.08, 0.1, 0.92, 0.88];
            r.straighten = -3.5;
            r.sharpening = 0.5;
            r.hdr.limit = 2.0;
        }),
    ];

    for (name, edit) in &cases {
        let mut r = hdr_profiled_recipe(&im);
        edit(&mut r);
        let valid = r.checked()?;
        for max_edge in [0u32, 160u32] {
            backend.reset_stats();
            let cpu_hdr = crate::develop::render_hdr(&im, &valid, None, max_edge, Cancel::NEVER)?;
            let gpu_hdr = backend.render_hdr(&im, &valid, None, max_edge, Cancel::NEVER)?;
            assert_eq!(backend.status().gpu_renders, 1, "{name} edge={max_edge}");
            assert_eq!(backend.status().cpu_renders, 0, "{name} edge={max_edge}");
            assert_hdr_matches(&gpu_hdr, &cpu_hdr, &format!("hdr {name} edge={max_edge}"));

            // Single-pass gain-map pair shares photo upload + NR + Texture and records 1 GPU render.
            backend.reset_stats();
            let cpu_sdr = crate::develop::render_export(
                &im,
                &valid,
                None,
                max_edge,
                OutputSpace::Srgb,
                Cancel::NEVER,
            )?;
            let (pair_hdr, pair_sdr) =
                backend.render_hdr_and_sdr(&im, &valid, None, max_edge, Cancel::NEVER)?;
            assert_eq!(
                backend.status().gpu_renders,
                1,
                "pair {name} edge={max_edge} should execute in one GPU pass"
            );
            assert_eq!(
                backend.status().cpu_renders,
                0,
                "pair {name} edge={max_edge}"
            );
            assert_hdr_matches(
                &pair_hdr,
                &cpu_hdr,
                &format!("pair_hdr {name} edge={max_edge}"),
            );
            let (max8, diff_px) = max_u8_diff(&pair_sdr.rgb8(), &cpu_sdr.rgb8());
            let total = (cpu_sdr.width() * cpu_sdr.height()) as usize;
            let exact = 1.0 - diff_px as f64 / total as f64;
            assert!(
                max8 <= 1 && exact >= 0.995,
                "pair_sdr {name} edge={max_edge}: max8={max8}, exact={exact:.5}"
            );
        }
    }

    // HDR file (`metadata.hdr_source.is_some()`): `render_hdr` runs on GPU and
    // `render_hdr_and_sdr` falls back to separate HDR + SDR GPU passes cleanly.
    let hdr_file = synthetic_hdr_file(160, 120);
    let mut file_r = Recipe::for_new_rendered(&hdr_file.metadata);
    file_r.exposure = 0.5;
    file_r.hdr.limit = 4.0;
    let file_valid = file_r.checked()?;
    for max_edge in [0u32, 96u32] {
        backend.reset_stats();
        let cpu_hdr =
            crate::develop::render_hdr(&hdr_file, &file_valid, None, max_edge, Cancel::NEVER)?;
        let gpu_hdr = backend.render_hdr(&hdr_file, &file_valid, None, max_edge, Cancel::NEVER)?;
        assert_eq!(backend.status().gpu_renders, 1);
        assert_eq!(backend.status().cpu_renders, 0);
        assert_hdr_matches(&gpu_hdr, &cpu_hdr, &format!("hdr_file edge={max_edge}"));

        // Gain-map pair on an HDR file separates into HDR GPU + SDR GPU passes cleanly.
        backend.reset_stats();
        let cpu_sdr = crate::develop::render_export(
            &hdr_file,
            &file_valid,
            None,
            max_edge,
            OutputSpace::Srgb,
            Cancel::NEVER,
        )?;
        let (pair_hdr, pair_sdr) =
            backend.render_hdr_and_sdr(&hdr_file, &file_valid, None, max_edge, Cancel::NEVER)?;
        assert_hdr_matches(
            &pair_hdr,
            &cpu_hdr,
            &format!("hdr_file pair_hdr edge={max_edge}"),
        );
        let (max8, _) = max_u8_diff(&pair_sdr.rgb8(), &cpu_sdr.rgb8());
        assert!(max8 <= 1, "hdr_file pair_sdr edge={max_edge}: max8={max8}");
    }
    Ok(())
}

/// Hardware GPU test: verifies HDR master band seams are bit-exact between single-band and
/// multi-band (6 bands with double-buffered staging overlap) for both full-res and resized output.
#[test]
#[ignore = "requires a hardware GPU compute adapter"]
fn gpu_hdr_export_band_seam_bit_exactness() -> Result<()> {
    let im = synthetic_hdr_photo(384, 340);
    let mut r = hdr_profiled_recipe(&im);
    r.sharpening = 0.7;
    r.noise_chroma = 0.3;
    r.noise_luma = 0.25;
    r.effects.texture = 0.3;
    r.hdr.limit = 3.0;
    let valid = r.checked()?;

    let backend = ExportBackend::auto();
    for max_edge in [0u32, 260u32] {
        backend.set_band_height_override(Some(u32::MAX));
        let single = backend.render_hdr(&im, &valid, None, max_edge, Cancel::NEVER)?;
        backend.set_band_height_override(Some(64));
        let multi = backend.render_hdr(&im, &valid, None, max_edge, Cancel::NEVER)?;
        backend.set_band_height_override(None);

        assert_eq!(backend.status().cpu_renders, 0);
        assert_eq!((single.width, single.height), (multi.width, multi.height));
        for (i, (a, b)) in single.pixels.iter().zip(&multi.pixels).enumerate() {
            let bits_a = a.map(f32::to_bits);
            let bits_b = b.map(f32::to_bits);
            assert_eq!(
                bits_a, bits_b,
                "HDR seam mismatch at pixel {i} (max_edge={max_edge}): {a:?} vs {b:?}"
            );
        }
    }
    Ok(())
}

/// Hardware GPU test: forces source-strip upload mode via `set_max_binding_override` and
/// verifies bit-accurate SDR, HDR, and gain-map parity across crop, straighten, lens
/// distortion/CA, X-Trans, NR, and Texture, plus clean Tier-1 `ExceedsLimits` fallback when
/// a 90-degree rotation makes even a single-row strip exceed the storage limit.
#[test]
#[ignore = "requires a hardware GPU compute adapter"]
fn gpu_export_source_strip_banding_under_simulated_storage_limit() -> Result<()> {
    let mut im = synthetic_hdr_photo(320, 260);
    let distortion = rawmakase_model::optics::Radial::new(
        vec![0.0, 0.5, 1.0, 1.5],
        vec![1.0, 1.008, 1.02, 1.04],
    )
    .unwrap();
    let ca_r =
        rawmakase_model::optics::Radial::new(vec![0.0, 1.0, 1.5], vec![1.0, 1.002, 1.004]).unwrap();
    let ca_b =
        rawmakase_model::optics::Radial::new(vec![0.0, 1.0, 1.5], vec![1.0, 0.998, 0.996]).unwrap();
    im.metadata.lens = Some(rawmakase_model::optics::LensCorrection {
        distortion: Some(distortion),
        chromatic: Some([ca_r, ca_b]),
        default_on: true,
        ..Default::default()
    });

    // Full photo is 320 * 260 * 12 = 998,400 bytes; cap at 450,000 bytes so full-photo upload
    // cannot fit and source-strip upload banding is forced.
    let strip_limit = 450_000u64;
    let backend = ExportBackend::auto();

    let mut r = hdr_profiled_recipe(&im);
    r.crop = [0.06, 0.08, 0.94, 0.92];
    r.straighten = 3.0;
    r.lens_builtin = true;
    r.lens_ca = true;
    r.noise_chroma = 0.35;
    r.noise_luma = 0.25;
    r.effects.texture = 0.35;
    r.sharpening = 0.6;
    let valid = r.checked()?;

    for max_edge in [0u32, 200u32] {
        let cpu_sdr = crate::develop::render_export(
            &im,
            &valid,
            None,
            max_edge,
            OutputSpace::Srgb,
            Cancel::NEVER,
        )?;
        let cpu_hdr = crate::develop::render_hdr(&im, &valid, None, max_edge, Cancel::NEVER)?;

        backend.reset_stats();
        backend.set_max_binding_override(Some(strip_limit));
        let gpu_sdr = backend.render_export(
            &im,
            &valid,
            None,
            max_edge,
            OutputSpace::Srgb,
            ExportFormat::Rgb8,
            Cancel::NEVER,
        )?;
        let (gpu_hdr, pair_sdr) =
            backend.render_hdr_and_sdr(&im, &valid, None, max_edge, Cancel::NEVER)?;
        backend.set_max_binding_override(None);

        assert_eq!(
            backend.status().gpu_renders,
            2,
            "strip mode edge={max_edge}"
        );
        assert_eq!(
            backend.status().cpu_renders,
            0,
            "strip mode edge={max_edge}"
        );

        let (max8, diff_px) = max_u8_diff(gpu_sdr.as_rgb8().unwrap(), &cpu_sdr.rgb8());
        let exact = 1.0 - diff_px as f64 / (cpu_sdr.width() * cpu_sdr.height()) as f64;
        assert!(
            max8 <= 1 && exact >= 0.995,
            "strip SDR edge={max_edge}: max8={max8}, exact={exact:.5}"
        );
        let (pmax8, pdiff) = max_u8_diff(&pair_sdr.rgb8(), &cpu_sdr.rgb8());
        let pexact = 1.0 - pdiff as f64 / (cpu_sdr.width() * cpu_sdr.height()) as f64;
        assert!(
            pmax8 <= 1 && pexact >= 0.995,
            "strip pair SDR edge={max_edge}: max8={pmax8}, exact={pexact:.5}"
        );
        assert_hdr_matches(&gpu_hdr, &cpu_hdr, &format!("strip HDR edge={max_edge}"));
    }

    // X-Trans with lateral CA under strip mode.
    im.metadata.xtrans = true;
    let mut xtrans_r = profiled_recipe(&im);
    xtrans_r.lens_builtin = true;
    xtrans_r.lens_ca = true;
    xtrans_r.sharpening = 0.4;
    let xtrans_valid = xtrans_r.checked()?;
    let cpu_xtrans = crate::develop::render_export(
        &im,
        &xtrans_valid,
        None,
        0,
        OutputSpace::Srgb,
        Cancel::NEVER,
    )?;
    backend.reset_stats();
    backend.set_max_binding_override(Some(strip_limit));
    let gpu_xtrans = backend.render_export(
        &im,
        &xtrans_valid,
        None,
        0,
        OutputSpace::Srgb,
        ExportFormat::Rgb8,
        Cancel::NEVER,
    )?;
    backend.set_max_binding_override(None);
    assert_eq!(backend.status().gpu_renders, 1);
    assert_eq!(backend.status().cpu_renders, 0);
    let (max8, _) = max_u8_diff(gpu_xtrans.as_rgb8().unwrap(), &cpu_xtrans.rgb8());
    assert!(max8 <= 1, "strip X-Trans max8={max8}");

    // 90-degree rotation makes even a 1-row output band span the full source height
    // (~998 KB > 450 KB), triggering Tier-1 `ExceedsLimits` while keeping GPU active.
    im.metadata.xtrans = false;
    let mut rot_r = profiled_recipe(&im);
    rot_r.rotation = 1;
    let rot_valid = rot_r.checked()?;
    backend.reset_stats();
    backend.set_max_binding_override(Some(strip_limit));
    let rot_out = backend.render_export(
        &im,
        &rot_valid,
        None,
        0,
        OutputSpace::Srgb,
        ExportFormat::Rgb8,
        Cancel::NEVER,
    )?;
    backend.set_max_binding_override(None);
    let rot_cpu =
        crate::develop::render_export(&im, &rot_valid, None, 0, OutputSpace::Srgb, Cancel::NEVER)?;
    assert_eq!(rot_out.as_rgb8().unwrap(), rot_cpu.rgb8().as_slice());
    let st = backend.status();
    assert!(st.gpu_active, "Tier-1 ExceedsLimits must keep GPU active");
    assert_eq!(st.cpu_renders, 1);
    assert_eq!(st.fallback_count(ExportFallbackKind::ExceedsLimits), 1);

    // Regression: after a strip-mode export creates `buffers.photo` without `COPY_SRC`,
    // a subsequent smaller full-photo export whose size sits inside the cached buffer's
    // reuse window and triggers `readback_photo` (NR + Clone retouch) must reallocate with
    // `COPY_SRC` rather than hitting a validation error and disabling the GPU.
    use crate::model::retouch::{RetouchMode, RetouchOp, RetouchShape};
    let small = synthetic_photo(140, 140);
    let mut nr_retouch = profiled_recipe(&small);
    nr_retouch.noise_chroma = 0.35;
    nr_retouch.noise_luma = 0.3;
    nr_retouch.retouch.push(RetouchOp {
        mode: RetouchMode::Clone,
        shape: RetouchShape::Spot {
            center: [0.45, 0.5],
            radius: 0.06,
        },
        feather: 0.5,
        opacity: 0.9,
        offset: [0.15, 0.0],
        patch: None,
    });
    let nr_retouch_valid = nr_retouch.checked()?;
    backend.reset_stats();
    let small_gpu = backend.render_export(
        &small,
        &nr_retouch_valid,
        None,
        0,
        OutputSpace::Srgb,
        ExportFormat::Rgb8,
        Cancel::NEVER,
    )?;
    let small_cpu = crate::develop::render_export(
        &small,
        &nr_retouch_valid,
        None,
        0,
        OutputSpace::Srgb,
        Cancel::NEVER,
    )?;
    let st = backend.status();
    assert!(
        st.gpu_active,
        "GPU must remain active after strip -> full-photo readback transition: {:?}",
        st.fallback_reason
    );
    assert_eq!(st.gpu_renders, 1);
    assert_eq!(st.cpu_renders, 0);
    let (max8, _) = max_u8_diff(small_gpu.as_rgb8().unwrap(), &small_cpu.rgb8());
    assert!(max8 <= 1, "strip -> full-photo NR+retouch max8={max8}");
    Ok(())
}

/// Deterministic regression test: `build_pass_plan` bails out immediately when a
/// 90-degree-rotated large sensor cannot fit even a single-row source strip within
/// `effective_max_binding`, avoiding repeated full-photo band scans.
#[test]
fn build_pass_plan_bails_early_on_rotated_unfit_strip() {
    let im = synthetic_photo(640, 480);
    let mut r = profiled_recipe(&im);
    r.rotation = 1;
    let g = Geometry::new(&im, &r, 0);
    let strip_limit = 500_000u64; // full source is 640 * 480 * 12 = 3,686,400 B
    let res = build_pass_plan(
        &im,
        &r,
        &r,
        g,
        im.photo_scale(),
        g.width,
        g.height,
        ExportTarget::Sdr {
            space: OutputSpace::Srgb,
            format: ExportFormat::Rgb8,
        },
        true,
        None,
        strip_limit,
        65535,
    );
    match res {
        Err(GpuFailure::PerPhoto(ExportFallbackKind::ExceedsLimits, _)) => {}
        Ok(_) => panic!("expected ExceedsLimits for rotated unfit strip"),
        Err(_) => panic!("expected PerPhoto(ExceedsLimits)"),
    }
}
