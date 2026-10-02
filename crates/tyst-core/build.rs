//! Generates Rust types for `proto/onnx.proto` (vendored from onnx v1.17.0, Apache-2.0), used by
//! `encoder_rewrite`. Pure-Rust parser, so no `protoc` is needed.

fn main() {
    protobuf_codegen::Codegen::new()
        .pure()
        .include("proto")
        .input("proto/onnx.proto")
        .cargo_out_dir("onnx_proto")
        .run_from_script();
}
