//! Rewrites the Pianissimo encoder's block-local attention as banded full attention.
//!
//! The released export (NeMo `rel_pos_local_attn`, 256 frames on each side) pads every input to
//! 256-frame blocks and scores each block against 768 keys, so a 1 s segment costs about as much
//! encoder time as a 20 s one. The same attention is a T x T product with a |q - k| <= 256 band
//! mask. The rewrite keeps every quantized projection of each layer and replaces only the scores,
//! softmax and context part, so the output is the same as the original's: bit-identical on the
//! inputs checked, for segments both shorter and longer than the window.
//!
//! It runs locally on the downloaded file (`tyst-cli models fetch`); the result is pinned by
//! SHA-256 in the manifest like a downloaded file.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use protobuf::{EnumOrUnknown, Message};

use crate::{Error, Result};

mod generated {
    include!(concat!(env!("OUT_DIR"), "/onnx_proto/mod.rs"));
}

use generated::onnx as proto;
use proto::attribute_proto::AttributeType;
use proto::tensor_proto::DataType;
use proto::{AttributeProto, GraphProto, ModelProto, NodeProto, TensorProto};

/// Attention window on each side, in encoder frames (the model card's `att_context [256, 256]`).
const WINDOW: i64 = 256;
/// Rows of the precomputed relative-position projection: positions +256 down to -256.
const POSITIONS: i64 = 2 * WINDOW + 1;
/// The encoder's padding mask, `[1, T]`, true for padded frames.
const PAD_MASK: &str = "/Not_output_0";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RewriteSummary {
    pub layers: usize,
    pub nodes_before: usize,
    pub nodes_after: usize,
}

/// Reads `src`, rewrites its attention layers and writes the result to `dst`.
pub fn band_attention_file(src: &Path, dst: &Path) -> Result<RewriteSummary> {
    let bytes = std::fs::read(src).map_err(|e| Error::io(src, e))?;
    let mut model =
        ModelProto::parse_from_bytes(&bytes).map_err(|e| Error::Model(format!("{}: {e}", src.display())))?;
    drop(bytes);
    let summary = band_attention(&mut model)?;
    let out = model.write_to_bytes().map_err(|e| Error::Model(format!("{}: {e}", dst.display())))?;
    std::fs::write(dst, out).map_err(|e| Error::io(dst, e))?;
    Ok(summary)
}

