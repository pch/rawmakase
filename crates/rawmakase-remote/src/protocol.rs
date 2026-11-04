//! The wire protocol between RAWmakase and a remote selection worker: length
//! prefixed JSON frames over TCP, every one bounded, with the pixels and the
//! coverage inside them as base64.
//!
//! One request, one response, then the connection may carry the next one. The
//! worker answers in order, runs at most one model at a time and keeps at most
//! the last few photos' analyses, so a click after a selection is answered from
//! what it already computed. Cancelling is a separate small frame that may
//! arrive while a request is running or after it is done; a cancel for a
//! request that no longer runs is answered with an acknowledgement and changes
//! nothing.

use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

/// The protocol this build speaks. A worker that answers with another version
/// is refused, not half understood.
pub const VERSION: u32 = 2;

/// The largest frame accepted, in either direction: two coverages of the
/// largest raster the renderer stores (4096 × 4096, base64) plus room for the
/// JSON around them. A frame claiming more is refused before its body is read.
pub const MAX_FRAME_BYTES: usize = 64 << 20;

/// The largest photo accepted for analysis. Selections are asked about a
/// 1280-pixel-edge render (under 5 MB); the cap keeps a stray large request
/// from allocating an encoder-sized tensor of it.
pub const MAX_IMAGE_BYTES: usize = 32 << 20;

/// Longest `id` or `key` string accepted; they are labels, not data.
pub const MAX_LABEL_BYTES: usize = 128;

/// What a request asks the worker to do.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Request {
    /// The protocol version, [`VERSION`].
    pub v: u32,
    /// The client's name for this request, at most [`MAX_LABEL_BYTES`]: its
    /// response carries it back, and a `cancel` names it.
    pub id: String,
    /// What to do, tagged by `op`.
    #[serde(flatten)]
    pub op: Op,
}

/// The operations. `analyze` sends the photo; `segment` asks about one click or
/// box on a photo the worker still holds; `cancel` asks to stop one.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    /// What the worker runs: its models and provider ([`Reply::Ping`]).
    Ping,
    /// A photo's subject selection, kept under a `key` for later `segment`s.
    Analyze {
        /// The photo's width in pixels.
        width: u32,
        /// The photo's height in pixels.
        height: u32,
        /// 8-bit sRGB, interleaved R G B, row-major: `width × height × 3` bytes, at
        /// most [`MAX_IMAGE_BYTES`].
        #[serde(with = "base64_bytes")]
        pixels: Vec<u8>,
    },
    /// What one click or box marks on a photo the worker analysed.
    Segment {
        /// The `key` the photo's `analyze` answered.
        key: String,
        /// The clicks and box.
        prompt: WirePrompt,
    },
    /// The photo's sky, from its own model.
    Sky {
        /// The photo's width in pixels.
        width: u32,
        /// The photo's height in pixels.
        height: u32,
        /// 8-bit sRGB, interleaved R G B, row-major: `width × height × 3` bytes, at
        /// most [`MAX_IMAGE_BYTES`].
        #[serde(with = "base64_bytes")]
        pixels: Vec<u8>,
    },
    /// Asks the worker to stop a request; it is answered with [`Reply::Ack`].
    Cancel {
        /// The `id` of the request to stop.
        target: String,
    },
}

/// What the worker answered: exactly one of `ok` and `err`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Response {
    /// The protocol version the worker speaks.
    pub v: u32,
    /// The `id` of the request this answers.
    pub id: String,
    /// The answer, when the request succeeded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ok: Option<Reply>,
    /// Why it didn't.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub err: Option<WireError>,
}

impl Response {
    /// A successful answer to `id`.
    pub fn success(id: impl Into<String>, reply: Reply) -> Self {
        Self {
            v: VERSION,
            id: id.into(),
            ok: Some(reply),
            err: None,
        }
    }
    /// A refusal of the request to `id`.
    pub fn failure(id: impl Into<String>, code: Code, message: String) -> Self {
        Self {
            v: VERSION,
            id: id.into(),
            ok: None,
            err: Some(WireError { code, message }),
        }
    }
    /// The answer, when it is well formed; a response with neither or both of
    /// `ok` and `err` says nothing and is refused.
    pub fn result(self) -> Result<Reply, WireError> {
        match (self.ok, self.err) {
            (Some(reply), None) => Ok(reply),
            (None, Some(error)) => Err(error),
            _ => Err(WireError {
                code: Code::Invalid,
                message: "the response has no answer or two of them".into(),
            }),
        }
    }
}

