//! Lightroom's smart collection text: a Lua table literal such as
//! `s = { { criteria = "rating", operation = ">=", value = 5, }, combine =
//! "intersect", }`. It is read by a bounded parser and never run as code:
//! only tables, strings, numbers, booleans and `nil` are accepted, with
//! limits on the text's length, the nesting depth and the number of
//! entries. [`write()`] writes the layout Lightroom Classic 15.6 writes (tabs,
//! a trailing comma after every entry, numbers to 14 significant digits), so
//! a rule read from Lightroom is written back byte for byte.
use std::fmt::Write as _;

/// Longer text is refused unread.
pub const MAX_LEN: usize = 1 << 20;
/// Tables nested deeper are refused: Lightroom's editor nests groups a few
/// levels at most.
pub const MAX_DEPTH: usize = 32;
/// More entries, counted over every table, are refused.
pub const MAX_ENTRIES: usize = 100_000;

/// A Lua value as the literal gives it.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    /// Always finite.
    Number(f64),
    Str(String),
    Table(Table),
}

/// A table: its positional entries in order, then its named ones in the
/// order written. `nil` entries are left out, as Lua leaves them out.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Table {
    pub array: Vec<Value>,
    pub fields: Vec<(String, Value)>,
}
impl Table {
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.fields.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
}

/// Why a text was not read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    /// Byte offset in the text.
    pub at: usize,
    pub message: String,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (at byte {})", self.message, self.at)
    }
}
impl std::error::Error for Error {}

/// Reads `text`: one table, optionally assigned to a name (`s = {…}`, as
/// Lightroom writes it) or returned (`return {…}`).
pub fn parse(text: &str) -> Result<Table, Error> {
    if text.len() > MAX_LEN {
        return Err(Error {
            at: 0,
            message: format!("the text is longer than {MAX_LEN} bytes"),
        });
    }
    let mut p = Parser {
        s: text.as_bytes(),
        text,
        at: 0,
        entries: 0,
    };
    p.space()?;
    if p.word("return") {
        p.space()?;
    } else if let Some(name_end) = p.peek_name() {
        let save = p.at;
        p.at = name_end;
        p.space()?;
        if p.eat(b'=') {
            p.space()?;
        } else {
            p.at = save;
        }
    }
    let table = match p.value(0)? {
        Some(Value::Table(t)) => t,
        _ => return Err(p.error("expected a table")),
    };
    p.space()?;
    p.eat(b';');
    p.space()?;
    if p.at != p.s.len() {
        return Err(p.error("unexpected text after the table"));
    }
    Ok(table)
}

