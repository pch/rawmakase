//! Editing an ONNX model's bytes before it is loaded: making an internal tensor one of
//! the graph's outputs, so a run returns it alongside the outputs the file declares.
//!
//! The model is a protobuf `ModelProto`. Only the graph field's length and the end of
//! the graph change; every other byte is copied as it is, so the weights are never
//! decoded. The patch refuses a model whose graph has no node producing the tensor, so a
//! re-exported file that renamed it fails at load with the name, not inside ONNX Runtime.

use rawmakase_inference::InferenceError;

/// `ModelProto.graph`.
const MODEL_GRAPH: u64 = 7;
/// `GraphProto.node`.
const GRAPH_NODE: u64 = 1;
/// `GraphProto.output`.
const GRAPH_OUTPUT: u64 = 12;
/// `NodeProto.output`.
const NODE_OUTPUT: u64 = 2;
/// `TensorProto.DataType.FLOAT`.
const FLOAT: u64 = 1;

const VARINT: u64 = 0;
const FIXED64: u64 = 1;
const DELIMITED: u64 = 2;
const FIXED32: u64 = 5;

/// `model` (a serialized `ModelProto`) with `tensor` added to its graph's outputs as a
/// float32 tensor of unstated shape. The bytes are edited in place: only the inserted
/// bytes are allocated. An unreadable model, or one where no node produces `tensor`, is
/// dropped with the error.
pub(crate) fn expose_output(mut model: Vec<u8>, tensor: &str) -> Result<Vec<u8>, InferenceError> {
    let mut graph = None;
    for field in fields(&model)? {
        if field.number == MODEL_GRAPH {
            if field.wire != DELIMITED || graph.is_some() {
                return Err(invalid("its graph field is malformed or repeated"));
            }
            graph = Some(field);
        }
    }
    let graph = graph.ok_or_else(|| invalid("it has no graph"))?;
    if names_output(&model[graph.body.clone()], tensor)? {
        return Ok(model);
    }
    if !produces(&model[graph.body.clone()], tensor)? {
        return Err(invalid(&format!(
            "no node of its graph produces the tensor `{tensor}`"
        )));
    }
    let mut output = Vec::new();
    put_delimited(GRAPH_OUTPUT, &value_info(tensor), &mut output);
    let mut length = Vec::new();
    put_varint((graph.body.len() + output.len()) as u64, &mut length);
    // The output goes in first: the length's bytes sit before it and move nothing after.
    model.splice(graph.body.end..graph.body.end, output);
    model.splice(graph.after_tag..graph.body.start, length);
    Ok(model)
}

