//! The part of MessagePack (msgpack.org's specification) Lightroom's documents
//! use, decoded into values. The input is a file from elsewhere: every length
//! is checked against the bytes left and nesting is bounded, so a damaged
//! document is an error, never a panic or a huge allocation.
use anyhow::{Result, bail, ensure};

/// Deeper than any document Lightroom writes; bounds the recursion.
const MAX_DEPTH: usize = 64;

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Bin(Vec<u8>),
    Array(Vec<Value>),
    Map(Vec<(Value, Value)>),
    /// An extension type, kept as its type and bytes.
    Ext(i8, Vec<u8>),
}

impl Value {
    /// The value of `key` in a map with string keys.
    pub(super) fn get(&self, key: &str) -> Option<&Value> {
        self.entries()?
            .iter()
            .find(|(k, _)| k.as_str() == Some(key))
            .map(|(_, v)| v)
    }
    pub(super) fn entries(&self) -> Option<&[(Value, Value)]> {
        match self {
            Value::Map(entries) => Some(entries),
            _ => None,
        }
    }
    pub(super) fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
    pub(super) fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(n) => Some(*n),
            _ => None,
        }
    }
}

/// The one value `bytes` hold; an error when there is more after it.
pub(super) fn decode(bytes: &[u8]) -> Result<Value> {
    let mut reader = Reader { bytes, at: 0 };
    let value = reader.value(0)?;
    ensure!(reader.at == bytes.len(), "Bytes after the value");
    Ok(value)
}

/// `v` in MessagePack's shortest forms, as Lightroom writes its documents.
/// Only what new documents hold: maps, strings and booleans.
pub(super) fn encode(v: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    encode_into(v, &mut out);
    out
}

fn encode_into(v: &Value, out: &mut Vec<u8>) {
    match v {
        Value::Bool(b) => out.push(if *b { 0xc3 } else { 0xc2 }),
        Value::Str(s) => {
            let n = s.len();
            match n {
                0..=31 => out.push(0xa0 | n as u8),
                32..=0xff => out.extend([0xd9, n as u8]),
                0x100..=0xffff => {
                    out.push(0xda);
                    out.extend((n as u16).to_be_bytes());
                }
                _ => {
                    out.push(0xdb);
                    out.extend((n as u32).to_be_bytes());
                }
            }
            out.extend(s.as_bytes());
        }
        Value::Map(entries) => {
            out.extend(map_header(entries.len()));
            for (k, v) in entries {
                encode_into(k, out);
                encode_into(v, out);
            }
        }
        Value::Array(items) if items.is_empty() => out.push(0x90),
        other => unreachable!("not written in new documents: {other:?}"),
    }
}

/// `bytes`, a map, with `key` in its map `outer` set to true, every other
/// byte as it was: the entry is moved to the end of `outer`, and `outer` is
/// added at the end when it isn't there. None when it is true already.
pub(super) fn set_true(bytes: &[u8], outer: &str, key: &str) -> Result<Option<Vec<u8>>> {
    let top = entries(bytes, 0)?;
    ensure!(top.end == bytes.len(), "Bytes after the value");
    let Some(&(_, (start, end))) = top.find(bytes, outer)? else {
        let mut out = map_header(top.items.len() + 1);
        out.extend_from_slice(&bytes[top.body..]);
        out.extend(fixstr(outer));
        out.extend(map_header(1));
        out.extend(fixstr(key));
        out.push(0xc3);
        return Ok(Some(out));
    };
    let inner = entries(bytes, start)?;
    if let Some(&(_, (v, _))) = inner.find(bytes, key)?
        && bytes[v] == 0xc3
    {
        return Ok(None);
    }
    let kept: Vec<_> = inner
        .items
        .iter()
        .filter(|((k, _), _)| {
            Reader::at(bytes, *k)
                .value(0)
                .ok()
                .as_ref()
                .and_then(Value::as_str)
                != Some(key)
        })
        .collect();
    let mut out = bytes[..start].to_vec();
    out.extend(map_header(kept.len() + 1));
    for ((k, _), (_, v_end)) in kept {
        out.extend_from_slice(&bytes[*k..*v_end]);
    }
    out.extend(fixstr(key));
    out.push(0xc3);
    out.extend_from_slice(&bytes[end..]);
    Ok(Some(out))
}

/// Where a value is in a document's bytes: its start and end.
type Span = (usize, usize);

/// A map's entries where they are in its bytes: each key's and value's
/// start and end.
struct Entries {
    body: usize,
    end: usize,
    items: Vec<(Span, Span)>,
}

impl Entries {
    /// The entry whose key is the string `key`.
    fn find(&self, bytes: &[u8], key: &str) -> Result<Option<&(Span, Span)>> {
        for item in &self.items {
            if Reader::at(bytes, item.0.0).value(0)?.as_str() == Some(key) {
                return Ok(Some(item));
            }
        }
        Ok(None)
    }
}

