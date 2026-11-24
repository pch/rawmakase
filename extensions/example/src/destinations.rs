//! A test-only extension of export destinations
//! (`destinations/rawmakase-extension.toml`), so the app's tests can check
//! deliveries end to end:
//!
//! - `index` collects a delivery ([`collected`]) and writes `index.txt` into
//!   its folder: its `heading` option, or the folder's name when the heading
//!   is empty, then one line for each file in the export's order,
//!   `name width x height captured label uuid`. A photo labelled red is
//!   failed instead, so results can differ file by file; each other file is
//!   sent, with its line number as its remote id. It answers with a message
//!   and the index's path as the location, and refuses an export with no
//!   one folder.
//! - `closed` refuses every delivery with `unavailable`, as a service that
//!   can't be reached.
use rawmakase_extension::protocol::method::{
    ExportBegin, ExportEnd, ExportEnded, ExportFile, FileResult, Sent,
};
use rawmakase_extension::{
    Context, Delivery, ErrorCode, ExportDestination, Extension, RemoteError, collected,
};
use std::fmt::Write;

/// The index's file name, in the delivery's folder.
pub const INDEX: &str = "index.txt";

fn index(_: &Context, mut delivery: Delivery) -> Result<ExportEnded, RemoteError> {
    let Some(folder) = delivery.begin.folder.clone() else {
        return Err(RemoteError::new(
            ErrorCode::InvalidRequest,
            "an index needs the export in one folder",
        ));
    };
    let heading = match delivery.begin.option_str("heading") {
        Some(heading) if !heading.is_empty() => heading.to_string(),
        _ => folder
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
    };
    delivery.files.sort_by_key(|f| f.index);
    let mut text = format!("{heading}\n");
    let mut results = Vec::new();
    for file in &delivery.files {
        let label = file.catalog.as_ref().and_then(|c| c.label.as_deref());
        if label == Some("red") {
            results.push(FileResult {
                index: file.index,
                result: Sent::Failed,
                message: Some("red photos aren't indexed".into()),
                remote_id: None,
                url: None,
            });
            continue;
        }
        let size = match (file.width, file.height) {
            (Some(w), Some(h)) => format!("{w} x {h}"),
            _ => "unknown size".into(),
        };
        let captured = file.captured.as_deref().unwrap_or("undated");
        let uuid = file.photo_uuid.as_deref().unwrap_or("no-uuid");
        let _ = writeln!(
            text,
            "{} {size} {captured} {} {uuid}",
            file.name,
            label.unwrap_or("unlabelled")
        );
        results.push(FileResult {
            index: file.index,
            result: Sent::Sent,
            message: None,
            remote_id: Some(format!("line-{}", text.lines().count())),
            url: None,
        });
    }
    let path = folder.join(INDEX);
    std::fs::write(&path, text).map_err(|e| {
        RemoteError::new(
            ErrorCode::Unavailable,
            format!("{} not written: {e}", path.display()),
        )
    })?;
    Ok(ExportEnded {
        files: results,
        message: Some(format!("Indexed {heading}")),
        location: Some(path.display().to_string()),
    })
}

/// A destination that can't be reached.
struct Closed;
impl ExportDestination for Closed {
    fn begin(&mut self, _: &Context, _: ExportBegin) -> Result<(), RemoteError> {
        Err(RemoteError::new(
            ErrorCode::Unavailable,
            "the lab is closed today",
        ))
    }
    fn file(&mut self, _: &Context, _: ExportFile) -> Result<(), RemoteError> {
        Err(RemoteError::new(ErrorCode::InvalidRequest, "no delivery"))
    }
    fn end(&mut self, _: &Context, _: ExportEnd) -> Result<ExportEnded, RemoteError> {
        Err(RemoteError::new(ErrorCode::InvalidRequest, "no delivery"))
    }
}

/// The program's extension.
pub fn extension() -> Extension {
    Extension::new()
        .export_destination("index", collected(index))
        .export_destination("closed", Closed)
}
