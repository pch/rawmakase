//! The few fields of an ONNX model (a serialized `ModelProto`) a converted model's weights
//! are made from: the float tensors stored with their data inline.
use crate::InferenceError;
use std::collections::HashMap;

/// A float tensor of an ONNX model: its shape and its little-endian bytes, `f32`s or, when
/// `half`, `f16`s.
pub struct Tensor<'a> {
    pub dims: Vec<usize>,
    pub half: bool,
    pub data: &'a [u8],
}

/// `ModelProto.graph`, `GraphProto.initializer`, and the `TensorProto` fields read.
pub(crate) const MODEL_GRAPH: u64 = 7;
pub(crate) const GRAPH_INITIALIZER: u64 = 5;
pub(crate) const TENSOR_DIMS: u64 = 1;
pub(crate) const TENSOR_DATA_TYPE: u64 = 2;
pub(crate) const TENSOR_NAME: u64 = 8;
pub(crate) const TENSOR_RAW_DATA: u64 = 9;
pub(crate) const TENSOR_DATA_LOCATION: u64 = 14;
/// `TensorProto.DataType.FLOAT` and `FLOAT16`.
pub(crate) const FLOAT: u64 = 1;
pub(crate) const FLOAT16: u64 = 10;
pub(crate) const VARINT: u64 = 0;
pub(crate) const FIXED64: u64 = 1;
pub(crate) const DELIMITED: u64 = 2;
pub(crate) const FIXED32: u64 = 5;

/// The single- and half-precision tensors stored in the ONNX model `onnx` (a serialized `ModelProto`) with
/// their data inline, by name. Only those fields are read; nothing is copied.
pub fn initializers(onnx: &[u8]) -> Result<HashMap<&str, Tensor<'_>>, InferenceError> {
    let mut out = HashMap::new();
    for graph in fields(onnx)?
        .into_iter()
        .filter(|f| f.number == MODEL_GRAPH && f.wire == DELIMITED)
    {
        for init in fields(graph.body)?
            .into_iter()
            .filter(|f| f.number == GRAPH_INITIALIZER && f.wire == DELIMITED)
        {
            let (mut dims, mut kind, mut name, mut data, mut external) =
                (Vec::new(), None, None, None, false);
            for field in fields(init.body)? {
                match (field.number, field.wire) {
                    (TENSOR_DIMS, VARINT) => dims.push(usize::try_from(field.value).unwrap_or(0)),
                    (TENSOR_DIMS, DELIMITED) => {
                        let mut at = 0;
                        while at < field.body.len() {
                            let d = read_varint(field.body, &mut at)?;
                            dims.push(usize::try_from(d).unwrap_or(0));
                        }
                    }
                    (TENSOR_DATA_TYPE, VARINT) => kind = Some(field.value),
                    (TENSOR_NAME, DELIMITED) => name = std::str::from_utf8(field.body).ok(),
                    (TENSOR_RAW_DATA, DELIMITED) => data = Some(field.body),
                    (TENSOR_DATA_LOCATION, VARINT) => external = field.value != 0,
                    _ => {}
                }
            }
            let half = match kind {
                Some(FLOAT) => false,
                Some(FLOAT16) => true,
                _ => continue,
            };
            if let (Some(name), Some(data), false) = (name, data, external) {
                out.insert(name, Tensor { dims, half, data });
            }
        }
    }
    Ok(out)
}

/// One field of a protobuf message: its number, wire type, its value for a varint and
/// its bytes for a length-delimited field.
struct Field<'a> {
    number: u64,
    wire: u64,
    value: u64,
    body: &'a [u8],
}

/// The top-level fields of a message, checked to fill it exactly.
fn fields(message: &[u8]) -> Result<Vec<Field<'_>>, InferenceError> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < message.len() {
        let tag = read_varint(message, &mut at)?;
        let (number, wire) = (tag >> 3, tag & 7);
        let (mut value, len) = (
            0,
            match wire {
                VARINT => 0,
                FIXED64 => 8,
                FIXED32 => 4,
                DELIMITED => usize::try_from(read_varint(message, &mut at)?)
                    .map_err(|_| unreadable("a field is too long"))?,
                _ => return Err(unreadable("a field has an unsupported wire type")),
            },
        );
        if wire == VARINT {
            value = read_varint(message, &mut at)?;
        }
        let end = at
            .checked_add(len)
            .filter(|end| *end <= message.len())
            .ok_or_else(|| unreadable("it ends inside a field"))?;
        out.push(Field {
            number,
            wire,
            value,
            body: &message[at..end],
        });
        at = end;
    }
    Ok(out)
}

fn read_varint(bytes: &[u8], at: &mut usize) -> Result<u64, InferenceError> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *bytes
            .get(*at)
            .ok_or_else(|| unreadable("it ends inside a number"))?;
        *at += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(unreadable("a number is too long"))
}

fn unreadable(why: &str) -> InferenceError {
    InferenceError::ModelInvalid(format!("the ONNX model cannot be read: {why}"))
}
