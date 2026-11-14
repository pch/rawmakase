use super::*;
use crate::rendered::Rendered;

fn gray(width: u32, height: u32, v: f32) -> Rendered {
    Rendered {
        width,
        height,
        pixels: vec![[v; 3]; (width * height) as usize],
    }
}
/// A graphic preset over a `w`×`h` PNG (red, with a transparent left half)
/// saved in a fresh folder.
fn graphic(w: u32, h: u32, sixteen: bool) -> Result<(tempfile::TempDir, Watermark)> {
    let dir = tempfile::tempdir()?;
    let source = dir.path().join("logo.png");
    if sixteen {
        let img = image::ImageBuffer::from_fn(w, h, |x, _| {
            image::Rgba([65535u16, 0, 0, if x < w / 2 { 0 } else { 65535 }])
        });
        img.save(&source)?;
    } else {
        let img = image::ImageBuffer::from_fn(w, h, |x, _| {
            image::Rgba([255u8, 0, 0, if x < w / 2 { 0 } else { 255 }])
        });
        img.save(&source)?;
    }
    let store = dir.path().join("watermarks");
    let saved = save_in(
        &store,
        &Watermark {
            name: "Logo".into(),
            style: Style::Graphic,
            inset: [0., 0.],
            ..Default::default()
        },
        None,
        Some(&source),
    )?;
    Ok((dir, saved))
}

#[test]
fn each_anchor_puts_the_mark_on_its_point_with_its_inset() -> Result<()> {
    let (dir, mut w) = graphic(10, 10, false)?;
    let images = dir.path().join("watermarks/images");
    w.size = Size::Proportional(0.1);
    w.inset = [0.05, 0.1];
    for (anchor, at) in [
        (Anchor(0, 0), (10, 10)),
        (Anchor(1, 1), (90, 40)),
        (Anchor(2, 2), (170, 70)),
        (Anchor(2, 0), (170, 10)),
        (Anchor(0, 2), (10, 70)),
    ] {
        w.anchor = anchor;
        let placed = w.ready_in(&images)?.place(200, 100).unwrap();
        assert_eq!((placed.width, placed.height), (20, 20), "{anchor:?}");
        assert_eq!((placed.x, placed.y), at, "{anchor:?}");
    }
    Ok(())
}

#[test]
fn fit_and_fill_size_the_mark_to_the_photo() -> Result<()> {
    let (dir, mut w) = graphic(40, 10, false)?;
    let images = dir.path().join("watermarks/images");
    w.inset = [0.1, 0.1];
    w.size = Size::Fit;
    let p = w.ready_in(&images)?.place(200, 100).unwrap();
    assert_eq!((p.width, p.height), (160, 40));
    // Fit stays within the photo; Fill covers it, cut at its edges.
    let (dir, mut w) = graphic(10, 40, false)?;
    let images = dir.path().join("watermarks/images");
    w.inset = [0., 0.];
    w.size = Size::Fit;
    let p = w.ready_in(&images)?.place(200, 100).unwrap();
    assert_eq!((p.width, p.height), (25, 100));
    w.size = Size::Fill;
    let p = w.ready_in(&images)?.place(200, 100).unwrap();
    // Twice the photo at most: 200 high, so 50 wide.
    assert_eq!((p.width, p.height), (50, 200));
    Ok(())
}

#[test]
fn rotation_turns_the_mark_in_quarter_turns() -> Result<()> {
    let (dir, mut w) = graphic(20, 10, false)?;
    let images = dir.path().join("watermarks/images");
    w.size = Size::Proportional(0.1);
    w.rotation = 1;
    let p = w.ready_in(&images)?.place(100, 100).unwrap();
    // Turned upright: 10 wide (a tenth of the photo), 20 tall.
    assert_eq!((p.width, p.height), (10, 20));
    // The transparent left half is now at the bottom.
    assert_eq!(p.rgba[(p.height - 1) * p.width][3], 0.);
    assert_eq!(p.rgba[0][3], 1.);
    Ok(())
}

