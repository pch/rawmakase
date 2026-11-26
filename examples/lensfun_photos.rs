//! What RAWmakase's Lensfun matcher makes of photos, as Develop would: each photo
//! opened as Develop opens it, its lens matched in the installed Lensfun database (or
//! the one given), and what the match corrects against the photo's built-in data.
//!
//!     cargo run --release --example lensfun_photos -- [--db data/db] PHOTO...
//!
//! Where the photo has distortion data of its own, it also says how far Lensfun's
//! differs: as sampled at the frame's edge, and in shape (corners matched).
use rawmakase::lens::lensfun::choice::{camera_for, frame_size, photo_of};
use rawmakase::lens::lensfun::matching::{Outcome, find};
use rawmakase::lens::lensfun::{Database, LensfunChoice, installed};
use rawmakase::optics::lensfun::Components;

fn main() -> anyhow::Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let db: Database = if args.first().is_some_and(|a| a == "--db") {
        let dir = args.remove(1);
        args.remove(0);
        installed::read_folder(std::path::Path::new(&dir))?
    } else {
        let lensfun = rawmakase::environment::lensfun(&rawmakase::environment::context()).clone();
        let i = lensfun
            .get()
            .ok_or_else(|| anyhow::anyhow!("Lensfun isn't installed"))?;
        i.db.clone()
    };
    for path in &args {
        let raw = match rawmakase::photo::open(
            std::path::Path::new(path),
            &rawmakase::photo::LensProfiles::none(),
        ) {
            Ok(r) => r,
            Err(e) => {
                println!("{path}: can't open: {e:#}");
                continue;
            }
        };
        let m = &raw.metadata;
        let builtin = [m.lens.as_ref(), m.lens_stand_in.as_ref()]
            .into_iter()
            .flatten()
            .map(Components::of)
            .fold(Components::default(), |a, b| Components {
                distortion: a.distortion || b.distortion,
                chromatic: a.chromatic || b.chromatic,
                vignetting: a.vignetting || b.vignetting,
            });
        if std::env::var_os("DUMP").is_some() {
            dump(m);
        }
        let photo = photo_of(m, frame_size(m));
        let found = find(&db, &photo);
        println!(
            "{}\n  EXIF {:?} {:?}, lens {:?}, {} mm (35 mm: {}), f/{}, frame {:?}; built-in: {}",
            path,
            photo.make,
            photo.model,
            photo.lens,
            m.focal,
            m.focal_35mm,
            m.aperture,
            photo.frame,
            builtin.describe()
        );
        println!(
            "  camera: {}, crop {:?}",
            found.camera.map_or("unknown".to_string(), |c| format!(
                "{} {} (mount {}, crop {})",
                c.maker.id, c.model.id, c.mount, c.crop
            )),
            found.crop
        );
        let lenses = match &found.outcome {
            Outcome::Unique(i) => {
                println!("  match: {}", db.lenses[*i].model.id);
                vec![*i]
            }
            Outcome::Ambiguous(all) => {
                let names: Vec<String> = all
                    .iter()
                    .map(|i| {
                        format!(
                            "{} (crop {})",
                            db.lenses[*i].model.id, db.lenses[*i].calibration.crop
                        )
                    })
                    .collect();
                println!("  ambiguous: {}", names.join(" | "));
                all.clone()
            }
            other => {
                println!("  {other:?}");
                vec![]
            }
        };
        for i in lenses.iter().take(3) {
            let lens = &db.lenses[*i];
            if let Some(why) = lens.refusal() {
                println!("    {}: {why}", lens.model.id);
                continue;
            }
            let choice = LensfunChoice::of(lens, &db, camera_for(&db, m));
            let applied = choice.apply(m);
            let gives = applied
                .correction
                .as_ref()
                .map(Components::of)
                .unwrap_or_default();
            // Against the photo's own distortion data, where it has some: green's
            // sample position at the frame's corners and edge midpoints, in pixels.
            let own = m.lens_stand_in.as_ref().or(m.lens.as_ref());
            if let (Some(c), Some(own)) = (&applied.correction, own)
                && c.distortion.is_some()
                && own.distortion.is_some()
            {
                let [left, top, w, h] = rawmakase::lens::lensfun::choice::frame_rect(m);
                let size = [m.width as f32, m.height as f32];
                let half = size[0].hypot(size[1]) * 0.5;
                let at = |l: &rawmakase::optics::LensCorrection, x: f32, y: f32| {
                    let [cx, cy] = l.centre_in(size);
                    let (dx, dy) = (x - cx, y - cy);
                    let g = l.radial_scale_with(dx.hypot(dy) / half, 1.)[1];
                    [cx + dx * g, cy + dy * g]
                };
                let points = [
                    (left, top),
                    (left + w, top),
                    (left + w / 2., top),
                    (left, top + h / 2.),
                    (left + w, top + h),
                ];
                let worst = points
                    .iter()
                    .map(|&(x, y)| {
                        let (a, b) = (at(c, x, y), at(own, x, y));
                        (a[0] - b[0]).hypot(a[1] - b[1])
                    })
                    .fold(0f32, f32::max);
                // The shape alone: each scaled so the frame's corner samples the same
                // radius (the corrections frame differently), then compared at the
                // edges' midpoints, where barrel or pincushion shows.
                let radius = |l: &rawmakase::optics::LensCorrection, x: f32, y: f32| {
                    let [cx, cy] = l.centre_in(size);
                    let r = (x - cx).hypot(y - cy);
                    r * l.radial_scale_with(r / half, 1.)[1]
                };
                let corner = (left + w, top + h);
                let shape = [(left + w / 2., top), (left, top + h / 2.)]
                    .iter()
                    .map(|&(x, y)| {
                        let a = radius(c, x, y) / radius(c, corner.0, corner.1);
                        let b = radius(own, x, y) / radius(own, corner.0, corner.1);
                        (a - b).abs() * radius(own, corner.0, corner.1)
                    })
                    .fold(0f32, f32::max);
                println!(
                    "    against {}: up to {worst:.1} px apart at the frame's edge as sampled; {shape:.1} px in shape (corners matched)",
                    own.source
                );
            }
            println!(
                "    {} gives {}{}",
                lens.model.id,
                gives.describe(),
                if applied.notes.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", applied.notes.join("; "))
                }
            );
        }
    }
    Ok(())
}
/// The photo's decoded and default-crop geometry (with `DUMP=1`).
fn dump(m: &rawmakase::camera_data::Metadata) {
    eprintln!(
        "  decoded {}x{} raw {}x{} crop l{} t{} {}x{} camera_crop {:?} flip {}",
        m.width,
        m.height,
        m.raw_width,
        m.raw_height,
        m.crop_left,
        m.crop_top,
        m.crop_width,
        m.crop_height,
        m.camera_crop,
        m.flip
    );
}
