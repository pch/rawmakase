//! `rawmakase import-catalog --importer`, end to end: the command starts the
//! test-only importer extension's program (`rawmakase_example::importer`)
//! from a developer folder, polls its session itself, then imports and
//! publishes its plan. This binary is that program too: a copy of it, started
//! with [`PROGRAM`] in its environment, which the command passes on. It has
//! its own `main` (no test harness), so it runs as nothing else then.
use rawmakase_extensions::store::Store;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const PROGRAM: &str = "RAWMAKASE_EXAMPLE_IMPORTER";
const ID: &str = "dev.rawmakase.importer";
const IMPORTER: &str = "dev.rawmakase.importer/script";

fn main() {
    if std::env::var_os(PROGRAM).is_some() {
        let code = i32::from(rawmakase_example::importer::extension().run().is_err());
        std::process::exit(code);
    }
    imports_with_an_extensions_importer();
    a_disabled_extension_is_named_as_disabled();
    an_importer_without_its_extension_is_refused();
    println!("import_catalog: 3 tests passed");
}

struct Fixture {
    dir: tempfile::TempDir,
    data: PathBuf,
}

/// A data folder with the importer extension loaded from a developer
/// folder, whose program is a copy of this binary.
fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("importer");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(
        folder.join("rawmakase-extension.toml"),
        include_str!("../extensions/example/importer/rawmakase-extension.toml"),
    )
    .unwrap();
    let program = folder.join(if cfg!(windows) {
        "rawmakase-example-importer.exe"
    } else {
        "rawmakase-example-importer"
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
        Command::new(env!("CARGO_BIN_EXE_rawmakase"))
            .arg("import-catalog")
            .args(["--importer", importer])
            .arg(source)
            .arg(output)
            .current_dir(self.dir.path())
            .env("RAWMAKASE_DATA_DIR", &self.data)
            .env(PROGRAM, "1")
            .output()
            .unwrap()
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
        stderr.contains("Example Importer is disabled in Preferences > Extensions"),
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
