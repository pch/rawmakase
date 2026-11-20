//! Walking a JPEG's marker segments up to its image data, without reading
//! the rest of the file.
use std::io::{self, Read, Seek, SeekFrom};

pub const APP1: u8 = 0xe1;
const SOS: u8 = 0xda;
const EOI: u8 = 0xd9;

/// A marker segment: its code and where its data (after the length) starts
/// and how long it is, from the start of the JPEG.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    pub marker: u8,
    pub offset: u64,
    pub length: usize,
}

/// The segments of a JPEG, read from `reader` where it starts.
pub struct Segments<R> {
    reader: R,
    start: u64,
    /// Where the next marker starts.
    next: u64,
}
impl<R: Read + Seek> Segments<R> {
    /// `None` when `reader` is not at a JPEG's start of image.
    pub fn new(mut reader: R) -> io::Result<Option<Self>> {
        let start = reader.stream_position()?;
        let mut soi = [0u8; 2];
        reader.read_exact(&mut soi)?;
        Ok((soi == [0xff, 0xd8]).then_some(Self {
            reader,
            start,
            next: start + 2,
        }))
    }
    /// The next segment, with the reader at its data; `None` at the start of
    /// scan or end of image, or where the file ends or stops being a JPEG.
    /// Fill bytes (0xff) are skipped, as are the markers without a length:
    /// TEM and the restarts.
    pub fn next_segment(&mut self) -> io::Result<Option<Segment>> {
        self.reader.seek(SeekFrom::Start(self.next))?;
        let mut byte = [0u8; 1];
        loop {
            // A marker: 0xff, any fill bytes of 0xff, then its code.
            if self.reader.read_exact(&mut byte).is_err() || byte[0] != 0xff {
                return Ok(None);
            }
            while byte[0] == 0xff {
                if self.reader.read_exact(&mut byte).is_err() {
                    return Ok(None);
                }
            }
            if byte[0] == SOS || byte[0] == EOI {
                return Ok(None);
            }
            if byte[0] != 0x01 && !(0xd0..=0xd7).contains(&byte[0]) {
                break;
            }
        }
        let mut length = [0u8; 2];
        if self.reader.read_exact(&mut length).is_err() {
            return Ok(None);
        }
        let offset = self.reader.stream_position()?;
        let length = (u16::from_be_bytes(length) as usize).saturating_sub(2);
        self.next = offset + length as u64;
        Ok(Some(Segment {
            marker: byte[0],
            offset: offset - self.start,
            length,
        }))
    }
    /// The reader, at the data of the segment [`Segments::next_segment`] returned last.
    pub fn reader(&mut self) -> &mut R {
        &mut self.reader
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_skip_fill_bytes_and_markers_without_a_length() {
        let jpeg = [
            0xff, 0xd8, // SOI
            0xff, 0xe0, 0, 4, 1, 2, // APP0
            0xff, 0xff, 0xd0, // fill byte, RST0
            0xff, 0xe1, 0, 3, 9, // APP1
            0xff, 0xda, 0, 2, // SOS
        ];
        let mut s = Segments::new(io::Cursor::new(&jpeg[..])).unwrap().unwrap();
        let mut found = Vec::new();
        while let Some(segment) = s.next_segment().unwrap() {
            found.push((segment.marker, segment.offset, segment.length));
        }
        assert_eq!(found, [(0xe0, 6, 2), (0xe1, 15, 1)]);
        assert!(
            Segments::new(io::Cursor::new(&[0u8, 0][..]))
                .unwrap()
                .is_none()
        );
    }
}
