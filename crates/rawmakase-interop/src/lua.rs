//! Lightroom's serialised Lua tables, read as data: no interpreter runs.
//! Develop settings in a catalog are such tables, and so are `.lrtemplate`
//! files (`s = { … }`), whose localisable strings are written `ZSTR "…"`.
use crate::xmp::local::Node;
use anyhow::{Context, Result, bail, ensure};
use std::collections::BTreeMap;

/// A Lua table literal, `{ … }`.
pub fn table(text: &str) -> Result<Node> {
    let mut p = Lua {
        s: text.as_bytes(),
        i: 0,
        depth: 0,
    };
    let node = p.value()?;
    p.space();
    ensure!(p.i == p.s.len(), "Trailing text after Lightroom table");
    Ok(node)
}

/// A `.lrtemplate` file's table: `s = { … }`.
pub fn template(text: &str) -> Result<Node> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text).trim_start();
    let rest = text
        .strip_prefix('s')
        .map(str::trim_start)
        .and_then(|t| t.strip_prefix('='))
        .context("Not a Lightroom template")?;
    table(rest)
}

/// A localisable string as Lightroom writes it, `$$$/Key=Text`: its text;
/// any other string as it is.
pub fn localized(text: &str) -> &str {
    match text.strip_prefix("$$$/") {
        Some(rest) => rest.split_once('=').map_or(text, |(_, shown)| shown),
        None => text,
    }
}

/// `text` as a Lua string literal, escaped as Lightroom's `%q` writes it.
pub fn quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\\n"),
            '\r' => out.push_str("\\r"),
            '\0' => out.push_str("\\0"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

struct Lua<'a> {
    s: &'a [u8],
    i: usize,
    depth: usize,
}
impl Lua<'_> {
    fn space(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }
    fn peek(&mut self) -> Option<u8> {
        self.space();
        self.s.get(self.i).copied()
    }
    fn value(&mut self) -> Result<Node> {
        match self.peek().context("Unexpected end of Lightroom table")? {
            b'{' => self.table(),
            b'"' | b'\'' => self.string().map(Node::Text),
            // A localisable string: `ZSTR "$$$/Key=Text"`.
            b'Z' if self.s[self.i..].starts_with(b"ZSTR") => {
                self.i += 4;
                match self.peek() {
                    Some(b'"' | b'\'') => self.string().map(Node::Text),
                    _ => bail!("Expected a string after ZSTR"),
                }
            }
            _ => {
                let start = self.i;
                while self.i < self.s.len()
                    && !matches!(self.s[self.i], b',' | b'}' | b'=')
                    && !self.s[self.i].is_ascii_whitespace()
                {
                    self.i += 1;
                }
                let token = std::str::from_utf8(&self.s[start..self.i])?;
                ensure!(
                    token == "true" || token == "false" || token.parse::<f64>().is_ok(),
                    "Unsupported Lightroom value {token}"
                );
                Ok(Node::Text(token.to_string()))
            }
        }
    }
    fn string(&mut self) -> Result<String> {
        let quote = self.s[self.i];
        self.i += 1;
        let mut out = Vec::new();
        while let Some(&c) = self.s.get(self.i) {
            self.i += 1;
            match c {
                b'\\' => {
                    let e = *self.s.get(self.i).context("Unterminated escape")?;
                    self.i += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b't' => out.push(b'\t'),
                        b'r' => out.push(b'\r'),
                        // `%q` writes a newline as a backslash before it.
                        b'\n' => out.push(b'\n'),
                        // `\ddd`: a byte, in decimal.
                        b'0'..=b'9' => {
                            let mut value = u32::from(e - b'0');
                            for _ in 0..2 {
                                match self.s.get(self.i) {
                                    Some(d @ b'0'..=b'9') => {
                                        value = value * 10 + u32::from(d - b'0');
                                        self.i += 1;
                                    }
                                    _ => break,
                                }
                            }
                            out.push(u8::try_from(value).context("Invalid escape")?);
                        }
                        other => out.push(other),
                    }
                }
                c if c == quote => return Ok(String::from_utf8(out)?),
                c => out.push(c),
            }
        }
        bail!("Unterminated Lightroom string")
    }
    fn table(&mut self) -> Result<Node> {
        self.depth += 1;
        ensure!(self.depth < 64, "Lightroom table nesting too deep");
        self.i += 1;
        let (mut list, mut record) = (Vec::new(), BTreeMap::new());
        loop {
            match self.peek().context("Unterminated Lightroom table")? {
                b'}' => {
                    self.i += 1;
                    break;
                }
                b',' | b';' => self.i += 1,
                _ => {
                    // `key = value`, or a positional value.
                    let save = self.i;
                    let mut key = None;
                    if self.s[self.i].is_ascii_alphabetic() || self.s[self.i] == b'_' {
                        let start = self.i;
                        while self.i < self.s.len()
                            && (self.s[self.i].is_ascii_alphanumeric() || self.s[self.i] == b'_')
                        {
                            self.i += 1;
                        }
                        let name = std::str::from_utf8(&self.s[start..self.i])?.to_string();
                        if self.peek() == Some(b'=') {
                            self.i += 1;
                            key = Some(name);
                        } else {
                            self.i = save;
                        }
                    }
                    let v = self.value()?;
                    match key {
                        Some(k) => {
                            record.insert(k, v);
                        }
                        None => list.push(v),
                    }
                }
            }
        }
        self.depth -= 1;
        Ok(if record.is_empty() {
            Node::List(list)
        } else {
            Node::Record(record)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_read_localisable_strings_and_escapes() -> Result<()> {
        let text = "s = {\n\ttitle = ZSTR \"$$$/A/B=Wedding \\\"Day\\\"\",\n\tvalue = { k = \"a\\\nb\\195\\169c\", },\n}\n";
        let Node::Record(r) = template(text)? else {
            panic!("a record");
        };
        let Some(Node::Text(title)) = r.get("title") else {
            panic!("a title");
        };
        assert_eq!(localized(title), "Wedding \"Day\"");
        let Some(Node::Record(value)) = r.get("value") else {
            panic!("a value");
        };
        assert_eq!(value.get("k"), Some(&Node::Text("a\nb\u{e9}c".into())));
        assert_eq!(localized("Plain"), "Plain");
        assert!(template("x = {}").is_err());
        assert!(table("{ a = ZSTR 1 }").is_err());
        Ok(())
    }

    #[test]
    fn quoted_strings_read_back() -> Result<()> {
        for text in ["Plain", "Quote \" and \\ back", "Line\nbreak", "Zażółć 🎸"] {
            assert_eq!(
                table(&format!("{{ {} }}", quoted(text)))?,
                Node::List(vec![Node::Text(text.into())])
            );
        }
        Ok(())
    }
}
