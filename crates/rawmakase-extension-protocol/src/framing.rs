//! One JSON object per line, at most [`MAX_LINE`] bytes, over any byte stream.
//!
//! [`LineReader`] never holds more than one line's worth of bytes, however long
//! the other side's line is: a line past the limit is skipped up to its newline
//! and reported, and the next line reads normally. [`write_line`] refuses a
//! line past the limit before writing any of it.
use crate::MAX_LINE;
use crate::message::Message;
use std::io::{self, Read, Write};

/// Why a line couldn't be read or written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrameError {
    /// The line was longer than [`MAX_LINE`]; a reader skipped it.
    TooLong,
    /// The stream ended inside a line.
    Truncated,
    /// The line wasn't a protocol message (or couldn't be encoded as one).
    Malformed {
        /// What was wrong, for the log.
        reason: String,
    },
}
impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLong => write!(f, "a line longer than {MAX_LINE} bytes"),
            Self::Truncated => f.write_str("the stream ended inside a line"),
            Self::Malformed { reason } => write!(f, "a malformed line: {reason}"),
        }
    }
}
impl std::error::Error for FrameError {}

/// Reads lines from a byte stream, each at most [`MAX_LINE`] bytes.
pub struct LineReader<R> {
    inner: R,
    buffer: Vec<u8>,
    /// The bytes of `buffer` already read from `inner`, from `start`.
    start: usize,
    end: usize,
    /// Inside a line that is too long: skip to its newline.
    skipping: bool,
}

impl<R: Read> LineReader<R> {
    /// A reader of `inner`'s lines.
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            // A line, its newline, and room to read more.
            buffer: vec![0; MAX_LINE + 1 + (64 << 10)],
            start: 0,
            end: 0,
            skipping: false,
        }
    }

    /// The next line, without its newline; `Ok(None)` at the end of the
    /// stream. An `Err` of [`FrameError`] concerns one line only: the next call
    /// reads the line after it. An I/O error ends the stream.
    pub fn next_line(&mut self) -> io::Result<Option<Result<Vec<u8>, FrameError>>> {
        loop {
            if let Some(at) = self.buffer[self.start..self.end]
                .iter()
                .position(|&b| b == b'\n')
            {
                let line = self.start..self.start + at;
                self.start += at + 1;
                if std::mem::take(&mut self.skipping) {
                    return Ok(Some(Err(FrameError::TooLong)));
                }
                if line.len() > MAX_LINE {
                    return Ok(Some(Err(FrameError::TooLong)));
                }
                return Ok(Some(Ok(self.buffer[line].to_vec())));
            }
            // No newline yet. A line already past the limit is dropped as it
            // comes, so the buffer never grows.
            if self.end - self.start > MAX_LINE {
                self.skipping = true;
                self.start = 0;
                self.end = 0;
            } else if self.start > 0 {
                self.buffer.copy_within(self.start..self.end, 0);
                self.end -= self.start;
                self.start = 0;
            }
            let read = loop {
                match self.inner.read(&mut self.buffer[self.end..]) {
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    other => break other?,
                }
            };
            if read == 0 {
                let partial = self.end > self.start || self.skipping;
                self.start = 0;
                self.end = 0;
                self.skipping = false;
                return Ok(partial.then_some(Err(FrameError::Truncated)));
            }
            self.end += read;
        }
    }

    /// Whether every byte read so far belongs to lines already returned: no
    /// unread byte is buffered and no oversized line is being skipped. A
    /// reader of handles checks here that none is left over, since a
    /// conforming sender's handles always arrive with their line.
    pub fn at_boundary(&self) -> bool {
        self.start == self.end && !self.skipping
    }

    /// The stream being read, for a reader that also takes what arrives
    /// beside the bytes (handles).
    pub fn get_mut(&mut self) -> &mut R {
        &mut self.inner
    }

    /// The next message: a line read and decoded. `Ok(None)` at the end of the
    /// stream; an `Err` of [`FrameError`] concerns one line only.
    pub fn next_message(&mut self) -> io::Result<Option<Result<Message, LineError>>> {
        Ok(self.next_line()?.map(|line| match line {
            Ok(line) => Message::decode(&line).map_err(LineError::Decode),
            Err(e) => Err(LineError::Frame(e)),
        }))
    }
}

/// A line that couldn't be read as a message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LineError {
    /// The line was too long, or cut off.
    Frame(FrameError),
    /// The line wasn't a message.
    Decode(crate::message::DecodeError),
}
impl std::fmt::Display for LineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Frame(e) => e.fmt(f),
            Self::Decode(e) => e.fmt(f),
        }
    }
}
impl std::error::Error for LineError {}