/// A successful answer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "reply", rename_all = "snake_case")]
pub enum Reply {
    /// What the worker runs.
    Ping {
        /// The selection model, as its id, `@`, and the first 16 hex digits of
        /// its weights' SHA-256.
        model: String,
        /// The panoptic model's id, which finds people and animals.
        panoptic: String,
        /// The sky model, empty when the worker was started without one.
        sky: String,
        /// The execution provider the models run on, as the runtime names it.
        provider: String,
    },
    /// A photo analysed.
    Analyze {
        /// What the worker calls this photo; `segment` asks under this name.
        key: String,
        /// The photo's people and animals, all zero where there are none.
        subject: WireCoverage,
    },
    /// What the prompt marks.
    Segment {
        /// The object, at the photo's aspect ratio.
        coverage: WireCoverage,
    },
    /// The photo's sky.
    Sky {
        /// Where the sky model finds sky.
        coverage: WireCoverage,
    },
    /// A `cancel` was noted; whether it stopped anything is said by the request
    /// it targeted, which answers cancelled or completes.
    Ack,
}

/// Why a request produced nothing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WireError {
    /// What the client may decide on.
    pub code: Code,
    /// What went wrong, for a person to read.
    pub message: String,
}

/// The error codes, each one something the client can decide on.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Code {
    /// The request's cancel flag was raised in time.
    Cancelled,
    /// The request was not something this protocol allows.
    Invalid,
    /// The worker speaks another protocol version.
    Unsupported,
    /// The worker no longer holds the analysis this `key` named; ask for it
    /// again with `analyze`.
    UnknownKey,
    /// The worker tried and failed.
    Failed,
}

/// A prompt as it travels; the same clicks and box the model takes, as
/// fractions of the photo's width and height.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct WirePrompt {
    /// The clicks, at most 32.
    #[serde(default)]
    pub points: Vec<WirePoint>,
    /// A box, `[left, top, right, bottom]`, with left < right and top < bottom.
    #[serde(default)]
    pub bounds: Option<[f32; 4]>,
}

/// One click: where, and whether it marks the object.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct WirePoint {
    /// Across, from 0 at the left edge to 1 at the right.
    pub x: f32,
    /// Down, from 0 at the top edge to 1 at the bottom.
    pub y: f32,
    /// Whether the click marks the object, rather than what isn't it.
    pub positive: bool,
}

impl WirePrompt {
    /// The app's prompt, for sending.
    pub fn of(prompt: &rawmakase_inference::Prompt) -> Self {
        Self {
            points: prompt
                .points
                .iter()
                .map(|p| WirePoint {
                    x: p.x,
                    y: p.y,
                    positive: p.positive,
                })
                .collect(),
            bounds: prompt.bounds,
        }
    }
    /// The model's prompt, with everything it refuses rejected here, so a bad
    /// one is an error about the request, not a model run.
    pub fn to_prompt(&self) -> Result<rawmakase_inference::Prompt, String> {
        let prompt = rawmakase_inference::Prompt {
            points: self
                .points
                .iter()
                .map(|p| rawmakase_inference::Point {
                    x: p.x,
                    y: p.y,
                    positive: p.positive,
                })
                .collect(),
            bounds: self.bounds,
        };
        prompt.validate().map_err(|e| e.to_string())?;
        Ok(prompt)
    }
}

/// 8-bit coverage as it travels.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WireCoverage {
    /// Its width in pixels.
    pub width: usize,
    /// Its height in pixels.
    pub height: usize,
    /// One byte per pixel, row-major, from 0 (unselected) to 255 (selected).
    #[serde(with = "base64_bytes")]
    pub data: Vec<u8>,
}

impl WireCoverage {
    /// The app's coverage, after checking it is the size it claims and its
    /// sides are ones a coverage can have: a malformed one is an error, never
    /// a partially filled raster or a zero-sided one a consumer divides by.
    pub fn to_coverage(&self) -> Result<rawmakase_inference::Coverage, String> {
        let capped = rawmakase_inference::process::MAX_COVERAGE_SIDE;
        let plausible = self.width > 0
            && self.height > 0
            && self.width <= capped
            && self.height <= capped
            && self.width.checked_mul(self.height) == Some(self.data.len());
        if !plausible {
            return Err(format!(
                "coverage claims {} x {} but holds {} bytes",
                self.width,
                self.height,
                self.data.len()
            ));
        }
        Ok(rawmakase_inference::Coverage {
            width: self.width,
            height: self.height,
            data: self.data.clone(),
        })
    }
    /// What was computed, for sending.
    pub fn of(coverage: &rawmakase_inference::Coverage) -> Self {
        Self {
            width: coverage.width,
            height: coverage.height,
            data: coverage.data.clone(),
        }
    }
}

