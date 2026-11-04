//! The interface's design tokens: spacing, radii, type sizes, control
//! heights and the greys everything is drawn in. Change a value here and it
//! changes everywhere; drawing code names a token rather than a number.
//!
//! Colours go through [`Tone`]: a step on the default palette's grey ramp,
//! which [`theme::Palette::tone`](crate::app::ui::theme::Palette::tone) maps onto the
//! palette in use. Accent, danger and warning stay the palette's own.
use eframe::egui;

/// Gaps and padding, on a 4-point grid with 2 and 6 for tight spots.
pub(in crate::app) mod space {
    /// A hairline: a list row's padding, an underline's offset.
    pub(in crate::app) const HAIR: f32 = 1.;
    pub(in crate::app) const XXS: f32 = 2.;
    pub(in crate::app) const XS: f32 = 4.;
    pub(in crate::app) const S: f32 = 6.;
    pub(in crate::app) const M: f32 = 8.;
    pub(in crate::app) const L: f32 = 12.;
    pub(in crate::app) const XL: f32 = 16.;
    pub(in crate::app) const XXL: f32 = 24.;
    pub(in crate::app) const XXXL: f32 = 32.;
    /// Around an empty view's or the setup assistant's content.
    pub(in crate::app) const HUGE: f32 = 48.;
}

/// Insets shared by whole areas of the window.
pub(in crate::app) mod inset {
    use super::{margin, space};
    use eframe::egui::Margin;
    /// Around the photo, the Library's views and the status line, the same in
    /// every module so nothing shifts on a switch.
    pub(in crate::app) const STAGE: f32 = space::XL;
    /// A side panel's content from its edges.
    pub(in crate::app) const PANEL: f32 = space::M;
    /// A modal window's content from its border.
    pub(in crate::app) const DIALOG: f32 = space::XXL;
    /// Above a side panel's section header or caption band, so the first
    /// headers of neighbouring columns (Presets beside the adjustments) line
    /// up. The catalog's sidebar, whose headers are quiet captions, uses
    /// [`space::M`].
    pub(in crate::app) const SECTION: f32 = space::L;
    /// A card's content (a group inside a dialog) from its border.
    pub(in crate::app) const CARD: Margin = margin(space::XL, space::L);
    /// A toolbar strip's content (the Library's views, its filter bar).
    pub(in crate::app) const BAR: Margin = margin(space::L, space::XS);
}

/// Corner radii.
pub(in crate::app) mod radius {
    /// Bars, tracks, swatches and small tags.
    pub(in crate::app) const SMALL: f32 = 2.;
    /// Buttons, fields, chips, list highlights.
    pub(in crate::app) const CONTROL: f32 = 4.;
    /// Cards, menus, overlays on the photo.
    pub(in crate::app) const CARD: f32 = 8.;
    /// Modal windows.
    pub(in crate::app) const DIALOG: f32 = 10.;
}

/// Type sizes, in points.
pub(in crate::app) mod text {
    /// Badges, counts, captions under thumbnails.
    pub(in crate::app) const MICRO: f32 = 10.;
    /// Panel captions and secondary rows: egui's Small.
    pub(in crate::app) const SMALL: f32 = 11.;
    /// List rows, values, dialog body text.
    pub(in crate::app) const LABEL: f32 = 12.;
    /// Buttons and default text: egui's Body.
    pub(in crate::app) const BODY: f32 = 13.;
    /// Emphasised lines and sub-headings.
    pub(in crate::app) const LARGE: f32 = 14.;
    /// A modal window's title.
    pub(in crate::app) const TITLE: f32 = 16.;
    /// The setup assistant's and Preferences' headings, drop hints.
    pub(in crate::app) const HERO: f32 = 18.;
    /// The setup assistant's first heading.
    pub(in crate::app) const DISPLAY: f32 = 26.;
}

