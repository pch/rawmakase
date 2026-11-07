//! The record encoding every section of an editable preview but its pixels uses: a
//! list of fields, each a `u16` id, a `u32` byte length and that many bytes, all
//! little-endian, in increasing id order with no id twice. A reader looks fields
//! up by id, so a field it doesn't know is skipped and a field missing takes its
//! documented default. A field of the wrong length is an error, never a guess.
use anyhow::{Context, Result, bail, ensure};

/// A record being written.
#[derive(Default)]
pub(super) struct Writer {
    bytes: Vec<u8>,
    last: Option<u16>,
}

impl Writer {
    pub(super) fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
    /// Field `id` holding `value`. Ids must be written in increasing order.
    pub(super) fn bytes(&mut self, id: u16, value: &[u8]) {
        assert!(
            self.last.is_none_or(|last| last < id),
            "editable preview field {id} written out of order"
        );
        self.last = Some(id);
        let len = u32::try_from(value.len()).expect("an editable preview field under 4 GiB");
        self.bytes.extend(id.to_le_bytes());
        self.bytes.extend(len.to_le_bytes());
        self.bytes.extend_from_slice(value);
    }
    pub(super) fn record(&mut self, id: u16, record: Writer) {
        self.bytes(id, &record.bytes);
    }
    pub(super) fn u8(&mut self, id: u16, v: u8) {
        self.bytes(id, &[v]);
    }
    pub(super) fn bool(&mut self, id: u16, v: bool) {
        self.u8(id, u8::from(v));
    }
    pub(super) fn u32s(&mut self, id: u16, v: &[u32]) {
        self.bytes(
            id,
            &v.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>(),
        );
    }
    pub(super) fn u32(&mut self, id: u16, v: u32) {
        self.u32s(id, &[v]);
    }
    pub(super) fn i32(&mut self, id: u16, v: i32) {
        self.bytes(id, &v.to_le_bytes());
    }
    pub(super) fn u64(&mut self, id: u16, v: u64) {
        self.bytes(id, &v.to_le_bytes());
    }
    pub(super) fn u128(&mut self, id: u16, v: u128) {
        self.bytes(id, &v.to_le_bytes());
    }
    /// IEEE 754 single precision, bit for bit: a NaN or an infinity is kept as it is.
    pub(super) fn f32s(&mut self, id: u16, v: &[f32]) {
        self.bytes(
            id,
            &v.iter()
                .flat_map(|v| v.to_bits().to_le_bytes())
                .collect::<Vec<_>>(),
        );
    }
    pub(super) fn f32(&mut self, id: u16, v: f32) {
        self.f32s(id, &[v]);
    }
    pub(super) fn str(&mut self, id: u16, v: &str) {
        self.bytes(id, v.as_bytes());
    }
    /// A list of strings: each a `u32` byte length and its UTF-8 bytes.
    pub(super) fn strs(&mut self, id: u16, v: &[String]) {
        let mut out = Vec::new();
        for s in v {
            out.extend(
                u32::try_from(s.len())
                    .expect("a string under 4 GiB")
                    .to_le_bytes(),
            );
            out.extend_from_slice(s.as_bytes());
        }
        self.bytes(id, &out);
    }
}

/// A record read: its fields by id.
pub(super) struct Record<'a> {
    fields: Vec<(u16, &'a [u8])>,
}

