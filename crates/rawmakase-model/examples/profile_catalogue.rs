//! Generates or checks `data/camera-profiles/catalogue.toml` and `excluded.toml`
//! from local checkouts of the revisions `sources.toml` pins, with RAWmakase's own
//! DCP parser (docs/lightroom-profiles.md#camera-profile-catalogue). Reads no
//! network: check out each source at its revision first.
//!
//!     cargo run -p rawmakase-model --example profile_catalogue -- generate \
//!         rawtherapee=<checkout>/rtdata/dcpprofiles art=<checkout>/data/dcpprofiles
//!     cargo run -p rawmakase-model --example profile_catalogue -- check <same arguments>
use anyhow::{Context, Result, bail, ensure};
use rawmakase_model::camera_profiles::catalogue::{Found, generate, parse_sources};
use std::path::{Path, PathBuf};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mode = args.next().unwrap_or_default();
    let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/camera-profiles");
    let sources = parse_sources(&std::fs::read_to_string(data.join("sources.toml"))?)?;
    let mut found = Vec::new();
    let mut given = Vec::new();
    for arg in args {
        let (source, dir) = arg
            .split_once('=')
            .with_context(|| format!("{arg}: expected <source>=<directory>"))?;
        ensure!(
            sources.iter().any(|s| s.id == source),
            "{source}: not in sources.toml"
        );
        given.push(source.to_string());
        let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
            .with_context(|| dir.to_string())?
            .map(|e| e.map(|e| e.path()))
            .collect::<std::io::Result<_>>()?;
        paths.sort();
        for path in paths {
            if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("dcp"))
            {
                found.push(Found {
                    source: source.into(),
                    file: path.file_name().unwrap().to_string_lossy().into_owned(),
                    bytes: std::fs::read(&path)?,
                });
            }
        }
    }
    for s in &sources {
        ensure!(
            given.contains(&s.id),
            "{}: give its checkout at {}",
            s.id,
            s.revision
        );
    }
    let generated = generate(&sources, found)?;
    let files = [
        ("catalogue.toml", generated.catalogue_toml()),
        ("excluded.toml", generated.excluded_toml()),
    ];
    match mode.as_str() {
        "generate" => {
            for (name, text) in &files {
                std::fs::write(data.join(name), text)?;
            }
        }
        "check" => {
            for (name, text) in &files {
                if std::fs::read_to_string(data.join(name))? != *text {
                    bail!("{name} differs from these checkouts: generate it again and review");
                }
            }
        }
        _ => bail!("usage: profile_catalogue generate|check <source>=<directory>..."),
    }
    for s in &sources {
        let listed = generated
            .profiles
            .iter()
            .filter(|p| p.source == s.id)
            .count();
        let left = generated
            .excluded
            .iter()
            .filter(|e| e.source == s.id)
            .count();
        println!("{}: {listed} listed, {left} excluded", s.name);
    }
    Ok(())
}
