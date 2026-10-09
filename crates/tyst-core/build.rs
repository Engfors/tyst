//! Generates Rust types for the vendored protos (both Apache-2.0): `proto/onnx.proto` (onnx v1.17.0),
//! used by `encoder_rewrite`, and `proto/sentencepiece_model.proto` (sentencepiece v0.2.0), used by
//! `asr::spm`. Pure-Rust parser, so no `protoc` is needed.

fn main() {
    protobuf_codegen::Codegen::new()
        .pure()
        .include("proto")
        .input("proto/onnx.proto")
        .cargo_out_dir("onnx_proto")
        .run_from_script();
    protobuf_codegen::Codegen::new()
        .pure()
        .include("proto")
        .input("proto/sentencepiece_model.proto")
        .cargo_out_dir("spm_proto")
        .run_from_script();
}