/// Rewrites every `/layers.N/self_attn/` block of the graph in place.
pub fn band_attention(model: &mut ModelProto) -> Result<RewriteSummary> {
    let graph = model.graph.as_mut().ok_or_else(|| Error::Model("encoder has no graph".into()))?;
    let nodes_before = graph.node.len();
    let by_name: HashMap<String, usize> =
        graph.node.iter().enumerate().map(|(i, n)| (n.name().to_string(), i)).collect();
    let find = |name: &str| -> Result<&NodeProto> {
        by_name
            .get(name)
            .map(|&i| &graph.node[i])
            .ok_or_else(|| Error::Model(format!("encoder rewrite: node {name} not found (unexpected export)")))
    };
    if !graph.node.iter().any(|n| n.output.iter().any(|o| o == PAD_MASK)) {
        return Err(Error::Model(format!("encoder rewrite: no {PAD_MASK} (unexpected export)")));
    }

    let mut b = Builder::default();
    let shared = Shared::build(&mut b);
    let mut removed = HashSet::new();
    let mut layers = 0;
    while by_name.contains_key(&format!("/layers.{layers}/self_attn/Reshape_22")) {
        let p = format!("/layers.{layers}/self_attn/");
        let out = |s: &str| format!("{p}{s}_output_0");
        let bias = |add: &str| -> Result<String> {
            let node = find(&format!("{p}{add}"))?;
            node.input
                .iter()
                .find(|x| **x != out("Pad"))
                .cloned()
                .ok_or_else(|| Error::Model(format!("encoder rewrite: {p}{add} has no bias input")))
        };
        let bias_u = bias("Add")?;
        let bias_v = bias("Add_1")?;
        let scale = find(&format!("{p}Div"))?.input[1].clone();
        let softmax_attrs = find(&format!("{p}Softmax"))?.attribute.clone();

        let l = format!("/banded/l{layers}/");
        let (q, k, v, pos) = (out("Transpose"), out("Transpose_1"), out("Transpose_2"), out("Transpose_6"));
        let qu = b.node("Add", &[&q, &bias_u], &format!("{l}qu"), vec![]);
        let qv = b.node("Add", &[&q, &bias_v], &format!("{l}qv"), vec![]);
        let kt = b.node("Transpose", &[&k], &format!("{l}kt"), vec![ints("perm", &[0, 1, 3, 2])]);
        let ac = b.node("MatMul", &[&qu, &kt], &format!("{l}ac"), vec![]);
        let pt = b.node("Transpose", &[&pos], &format!("{l}pt"), vec![ints("perm", &[0, 1, 3, 2])]);
        let bd_all = b.node("MatMul", &[&qv, &pt], &format!("{l}bdfull"), vec![]);
        let bd = b.node("GatherElements", &[&bd_all, &shared.rel_index], &format!("{l}bd"), vec![int("axis", 3)]);
        let sum = b.node("Add", &[&ac, &bd], &format!("{l}acbd"), vec![]);
        let scores = b.node("Div", &[&sum, &scale], &format!("{l}scores"), vec![]);
        let scores = b.node("Where", &[&shared.masked, &shared.neg, &scores], &format!("{l}masked"), vec![]);
        let attn = b.node("Softmax", &[&scores], &format!("{l}softmax"), softmax_attrs);
        let attn = b.node("Where", &[&shared.query_pad, &shared.zero, &attn], &format!("{l}zeroq"), vec![]);
        let ctx = b.node("MatMul", &[&attn, &v], &format!("{l}ctx"), vec![]);
        let ctx = b.node("Transpose", &[&ctx], &format!("{l}ctxt"), vec![ints("perm", &[0, 2, 1, 3])]);
        b.node_to("Reshape", &[&ctx, &shared.out_shape], out("Reshape_22"), &format!("{l}out"), vec![]);
        removed.insert(format!("{p}Reshape_22"));
        layers += 1;
    }
    if layers == 0 {
        return Err(Error::Model("encoder rewrite: no attention layers found (unexpected export)".into()));
    }

    graph.node.retain(|n| !removed.contains(n.name()));
    graph.node.extend(b.nodes);
    graph.initializer.extend(b.inits);
    prune(graph);
    topo_sort(graph)?;
    Ok(RewriteSummary { layers, nodes_before, nodes_after: graph.node.len() })
}

/// Tensors shared by all layers, derived from the padding mask.
struct Shared {
    /// `[1, 8, T, T]` index into the 513 relative positions: row 256 - (q - k), clipped.
    rel_index: String,
    /// `[1, 1, T, T]`, true where a query may not attend a key (outside the band, or a padded key).
    masked: String,
    /// `[1, 1, T, 1]`, true for padded queries (their attention row is zeroed, as in the original).
    query_pad: String,
    neg: String,
    zero: String,
    out_shape: String,
}