/// Writes `message` as one line and flushes. A message too long for a line is
/// refused with [`io::ErrorKind::InvalidInput`] and nothing is written.
pub fn write_line(out: &mut impl Write, message: &Message) -> io::Result<()> {
    let mut line = message
        .encode()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    line.push(b'\n');
    out.write_all(&line)?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Hands out its bytes a few at a time, as a socket may.
    struct Trickle(Vec<u8>, usize);
    impl Read for Trickle {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            let n = out.len().min(self.1).min(self.0.len());
            out[..n].copy_from_slice(&self.0[..n]);
            self.0.drain(..n);
            Ok(n)
        }
    }

    fn lines(bytes: Vec<u8>, chunk: usize) -> Vec<Result<Vec<u8>, FrameError>> {
        let mut reader = LineReader::new(Trickle(bytes, chunk));
        std::iter::from_fn(|| reader.next_line().unwrap()).collect()
    }

    #[test]
    fn lines_arriving_in_pieces_read_whole() {
        for chunk in [1, 3, 1 << 20] {
            assert_eq!(
                lines(b"{\"a\":1}\n\n{\"b\":2}\n".to_vec(), chunk),
                vec![
                    Ok(b"{\"a\":1}".to_vec()),
                    Ok(vec![]),
                    Ok(b"{\"b\":2}".to_vec())
                ]
            );
        }
    }

    #[test]
    fn an_oversized_line_is_skipped_and_the_next_one_reads() {
        for size in [MAX_LINE + 1, 3 * MAX_LINE] {
            let mut bytes = vec![b'x'; size];
            bytes.extend(b"\nnext\n");
            let read = lines(bytes, 1 << 16);
            assert_eq!(read, vec![Err(FrameError::TooLong), Ok(b"next".to_vec())]);
        }
        // The longest line allowed still reads.
        let mut bytes = vec![b'y'; MAX_LINE];
        bytes.push(b'\n');
        assert_eq!(lines(bytes, 1 << 16), vec![Ok(vec![b'y'; MAX_LINE])]);
    }

    #[test]
    fn a_stream_cut_inside_a_line_says_so() {
        assert_eq!(
            lines(b"whole\npart".to_vec(), 2),
            vec![Ok(b"whole".to_vec()), Err(FrameError::Truncated)]
        );
        assert_eq!(
            lines(vec![b'z'; MAX_LINE + 5], 1 << 16),
            vec![Err(FrameError::Truncated)]
        );
    }

    #[test]
    fn malformed_lines_are_reported_one_by_one() {
        let bytes = b"garbage\n{\"t\":\"cancel\",\"id\":1}\n\xff\xfe\n".to_vec();
        let mut reader = LineReader::new(bytes.as_slice());
        assert!(matches!(
            reader.next_message().unwrap(),
            Some(Err(LineError::Decode(_)))
        ));
        assert_eq!(
            reader.next_message().unwrap(),
            Some(Ok(Message::Cancel { id: 1 }))
        );
        assert!(matches!(
            reader.next_message().unwrap(),
            Some(Err(LineError::Decode(_)))
        ));
        assert_eq!(reader.next_message().unwrap(), None);
    }

    #[test]
    fn the_boundary_is_where_no_byte_is_left_unread() {
        let mut reader = LineReader::new(Trickle(b"one\ntwo\nthr".to_vec(), 1 << 10));
        assert!(reader.at_boundary());
        assert_eq!(reader.next_line().unwrap(), Some(Ok(b"one".to_vec())));
        assert!(
            !reader.at_boundary(),
            "two lines and a part came in one read"
        );
        assert_eq!(reader.next_line().unwrap(), Some(Ok(b"two".to_vec())));
        assert!(!reader.at_boundary(), "a partial line waits");
        assert_eq!(
            reader.next_line().unwrap(),
            Some(Err(FrameError::Truncated))
        );
        assert!(reader.at_boundary());
        let mut whole = LineReader::new(Trickle(b"one\n".to_vec(), 1 << 10));
        whole.next_line().unwrap();
        assert!(whole.at_boundary());
    }

    #[test]
    fn written_lines_read_back_and_oversized_ones_write_nothing() {
        let mut wire = Vec::new();
        let message = Message::request(1, "panel.open", json!({"panel": "p"}));
        write_line(&mut wire, &message).unwrap();
        let too_big = Message::reply(2, Ok(json!("x".repeat(MAX_LINE))));
        let error = write_line(&mut wire, &too_big).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        let mut reader = LineReader::new(wire.as_slice());
        assert_eq!(reader.next_message().unwrap(), Some(Ok(message)));
        assert_eq!(reader.next_message().unwrap(), None);
    }
}