/// Heights of rows and controls.
pub(in crate::app) mod size {
    /// A list row: folders, presets, metadata fields.
    pub(in crate::app) const ROW: f32 = 20.;
    /// A compact control: segments, small fields, panel switches.
    pub(in crate::app) const CONTROL: f32 = 22.;
    /// A row in a menu.
    pub(in crate::app) const MENU: f32 = 24.;
    /// A control in a dialog or form.
    pub(in crate::app) const FIELD: f32 = 26.;
    /// A side panel's section header or caption band.
    pub(in crate::app) const HEADER: f32 = 28.;
    /// A tool button on Develop's toolbar.
    pub(in crate::app) const TOOL: f32 = 32.;
}

/// Padding inside buttons.
pub(in crate::app) mod button {
    use super::space;
    use eframe::egui::Vec2;
    /// The default button.
    pub(in crate::app) const PADDING: Vec2 = Vec2::new(space::M, space::S);
    /// A modal window's footer buttons ("Cancel", "Export").
    pub(in crate::app) const DIALOG: Vec2 = Vec2::new(space::XL, space::S);
    /// A button in a dense panel row.
    pub(in crate::app) const COMPACT: Vec2 = Vec2::new(space::S, space::XXS);
}

/// A margin of `x` at the sides and `y` at the top and bottom, from tokens.
pub(in crate::app) const fn margin(x: f32, y: f32) -> egui::Margin {
    egui::Margin::symmetric(x as i8, y as i8)
}
/// The same margin all round, from a token.
pub(in crate::app) const fn margin_same(v: f32) -> egui::Margin {
    egui::Margin::same(v as i8)
}

/// A grey of the interface, named by what it is for. Its level is the grey
/// in the default palette; other palettes map it between their own colours.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) struct Tone(u8);

impl Tone {
    // Surfaces, darkest first.
    /// Behind everything: the window's clear colour, thumbnail outlines.
    pub(in crate::app) const DEEP: Tone = Tone(15);
    /// Recessed wells: the histogram's and curve's backgrounds.
    pub(in crate::app) const WELL: Tone = Tone(20);
    /// The window and the side panels.
    pub(in crate::app) const WINDOW: Tone = Tone(22);
    /// The workspace bar, filmstrip and other strips along the window.
    pub(in crate::app) const BAR: Tone = Tone(26);
    /// A card or group set on a panel or dialog.
    pub(in crate::app) const CARD: Tone = Tone(32);
    /// Dialogs, pop-ups and the photo's backdrop.
    pub(in crate::app) const PANEL: Tone = Tone(35);
    /// Text fields, toolbars and the Library's grid.
    pub(in crate::app) const FIELD: Tone = Tone(38);
    /// Buttons, chips, key caps and a selected list row.
    pub(in crate::app) const RAISED: Tone = Tone(43);
    /// Borders, dividers, slider tracks and progress bars.
    pub(in crate::app) const OUTLINE: Tone = Tone(52);
    /// A control under the pointer.
    pub(in crate::app) const HOVER: Tone = Tone(59);
    /// A control being pressed or switched on.
    pub(in crate::app) const PRESSED: Tone = Tone(67);
    /// A border that has to stand out: focus, an on switch's track.
    pub(in crate::app) const OUTLINE_STRONG: Tone = Tone(82);
    // Text and icons, faintest first.
    /// Disabled text and icons.
    pub(in crate::app) const DISABLED: Tone = Tone(110);
    /// Placeholders, counts and hints.
    pub(in crate::app) const FAINT: Tone = Tone(125);
    /// Captions and secondary details.
    pub(in crate::app) const MUTED: Tone = Tone(140);
    /// Labels beside values, section titles.
    pub(in crate::app) const SECONDARY: Tone = Tone(150);
    /// Icons and labels a step above secondary.
    pub(in crate::app) const LABEL: Tone = Tone(170);
    /// Values and body text that is not the focus.
    pub(in crate::app) const SOFT: Tone = Tone(200);
    /// Text.
    pub(in crate::app) const TEXT: Tone = Tone(225);
    /// Titles, values being edited, hovered text.
    pub(in crate::app) const STRONG: Tone = Tone(236);
    /// The brightest marks: text on the accent, handles on the photo.
    pub(in crate::app) const BRIGHT: Tone = Tone(250);