/// The entries of the map at `at` in `bytes`; an error when it isn't one.
fn entries(bytes: &[u8], at: usize) -> Result<Entries> {
    let mut r = Reader::at(bytes, at);
    let tag = r.take(1)?[0];
    let n = match tag {
        0x80..=0x8f => usize::from(tag & 0x0f),
        0xde => r.len(2)?,
        0xdf => r.len(4)?,
        _ => bail!("Not a map at byte {at}"),
    };
    ensure!(
        n <= (bytes.len() - r.at) / 2,
        "Map longer than its document"
    );
    let body = r.at;
    let mut items = Vec::with_capacity(n);
    for _ in 0..n {
        let k = r.at;
        r.value(1)?;
        let v = r.at;
        r.value(1)?;
        items.push(((k, v), (v, r.at)));
    }
    Ok(Entries {
        body,
        end: r.at,
        items,
    })
}

fn map_header(n: usize) -> Vec<u8> {
    match n {
        0..=15 => vec![0x80 | n as u8],
        16..=0xffff => [&[0xde][..], &(n as u16).to_be_bytes()].concat(),
        _ => [&[0xdf][..], &(n as u32).to_be_bytes()].concat(),
    }
}

/// A string under 32 bytes, as the keys written here are.
fn fixstr(s: &str) -> Vec<u8> {
    debug_assert!(s.len() < 32);
    [&[0xa0 | s.len() as u8][..], s.as_bytes()].concat()
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn at(bytes: &'a [u8], at: usize) -> Self {
        Reader { bytes, at }
    }
    fn take(&mut self, n: usize) -> Result<&[u8]> {
        let end = self.at.checked_add(n).filter(|&e| e <= self.bytes.len());
        let Some(end) = end else {
            bail!("Truncated at byte {}", self.at);
        };
        let taken = &self.bytes[self.at..end];
        self.at = end;
        Ok(taken)
    }
    fn uint(&mut self, width: usize) -> Result<u64> {
        Ok(self
            .take(width)?
            .iter()
            .fold(0, |n, &b| n << 8 | u64::from(b)))
    }
    fn int(&mut self, width: usize) -> Result<i64> {
        let n = self.uint(width)?;
        let shift = 64 - 8 * width as u32;
        Ok(((n << shift) as i64) >> shift)
    }
    fn len(&mut self, width: usize) -> Result<usize> {
        Ok(usize::try_from(self.uint(width)?)?)
    }
    fn value(&mut self, depth: usize) -> Result<Value> {
        ensure!(depth < MAX_DEPTH, "Nested too deeply");
        let tag = self.take(1)?[0];
        Ok(match tag {
            0x00..=0x7f => Value::Int(i64::from(tag)),
            0x80..=0x8f => self.map(usize::from(tag & 0x0f), depth)?,
            0x90..=0x9f => self.array(usize::from(tag & 0x0f), depth)?,
            0xa0..=0xbf => self.str(usize::from(tag & 0x1f))?,
            0xc0 => Value::Nil,
            0xc2 => Value::Bool(false),
            0xc3 => Value::Bool(true),
            0xc4..=0xc6 => {
                let n = self.len(1 << (tag - 0xc4))?;
                Value::Bin(self.take(n)?.to_vec())
            }
            0xc7..=0xc9 => {
                let n = self.len(1 << (tag - 0xc7))?;
                self.ext(n)?
            }
            0xca => Value::Float(f64::from(f32::from_bits(self.uint(4)? as u32))),
            0xcb => Value::Float(f64::from_bits(self.uint(8)?)),
            0xcc..=0xce => Value::Int(self.uint(1 << (tag - 0xcc))? as i64),
            // A uint 64 beyond i64 is no count or size Lightroom keeps.
            0xcf => Value::Int(i64::try_from(self.uint(8)?)?),
            0xd0..=0xd3 => Value::Int(self.int(1 << (tag - 0xd0))?),
            0xd4..=0xd8 => self.ext(1 << (tag - 0xd4))?,
            0xd9..=0xdb => {
                let n = self.len(1 << (tag - 0xd9))?;
                self.str(n)?
            }
            0xdc | 0xdd => {
                let n = self.len(if tag == 0xdc { 2 } else { 4 })?;
                self.array(n, depth)?
            }
            0xde | 0xdf => {
                let n = self.len(if tag == 0xde { 2 } else { 4 })?;
                self.map(n, depth)?
            }
            0xe0..=0xff => Value::Int(i64::from(tag as i8)),
            0xc1 => bail!("Reserved byte 0xc1 at {}", self.at - 1),
        })
    }
    fn str(&mut self, n: usize) -> Result<Value> {
        Ok(Value::Str(
            String::from_utf8_lossy(self.take(n)?).into_owned(),
        ))
    }
    fn ext(&mut self, n: usize) -> Result<Value> {
        let kind = self.take(1)?[0] as i8;
        Ok(Value::Ext(kind, self.take(n)?.to_vec()))
    }
    /// `n` values; each takes at least a byte, so a count beyond the bytes
    /// left is refused before anything is allocated for it.
    fn array(&mut self, n: usize, depth: usize) -> Result<Value> {
        ensure!(
            n <= self.bytes.len() - self.at,
            "Array longer than its document"
        );
        (0..n)
            .map(|_| self.value(depth + 1))
            .collect::<Result<_>>()
            .map(Value::Array)
    }
    fn map(&mut self, n: usize, depth: usize) -> Result<Value> {
        ensure!(
            n <= (self.bytes.len() - self.at) / 2,
            "Map longer than its document"
        );
        (0..n)
            .map(|_| Ok((self.value(depth + 1)?, self.value(depth + 1)?)))
            .collect::<Result<_>>()
            .map(Value::Map)
    }
}
