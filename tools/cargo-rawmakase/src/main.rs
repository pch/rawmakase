//! `cargo rawmakase`: make, build, package and try RAWmakase extensions.
//!
//! ```text
//! cargo rawmakase new my-extension --id dev.me.my-extension
//! cargo rawmakase build
//! cargo rawmakase package
//! cargo rawmakase release
//! cargo rawmakase dev
//! ```
//!
//! Every command but `new` runs in the extension's folder, the one holding
//! `rawmakase-extension.toml` and `Cargo.toml`.
mod templates;

use clap::{Parser, Subcommand, ValueEnum};
use rawmakase_extension_protocol::manifest::{self, Manifest};
use rawmakase_extensions::package;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Cargo runs `cargo-rawmakase rawmakase <command>`: the first argument is
/// the subcommand's own name.
#[derive(Parser)]
#[command(name = "cargo", bin_name = "cargo")]
enum Cargo {
    /// Make, build and package RAWmakase extensions.
    #[command(subcommand)]
    Rawmakase(Command),
}

#[derive(Subcommand)]
enum Command {
    /// Make a new extension in a new folder.
    New {
        /// The folder to make; its name is the crate's.
        path: PathBuf,
        /// What the extension contributes to start with.
        #[arg(long, value_enum, default_value_t = Template::Command)]
        template: Template,
        /// Its id, reverse-DNS (dev.you.name).
        #[arg(long)]
        id: String,
        /// The RAWmakase versions it supports.
        #[arg(long, default_value = templates::DEFAULT_RANGE)]
        rawmakase: String,
    },
    /// Build the program (cargo build --release), passing on any arguments.
    Build {
        /// Arguments for cargo build.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Write `target/rmext/<id>-<version>-<target>.rmext` from the built program.
    Package {
        /// The target the program was built for; this computer's by default.
        #[arg(long)]
        target: Option<String>,
        /// The built program; `target/release/<name>` by default.
        #[arg(long)]
        program: Option<PathBuf>,
        /// More files to include, at the same path in the package.
        #[arg(long)]
        include: Vec<PathBuf>,
    },
    /// Package for this computer and write a GitHub Actions workflow that
    /// packages every target and attaches them to a release.
    Release,
    /// Build, then load the extension into RAWmakase in developer mode.
    Dev {
        /// RAWmakase's data folder; the one the app uses by default.
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
}

/// What `new` starts from.
#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Template {
    /// A command the user runs from a menu or a key.
    Command,
    /// A control adapter: a device translated into editing operations.
    ControlAdapter,
}

fn main() -> ExitCode {
    let Cargo::Rawmakase(command) = Cargo::parse();
    let cwd = PathBuf::from(".");
    let result = match command {
        Command::New {
            path,
            template,
            id,
            rawmakase,
        } => new(&path, template, &id, &rawmakase),
        Command::Build { args } => build(&cwd, &args),
        Command::Package {
            target,
            program,
            include,
        } => package(&cwd, target.as_deref(), program.as_deref(), &include).map(|path| {
            println!("Wrote {}", path.display());
        }),
        Command::Release => release(&cwd),
        Command::Dev { data_dir } => dev(&cwd, data_dir),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

type Result<T> = std::result::Result<T, String>;

fn new(path: &Path, template: Template, id: &str, rawmakase: &str) -> Result<()> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| manifest::is_local_id(n))
        .ok_or("the folder's name must be lowercase letters, digits and hyphens")?;
    if path.exists() {
        return Err(format!("{} exists already", path.display()));
    }
    let files = templates::files(template == Template::ControlAdapter, name, id, rawmakase);
    // The manifest is checked before anything is written.
    let text = &files
        .iter()
        .find(|(p, _)| *p == manifest::FILE)
        .expect("templates have a manifest")
        .1;
    Manifest::parse(text).map_err(|e| format!("the manifest would be invalid: {e}"))?;
    for (file, contents) in &files {
        let target = path.join(file);
        std::fs::create_dir_all(target.parent().expect("files are in the folder"))
            .map_err(|e| e.to_string())?;
        std::fs::write(&target, contents).map_err(|e| format!("{}: {e}", target.display()))?;
    }
    println!(
        "Made {}. Build it with `cargo rawmakase build`, try it with `cargo rawmakase dev`.",
        path.display()
    );
    Ok(())
}

fn manifest(dir: &Path) -> Result<Manifest> {
    rawmakase_extensions::read_manifest(dir).map_err(|e| e.to_string())
}

fn cargo(dir: &Path, args: &[&str]) -> Result<()> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let status = std::process::Command::new(cargo)
        .args(args)
        .current_dir(dir)
        .status()
        .map_err(|e| format!("cargo: {e}"))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| format!("cargo {} failed", args.join(" ")))
}