#[test]
fn png_alpha_is_honoured_and_untouched_pixels_keep_full_precision() -> Result<()> {
    let (dir, mut w) = graphic(100, 50, true)?;
    let images = dir.path().join("watermarks/images");
    w.size = Size::Fit;
    w.opacity = 0.5;
    w.anchor = Anchor(0, 0);
    // A value 16-bit TIFF holds and 8-bit can't.
    let v = 1000. / 65535.;
    let mut image = crate::develop::ExportImage::srgb(gray(100, 100, v));
    w.ready_in(&images)?.apply(&mut image);
    let pixels = image.pixels();
    // Outside the mark, and under its transparent half, exactly as rendered.
    assert_eq!(pixels[99 * 100 + 99], [v; 3]);
    assert_eq!(pixels[10 * 100 + 10], [v; 3]);
    // Under the half-opaque red, the exact mix.
    let p = pixels[10 * 100 + 90];
    assert!((p[0] - (0.5 + v * 0.5)).abs() < 1e-6);
    assert!((p[1] - v * 0.5).abs() < 1e-6);
    Ok(())
}

#[test]
fn a_missing_image_fails_before_anything_is_drawn() {
    let w = Watermark {
        style: Style::Graphic,
        image: Some("gone.png".into()),
        ..Default::default()
    };
    let error = w.ready_in(Path::new("/nonexistent")).err().unwrap();
    assert_eq!(error.to_string(), "Watermark image not found: gone.png");
}

#[test]
fn deleting_a_preset_removes_its_own_image() -> Result<()> {
    let (dir, w) = graphic(4, 4, false)?;
    let store = dir.path().join("watermarks");
    let image = store.join("images").join(w.image.clone().unwrap());
    assert!(image.is_file());
    assert_eq!(presets_in(&store), vec![w.clone()]);
    // The source can go: the preset has its own copy.
    std::fs::remove_file(dir.path().join("logo.png"))?;
    assert!(w.ready_in(&store.join("images")).is_ok());
    delete_in(&store, &w)?;
    assert!(!image.exists());
    assert!(presets_in(&store).is_empty());
    Ok(())
}

#[test]
fn text_is_drawn_with_smooth_edges_and_a_shadow() -> Result<()> {
    let mut w = Watermark {
        text: "Ag © 2026".into(),
        size: Size::Proportional(0.5),
        color: [1., 1., 1.],
        ..Default::default()
    };
    let plain = w.ready()?.place(400, 200).unwrap();
    assert!((plain.width as f32 - 200.).abs() < 12.);
    let alphas: Vec<f32> = plain.rgba.iter().map(|p| p[3]).collect();
    assert!(alphas.iter().any(|a| *a > 0.99), "solid strokes");
    assert!(
        alphas.iter().any(|a| *a > 0.05 && *a < 0.95),
        "antialiased edges"
    );
    assert!(alphas.iter().all(|a| (0. ..=1.).contains(a)));
    // The shadow darkens around the text, not over it.
    w.shadow.enabled = true;
    w.shadow.opacity = 1.;
    let shadowed = w.ready()?.place(400, 200).unwrap();
    let covered = |p: &Placed| p.rgba.iter().filter(|c| c[3] > 0.01).count();
    assert!(covered(&shadowed) > covered(&plain));
    assert!(
        shadowed.rgba.iter().any(|c| c[3] > 0.3 && c[0] < 0.2),
        "dark shadow pixels"
    );
    Ok(())
}

#[test]
fn the_weight_follows_the_style() -> Result<()> {
    let ink = |face: &str| -> Result<f32> {
        let w = Watermark {
            text: "Heavy".into(),
            face: face.into(),
            size: Size::Proportional(0.5),
            ..Default::default()
        };
        let placed = w.ready()?.place(400, 200).unwrap();
        Ok(placed.rgba.iter().map(|p| p[3]).sum::<f32>() / (placed.width * placed.height) as f32)
    };
    assert!(ink("Bold")? > ink("Regular")? * 1.2);
    Ok(())
}

#[test]
fn renaming_a_preset_keeps_its_image() -> Result<()> {
    let (dir, w) = graphic(4, 4, false)?;
    let store = dir.path().join("watermarks");
    let renamed = save_in(
        &store,
        &Watermark {
            name: "Studio".into(),
            ..w.clone()
        },
        Some(&w.name),
        None,
    )?;
    let names: Vec<String> = presets_in(&store).into_iter().map(|p| p.name).collect();
    assert_eq!(names, ["Studio"]);
    assert!(renamed.ready_in(&store.join("images")).is_ok());
    // A new preset of the old name doesn't share the renamed one's image.
    assert_ne!(renamed.image, w.image);
    // Renamed with a new image: the old preset and its image go.
    let source = dir.path().join("other.png");
    image::RgbaImage::from_pixel(2, 2, image::Rgba([0, 0, 255, 255])).save(&source)?;
    let again = save_in(
        &store,
        &Watermark {
            name: "Office".into(),
            ..renamed
        },
        Some("Studio"),
        Some(&source),
    )?;
    let names: Vec<String> = presets_in(&store).into_iter().map(|p| p.name).collect();
    assert_eq!(names, ["Office"]);
    let images: Vec<_> = std::fs::read_dir(store.join("images"))?.flatten().collect();
    assert_eq!(images.len(), 1);
    assert!(again.ready_in(&store.join("images")).is_ok());
    Ok(())
}