/// Reads one length prefixed frame. `None` is a clean end: the peer closed the
/// connection between frames. A frame claiming no bytes or more than `max` is
/// refused without reading its body. The length prefix is accumulated byte by
/// byte, because TCP hands out short reads on healthy connections too.
pub fn read_frame(stream: &mut impl Read, max: usize) -> io::Result<Option<Vec<u8>>> {
    let mut prefix = [0u8; 4];
    loop {
        match stream.read(&mut prefix[..1]) {
            Ok(0) => return Ok(None),
            Ok(_) => break,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    stream.read_exact(&mut prefix[1..])?;
    let len = u32::from_le_bytes(prefix) as usize;
    if len == 0 || len > max {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame of {len} bytes"),
        ));
    }
    let mut frame = vec![0u8; len];
    stream.read_exact(&mut frame)?;
    Ok(Some(frame))
}

/// Writes one length prefixed frame.
pub fn write_frame(stream: &mut impl Write, frame: &[u8]) -> io::Result<()> {
    let len = u32::try_from(frame.len()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "frame too large for the protocol",
        )
    })?;
    stream.write_all(&len.to_le_bytes())?;
    stream.write_all(frame)?;
    stream.flush()
}

/// The bytes as base64 inside JSON.
pub mod base64_bytes {
    use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    /// Writes `bytes` as a base64 string.
    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&BASE64.encode(bytes))
    }

    /// Reads a base64 string back into bytes.
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(deserializer)?;
        BASE64.decode(text.as_bytes()).map_err(Error::custom)
    }
}

/// Identifies what the worker was given, from the same pixels and model
/// contract that produce the result: the same photo analysed twice keeps one
/// entry, and a `segment` for another photo's `key` finds nothing.
pub fn analysis_key(image: &rawmakase_inference::RgbImage) -> String {
    let spec = rawmakase_inference::SUBJECT;
    let header = [
        (image.width as u64).to_le_bytes().as_slice(),
        (image.height as u64).to_le_bytes().as_slice(),
        spec.id.as_bytes(),
        spec.files[1].sha256.as_bytes(),
        &spec.version.to_le_bytes(),
        &spec.processing_version.to_le_bytes(),
    ]
    .concat();
    let hash = |seed| fnv1a(fnv1a(seed, &header), &image.data);
    format!(
        "{:016x}{:016x}",
        hash(0xcbf29ce484222325),
        hash(0x84222325cbf29ce4)
    )
}

