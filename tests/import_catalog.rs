//! `rawmakase import-catalog`, end to end: the command starts an importer's
//! program from a developer folder (the test-only importer extension's,
//! `rawmakase_example::importer`, or the bundled Lightroom extension's),
//! polls its session itself, then imports and publishes its plan. This
//! binary is that program too: a copy of it, started with [`PROGRAM`] (or
//! [`LIGHTROOM`]) in its environment, which the command passes on. It has
//! its own `main` (no test harness), so it runs as nothing else then.
use rawmakase_extensions::store::Store;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const PROGRAM: &str = "RAWMAKASE_EXAMPLE_IMPORTER";
const ID: &str = "dev.rawmakase.importer";
const IMPORTER: &str = "dev.rawmakase.importer/script";
const LIGHTROOM: &str = "RAWMAKASE_LIGHTROOM_PROGRAM";

fn main() {
    if std::env::var_os(PROGRAM).is_some() {
        let code = i32::from(rawmakase_example::importer::extension().run().is_err());
        std::process::exit(code);
    }
    if std::env::var_os(LIGHTROOM).is_some() {
        let roots = rawmakase_lightroom::Roots::default();
        let code = i32::from(rawmakase_lightroom::extension(roots).run().is_err());
        std::process::exit(code);
    }
    imports_with_an_extensions_importer();
    a_disabled_extension_is_named_as_disabled();
    an_importer_without_its_extension_is_refused();
    imports_a_lightroom_catalog_with_the_lightroom_extension();
    a_disabled_extension_without_its_program_is_named_as_disabled();
    println!("import_catalog: 5 tests passed");
}

struct Fixture {
    dir: tempfile::TempDir,
    data: PathBuf,
}

/// A data folder with the importer extension loaded from a developer
/// folder, whose program is a copy of this binary.
fn fixture() -> Fixture {
    developer(
        include_str!("../extensions/example/importer/rawmakase-extension.toml"),
        "rawmakase-example-importer",
    )
}

/// A data folder with the extension of `manifest` loaded from a developer
/// folder, whose program `name` is a copy of this binary.
fn developer(manifest: &str, name: &str) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("importer");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("rawmakase-extension.toml"), manifest).unwrap();
    let program = folder.join(if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    });
    std::fs::copy(std::env::current_exe().unwrap(), &program).unwrap();
    let data = dir.path().join("data");
    Store::new(&data).add_developer(&folder).unwrap();
    Fixture { dir, data }
}

impl Fixture {
    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// `rawmakase import-catalog --importer importer source output`, run in
    /// this fixture's folder, so relative paths name files there.
    fn run(&self, importer: &str, source: &Path, output: &Path) -> Output {
        self.command(source, output)
            .args(["--importer", importer])
            .env(PROGRAM, "1")
            .output()
            .unwrap()
    }