struct Parser<'a> {
    s: &'a [u8],
    text: &'a str,
    at: usize,
    entries: usize,
}
impl Parser<'_> {
    fn error(&self, message: &str) -> Error {
        Error {
            at: self.at,
            message: message.into(),
        }
    }
    fn peek(&self) -> Option<u8> {
        self.s.get(self.at).copied()
    }
    fn eat(&mut self, c: u8) -> bool {
        if self.peek() == Some(c) {
            self.at += 1;
            true
        } else {
            false
        }
    }
    /// Skips white space and comments.
    fn space(&mut self) -> Result<(), Error> {
        loop {
            match self.peek() {
                Some(c) if c.is_ascii_whitespace() => self.at += 1,
                Some(b'-') if self.s.get(self.at + 1) == Some(&b'-') => {
                    self.at += 2;
                    if let Some(level) = self.long_bracket_level() {
                        self.long_string(level)?;
                    } else {
                        while let Some(c) = self.peek() {
                            self.at += 1;
                            if c == b'\n' {
                                break;
                            }
                        }
                    }
                }
                _ => return Ok(()),
            }
        }
    }
    /// The end of the name at the cursor, if one starts there.
    fn peek_name(&self) -> Option<usize> {
        let first = self.peek()?;
        if !(first.is_ascii_alphabetic() || first == b'_') {
            return None;
        }
        let len = self.s[self.at..]
            .iter()
            .position(|c| !(c.is_ascii_alphanumeric() || *c == b'_'))
            .unwrap_or(self.s.len() - self.at);
        Some(self.at + len)
    }
    /// Takes `word` if it is the whole name at the cursor.
    fn word(&mut self, word: &str) -> bool {
        match self.peek_name() {
            Some(end) if &self.text[self.at..end] == word => {
                self.at = end;
                true
            }
            _ => false,
        }
    }
    /// `None` for `nil`.
    fn value(&mut self, depth: usize) -> Result<Option<Value>, Error> {
        match self.peek() {
            Some(b'{') => self.table(depth + 1).map(|t| Some(Value::Table(t))),
            Some(b'"' | b'\'') => self.short_string().map(|s| Some(Value::Str(s))),
            Some(b'[') => match self.long_bracket_level() {
                Some(level) => self.long_string(level).map(|s| Some(Value::Str(s))),
                None => Err(self.error("expected a value")),
            },
            Some(b'-' | b'.' | b'0'..=b'9') => self.number().map(|n| Some(Value::Number(n))),
            _ if self.word("true") => Ok(Some(Value::Bool(true))),
            _ if self.word("false") => Ok(Some(Value::Bool(false))),
            _ if self.word("nil") => Ok(None),
            _ => Err(self.error("expected a value")),
        }
    }
    fn table(&mut self, depth: usize) -> Result<Table, Error> {
        if depth > MAX_DEPTH {
            return Err(self.error("tables are nested too deeply"));
        }
        self.at += 1; // `{`
        let mut table = Table::default();
        loop {
            self.space()?;
            if self.eat(b'}') {
                return Ok(table);
            }
            self.entries += 1;
            if self.entries > MAX_ENTRIES {
                return Err(self.error("too many entries"));
            }
            // A named entry: `name = value` or `["name"] = value`.
            let mut key = None;
            if let Some(end) = self.peek_name() {
                let save = self.at;
                let name = &self.text[self.at..end];
                self.at = end;
                self.space()?;
                if self.eat(b'=') {
                    key = Some(name.to_string());
                } else {
                    self.at = save;
                }
            } else if self.peek() == Some(b'[') && self.long_bracket_level().is_none() {
                self.at += 1;
                self.space()?;
                let Some(b'"' | b'\'') = self.peek() else {
                    return Err(self.error("only string keys are read in brackets"));
                };
                let name = self.short_string()?;
                self.space()?;
                if !self.eat(b']') {
                    return Err(self.error("expected `]`"));
                }
                self.space()?;
                if !self.eat(b'=') {
                    return Err(self.error("expected `=`"));
                }
                key = Some(name);
            }
            self.space()?;
            let value = self.value(depth)?;
            match (key, value) {
                (Some(key), Some(value)) => {
                    if table.fields.iter().any(|(k, _)| *k == key) {
                        return Err(self.error("a key is given twice"));
                    }
                    table.fields.push((key, value));
                }
                (None, Some(value)) => table.array.push(value),
                (_, None) => {}
            }
            self.space()?;
            if !(self.eat(b',') || self.eat(b';')) {
                self.space()?;
                if self.eat(b'}') {
                    return Ok(table);
                }
                return Err(self.error("expected `,` or `}`"));
            }
        }
    }
    fn number(&mut self) -> Result<f64, Error> {
        let start = self.at;
        let negative = self.eat(b'-');
        if negative {
            self.space()?;
        }
        let digits_at = self.at;
        let hex = self.s[self.at..].starts_with(b"0x") || self.s[self.at..].starts_with(b"0X");
        if hex {
            self.at += 2;
            let from = self.at;
            while self.peek().is_some_and(|c| c.is_ascii_hexdigit()) {
                self.at += 1;
            }
            let n = u64::from_str_radix(&self.text[from..self.at], 16)
                .map_err(|_| self.error("a number could not be read"))?;
            #[allow(clippy::cast_precision_loss)]
            let n = n as f64;
            return Ok(if negative { -n } else { n });
        }
        while let Some(c) = self.peek() {
            let exponent_sign =
                matches!(c, b'+' | b'-') && matches!(self.s.get(self.at - 1), Some(b'e' | b'E'));
            if c.is_ascii_digit() || c == b'.' || c == b'e' || c == b'E' || exponent_sign {
                self.at += 1;
            } else {
                break;
            }
        }
        let n: f64 = self.text[digits_at..self.at].parse().map_err(|_| Error {
            at: start,
            message: "a number could not be read".into(),
        })?;
        if !n.is_finite() {
            return Err(Error {
                at: start,
                message: "a number is out of range".into(),
            });
        }
        Ok(if negative { -n } else { n })
    }
    /// The level of the long bracket (`[[`, `[=[`…) at the cursor.
    fn long_bracket_level(&self) -> Option<usize> {
        let rest = self.s.get(self.at..)?;
        if rest.first() != Some(&b'[') {
            return None;
        }
        let level = rest[1..].iter().take_while(|c| **c == b'=').count();
        (rest.get(1 + level) == Some(&b'[')).then_some(level)
    }
    fn long_string(&mut self, level: usize) -> Result<String, Error> {
        self.at += level + 2;
        // A newline right after the opening bracket is not part of it.
        if self.s[self.at..].starts_with(b"\r\n") {
            self.at += 2;
        } else if self.peek() == Some(b'\n') {
            self.at += 1;
        }
        let close = format!("]{}]", "=".repeat(level));
        let Some(len) = self.text[self.at..].find(&close) else {
            return Err(self.error("a long string is not closed"));
        };
        let s = self.text[self.at..self.at + len].to_string();
        self.at += len + close.len();
        Ok(s)
    }
    fn short_string(&mut self) -> Result<String, Error> {
        let quote = self.s[self.at];
        self.at += 1;
        let mut bytes = Vec::new();
        loop {
            let Some(c) = self.peek() else {
                return Err(self.error("a string is not closed"));
            };
            self.at += 1;
            match c {
                b'\n' | b'\r' => return Err(self.error("a string is not closed")),
                c if c == quote => break,
                b'\\' => self.escape(&mut bytes)?,
                c => bytes.push(c),
            }
        }
        String::from_utf8(bytes).map_err(|_| self.error("a string is not UTF-8"))
    }
    fn escape(&mut self, out: &mut Vec<u8>) -> Result<(), Error> {
        let Some(c) = self.peek() else {
            return Err(self.error("a string is not closed"));
        };
        self.at += 1;
        match c {
            b'a' => out.push(7),
            b'b' => out.push(8),
            b'f' => out.push(12),
            b'n' | b'\n' => out.push(b'\n'),
            b'r' => out.push(b'\r'),
            b't' => out.push(b'\t'),
            b'v' => out.push(11),
            b'\\' | b'"' | b'\'' => out.push(c),
            b'z' => {
                while self.peek().is_some_and(|c| c.is_ascii_whitespace()) {
                    self.at += 1;
                }
            }
            b'x' => {
                let hex = self.text.get(self.at..self.at + 2).unwrap_or("");
                let byte =
                    u8::from_str_radix(hex, 16).map_err(|_| self.error("a bad `\\x` escape"))?;
                self.at += 2;
                out.push(byte);
            }
            b'0'..=b'9' => {
                let from = self.at - 1;
                while self.at - from < 3 && self.peek().is_some_and(|c| c.is_ascii_digit()) {
                    self.at += 1;
                }
                let byte: u8 = self.text[from..self.at]
                    .parse()
                    .map_err(|_| self.error("a bad decimal escape"))?;
                out.push(byte);
            }
            b'u' => {
                if !self.eat(b'{') {
                    return Err(self.error("a bad `\\u` escape"));
                }
                let from = self.at;
                while self.peek().is_some_and(|c| c.is_ascii_hexdigit()) {
                    self.at += 1;
                }
                let code = u32::from_str_radix(&self.text[from..self.at], 16)
                    .ok()
                    .and_then(char::from_u32);
                let (Some(code), true) = (code, self.eat(b'}')) else {
                    return Err(self.error("a bad `\\u` escape"));
                };
                let mut buf = [0; 4];
                out.extend(code.encode_utf8(&mut buf).as_bytes());
            }
            _ => return Err(self.error("an unknown escape")),
        }
        Ok(())
    }
}

