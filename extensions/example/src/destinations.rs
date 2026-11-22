//! A test-only extension of export destinations
//! (`destinations/rawmakase-extension.toml`), so the app's tests can check
//! deliveries end to end:
//!
//! - `index` writes `index.txt` into the delivery's folder: its `heading`
//!   option, or its label when the heading is empty, then one line for each
//!   file, `name width x height captured label`, in the order sent. It answers with the number of files, a message and the index's
//!   path as the location.
//! - `closed` answers `unavailable`, as a service that can't be reached.
use rawmakase_extension::protocol::method::{Delivered, ExportDeliver};
use rawmakase_extension::{Context, ErrorCode, Extension, RemoteError};
use std::fmt::Write;

/// The index's file name, in the delivery's folder.
pub const INDEX: &str = "index.txt";

fn index(_: &Context, deliver: ExportDeliver) -> Result<Delivered, RemoteError> {
    let heading = match deliver.option_str("heading") {
        Some(heading) if !heading.is_empty() => heading,
        _ => &deliver.label,
    };
    let mut text = format!("{heading}\n");
    for file in &deliver.files {
        let size = match (file.width, file.height) {
            (Some(w), Some(h)) => format!("{w} x {h}"),
            _ => "unknown size".into(),
        };
        let captured = file.captured.as_deref().unwrap_or("undated");
        let label = file.label.as_deref().unwrap_or("unlabelled");
        let _ = writeln!(text, "{} {size} {captured} {label}", file.name);
    }
    let path = deliver.folder.join(INDEX);
    std::fs::write(&path, text).map_err(|e| {
        RemoteError::new(
            ErrorCode::Unavailable,
            format!("{} not written: {e}", path.display()),
        )
    })?;
    Ok(Delivered {
        sent: u32::try_from(deliver.files.len()).unwrap_or(u32::MAX),
        message: Some(format!("Indexed {}", deliver.label)),
        location: Some(path.display().to_string()),
    })
}

fn closed(_: &Context, _: ExportDeliver) -> Result<Delivered, RemoteError> {
    Err(RemoteError::new(
        ErrorCode::Unavailable,
        "the lab is closed today",
    ))
}

/// The program's extension.
pub fn extension() -> Extension {
    Extension::new()
        .export_destination("index", index)
        .export_destination("closed", closed)
}
