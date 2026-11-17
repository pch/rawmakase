//! Lightroom's serialised Lua tables, read as data: no interpreter runs.
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
                    out.push(match e {
                        b'n' => b'\n',
                        b't' => b'\t',
                        other => other,
                    });
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