/// `table` as Lightroom writes a smart collection: `s = {…}` and a newline,
/// each table's positional entries first, then its named ones in their
/// order.
pub fn write(table: &Table) -> String {
    let mut out = String::from("s = ");
    write_table(&mut out, table, 0);
    out.push('\n');
    out
}

fn write_table(out: &mut String, table: &Table, depth: usize) {
    out.push_str("{\n");
    let indent = "\t".repeat(depth + 1);
    for value in &table.array {
        out.push_str(&indent);
        write_value(out, value, depth + 1);
        out.push_str(",\n");
    }
    for (key, value) in &table.fields {
        out.push_str(&indent);
        if is_name(key) {
            out.push_str(key);
        } else {
            out.push('[');
            write_string(out, key);
            out.push(']');
        }
        out.push_str(" = ");
        write_value(out, value, depth + 1);
        out.push_str(",\n");
    }
    out.push_str(&"\t".repeat(depth));
    out.push('}');
}

fn write_value(out: &mut String, value: &Value, depth: usize) {
    match value {
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&number(*n)),
        Value::Str(s) => write_string(out, s),
        Value::Table(t) => write_table(out, t, depth),
    }
}

/// Whether `key` can be written bare, as a Lua name that is not a keyword.
fn is_name(key: &str) -> bool {
    const KEYWORDS: [&str; 22] = [
        "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if",
        "in", "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
    ];
    let mut chars = key.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !KEYWORDS.contains(&key)
}