    /// The tone's grey in the default palette.
    pub(in crate::app) const fn level(self) -> u8 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spacing_is_on_the_grid() {
        for v in [
            space::XS,
            space::M,
            space::L,
            space::XL,
            space::XXL,
            space::XXXL,
            space::HUGE,
        ] {
            assert_eq!(v % 4., 0., "{v} is off the 4-point grid");
        }
    }

    #[test]
    fn tones_get_lighter_in_order() {
        let ramp = [
            Tone::DEEP,
            Tone::WELL,
            Tone::WINDOW,
            Tone::BAR,
            Tone::CARD,
            Tone::PANEL,
            Tone::FIELD,
            Tone::RAISED,
            Tone::OUTLINE,
            Tone::HOVER,
            Tone::PRESSED,
            Tone::OUTLINE_STRONG,
            Tone::DISABLED,
            Tone::FAINT,
            Tone::MUTED,
            Tone::SECONDARY,
            Tone::LABEL,
            Tone::SOFT,
            Tone::TEXT,
            Tone::STRONG,
            Tone::BRIGHT,
        ];
        for pair in ramp.windows(2) {
            assert!(pair[0].level() < pair[1].level(), "{pair:?}");
        }
    }

    #[test]
    fn margins_keep_their_tokens() {
        assert_eq!(margin(space::XL, space::L), egui::Margin::symmetric(16, 12));
        assert_eq!(margin_same(inset::DIALOG), egui::Margin::same(24));
        assert_eq!(inset::CARD, egui::Margin::symmetric(16, 12));
    }

    /// Calls that take a size, spacing or radius: the interface's code
    /// gives them a token, so a number here is a value outside the system.
    const SIZED: [&str; 7] = [
        "add_space(",
        "FontId::proportional(",
        "FontId::monospace(",
        ".size(",
        "corner_radius(",
        "Margin::same(",
        "Margin::symmetric(",
    ];

