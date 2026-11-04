use rawmakase::model::recipe::Recipe;
use rawmakase::{camera_data::Decode, develop, export, export_settings::ExportOptions, storage};
use rawmakase_cancel::Cancel;
use std::{path::PathBuf, sync::atomic::AtomicBool};
#[test]
#[ignore = "Set RAWMAKASE_FIXTURES to a folder containing private ARW and RAF fixtures"]
fn raw_development_and_export() -> anyhow::Result<()> {
    rayon::ThreadPoolBuilder::new()
        .num_threads(8)
        .build_global()
        .ok();
    let dir = PathBuf::from(std::env::var("RAWMAKASE_FIXTURES").expect("Set RAWMAKASE_FIXTURES"));
    let files = storage::list_raws(&dir)?;
    assert!(
        files
            .iter()
            .any(|p| p.extension().unwrap().eq_ignore_ascii_case("arw"))
    );
    assert!(
        files
            .iter()
            .any(|p| p.extension().unwrap().eq_ignore_ascii_case("raf"))
    );
    let temp = tempfile::tempdir()?;
    for (index, path) in files.iter().enumerate() {
        let before = storage::Identity::read(path)?;
        assert!(
            rawmakase::photo::open(path)?
                .develop(
                    Decode::full(Default::default()),
                    Cancel::new(&AtomicBool::new(true))
                )
                .is_err()
        );
        let fast = rawmakase::photo::open(path)?.develop(Decode::Half, Cancel::NEVER)?;
        assert!(fast.width < fast.metadata.width && fast.height < fast.metadata.height);
        drop(fast);
        let mut raw = rawmakase::photo::open(path)?;
        assert!(!raw.thumbnail()?.is_empty());
        let image = raw.develop(Decode::full(Default::default()), Cancel::NEVER)?;
        assert_eq!(image.scale_clipped, 0);
        assert!(image.pixels.iter().flatten().all(|p| p.is_finite()));
        let mut recipe = Recipe::for_metadata(&image.metadata);
        recipe.exposure = -1.;
        recipe.rotation = 1;
        recipe.crop = [0.1, 0.1, 0.9, 0.9];
        let output = develop::render(&image, &recipe.checked()?, 320, Cancel::NEVER)?;
        assert!(output.height > output.width);
        for extension in ["jpg", "tiff"] {
            let p = temp.path().join(format!("{index}.{extension}"));
            export::export(
                &p,
                path,
                &output,
                &image.metadata,
                &ExportOptions::default(),
                export::Replace::NoClobber,
            )?;
            let decoded = image::open(&p)?;
            assert_eq!(
                (decoded.width(), decoded.height()),
                (output.width, output.height)
            );
            if extension == "tiff" {
                assert_eq!(decoded.color(), image::ColorType::Rgb16);
                let mut decoder = tiff::decoder::Decoder::new(std::fs::File::open(&p)?)?;
                assert!(
                    !decoder
                        .get_tag_u8_vec(tiff::tags::Tag::Unknown(34675))?
                        .is_empty()
                );
            }
            assert!(
                export::export(
                    &p,
                    path,
                    &output,
                    &image.metadata,
                    &ExportOptions::default(),
                    export::Replace::NoClobber
                )
                .is_err()
            );
        }
        assert_eq!(before, storage::Identity::read(path)?);
    }
    Ok(())
}

#[test]
#[ignore = "Private fixtures; performs 50 sequential RAW developments to check memory"]
fn navigation_memory_stress() -> anyhow::Result<()> {
    rayon::ThreadPoolBuilder::new()
        .num_threads(8)
        .build_global()
        .ok();
    let dir = PathBuf::from(std::env::var("RAWMAKASE_FIXTURES")?);
    let files = storage::list_raws(&dir)?;
    assert!(!files.is_empty());
    fn rss() -> u64 {
        #[cfg(target_os = "macos")]
        {
            let out = std::process::Command::new("ps")
                .args(["-o", "rss=", "-p", &std::process::id().to_string()])
                .output()
                .unwrap();
            assert!(out.status.success());
            String::from_utf8(out.stdout)
                .unwrap()
                .trim()
                .parse()
                .unwrap()
        }
        #[cfg(not(target_os = "macos"))]
        std::fs::read_to_string("/proc/self/status")
            .unwrap()
            .lines()
            .find(|l| l.starts_with("VmRSS:"))
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap()
    }
    let mut warm = 0;
    for i in 0..50 {
        {
            let im = rawmakase::photo::open(&files[i % files.len()])?
                .develop(Decode::full(Default::default()), Cancel::NEVER)?;
            let small = develop::preview(&im, 1600);
            let r = Recipe::for_metadata(&im.metadata);
            let _ = develop::render(&small, &r.checked()?, 1600, Cancel::NEVER)?;
        }
        let memory = rss();
        if i == 5 {
            warm = memory;
        }
        if i % 10 == 9 {
            println!(
                "after {} images: {:.1} MiB RSS",
                i + 1,
                memory as f64 / 1024.
            );
        }
        if i > 5 {
            assert!(
                memory < warm + 200 * 1024,
                "Memory grew unexpectedly: {memory} KiB vs {warm}"
            );
        }
    }
    Ok(())
}
