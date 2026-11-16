//! Where the computer keeps installed fonts, for the text watermark's families.
//!
//! Copied from fastframe-fonts' `system::font_directories` at its tag v0.13.0
//! (github.com/crmne/fastframe, MIT, © 2026 Carmine Paolino), which the app's
//! interface fonts still come from: that crate is built for egui and brings it
//! along, and export builds without the GUI (`scripts/crate-closures.sh`). The
//! app's tests check that both list the same folders, so watermarks find the same
//! fonts as before.

// The code below is fastframe's, under its license:
//
// MIT License
//
// Copyright (c) 2026 Carmine Paolino
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

use std::path::{Path, PathBuf};

/// Where the platform keeps installed fonts, in the order they are probed.
///
/// On Linux: the directories fontconfig's configuration names, the XDG data
/// directories' `fonts`, Flatpak's `/run/host/fonts`, `~/.fonts` and
/// `$XDG_DATA_HOME/fonts`. For an app that looks for a particular family
/// itself (Spotifast's playlist face).
pub fn font_directories() -> Vec<PathBuf> {
    let user = directories::UserDirs::new();
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut add = |dir: PathBuf| {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    };
    if cfg!(target_os = "macos") {
        add(PathBuf::from("/System/Library/Fonts"));
        add(PathBuf::from("/Library/Fonts"));
    } else if cfg!(target_os = "windows") {
        add(std::env::var_os("SystemRoot")
            .map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from)
            .join("Fonts"));
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            add(PathBuf::from(local).join(r"Microsoft\Windows\Fonts"));
        }
    } else {
        let home = user.as_ref().map(|user| user.home_dir().to_path_buf());
        for dir in fontconfig_dirs(&fontconfig_root(), home.as_deref()) {
            add(dir);
        }
        // Every data directory fontconfig looks in, in its order (Guix keeps
        // its fonts outside the usual two).
        let data_dirs = std::env::var("XDG_DATA_DIRS")
            .ok()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "/usr/local/share:/usr/share".to_owned());
        for dir in data_dirs.split(':').filter(|dir| !dir.is_empty()) {
            add(PathBuf::from(dir).join("fonts"));
        }
        // What a Flatpak sees of the host's fonts.
        add(PathBuf::from("/run/host/fonts"));
        // The pre-XDG per-user directory, which fontconfig still honours.
        if let Some(home) = home {
            add(home.join(".fonts"));
        }
    }
    // ~/Library/Fonts on macOS, $XDG_DATA_HOME/fonts on Linux.
    if let Some(font_dir) = user.as_ref().and_then(|user| user.font_dir()) {
        add(font_dir.to_path_buf());
    }
    dirs
}

/// fontconfig's main configuration file: `FONTCONFIG_FILE` when absolute,
/// else `fonts.conf` in `FONTCONFIG_PATH` or `/etc/fonts`.
fn fontconfig_root() -> PathBuf {
    std::env::var_os("FONTCONFIG_FILE")
        .map(PathBuf::from)
        .filter(|file| file.is_absolute())
        .unwrap_or_else(|| {
            std::env::var_os("FONTCONFIG_PATH")
                .map_or_else(|| PathBuf::from("/etc/fonts"), PathBuf::from)
                .join("fonts.conf")
        })
}

/// How deeply configuration files may include one another.
const FONTCONFIG_INCLUDE_DEPTH: usize = 8;

/// The font directories fontconfig's own configuration names. NixOS keeps
/// each font package in its own store path and lists those paths only
/// there, so the data directories alone find none of them.
fn fontconfig_dirs(root: &Path, home: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut read = Vec::new();
    read_fontconfig(root, home, 0, &mut read, &mut dirs);
    dirs
}