#[test]
fn names_that_make_the_same_file_and_unreadable_images_are_refused() -> Result<()> {
    let (dir, w) = graphic(4, 4, false)?;
    let store = dir.path().join("watermarks");
    let clash = Watermark {
        name: "Logo?".into(),
        style: Style::Text,
        image: None,
        ..w.clone()
    };
    let named = |name: &str| Watermark {
        name: name.into(),
        ..clash.clone()
    };
    save_in(&store, &named("A/B"), None, None)?;
    assert!(save_in(&store, &named("A?B"), None, None).is_err());
    // A new one, or a rename, onto an existing name is refused too.
    assert!(save_in(&store, &named("Logo"), None, None).is_err());
    assert!(save_in(&store, &named("Logo"), Some("A/B"), None).is_err());
    assert!(save_in(&store, &named("logo"), None, None).is_err());
    assert!(save_in(&store, &named(SIMPLE_COPYRIGHT), None, None).is_err());
    let broken = dir.path().join("broken.png");
    std::fs::write(&broken, b"not a png")?;
    assert!(save_in(&store, &w, Some(&w.name), Some(&broken)).is_err());
    // The preset's image is untouched by the failed save.
    assert!(w.ready_in(&store.join("images")).is_ok());
    Ok(())
}

#[test]
fn transparent_pixels_dont_darken_scaled_edges() -> Result<()> {
    // White where opaque, black (and transparent) elsewhere.
    let dir = tempfile::tempdir()?;
    let source = dir.path().join("edge.png");
    image::ImageBuffer::from_fn(40, 40, |x, _| {
        if x < 20 {
            image::Rgba([0u8, 0, 0, 0])
        } else {
            image::Rgba([255u8, 255, 255, 255])
        }
    })
    .save(&source)?;
    let store = dir.path().join("watermarks");
    let w = save_in(
        &store,
        &Watermark {
            name: "Edge".into(),
            style: Style::Graphic,
            size: Size::Proportional(0.13),
            ..Default::default()
        },
        None,
        Some(&source),
    )?;
    let placed = w.ready_in(&store.join("images"))?.place(100, 100).unwrap();
    // Every visible pixel stays white.
    for p in placed.rgba.iter().filter(|p| p[3] > 0.01) {
        assert!(p[0] > 0.99, "{p:?}");
    }
    Ok(())
}

#[test]
fn a_wide_shadow_on_large_text_stays_cheap() -> Result<()> {
    let mut w = Watermark {
        text: "Wide".into(),
        size: Size::Proportional(1.),
        ..Default::default()
    };
    w.shadow = Shadow {
        enabled: true,
        opacity: 1.,
        offset: 0.4,
        radius: 0.4,
        angle: -45.,
    };
    let started = std::time::Instant::now();
    let placed = w.ready()?.place(3000, 1000).unwrap();
    assert!(started.elapsed().as_secs() < 20);
    assert!(placed.rgba.iter().any(|p| p[3] > 0.5));
    Ok(())
}

#[test]
fn a_replaced_image_of_another_kind_leaves_no_old_copy() -> Result<()> {
    let (dir, w) = graphic(4, 4, false)?;
    let store = dir.path().join("watermarks");
    let jpeg = dir.path().join("new.jpg");
    image::RgbImage::from_pixel(4, 4, image::Rgb([0, 255, 0])).save(&jpeg)?;
    let saved = save_in(&store, &w, Some(&w.name), Some(&jpeg))?;
    let images: Vec<_> = std::fs::read_dir(store.join("images"))?.flatten().collect();
    assert_eq!(images.len(), 1);
    assert_eq!(saved.image.as_deref(), Some("Logo.jpg"));
    Ok(())
}