impl Shared {
    fn build(b: &mut Builder) -> Self {
        let g = "/banded/";
        let shape = b.node("Shape", &[PAD_MASK], &format!("{g}shape"), vec![]);
        let one = b.i64s(&format!("{g}one"), &[], &[1]);
        let t = b.node("Gather", &[&shape, &one], &format!("{g}T"), vec![]);
        let zero_i = b.i64s(&format!("{g}zero_i"), &[], &[0]);
        let range = b.node("Range", &[&zero_i, &t, &one], &format!("{g}range"), vec![]);
        let ax0 = b.i64s(&format!("{g}ax0"), &[1], &[0]);
        let ax1 = b.i64s(&format!("{g}ax1"), &[1], &[1]);
        let tq = b.node("Unsqueeze", &[&range, &ax1], &format!("{g}tq"), vec![]);
        let tk = b.node("Unsqueeze", &[&range, &ax0], &format!("{g}tk"), vec![]);
        let rel = b.node("Sub", &[&tq, &tk], &format!("{g}rel"), vec![]);
        let window = b.i64s(&format!("{g}window"), &[], &[WINDOW]);
        let row = b.node("Sub", &[&window, &rel], &format!("{g}row"), vec![]);
        let last = b.i64s(&format!("{g}last"), &[], &[POSITIONS - 1]);
        let row = b.node("Clip", &[&row, &zero_i, &last], &format!("{g}rowc"), vec![]);
        let t1 = b.node("Unsqueeze", &[&t, &ax0], &format!("{g}t1"), vec![]);
        let heads = b.i64s(&format!("{g}batch_heads"), &[2], &[1, 8]);
        let index_shape = b.node("Concat", &[&heads, &t1, &t1], &format!("{g}index_shape"), vec![int("axis", 0)]);
        let rel_index = b.node("Expand", &[&row, &index_shape], &format!("{g}rel_index"), vec![]);
        let dist = b.node("Abs", &[&rel], &format!("{g}dist"), vec![]);
        let band = b.node("LessOrEqual", &[&dist, &window], &format!("{g}band"), vec![]);
        let valid = b.node("Not", &[PAD_MASK], &format!("{g}valid"), vec![]);
        let kshape = b.i64s(&format!("{g}key_shape"), &[4], &[0, 1, 1, -1]);
        let key_valid = b.node("Reshape", &[&valid, &kshape], &format!("{g}key_valid"), vec![]);
        let allowed = b.node("And", &[&band, &key_valid], &format!("{g}allowed"), vec![]);
        let masked = b.node("Not", &[&allowed], &format!("{g}masked"), vec![]);
        let qshape = b.i64s(&format!("{g}query_shape"), &[4], &[0, 1, -1, 1]);
        let query_pad = b.node("Reshape", &[PAD_MASK, &qshape], &format!("{g}query_pad"), vec![]);
        Shared {
            rel_index,
            masked,
            query_pad,
            neg: b.f32(&format!("{g}neg"), -10000.0),
            zero: b.f32(&format!("{g}zero_f"), 0.0),
            out_shape: b.i64s(&format!("{g}out_shape"), &[3], &[0, 0, 1024]),
        }
    }
}

#[derive(Default)]
struct Builder {
    nodes: Vec<NodeProto>,
    inits: Vec<TensorProto>,
}

impl Builder {
    fn node(&mut self, op: &str, inputs: &[&str], name: &str, attrs: Vec<AttributeProto>) -> String {
        let out = format!("{name}_out");
        self.node_to(op, inputs, out.clone(), name, attrs);
        out
    }

    fn node_to(&mut self, op: &str, inputs: &[&str], output: String, name: &str, attrs: Vec<AttributeProto>) {
        let mut n = NodeProto::new();
        n.set_op_type(op.into());
        n.set_name(name.into());
        n.input = inputs.iter().map(|s| s.to_string()).collect();
        n.output = vec![output];
        n.attribute = attrs;
        self.nodes.push(n);
    }

    fn i64s(&mut self, name: &str, dims: &[i64], values: &[i64]) -> String {
        let raw = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        self.tensor(name, dims, DataType::INT64, raw)
    }

    fn f32(&mut self, name: &str, value: f32) -> String {
        self.tensor(name, &[], DataType::FLOAT, value.to_le_bytes().to_vec())
    }

    fn tensor(&mut self, name: &str, dims: &[i64], ty: DataType, raw: Vec<u8>) -> String {
        let mut t = TensorProto::new();
        t.set_name(name.into());
        t.dims = dims.to_vec();
        t.set_data_type(ty as i32);
        t.set_raw_data(raw);
        self.inits.push(t);
        name.to_string()
    }
}

fn int(name: &str, value: i64) -> AttributeProto {
    let mut a = AttributeProto::new();
    a.set_name(name.into());
    a.type_ = Some(EnumOrUnknown::new(AttributeType::INT));
    a.set_i(value);
    a
}

fn ints(name: &str, values: &[i64]) -> AttributeProto {
    let mut a = AttributeProto::new();
    a.set_name(name.into());
    a.type_ = Some(EnumOrUnknown::new(AttributeType::INTS));
    a.ints = values.to_vec();
    a
}