fn fnv1a(seed: u64, bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .fold(seed, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x100000001b3))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_round_trips() {
        let mut buffer = Vec::new();
        write_frame(&mut buffer, b"{\"op\":\"ping\"}").unwrap();
        assert_eq!(buffer.len(), 4 + 13);
        let mut wire = buffer.as_slice();
        let read = read_frame(&mut wire, MAX_FRAME_BYTES).unwrap().unwrap();
        assert_eq!(read, b"{\"op\":\"ping\"}");
        assert!(read_frame(&mut wire, MAX_FRAME_BYTES).unwrap().is_none());
    }

    #[test]
    fn an_empty_or_oversized_frame_is_refused_before_its_body() {
        let mut buffer = Vec::new();
        buffer.extend(0u32.to_le_bytes());
        assert!(read_frame(&mut buffer.as_slice(), MAX_FRAME_BYTES).is_err());
        let mut buffer = Vec::new();
        buffer.extend((MAX_FRAME_BYTES as u32 + 1).to_le_bytes());
        buffer.extend([0u8; 8]);
        let error = read_frame(&mut buffer.as_slice(), MAX_FRAME_BYTES).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn a_truncated_frame_is_an_error_not_a_hang() {
        let mut buffer = Vec::new();
        buffer.extend(1000u32.to_le_bytes());
        buffer.extend(b"short");
        assert!(read_frame(&mut buffer.as_slice(), MAX_FRAME_BYTES).is_err());
    }

    /// A reader that hands out one byte per call, as a TCP stream may on a
    /// healthy connection; the framing must not call a short read an end.
    struct OneByteAtATime<'a> {
        bytes: &'a [u8],
    }
    impl Read for OneByteAtATime<'_> {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            if self.bytes.is_empty() {
                return Ok(0);
            }
            let n = out.len().min(1);
            out[..n].copy_from_slice(&self.bytes[..n]);
            self.bytes = &self.bytes[n..];
            Ok(n)
        }
    }

    #[test]
    fn a_frame_arriving_one_byte_at_a_time_is_read_whole() {
        let mut buffer = Vec::new();
        write_frame(&mut buffer, b"12345").unwrap();
        let mut wire = OneByteAtATime { bytes: &buffer };
        let read = read_frame(&mut wire, MAX_FRAME_BYTES).unwrap().unwrap();
        assert_eq!(read, b"12345");
        // And the clean end is still told from a cut-off prefix.
        assert!(read_frame(&mut wire, MAX_FRAME_BYTES).unwrap().is_none());
        let mut cut = Vec::new();
        cut.extend(100u32.to_le_bytes());
        let mut wire = OneByteAtATime { bytes: &cut[..2] };
        assert!(read_frame(&mut wire, MAX_FRAME_BYTES).is_err());
    }

    #[test]
    fn requests_and_responses_survive_the_wire() {
        let request = Request {
            v: VERSION,
            id: "7".into(),
            op: Op::Analyze {
                width: 2,
                height: 1,
                pixels: vec![1, 2, 3, 4, 5, 6],
            },
        };
        let text = serde_json::to_vec(&request).unwrap();
        let back: Request = serde_json::from_slice(&text).unwrap();
        assert_eq!(back, request);
        assert_eq!(
            serde_json::to_value(&request).unwrap()["op"],
            "analyze",
            "the operation is a flat `op` field"
        );

        let response = Response::failure("7", Code::UnknownKey, "gone".into());
        let text = serde_json::to_vec(&response).unwrap();
        let value = serde_json::to_value(&response).unwrap();
        assert_eq!(value["err"]["code"], "unknown_key");
        assert!(value.get("ok").is_none());
        let back: Response = serde_json::from_slice(&text).unwrap();
        assert_eq!(back, response);
        assert_eq!(
            back.result(),
            Err(WireError {
                code: Code::UnknownKey,
                message: "gone".into()
            })
        );
        let doubled = Response {
            ok: Some(Reply::Ack),
            ..response
        };
        assert!(
            doubled.result().is_err(),
            "an answer and an error is no answer"
        );
    }

    #[test]
    fn malformed_payloads_are_refused() {
        let bad_json = b"{\"v\":1,\"id\":\"1\",\"op\":\"nope\"}";
        assert!(serde_json::from_slice::<Request>(bad_json).is_err());
        let nan = b"{\"v\":1,\"id\":\"1\",\"op\":\"segment\",\"key\":\"k\",\
                    \"prompt\":{\"points\":[{\"x\":null,\"y\":0.5,\"positive\":true}]}}";
        assert!(serde_json::from_slice::<Request>(nan).is_err());

        let coverage = WireCoverage {
            width: 4,
            height: 3,
            data: vec![0; 11],
        };
        assert!(coverage.to_coverage().is_err());
        let overflowing = WireCoverage {
            width: usize::MAX,
            height: 2,
            data: vec![0; 4],
        };
        assert!(overflowing.to_coverage().is_err());
        // Zero-sided and unbounded-axis coverages are refused however many
        // bytes they carry: a consumer divides by these sides.
        for (width, height) in [(0, 0), (0, 3), (4, 0)] {
            let empty = WireCoverage {
                width,
                height,
                data: vec![0; width * height],
            };
            assert!(empty.to_coverage().is_err(), "{width} x {height}");
        }
        let tall = WireCoverage {
            width: 1,
            height: rawmakase_inference::process::MAX_COVERAGE_SIDE + 1,
            data: vec![0; rawmakase_inference::process::MAX_COVERAGE_SIDE + 1],
        };
        assert!(tall.to_coverage().is_err());
        let oversized = WireCoverage {
            width: rawmakase_inference::process::MAX_COVERAGE_SIDE + 1,
            height: rawmakase_inference::process::MAX_COVERAGE_SIDE,
            data: vec![],
        };
        assert!(oversized.to_coverage().is_err());
    }

    #[test]
    fn prompts_are_checked_before_any_model_sees_them() {
        assert!(WirePrompt::default().to_prompt().is_err(), "empty prompt");
        let far = WirePrompt {
            points: vec![WirePoint {
                x: 1.5,
                y: 0.5,
                positive: true,
            }],
            bounds: None,
        };
        assert!(far.to_prompt().is_err());
        let many = WirePrompt {
            points: (0..rawmakase_inference::process::MAX_POINTS + 1)
                .map(|i| WirePoint {
                    x: 0.5,
                    y: 0.5,
                    positive: i % 2 == 0,
                })
                .collect(),
            bounds: None,
        };
        assert!(many.to_prompt().is_err());
        let good = WirePrompt {
            points: vec![WirePoint {
                x: 0.5,
                y: 0.5,
                positive: true,
            }],
            bounds: Some([0., 0., 1., 1.]),
        };
        let prompt = good.to_prompt().unwrap();
        assert_eq!(prompt.points.len(), 1);
        assert_eq!(prompt.bounds, Some([0., 0., 1., 1.]));
    }

    #[test]
    fn the_key_depends_on_the_pixels_and_the_model() {
        let image = rawmakase_inference::RgbImage {
            width: 2,
            height: 2,
            data: vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12],
        };
        let same = analysis_key(&image);
        let mut changed = image.clone();
        changed.data[0] ^= 1;
        assert_eq!(analysis_key(&image), same);
        assert_ne!(analysis_key(&changed), same);
    }
}
