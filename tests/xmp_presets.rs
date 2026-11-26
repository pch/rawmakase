use rawmakase::{camera_data::Metadata, camera_profiles, model::recipe::Recipe, presets};
#[test]
#[ignore = "Private XMP library installed in user data directory"]
fn supplied_library_audit() -> anyhow::Result<()> {
    let library = presets::PresetLibrary::in_data_dirs(&rawmakase::storage::asset_dirs()).load();
    println!(
        "{} presets; {} parse failures",
        library.presets.len(),
        library.errors.len()
    );
    for e in &library.errors {
        println!("PARSE {e}");
    }
    assert!(library.presets.len() > 800);
    assert!(library.errors.is_empty());
    for (make, model) in [("Sony", "ILCE-7M2"), ("Fujifilm", "X100F")] {
        let m = Metadata {
            make: make.into(),
            model: model.into(),
            ..Default::default()
        };
        let (profiles, _) = camera_profiles::ProfileLibrary::for_tests().installed(&m);
        let base = Recipe::for_metadata(&m);
        let mut okay = 0;
        let mut errors = std::collections::BTreeMap::<String, usize>::new();
        for p in &library.presets {
            match p.apply(&base, &m, &profiles, None) {
                Ok(r) => {
                    r.validate()?;
                    okay += 1;
                }
                Err(e) => {
                    *errors.entry(e.to_string()).or_default() += 1;
                }
            }
        }
        println!("{model}: {okay} applicable");
        for (e, n) in errors {
            println!("{n}: {e}");
        }
        assert!(okay > 300);
    }
    Ok(())
}