    fn command(&self, source: &Path, output: &Path) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rawmakase"));
        command
            .arg("import-catalog")
            .arg(source)
            .arg(output)
            .current_dir(self.dir.path())
            .env("RAWMAKASE_DATA_DIR", &self.data);
        command
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Relative paths are taken from where the command runs, not from the
/// program's folder.
fn imports_with_an_extensions_importer() {
    let f = fixture();
    std::fs::write(f.path("source.json"), r#"{"photos": 2}"#).unwrap();
    let output = f.path("Photos.rawmakase");
    let run = f.run(
        IMPORTER,
        Path::new("source.json"),
        Path::new("Photos.rawmakase"),
    );
    let (stdout, stderr) = (text(&run.stdout), text(&run.stderr));
    assert!(run.status.success(), "{stdout}\n{stderr}");
    assert!(stdout.contains("Imported 2 photographs into"), "{stdout}");
    assert!(stderr.contains("Reading photos… 2 of 2"), "{stderr}");
    let db = rusqlite::Connection::open(&output).unwrap();
    let photos: i64 = db
        .query_row("SELECT count(*) FROM photos", [], |r| r.get(0))
        .unwrap();
    assert_eq!(photos, 2);
    let left: Vec<String> = std::fs::read_dir(f.dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".partial"))
        .collect();
    assert_eq!(left, Vec::<String>::new(), "no import folder is left");
    // An import never replaces a file.
    let again = f.run(IMPORTER, &f.path("source.json"), &output);
    assert!(!again.status.success());
    let stderr = text(&again.stderr);
    assert!(stderr.contains("already exists"), "{stderr}");
}

fn a_disabled_extension_is_named_as_disabled() {
    let f = fixture();
    Store::new(&f.data).set_enabled(ID, false, &[]).unwrap();
    let source = f.path("source.json");
    std::fs::write(&source, "{}").unwrap();
    let output = f.path("Photos.rawmakase");
    let run = f.run(IMPORTER, &source, &output);
    assert!(!run.status.success());
    let stderr = text(&run.stderr);
    assert!(
        stderr.contains("The Example Importer extension is disabled in Preferences > Extensions"),
        "{stderr}"
    );
    assert!(!output.exists());
}

fn an_importer_without_its_extension_is_refused() {
    let f = fixture();
    let source = f.path("source.json");
    std::fs::write(&source, "{}").unwrap();
    let output = f.path("Photos.rawmakase");
    for importer in ["script", "dev.nobody/script", "dev.rawmakase.importer/nope"] {
        let run = f.run(importer, &source, &output);
        assert!(!run.status.success(), "{importer}");
    }
    assert!(!output.exists());
}

/// Without `--importer` a Lightroom Classic catalog is imported with the
/// Lightroom extension's importer, and not at all with the extension off.
fn imports_a_lightroom_catalog_with_the_lightroom_extension() {
    let f = developer(
        include_str!("../extensions/lightroom/rawmakase-extension.toml"),
        "rawmakase-lightroom",
    );
    let source = f.path("Lightroom.lrcat");
    rawmakase_lightroom::fake::catalog(&source).unwrap();
    let output = f.path("Photos.rawmakase");
    let run = |output: &Path| {
        f.command(&source, output)
            .env(LIGHTROOM, "1")
            .output()
            .unwrap()
    };
    let imported = run(&output);
    let (stdout, stderr) = (text(&imported.stdout), text(&imported.stderr));
    assert!(imported.status.success(), "{stdout}\n{stderr}");
    assert!(stdout.contains("Imported 2 photographs into"), "{stdout}");
    let db = rusqlite::Connection::open(&output).unwrap();
    let ids: Vec<i64> = db
        .prepare("SELECT id FROM photos ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(ids, [40, 41], "Lightroom's ids");
    // Into a folder that doesn't exist yet: it is made.
    let nested = f.path("new-dir").join("Photos.rawmakase");
    let made = run(&nested);
    assert!(made.status.success(), "{}", text(&made.stderr));
    assert!(nested.exists());

    Store::new(&f.data)
        .set_enabled("rawmakase.lightroom", false, &[])
        .unwrap();
    let again = f.path("Again.rawmakase");
    let refused = run(&again);
    assert!(!refused.status.success());
    let stderr = text(&refused.stderr);
    assert!(
        stderr.contains("The Lightroom extension is disabled in Preferences > Extensions"),
        "{stderr}"
    );
    assert!(!again.exists());
}

/// A disabled extension is named as disabled even when its program isn't
/// installed either.
fn a_disabled_extension_without_its_program_is_named_as_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let bundled = rawmakase::app::bundled_extensions();
    Store::new(&data)
        .set_enabled("rawmakase.lightroom", false, &["rawmakase.lightroom"])
        .unwrap();
    let app = rawmakase_extensions::semver::Version::parse(env!("CARGO_PKG_VERSION")).unwrap();
    let nowhere = [dir.path().join("no-programs")];
    let error =
        rawmakase::catalog_import::locate(&data, "rawmakase.lightroom", bundled, &nowhere, &app)
            .unwrap_err()
            .to_string();
    assert_eq!(
        error,
        "The Lightroom extension is disabled in Preferences > Extensions"
    );
}