impl<'a> Record<'a> {
    pub(super) fn parse(mut bytes: &'a [u8]) -> Result<Self> {
        let mut fields: Vec<(u16, &[u8])> = Vec::new();
        while !bytes.is_empty() {
            ensure!(
                bytes.len() >= 6,
                "An editable preview record ends inside a field"
            );
            let id = u16::from_le_bytes([bytes[0], bytes[1]]);
            let len = u32::from_le_bytes([bytes[2], bytes[3], bytes[4], bytes[5]]) as usize;
            let value = bytes
                .get(6..6 + len)
                .context("An editable preview field runs past its record")?;
            if let Some((last, _)) = fields.last() {
                ensure!(*last < id, "Editable preview fields out of order");
            }
            fields.push((id, value));
            bytes = &bytes[6 + len..];
        }
        Ok(Self { fields })
    }
    pub(super) fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }
    pub(super) fn bytes(&self, id: u16) -> Option<&'a [u8]> {
        self.fields
            .binary_search_by_key(&id, |(id, _)| *id)
            .ok()
            .map(|i| self.fields[i].1)
    }
    /// Field `id` as `N` bytes; an error when it has another length.
    fn exact<const N: usize>(&self, id: u16) -> Result<Option<[u8; N]>> {
        self.bytes(id)
            .map(|b| {
                <[u8; N]>::try_from(b).ok().with_context(|| {
                    format!("Editable preview field {id} has {} bytes, not {N}", b.len())
                })
            })
            .transpose()
    }
    pub(super) fn record(&self, id: u16) -> Result<Option<Record<'a>>> {
        self.bytes(id).map(Record::parse).transpose()
    }
    pub(super) fn u8(&self, id: u16) -> Result<Option<u8>> {
        Ok(self.exact::<1>(id)?.map(|[v]| v))
    }
    pub(super) fn bool(&self, id: u16) -> Result<Option<bool>> {
        match self.u8(id)? {
            None => Ok(None),
            Some(0) => Ok(Some(false)),
            Some(1) => Ok(Some(true)),
            Some(v) => bail!("Editable preview field {id} is {v}, not a yes or no"),
        }
    }
    pub(super) fn u32(&self, id: u16) -> Result<Option<u32>> {
        Ok(self.exact::<4>(id)?.map(u32::from_le_bytes))
    }
    pub(super) fn u32_array<const N: usize>(&self, id: u16) -> Result<Option<[u32; N]>> {
        let Some(b) = self.bytes(id) else {
            return Ok(None);
        };
        ensure!(
            b.len() == N * 4,
            "Editable preview field {id} has {} bytes, not {}",
            b.len(),
            N * 4
        );
        Ok(Some(std::array::from_fn(|i| {
            u32::from_le_bytes([b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]])
        })))
    }
    pub(super) fn i32(&self, id: u16) -> Result<Option<i32>> {
        Ok(self.exact::<4>(id)?.map(i32::from_le_bytes))
    }
    pub(super) fn u64(&self, id: u16) -> Result<Option<u64>> {
        Ok(self.exact::<8>(id)?.map(u64::from_le_bytes))
    }
    pub(super) fn u128(&self, id: u16) -> Result<Option<u128>> {
        Ok(self.exact::<16>(id)?.map(u128::from_le_bytes))
    }
    pub(super) fn f32s(&self, id: u16) -> Result<Option<Vec<f32>>> {
        let Some(b) = self.bytes(id) else {
            return Ok(None);
        };
        ensure!(
            b.len().is_multiple_of(4),
            "Editable preview field {id} is not a list of numbers"
        );
        Ok(Some(
            b.as_chunks::<4>()
                .0
                .iter()
                .map(|c| f32::from_bits(u32::from_le_bytes(*c)))
                .collect(),
        ))
    }
    pub(super) fn f32_array<const N: usize>(&self, id: u16) -> Result<Option<[f32; N]>> {
        let Some(v) = self.f32s(id)? else {
            return Ok(None);
        };
        <[f32; N]>::try_from(v).map(Some).map_err(|v| {
            anyhow::anyhow!(
                "Editable preview field {id} has {} numbers, not {N}",
                v.len()
            )
        })
    }
    pub(super) fn f32(&self, id: u16) -> Result<Option<f32>> {
        Ok(self.f32_array::<1>(id)?.map(|[v]| v))
    }
    pub(super) fn str(&self, id: u16) -> Result<Option<String>> {
        self.bytes(id)
            .map(|b| {
                String::from_utf8(b.to_vec())
                    .ok()
                    .with_context(|| format!("Editable preview field {id} is not UTF-8 text"))
            })
            .transpose()
    }
    pub(super) fn strs(&self, id: u16) -> Result<Option<Vec<String>>> {
        let Some(mut b) = self.bytes(id) else {
            return Ok(None);
        };
        let mut out = Vec::new();
        while !b.is_empty() {
            ensure!(
                b.len() >= 4,
                "Editable preview field {id} ends inside a string"
            );
            let len = u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize;
            let s = b
                .get(4..4 + len)
                .with_context(|| format!("Editable preview field {id} ends inside a string"))?;
            out.push(
                String::from_utf8(s.to_vec())
                    .ok()
                    .with_context(|| format!("Editable preview field {id} is not UTF-8 text"))?,
            );
            b = &b[4 + len..];
        }
        Ok(Some(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_round_trip_and_unknown_ones_are_skipped() -> Result<()> {
        let mut w = Writer::default();
        w.u8(1, 7);
        w.f32s(3, &[1.5, f32::NAN]);
        w.str(9, "Café");
        w.strs(10, &["a".into(), String::new(), "ß".into()]);
        w.u128(40, u128::MAX - 1);
        let bytes = w.into_bytes();
        let r = Record::parse(&bytes)?;
        assert_eq!(r.u8(1)?, Some(7));
        assert_eq!(r.u8(2)?, None);
        let v = r.f32s(3)?.unwrap();
        assert_eq!(v[0], 1.5);
        assert!(v[1].is_nan());
        assert_eq!(r.str(9)?.as_deref(), Some("Café"));
        assert_eq!(r.strs(10)?.unwrap(), ["a", "", "ß"]);
        assert_eq!(r.u128(40)?, Some(u128::MAX - 1));
        // The wrong length is an error, not a value.
        assert!(r.u32(1).is_err());
        assert!(r.f32_array::<3>(3).is_err());
        assert!(r.bool(1).is_err());
        Ok(())
    }

    #[test]
    fn malformed_records_are_refused() {
        let mut w = Writer::default();
        w.u32(5, 1);
        let bytes = w.into_bytes();
        for cut in 1..bytes.len() {
            assert!(Record::parse(&bytes[..cut]).is_err(), "cut at {cut}");
        }
        // The same id twice, or out of order.
        let twice = [bytes.clone(), bytes].concat();
        assert!(Record::parse(&twice).is_err());
    }
}