fn write_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                let _ = write!(out, "\\{:03}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// `n` as Lua's `%.14g` writes it, which is how Lightroom wrote
/// `6.6438561897747` for log2(100): integers without a point, else 14
/// significant digits without trailing zeros.
pub fn number(n: f64) -> String {
    if n == n.trunc() && n.abs() < 1e15 {
        #[allow(clippy::cast_possible_truncation)]
        return format!("{}", n as i64);
    }
    let scientific = format!("{n:.13e}");
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    if !(-4..14).contains(&exponent) {
        let mantissa = trim_zeros(mantissa);
        let sign = if exponent < 0 { '-' } else { '+' };
        return format!("{mantissa}e{sign}{:02}", exponent.abs());
    }
    #[allow(clippy::cast_sign_loss)]
    let decimals = (13 - exponent) as usize;
    trim_zeros(&format!("{n:.decimals$}")).to_string()
}

fn trim_zeros(s: &str) -> &str {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.')
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lightroom Classic 15.6's text for its Five Stars collection.
    const FIVE_STARS: &str = "s = {\n\t{\n\t\tcriteria = \"rating\",\n\t\toperation = \">=\",\n\t\tvalue = 5,\n\t},\n\tcombine = \"intersect\",\n}\n";

    #[test]
    fn lightroom_text_is_read_and_written_back_byte_for_byte() {
        let table = parse(FIVE_STARS).unwrap();
        assert_eq!(table.get("combine"), Some(&Value::Str("intersect".into())));
        let Value::Table(rule) = &table.array[0] else {
            panic!("a rule");
        };
        assert_eq!(rule.get("value"), Some(&Value::Number(5.)));
        assert_eq!(write(&table), FIVE_STARS);
    }
    #[test]
    fn numbers_are_written_as_lightroom_writes_them() {
        assert_eq!(number(100f64.log2()), "6.6438561897747");
        assert_eq!(number(-(100f64.log2())), "-6.6438561897747");
        assert_eq!(number(5.), "5");
        assert_eq!(number(-1.), "-1");
        assert_eq!(number(0.5), "0.5");
        assert_eq!(number(1e20), "1e+20");
        assert_eq!(number(0.000_012_5), "1.25e-05");
    }
    #[test]
    fn strings_keep_escapes_and_unicode() {
        let table =
            parse(r#"return { a = "q\"\\\n\65\x41\u{e9}é", ['not a name'] = 'x', }"#).unwrap();
        assert_eq!(table.get("a"), Some(&Value::Str("q\"\\\nAAéé".into())));
        assert_eq!(table.get("not a name"), Some(&Value::Str("x".into())));
        let again = parse(&write(&table)).unwrap();
        assert_eq!(again, table);
    }
    #[test]
    fn comments_nil_and_long_strings_are_read() {
        let table = parse("-- note\ns = { --[[ x ]] a = nil, b = [==[\nlong]]]==], 1 }").unwrap();
        assert_eq!(table.get("a"), None);
        assert_eq!(table.get("b"), Some(&Value::Str("long]]".into())));
        assert_eq!(table.array, [Value::Number(1.)]);
    }
    #[test]
    fn code_and_malformed_text_are_refused() {
        for text in [
            "",
            "s = { a = f() }",
            "os.execute('x')",
            "s = { a = 1 } s = {}",
            "s = { a = 1, a = 2 }",
            "s = { [1] = 2 }",
            "s = { a = \"open }",
            "s = { a = 1e999 }",
            "s = { a = 1 b = 2 }",
            "s = 5",
            "s = { a = \"\\q\" }",
            "s = { a = \"\\xff\" }",
        ] {
            assert!(parse(text).is_err(), "{text}");
        }
    }
    #[test]
    fn oversized_input_is_refused() {
        let deep = format!(
            "s = {}{}",
            "{".repeat(MAX_DEPTH + 1),
            "}".repeat(MAX_DEPTH + 1)
        );
        assert!(parse(&deep).unwrap_err().message.contains("deeply"));
        let ok = format!("s = {}{}", "{".repeat(MAX_DEPTH), "}".repeat(MAX_DEPTH));
        assert!(parse(&ok).is_ok());
        let wide = format!("s = {{ {} }}", "1,".repeat(MAX_ENTRIES + 1));
        assert!(parse(&wide).unwrap_err().message.contains("entries"));
        let long = format!("s = {{ a = \"{}\" }}", "x".repeat(MAX_LEN));
        assert!(parse(&long).unwrap_err().message.contains("longer"));
    }
}
