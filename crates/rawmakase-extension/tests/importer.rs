//! Importers against the mock host: `import.run`, its answer, progress,
//! cancellation and the refusals. The importers here write no plan: what a
//! plan holds is the app's to check (`catalog::plan_import`).
#![cfg(feature = "importer")]
use rawmakase_extension::mock::{MockHost, Recorded};
use rawmakase_extension::protocol::method::{AppRequest, ImportRun};
use rawmakase_extension::{Context, ErrorCode, Extension, Importer, RemoteError, ServeOptions};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Answers with the number its source path ends in, after reporting
/// progress once per photo.
struct Count;
impl Importer for Count {
    fn import(&mut self, cx: &Context, source: &Path, plan: &Path) -> Result<u64, RemoteError> {
        assert!(plan.ends_with("plan.sqlite"));
        let photos: u64 = source
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .parse()
            .unwrap();
        for n in 1..=photos {
            cx.progress(None, Some(&format!("Reading photos… {n} of {photos}")));
        }
        Ok(photos)
    }
}

/// Reports progress until cancelled; `deaf` returns as if it finished.
struct UntilCancelled {
    deaf: bool,
}
impl Importer for UntilCancelled {
    fn import(&mut self, cx: &Context, _: &Path, _: &Path) -> Result<u64, RemoteError> {
        while !cx.is_cancelled() {
            cx.progress(None, Some("Waiting"));
            std::thread::sleep(Duration::from_millis(10));
        }
        if self.deaf {
            return Ok(1);
        }
        Err(RemoteError::new(ErrorCode::Cancelled, "cancelled"))
    }
}

fn host(api: u32) -> MockHost {
    let extension = Extension::new()
        .importer("count", Count)
        .importer("wait", UntilCancelled { deaf: false })
        .importer("deaf", UntilCancelled { deaf: true });
    let host = MockHost::start_with_options(extension, ServeOptions { api }, |_, _| {
        Ok(serde_json::Value::Null)
    });
    let hello = host.hello().unwrap();
    assert!(hello.implements.contains(&"importer/count".to_string()));
    host
}

fn plan() -> PathBuf {
    PathBuf::from("import").join("plan.sqlite")
}

#[test]
fn an_import_answers_the_photos_its_handler_wrote() {
    let host = host(4);
    let done = host.import("count", Path::new("3"), &plan()).unwrap();
    assert_eq!(done.photos, 3);
    let progress: Vec<_> = host
        .recorded()
        .into_iter()
        .filter_map(|r| match r {
            Recorded::Progress { text, .. } => text,
            _ => None,
        })
        .collect();
    assert_eq!(
        progress.last().map(String::as_str),
        Some("Reading photos… 3 of 3")
    );
    host.finish().unwrap();
}

#[test]
fn a_cancelled_import_answers_cancelled_even_when_its_handler_returns() {
    let host = host(4);
    for importer in ["wait", "deaf"] {
        let id = host.send_import(importer, Path::new("1"), &plan()).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        host.cancel(id);
        let error = host.wait(id).unwrap_err();
        assert_eq!(error.code, ErrorCode::Cancelled, "{importer}");
    }
    host.finish().unwrap();
}

#[test]
fn imports_are_refused_below_api_4_and_for_an_importer_not_implemented() {
    let old = host(3);
    let refused = old.import("count", Path::new("1"), &plan()).unwrap_err();
    assert_eq!(refused.code, ErrorCode::InvalidRequest);
    // Sent anyway, the program refuses it.
    let refused = old
        .request(AppRequest::ImportRun(ImportRun {
            importer: "count".into(),
            source: "1".into(),
            plan: plan(),
        }))
        .unwrap_err();
    assert_eq!(refused.code, ErrorCode::InvalidRequest);
    assert!(refused.message.contains("api 4"), "{refused}");
    old.finish().unwrap();

    let host = host(4);
    let refused = host.import("photos", Path::new("1"), &plan()).unwrap_err();
    assert_eq!(refused.code, ErrorCode::InvalidRequest);
    host.finish().unwrap();
}