/// Whether the graph (a serialized `GraphProto`) already lists `tensor` among its outputs.
fn names_output(graph: &[u8], tensor: &str) -> Result<bool, InferenceError> {
    for field in fields(graph)? {
        if field.number != GRAPH_OUTPUT {
            continue;
        }
        if field.wire != DELIMITED {
            return Err(invalid("a graph output is malformed"));
        }
        let info = &graph[field.body];
        for part in fields(info)? {
            if part.number == 1 && part.wire == DELIMITED && &info[part.body] == tensor.as_bytes() {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Whether a node of the graph (a serialized `GraphProto`) has `tensor` among its outputs.
fn produces(graph: &[u8], tensor: &str) -> Result<bool, InferenceError> {
    for field in fields(graph)? {
        if field.number != GRAPH_NODE || field.wire != DELIMITED {
            continue;
        }
        let node = &graph[field.body];
        for part in fields(node)? {
            if part.number == NODE_OUTPUT
                && part.wire == DELIMITED
                && &node[part.body] == tensor.as_bytes()
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Room `expose_output` needs beyond the model's own length for `tensor`: the output
/// field (the name plus its tags, lengths and type) and the graph length's growth.
pub(crate) fn patch_room(tensor: &str) -> usize {
    tensor.len() + 64
}

/// The file at `path`, read into a buffer with room for `expose_output` to patch it
/// without reallocating (a reallocation could hold two copies of the weights at once).
pub(crate) fn read_for_patch(path: &std::path::Path, tensor: &str) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let size = file.metadata().map_or(0, |m| m.len() as usize);
    let mut bytes = Vec::with_capacity(size + patch_room(tensor));
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// One field of a message: where its tag ends (where a length-delimited field's
/// length begins, however many bytes the tag was written in), its number and wire
/// type, and the bytes of its value (for a length-delimited field, without the length).
struct Field {
    after_tag: usize,
    number: u64,
    wire: u64,
    body: std::ops::Range<usize>,
}

/// The top-level fields of a message, checked to fill it exactly.
fn fields(message: &[u8]) -> Result<Vec<Field>, InferenceError> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < message.len() {
        let tag = read_varint(message, &mut at)?;
        let after_tag = at;
        let (number, wire) = (tag >> 3, tag & 7);
        if number == 0 {
            return Err(invalid("a field has number 0"));
        }
        let len = match wire {
            VARINT => {
                read_varint(message, &mut at)?;
                0
            }
            FIXED64 => 8,
            FIXED32 => 4,
            DELIMITED => usize::try_from(read_varint(message, &mut at)?)
                .map_err(|_| invalid("a field is too long"))?,
            _ => return Err(invalid("a field has an unsupported wire type")),
        };
        let end = at
            .checked_add(len)
            .filter(|end| *end <= message.len())
            .ok_or_else(|| invalid("it ends inside a field"))?;
        out.push(Field {
            after_tag,
            number,
            wire,
            body: at..end,
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
            .ok_or_else(|| invalid("it ends inside a number"))?;
        *at += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(invalid("a number is too long"))
}

fn put_varint(mut value: u64, out: &mut Vec<u8>) {
    while value >= 0x80 {
        out.push((value & 0x7f) as u8 | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn put_delimited(number: u64, body: &[u8], out: &mut Vec<u8>) {
    put_varint(number << 3 | DELIMITED, out);
    put_varint(body.len() as u64, out);
    out.extend_from_slice(body);
}

/// `ValueInfoProto { name, type: TypeProto { tensor_type: { elem_type: FLOAT } } }`.
fn value_info(name: &str) -> Vec<u8> {
    let mut tensor_type = Vec::new();
    put_varint(1 << 3 | VARINT, &mut tensor_type);
    put_varint(FLOAT, &mut tensor_type);
    let mut typed = Vec::new();
    put_delimited(1, &tensor_type, &mut typed);
    let mut out = Vec::new();
    put_delimited(1, name.as_bytes(), &mut out);
    put_delimited(2, &typed, &mut out);
    out
}

fn invalid(why: &str) -> InferenceError {
    InferenceError::ModelInvalid(format!("the model cannot be read: {why}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Written out by hand, independent of the module's encoder.
    fn delimited(field: u64, body: &[u8], out: &mut Vec<u8>) {
        let mut v = field << 3 | DELIMITED;
        while v >= 0x80 {
            out.push((v & 0x7f) as u8 | 0x80);
            v >>= 7;
        }
        out.push(v as u8);
        let mut n = body.len();
        while n >= 0x80 {
            out.push((n & 0x7f) as u8 | 0x80);
            n >>= 7;
        }
        out.push(n as u8);
        out.extend_from_slice(body);
    }

    /// The float32 `ValueInfoProto` of `name`, byte by byte.
    fn float_output(name: &str) -> Vec<u8> {
        let mut out = Vec::new();
        delimited(1, name.as_bytes(), &mut out);
        // type { tensor_type { elem_type: FLOAT } }
        delimited(2, &[0x0a, 0x02, 0x08, 0x01], &mut out);
        out
    }

    /// One node, one input, the outputs `y` and `hidden`, and an initializer long enough
    /// that the graph's length takes two bytes.
    fn graph() -> Vec<u8> {
        graph_producing(&["hidden"])
    }

    /// [`graph`] with its node producing `outputs` after `y` instead of `hidden`.
    fn graph_producing(outputs: &[&str]) -> Vec<u8> {
        let mut node = Vec::new();
        delimited(1, b"x", &mut node); // input
        delimited(2, b"y", &mut node); // output
        for output in outputs {
            delimited(2, output.as_bytes(), &mut node); // another output
        }
        delimited(4, b"Relu", &mut node); // op_type
        let mut graph = Vec::new();
        delimited(1, &node, &mut graph);
        delimited(2, b"g", &mut graph); // name
        delimited(5, &[7u8; 200], &mut graph); // initializer, opaque here
        delimited(11, &float_output("x"), &mut graph);
        delimited(12, &float_output("y"), &mut graph);
        graph
    }

    /// ir_version, the graph, then opset_import after it.
    fn model(graph: Option<&[u8]>) -> Vec<u8> {
        let mut out = vec![0x08, 0x08]; // ir_version: 8
        if let Some(graph) = graph {
            delimited(MODEL_GRAPH, graph, &mut out);
        }
        let mut opset = Vec::new();
        opset.extend([0x10, 0x11]); // version: 17
        delimited(8, &opset, &mut out);
        out
    }

    #[test]
    fn the_tensor_becomes_an_output_and_nothing_else_changes() {
        let before = graph();
        let mut after = before.clone();
        delimited(GRAPH_OUTPUT, &float_output("hidden"), &mut after);
        assert_eq!(
            expose_output(model(Some(&before)), "hidden").unwrap(),
            model(Some(&after))
        );
    }

    #[test]
    fn an_output_already_there_is_not_added_again() {
        let once = expose_output(model(Some(&graph())), "hidden").unwrap();
        let twice = expose_output(once.clone(), "hidden").unwrap();
        assert_eq!(twice, once);
    }

    #[test]
    fn an_output_with_a_malformed_value_info_is_invalid() {
        let mut broken = graph();
        delimited(GRAPH_OUTPUT, &[0x0a, 0x05, b'x'], &mut broken);
        assert!(matches!(
            expose_output(model(Some(&broken)), "hidden"),
            Err(InferenceError::ModelInvalid(_))
        ));
    }

    #[test]
    fn a_model_with_room_is_patched_without_moving() {
        let name = "n".repeat(300);
        let graph = graph_producing(&[&name]);
        let mut input = Vec::with_capacity(model(Some(&graph)).len() + patch_room(&name));
        input.extend(model(Some(&graph)));
        let start = input.as_ptr();
        let patched = expose_output(input, &name).unwrap();
        assert_eq!(patched.as_ptr(), start);
    }

    #[test]
    fn a_long_name_and_a_graph_after_other_fields_are_handled() {
        let name = "n".repeat(300);
        let mut prefix = vec![0x08, 0x08];
        delimited(2, b"producer", &mut prefix); // producer_name
        prefix.extend([0x29, 1, 2, 3, 4, 5, 6, 7, 8]); // field 5, fixed64
        prefix.extend([0x7d, 1, 2, 3, 4]); // field 15, fixed32
        let graph = graph_producing(&[&name]);
        let mut input = prefix.clone();
        delimited(MODEL_GRAPH, &graph, &mut input);
        let mut expected_graph = graph;
        delimited(GRAPH_OUTPUT, &float_output(&name), &mut expected_graph);
        let mut expected = prefix;
        delimited(MODEL_GRAPH, &expected_graph, &mut expected);
        assert_eq!(expose_output(input, &name).unwrap(), expected);
    }

    /// A graph tag written in two bytes instead of one, as protobuf allows: the tag is
    /// kept as it is and only the length after it changes.
    #[test]
    fn a_graph_tag_written_longer_than_it_needs_is_patched() {
        let padded = [(MODEL_GRAPH << 3 | DELIMITED) as u8 | 0x80, 0x00];
        let framed = |graph: &[u8]| {
            let mut out = vec![0x08, 0x08];
            let mut field = Vec::new();
            delimited(MODEL_GRAPH, graph, &mut field);
            out.extend(padded);
            out.extend(&field[1..]);
            out
        };
        let mut expected = graph();
        delimited(GRAPH_OUTPUT, &float_output("hidden"), &mut expected);
        assert_eq!(
            expose_output(framed(&graph()), "hidden").unwrap(),
            framed(&expected)
        );
    }

    #[test]
    fn truncated_bytes_are_invalid() {
        let whole = model(Some(&graph()));
        for cut in [1, 3, whole.len() / 2, whole.len() - 1] {
            assert!(
                matches!(
                    expose_output(whole[..cut].to_vec(), "hidden"),
                    Err(InferenceError::ModelInvalid(_))
                ),
                "cut at {cut}"
            );
        }
    }

    #[test]
    fn a_model_without_a_graph_is_invalid() {
        assert!(matches!(
            expose_output(model(None), "hidden"),
            Err(InferenceError::ModelInvalid(_))
        ));
        assert!(matches!(
            expose_output(Vec::new(), "hidden"),
            Err(InferenceError::ModelInvalid(_))
        ));
    }

    #[test]
    fn a_tensor_no_node_produces_is_invalid() {
        let graph = graph();
        let err = expose_output(model(Some(&graph)), "absent").unwrap_err();
        match err {
            InferenceError::ModelInvalid(why) => assert!(why.contains("absent"), "{why}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn two_graphs_are_invalid() {
        let mut twice = model(Some(&graph()));
        delimited(MODEL_GRAPH, &graph(), &mut twice);
        assert!(matches!(
            expose_output(twice, "hidden"),
            Err(InferenceError::ModelInvalid(_))
        ));
    }
}