#[test]
fn a_shadow_never_makes_the_mark_larger_than_the_photo() -> Result<()> {
    let mut w = Watermark {
        text: "Big".into(),
        size: Size::Fit,
        inset: [0., 0.],
        ..Default::default()
    };
    w.shadow = Shadow {
        enabled: true,
        opacity: 1.,
        offset: 0.4,
        radius: 0.4,
        angle: -45.,
    };
    let placed = w.ready()?.place(600, 300).unwrap();
    assert!(
        // The ink fits the photo; the shadow reaches no further than the
        // text's line box, whatever its offset and radius.
        placed.width <= 900 && placed.height <= 450,
        "{}x{}",
        placed.width,
        placed.height
    );
    Ok(())
}

#[test]
fn a_missing_font_style_fails_rather_than_drawing_another() {
    let w = Watermark {
        family: "Inter".into(),
        face: "Regular".into(),
        ..Default::default()
    };
    assert!(w.ready().is_ok());
    assert!(fonts::load("No Such Family", "Bold").is_err());
}

#[test]
fn renaming_by_case_alone_keeps_the_preset() -> Result<()> {
    let (dir, w) = graphic(4, 4, false)?;
    let store = dir.path().join("watermarks");
    let renamed = save_in(
        &store,
        &Watermark {
            name: "LOGO".into(),
            ..w.clone()
        },
        Some(&w.name),
        None,
    )?;
    let names: Vec<String> = presets_in(&store).into_iter().map(|p| p.name).collect();
    assert_eq!(names, ["LOGO"]);
    assert!(renamed.ready_in(&store.join("images")).is_ok());
    Ok(())
}

/// An HDR master gets the mark the sRGB export gets: its white is graphics white
/// (1.0 in linear light), not the light above white it covers; where the master is
/// within sRGB and white, the half-opaque mix is the sRGB export's; pixels outside
/// the mark, bright ones included, keep their exact values.
#[test]
fn an_hdr_master_gets_the_marks_white_at_graphics_white() -> Result<()> {
    use crate::color::{hdr_output::HdrGamut, mul, srgb_decode, srgb_encode};
    let (dir, mut w) = graphic(100, 50, false)?;
    let images = dir.path().join("watermarks/images");
    w.size = Size::Fit;
    w.anchor = Anchor(0, 0);
    let to_prophoto = crate::color::inverse(HdrGamut::Rec709.prophoto_to_rgb());
    let to_srgb = HdrGamut::Rec709.prophoto_to_rgb();
    // Light above white on the top rows, a mid grey (linear 0.2) below.
    let light = |y: u32| if y < 25 { 6. } else { 0.2 };
    let master = |pixels: Vec<[f32; 3]>| crate::rendered::HdrImage {
        width: 100,
        height: 100,
        pixels,
        peak: 16.,
    };
    let mut hdr = master(
        (0..100 * 100)
            .map(|i| mul(to_prophoto, [light(i / 100); 3]))
            .collect(),
    );
    let before = hdr.pixels.clone();
    // Opaque: the red is sRGB's red at graphics white over the bright rows.
    let ready = w.ready_in(&images)?;
    assert!(ready.apply_hdr(&mut hdr));
    let srgb = |p: [f32; 3]| mul(to_srgb, p);
    let red = srgb(hdr.pixels[10 * 100 + 90]);
    assert!((red[0] - 1.).abs() < 1e-4 && red[1].abs() < 1e-4, "{red:?}");
    assert_eq!(hdr.pixels[10 * 100 + 10], before[10 * 100 + 10]);
    assert_eq!(hdr.pixels[99 * 100 + 99], before[99 * 100 + 99]);
    // Half opaque over the grey: the sRGB export's mix, in its encoding.
    w.opacity = 0.5;
    let mut hdr = master(before);
    w.ready_in(&images)?.apply_hdr(&mut hdr);
    let mut sdr = crate::develop::ExportImage::srgb(gray(100, 100, srgb_encode(0.2)));
    w.ready_in(&images)?.apply(&mut sdr);
    let ours = srgb(hdr.pixels[40 * 100 + 90]).map(srgb_encode);
    let theirs = sdr.pixels()[40 * 100 + 90];
    for c in 0..3 {
        assert!((ours[c] - theirs[c]).abs() < 1e-4, "{ours:?} {theirs:?}");
    }
    // Over light above white, the half-opaque red sits between it and the mark.
    let bright = srgb(hdr.pixels[10 * 100 + 90]);
    let expected = srgb_decode(0.5 + 0.5 * srgb_encode(6.));
    assert!((bright[0] - expected).abs() < 1e-3, "{bright:?}");
    assert!(bright[0] < 6. && bright[0] > 1.);
    Ok(())
}