fn build(dir: &Path, extra: &[String]) -> Result<()> {
    manifest(dir)?;
    let mut args = vec!["build", "--release"];
    args.extend(extra.iter().map(String::as_str));
    cargo(dir, &args)
}

/// The program's file name for `target`: the manifest path's last part, with
/// `.exe` for Windows.
fn program_name(program: &manifest::Program, target: &str) -> String {
    let name = program.path.rsplit('/').next().unwrap_or(&program.path);
    if target.starts_with("windows-") && !name.contains('.') {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

fn package(
    dir: &Path,
    target: Option<&str>,
    program: Option<&Path>,
    include: &[PathBuf],
) -> Result<PathBuf> {
    let manifest = manifest(dir)?;
    let manifest_bytes = std::fs::read(dir.join(manifest::FILE)).map_err(|e| e.to_string())?;
    let mut files = vec![(manifest::FILE.to_string(), manifest_bytes)];
    let target = match (&manifest.program, target) {
        (None, _) => rawmakase_extensions::ANY_TARGET.to_string(),
        (Some(_), Some(target)) => target.to_string(),
        (Some(_), None) => rawmakase_extensions::current_target(),
    };
    let mut program_in_package = None;
    if let Some(declared) = &manifest.program {
        let built = match program {
            Some(path) => path.to_path_buf(),
            None => dir
                .join("target/release")
                .join(program_name(declared, &target)),
        };
        let bytes = std::fs::read(&built).map_err(|e| {
            format!(
                "{}: {e}; build it first, or name it with --program",
                built.display()
            )
        })?;
        let path = package::program_path(declared, &target);
        program_in_package = Some(path.clone());
        files.push((path, bytes));
    }
    let mut extra: Vec<String> = manifest.extension.icon.iter().cloned().collect();
    for path in include {
        let name = path
            .to_str()
            .map(|p| p.replace('\\', "/"))
            .filter(|p| manifest::is_package_path(p))
            .ok_or_else(|| {
                format!(
                    "{} is not a relative path inside the extension",
                    path.display()
                )
            })?;
        extra.push(name);
    }
    for name in extra {
        if files.iter().any(|(n, _)| *n == name) {
            continue;
        }
        let bytes = std::fs::read(dir.join(&name)).map_err(|e| format!("{name}: {e}"))?;
        files.push((name, bytes));
    }
    let out = dir.join("target/rmext");
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let path = out.join(package::file_name(&manifest, &target));
    package::write(&path, &files, program_in_package.as_deref()).map_err(|e| e.to_string())?;
    // What the host will check, checked now. Unpacking checks the program by
    // the name the package's target gives it, so a package for another
    // platform is checked in full too.
    let inspected = package::inspect(&path).map_err(|e| e.to_string())?;
    let check = out.join(format!(".check-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&check);
    std::fs::create_dir_all(&check).map_err(|e| e.to_string())?;
    let unpacked = package::unpack(&path, &check);
    let _ = std::fs::remove_dir_all(&check);
    match unpacked {
        Ok(_) if inspected.target == target => Ok(path),
        Ok(_) => Err("the package's name doesn't state its target".into()),
        Err(e) => Err(e.to_string()),
    }
}

fn release(dir: &Path) -> Result<()> {
    build(dir, &[])?;
    let path = package(dir, None, None, &[])?;
    println!("Wrote {}", path.display());
    let workflow = dir.join(".github/workflows/rawmakase-release.yml");
    if workflow.exists() {
        println!("{} exists; left as it is", workflow.display());
    } else {
        std::fs::create_dir_all(workflow.parent().expect("a folder")).map_err(|e| e.to_string())?;
        std::fs::write(&workflow, templates::RELEASE_WORKFLOW).map_err(|e| e.to_string())?;
        println!(
            "Wrote {}: push a tag v<version> and it attaches every target's package to the release",
            workflow.display()
        );
    }
    Ok(())
}

fn dev(dir: &Path, data_dir: Option<PathBuf>) -> Result<()> {
    let manifest = manifest(dir)?;
    // A debug build, laid out as the package would unpack, in target/.
    let staged = dir.join("target/rawmakase-dev");
    let _ = std::fs::remove_dir_all(&staged);
    std::fs::create_dir_all(&staged).map_err(|e| e.to_string())?;
    std::fs::copy(dir.join(manifest::FILE), staged.join(manifest::FILE))
        .map_err(|e| e.to_string())?;
    if let Some(program) = &manifest.program {
        cargo(dir, &["build"])?;
        let target = rawmakase_extensions::current_target();
        let built = dir
            .join("target/debug")
            .join(program_name(program, &target));
        let into = rawmakase_extensions::program_file(&staged, program);
        std::fs::create_dir_all(into.parent().expect("a folder")).map_err(|e| e.to_string())?;
        std::fs::copy(&built, &into).map_err(|e| format!("{}: {e}", built.display()))?;
    }
    let data_dir = data_dir.unwrap_or_else(rawmakase_protocol::paths::data_dir);
    let store = rawmakase_extensions::store::Store::new(&data_dir);
    store.add_developer(&staged).map_err(|e| e.to_string())?;
    println!(
        "Loaded {} from {} into {}; RAWmakase picks it up in developer mode",
        manifest.extension.id,
        staged.display(),
        data_dir.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_templates_make_valid_extensions() {
        for adapter in [false, true] {
            let files = templates::files(
                adapter,
                "my-tool",
                "dev.me.my-tool",
                templates::DEFAULT_RANGE,
            );
            let text = &files.iter().find(|(p, _)| *p == manifest::FILE).unwrap().1;
            let manifest = Manifest::parse(text).unwrap();
            assert_eq!(manifest.extension.id, "dev.me.my-tool");
            assert_eq!(manifest.control_adapters.len(), usize::from(adapter));
            assert_eq!(manifest.commands.len(), usize::from(!adapter));
            assert!(files.iter().any(|(p, _)| *p == "src/main.rs"));
            assert!(files.iter().any(|(p, _)| *p == "Cargo.toml"));
        }
        let dir = tempfile::tempdir().unwrap();
        assert!(
            new(
                &dir.path().join("Bad_Name"),
                Template::Command,
                "dev.me.x",
                ">=0.4"
            )
            .is_err()
        );
        assert!(new(&dir.path().join("ok"), Template::Command, "nodots", ">=0.4").is_err());
        assert!(!dir.path().join("ok").exists());
        new(
            &dir.path().join("ok"),
            Template::Command,
            "dev.me.ok",
            ">=0.4",
        )
        .unwrap();
        assert!(dir.path().join("ok").join(manifest::FILE).is_file());
    }

    #[test]
    fn a_package_holds_what_the_host_accepts() {
        let dir = tempfile::tempdir().unwrap();
        let ext = dir.path().join("tool");
        new(&ext, Template::Command, "dev.me.tool", ">=0.1").unwrap();
        let target = rawmakase_extensions::current_target();
        let built = dir.path().join("built");
        std::fs::write(&built, b"a program").unwrap();
        std::fs::write(ext.join("notes.txt"), b"notes").unwrap();
        let path = package(&ext, None, Some(&built), &[PathBuf::from("notes.txt")]).unwrap();
        assert_eq!(
            path.file_name().unwrap().to_str().unwrap(),
            format!("dev.me.tool-0.1.0-{target}.rmext")
        );
        let store = rawmakase_extensions::store::Store::new(dir.path().join("data"));
        let source = rawmakase_extensions::state::Source::File { path: path.clone() };
        let installed = store
            .install(&path, &source, &semver::Version::new(0, 4, 0))
            .unwrap();
        assert_eq!(
            std::fs::read(installed.dir.join("notes.txt")).unwrap(),
            b"notes"
        );
        // A package for another platform names its program as that platform
        // does.
        let windows = package(&ext, Some("windows-x86_64"), Some(&built), &[]).unwrap();
        assert!(windows.ends_with("dev.me.tool-0.1.0-windows-x86_64.rmext"));
        assert!(package(&ext, None, Some(&dir.path().join("missing")), &[]).is_err());
        assert!(package(&ext, None, Some(&built), &[PathBuf::from("../escape")]).is_err());
    }
}
