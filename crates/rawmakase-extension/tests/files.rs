//! `files.list` against the mock host: a Camera Raw install's listing fits
//! a line, one that doesn't is answered `too_large`, and API 3 is refused.
use rawmakase_extension::mock::MockHost;
use rawmakase_extension::protocol::method::{
    AppRequest, Empty, FileFolder, FileKind, FileName, FileOrigin, FilesListed,
};
use rawmakase_extension::{ErrorCode, Extension, RemoteError, ServeOptions};

/// `folders` of 100 lens profiles each, with names as long as Camera Raw's.
fn listing(folders: usize) -> FilesListed {
    let folders = (0..folders)
        .map(|i| FileFolder {
            kind: FileKind::LensProfile,
            origin: FileOrigin::Shared,
            path: format!(
                "/Library/Application Support/Adobe/CameraRaw/LensProfiles/1.0/Maker {i}"
            )
            .into(),
            camera: None,
            group: Some(format!("Maker {i}")),
            files: (0..100)
                .map(|j| FileName {
                    name: format!("Maker {i} (Lens {j:03} 18-35mm F1.8 DC HSM A013) - RAW.lcp"),
                    camera: None,
                    used: None,
                })
                .collect(),
        })
        .collect();
    FilesListed {
        folders,
        searched: Vec::new(),
    }
}

fn host(api: u32, folders: usize) -> MockHost {
    let extension = Extension::new().files(move |_| Ok(listing(folders)));
    let host = MockHost::start_with_options(extension, ServeOptions { api }, |_, _| {
        Ok(serde_json::Value::Null)
    });
    host.hello().unwrap();
    host
}

fn list(host: &MockHost) -> Result<FilesListed, RemoteError> {
    let value = host.request(AppRequest::FilesList(Empty {}))?;
    Ok(serde_json::from_value(value).unwrap())
}

#[test]
fn ten_thousand_files_fit_a_line() {
    let listed = list(&host(4, 100)).unwrap();
    assert_eq!(listed, listing(100));
}

#[test]
fn a_listing_longer_than_a_line_is_too_large() {
    let refused = list(&host(4, 300)).unwrap_err();
    assert_eq!(refused.code, ErrorCode::TooLarge);
}

/// A path that isn't UTF-8 can't be sent: the request fails, and the
/// program goes on serving.
#[cfg(unix)]
#[test]
fn a_listing_that_isn_t_text_fails_without_ending_the_program() {
    use std::os::unix::ffi::OsStrExt;
    let extension = Extension::new().files(|_| {
        let mut listed = listing(1);
        listed.folders[0].path = std::ffi::OsStr::from_bytes(b"/Pictures/\xff").into();
        Ok(listed)
    });
    let host = MockHost::start_with_options(extension, ServeOptions { api: 4 }, |_, _| {
        Ok(serde_json::Value::Null)
    });
    host.hello().unwrap();
    assert_eq!(list(&host).unwrap_err().code, ErrorCode::Unavailable);
    assert_eq!(list(&host).unwrap_err().code, ErrorCode::Unavailable);
}

#[test]
fn files_list_needs_api_4_and_a_lister() {
    let refused = list(&host(3, 1)).unwrap_err();
    assert_eq!(refused.code, ErrorCode::InvalidRequest);
    let none = MockHost::start_with_options(Extension::new(), ServeOptions { api: 4 }, |_, _| {
        Ok(serde_json::Value::Null)
    });
    none.hello().unwrap();
    assert_eq!(list(&none).unwrap_err().code, ErrorCode::InvalidRequest);
}