    /// `code` with its comments, string and character literals blanked to
    /// spaces, keeping newlines, so only code is scanned and line numbers hold.
    fn code_only(code: &str) -> String {
        let chars: Vec<char> = code.chars().collect();
        let mut out = String::with_capacity(code.len());
        let mut i = 0;
        let blank = |out: &mut String, c: char| out.push(if c == '\n' { '\n' } else { ' ' });
        while i < chars.len() {
            let c = chars[i];
            let next = chars.get(i + 1).copied();
            if c == '/' && next == Some('/') {
                while i < chars.len() && chars[i] != '\n' {
                    blank(&mut out, chars[i]);
                    i += 1;
                }
            } else if c == '/' && next == Some('*') {
                let mut depth = 0;
                loop {
                    if i >= chars.len() {
                        break;
                    }
                    if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
                        depth += 1;
                        out.push_str("  ");
                        i += 2;
                    } else if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                        depth -= 1;
                        out.push_str("  ");
                        i += 2;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        blank(&mut out, chars[i]);
                        i += 1;
                    }
                }
            } else if c == 'r' && raw_prefix(&chars, i) && matches!(next, Some('"' | '#')) {
                // A raw string: r"…", r#"…"#, and br/cr ones.
                let hashes = chars[i + 1..].iter().take_while(|&&c| c == '#').count();
                if chars.get(i + 1 + hashes) != Some(&'"') {
                    out.push(c);
                    i += 1;
                    continue;
                }
                let close: String = std::iter::once('"')
                    .chain(std::iter::repeat_n('#', hashes))
                    .collect();
                let start = i;
                i += 2 + hashes;
                let rest: String = chars[i..].iter().collect();
                let end = rest.find(&close).map_or(chars.len(), |at| {
                    i + rest[..at].chars().count() + close.chars().count()
                });
                for &c in &chars[start..end] {
                    blank(&mut out, c);
                }
                i = end;
            } else if c == '"' {
                blank(&mut out, c);
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    let escaped = chars[i] == '\\';
                    blank(&mut out, chars[i]);
                    i += 1;
                    if escaped && i < chars.len() {
                        blank(&mut out, chars[i]);
                        i += 1;
                    }
                }
                if i < chars.len() {
                    blank(&mut out, chars[i]);
                    i += 1;
                }
            } else if c == '\'' {
                // A character literal ('x', '\n', '\u{..}'), not a lifetime ('a).
                let len = if next == Some('\\') {
                    chars[i + 2..]
                        .iter()
                        .position(|&c| c == '\'')
                        .map(|p| p + 3)
                } else if chars.get(i + 2) == Some(&'\'') {
                    Some(3)
                } else {
                    None
                };
                match len {
                    Some(len) => {
                        for &c in &chars[i..(i + len).min(chars.len())] {
                            blank(&mut out, c);
                        }
                        i += len;
                    }
                    None => {
                        out.push(c);
                        i += 1;
                    }
                }
            } else {
                out.push(c);
                i += 1;
            }
        }
        out
    }

    /// Whether the `r` at `i` starts a raw string's prefix: at a token's
    /// start, or after a `b` or `c` that is (`br"…"`, `cr"…"`).
    fn raw_prefix(chars: &[char], i: usize) -> bool {
        let boundary =
            |at: usize| at == 0 || !(chars[at - 1].is_alphanumeric() || chars[at - 1] == '_');
        boundary(i) || (i > 0 && matches!(chars[i - 1], 'b' | 'c') && boundary(i - 1))
    }

    /// Whether the Rust literal `literal` is zero, in any spelling.
    fn is_zero(literal: &str) -> bool {
        let radix = ["0x", "0o", "0b"]
            .iter()
            .find_map(|p| literal.strip_prefix(p));
        if let Some(digits) = radix {
            return digits
                .chars()
                .take_while(|c| c.is_ascii_hexdigit() || *c == '_')
                .all(|c| c == '0' || c == '_');
        }
        let value: String = literal
            .chars()
            .take_while(|c| c.is_ascii_digit() || matches!(c, '_' | '.' | 'e' | 'E' | '+' | '-'))
            .filter(|&c| c != '_')
            .collect();
        value
            .trim_end_matches(['e', 'E'])
            .parse::<f64>()
            .is_ok_and(|v| v == 0.)
    }

    /// The number literals in `arg` that stand for a size: not zero, and not
    /// a factor or divisor (`/ 2.` centres, `* 0.1` scales).
    fn sized_numbers(arg: &str) -> Vec<&str> {
        let bytes = arg.as_bytes();
        let mut found = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            let starts = bytes[i].is_ascii_digit()
                && (i == 0
                    || !(bytes[i - 1].is_ascii_alphanumeric() || b"_.".contains(&bytes[i - 1])));
            if !starts {
                i += 1;
                continue;
            }
            // A whole Rust literal: digits and _, a fraction, an exponent, a suffix.
            let mut end = i;
            while end < bytes.len()
                && (bytes[end].is_ascii_alphanumeric() || b"_.".contains(&bytes[end]))
            {
                let exponent_sign = matches!(bytes[end], b'e' | b'E')
                    && matches!(bytes.get(end + 1), Some(b'+' | b'-'));
                end += if exponent_sign { 2 } else { 1 };
            }
            let literal = &arg[i..end];
            let zero = is_zero(literal);
            let follows_operator = arg[..i].trim_end().ends_with(['/', '*']);
            if !zero && !follows_operator {
                found.push(literal);
            }
            i = end;
        }
        found
    }

    /// Each call in `code` that gives a size a number: its line and text.
    fn violations(code: &str) -> Vec<(usize, String)> {
        let code = code_only(&code.replace("\r\n", "\n"));
        let mut found = Vec::new();
        for call in SIZED {
            for (at, _) in code.match_indices(call) {
                let start = at + call.len();
                let mut depth = 1;
                let end = code[start..]
                    .char_indices()
                    .find(|&(_, c)| {
                        depth += match c {
                            '(' => 1,
                            ')' => -1,
                            _ => 0,
                        };
                        depth == 0
                    })
                    .map_or(code.len(), |(j, _)| start + j);
                if !sized_numbers(&code[start..end]).is_empty() {
                    let line = code[..at].matches('\n').count() + 1;
                    let text = code[at..(end + 1).min(code.len())]
                        .split_whitespace()
                        .collect::<Vec<_>>();
                    found.push((line, text.join(" ")));
                }
            }
        }
        found.sort();
        found
    }

    /// The interface's production code, without its tests: each file's text
    /// up to its `#[cfg(test)]` module, files in path order.
    fn interface_sources() -> Vec<(std::path::PathBuf, String)> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/app");
        let mut dirs = vec![root];
        let mut files = Vec::new();
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                if path.is_dir() {
                    dirs.push(path);
                } else if name.ends_with(".rs")
                    && !name.contains("tests")
                    && !["design.rs", "theme.rs", "ui_snapshot.rs"].contains(&name.as_str())
                {
                    let text = std::fs::read_to_string(&path)
                        .unwrap()
                        .replace("\r\n", "\n");
                    let code = text
                        .split("#[cfg(test)]\nmod tests")
                        .next()
                        .unwrap()
                        .to_owned();
                    files.push((path, code));
                }
            }
        }
        files.sort();
        files
    }

    #[test]
    fn the_interface_names_tokens_not_numbers() {
        let mut found = Vec::new();
        for (path, code) in interface_sources() {
            for (line, text) in violations(&code) {
                found.push(format!("{}:{line}: {text}", path.display()));
            }
        }
        assert!(
            found.is_empty(),
            "use a token from design.rs instead of a number:\n{}",
            found.join("\n")
        );
    }

    #[test]
    fn the_guard_reads_numbers_as_rust_does() {
        assert_eq!(
            sized_numbers("if sidebar { 8. } else { 14. }"),
            ["8.", "14."]
        );
        assert_eq!(sized_numbers("-6."), ["6."]);
        assert_eq!(sized_numbers("14.0_f32"), ["14.0_f32"]);
        assert_eq!(sized_numbers("1e1"), ["1e1"]);
        assert_eq!(sized_numbers("1.5e-1f32"), ["1.5e-1f32"]);
        assert!(sized_numbers("space::M").is_empty());
        assert!(sized_numbers("text::H2").is_empty());
        assert!(sized_numbers("(w - x) / 2.").is_empty());
        assert!(sized_numbers("(h * 0.1).clamp(space::XXXL, space::HUGE * 2.)").is_empty());
        assert_eq!(sized_numbers("0x0e"), ["0x0e"]);
        assert_eq!(sized_numbers("0b10i8"), ["0b10i8"]);
        for zero in ["0", "0.", "0.0", "0_f32", "0.0e0", "0x00u8", "0o0", "0b0_0"] {
            assert!(sized_numbers(zero).is_empty(), "{zero}");
        }
    }

    #[test]
    fn the_guard_scans_code_not_comments_or_strings() {
        // A `//` in a string hides nothing after it.
        let url = "ui.label(RichText::new(\"https://a.co\").size(14.));";
        assert_eq!(violations(url).len(), 1);
        // Commented-out calls, and numbers or brackets in strings, are not code.
        assert!(violations("// ui.add_space(14.);\n/* ui.add_space(14.); */").is_empty());
        assert!(violations("ui.add_space(width(\"(12\"));").is_empty());
        assert!(violations("let s = r#\"ui.add_space(3.)\"#; let c = '(';").is_empty());
        // A prefixed raw string ends where it ends: the code after it is read.
        assert_eq!(
            violations("let p = br#\"[\"]\"#; ui.add_space(14.);").len(),
            1
        );
        assert_eq!(violations("let p = cr\"x\"; ui.add_space(14.);").len(), 1);
        // A call over several lines is found, at its first line.
        let multiline = "a();\r\nui.add_space(\r\n    if x { space::M } else { 9. },\r\n);";
        assert_eq!(violations(multiline)[0].0, 2);
        // Lifetimes are not character literals.
        assert_eq!(
            violations("fn f<'a>(ui: &'a mut Ui) { ui.add_space(5.); }").len(),
            1
        );
    }
}