/// Removes nodes whose outputs nothing uses, then unused initializers and stale value infos.
fn prune(graph: &mut GraphProto) {
    let outputs: HashSet<String> = graph.output.iter().map(|o| o.name().to_string()).collect();
    loop {
        let used: HashSet<&str> = graph
            .node
            .iter()
            .flat_map(|n| n.input.iter().map(String::as_str))
            .chain(outputs.iter().map(String::as_str))
            .collect();
        let keep: Vec<bool> = graph.node.iter().map(|n| n.output.iter().any(|o| used.contains(o.as_str()))).collect();
        if keep.iter().all(|&k| k) {
            break;
        }
        let mut it = keep.into_iter();
        graph.node.retain(|_| it.next().unwrap_or(true));
    }
    let used: HashSet<String> = graph.node.iter().flat_map(|n| n.input.iter().cloned()).collect();
    graph.initializer.retain(|t| used.contains(t.name()));
    let produced: HashSet<String> = graph.node.iter().flat_map(|n| n.output.iter().cloned()).collect();
    graph.value_info.retain(|v| produced.contains(v.name()));
}

/// Orders nodes so each one comes after the producers of its inputs (stable for the rest).
fn topo_sort(graph: &mut GraphProto) -> Result<()> {
    let mut avail: HashSet<String> = graph.input.iter().map(|i| i.name().to_string()).collect();
    avail.extend(graph.initializer.iter().map(|t| t.name().to_string()));
    avail.insert(String::new());
    let mut pending = std::mem::take(&mut graph.node);
    let mut ordered = Vec::with_capacity(pending.len());
    while !pending.is_empty() {
        let before = pending.len();
        let mut rest = Vec::new();
        for n in pending {
            if n.input.iter().all(|i| avail.contains(i)) {
                avail.extend(n.output.iter().cloned());
                ordered.push(n);
            } else {
                rest.push(n);
            }
        }
        if rest.len() == before {
            return Err(Error::Model(format!("encoder rewrite: unresolved inputs of {}", rest[0].name())));
        }
        pending = rest;
    }
    graph.node = ordered;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str, inputs: &[&str], outputs: &[&str]) -> NodeProto {
        let mut n = NodeProto::new();
        n.set_name(name.into());
        n.set_op_type("Identity".into());
        n.input = inputs.iter().map(|s| s.to_string()).collect();
        n.output = outputs.iter().map(|s| s.to_string()).collect();
        n
    }

    fn graph(nodes: Vec<NodeProto>) -> GraphProto {
        let mut g = GraphProto::new();
        let mut input = proto::ValueInfoProto::new();
        input.set_name("x".into());
        let mut output = proto::ValueInfoProto::new();
        output.set_name("y".into());
        g.input.push(input);
        g.output.push(output);
        g.node = nodes;
        g
    }

    fn names(g: &GraphProto) -> Vec<&str> {
        g.node.iter().map(|n| n.name()).collect()
    }

    #[test]
    fn prune_drops_dead_chains_and_initializers() {
        let mut g = graph(vec![
            node("a", &["x"], &["a"]),
            node("dead1", &["a", "w"], &["d1"]),
            node("dead2", &["d1"], &["d2"]),
            node("b", &["a"], &["y"]),
        ]);
        let mut w = TensorProto::new();
        w.set_name("w".into());
        g.initializer.push(w);
        prune(&mut g);
        assert_eq!(names(&g), ["a", "b"]);
        assert!(g.initializer.is_empty());
    }

    #[test]
    fn topo_sort_moves_consumers_after_producers() {
        let mut g = graph(vec![node("c", &["b"], &["y"]), node("a", &["x"], &["a"]), node("b", &["a", ""], &["b"])]);
        topo_sort(&mut g).unwrap();
        assert_eq!(names(&g), ["a", "b", "c"]);
    }

    #[test]
    fn topo_sort_reports_missing_inputs() {
        let mut g = graph(vec![node("a", &["nowhere"], &["y"])]);
        assert!(topo_sort(&mut g).is_err());
    }

    #[test]
    fn rejects_graphs_without_local_attention() {
        let mut model = ModelProto::new();
        model.graph = protobuf::MessageField::some(graph(vec![node("a", &["x"], &["y"])]));
        assert!(band_attention(&mut model).is_err());
    }
}