/// Collects the `<dir>` entries of one configuration file and of the files
/// and directories it includes. Paths relative to a file are relative to its
/// directory; `prefix="xdg"` entries are left out, because the data
/// directories are asked for separately.
fn read_fontconfig(
    path: &Path,
    home: Option<&Path>,
    depth: usize,
    read: &mut Vec<PathBuf>,
    dirs: &mut Vec<PathBuf>,
) {
    if depth > FONTCONFIG_INCLUDE_DEPTH || read.iter().any(|seen| seen == path) {
        return;
    }
    read.push(path.to_path_buf());
    if path.is_dir() {
        let Ok(entries) = std::fs::read_dir(path) else {
            return;
        };
        let mut files: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|file| {
                file.extension()
                    .is_some_and(|extension| extension == "conf")
            })
            .collect();
        files.sort();
        for file in files {
            read_fontconfig(&file, home, depth + 1, read, dirs);
        }
        return;
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let base = path.parent().unwrap_or(Path::new("/"));
    for (tag, attributes, value) in fontconfig_elements(&text) {
        if attributes.contains("prefix=\"xdg\"") {
            continue;
        }
        let Some(resolved) = fontconfig_path(&value, base, home) else {
            continue;
        };
        if tag == "dir" {
            if !dirs.contains(&resolved) {
                dirs.push(resolved);
            }
        } else {
            read_fontconfig(&resolved, home, depth + 1, read, dirs);
        }
    }
}

/// The `<dir>` and `<include>` elements of a configuration file, with their
/// attributes and text, outside comments.
fn fontconfig_elements(text: &str) -> Vec<(&'static str, String, String)> {
    let mut elements = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        rest = &rest[open..];
        if let Some(comment) = rest.strip_prefix("<!--") {
            rest = comment.find("-->").map_or("", |end| &comment[end + 3..]);
            continue;
        }
        let tag = ["dir", "include"].into_iter().find(|tag| {
            rest[1..].starts_with(tag)
                && rest[1 + tag.len()..].starts_with(|c: char| c == '>' || c.is_whitespace())
        });
        let Some(tag) = tag else {
            rest = &rest[1..];
            continue;
        };
        let Some(head_end) = rest.find('>') else {
            break;
        };
        let attributes = rest[1 + tag.len()..head_end].trim().to_owned();
        let body = &rest[head_end + 1..];
        let close = format!("</{tag}>");
        let Some(body_end) = body.find(&close) else {
            break;
        };
        let value = body[..body_end]
            .trim()
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&apos;", "'")
            .replace("&amp;", "&");
        elements.push((tag, attributes, value));
        rest = &body[body_end + close.len()..];
    }
    elements
}

/// Resolves a path from a configuration file: `~` is the home directory and
/// a relative path is relative to the file's own directory. The files are
/// POSIX, so a leading `/` is absolute on every platform the parser is tested
/// on (Windows would otherwise put it on the base's drive).
fn fontconfig_path(value: &str, base: &Path, home: Option<&Path>) -> Option<PathBuf> {
    if value.is_empty() {
        return None;
    }
    if let Some(rest) = value.strip_prefix('~') {
        return Some(home?.join(rest.trim_start_matches('/')));
    }
    let path = PathBuf::from(value);
    Some(if path.is_absolute() || value.starts_with('/') {
        path
    } else {
        base.join(path)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fontconfig_configuration_names_the_font_directories() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        std::fs::create_dir_all(root.join("conf.d")).unwrap();
        std::fs::write(
            root.join("fonts.conf"),
            r#"<?xml version="1.0"?>
    <!DOCTYPE fontconfig SYSTEM "urn:fontconfig:fonts.dtd">
    <fontconfig>
      <!-- <dir>/commented/out</dir> -->
      <dir>/nix/store/abc-noto-fonts-cjk-sans/share/fonts</dir>
      <dir prefix="xdg">fonts</dir>
      <dir>~/.local/fonts</dir>
      <include ignore_missing="yes">conf.d</include>
      <include ignore_missing="yes">missing.conf</include>
      <include>fonts.conf</include>
    </fontconfig>"#,
        )
        .unwrap();
        std::fs::write(
            root.join("conf.d/10-extra.conf"),
            "<fontconfig><dir>relative/fonts</dir><dir>/a&amp;b</dir></fontconfig>",
        )
        .unwrap();
        std::fs::write(root.join("conf.d/README"), "<dir>/not/a/conf</dir>").unwrap();
        let home = Path::new("/home/reader");
        assert_eq!(
            fontconfig_dirs(&root.join("fonts.conf"), Some(home)),
            vec![
                PathBuf::from("/nix/store/abc-noto-fonts-cjk-sans/share/fonts"),
                home.join(".local/fonts"),
                root.join("conf.d/relative/fonts"),
                PathBuf::from("/a&b"),
            ]
        );
        assert_eq!(fontconfig_path("~/x", root, None), None, "no home");
        assert!(fontconfig_dirs(&root.join("absent.conf"), None).is_empty());
    }
}
