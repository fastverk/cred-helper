//! Compiles the connection-registry proto (messages only — NO gRPC) into
//! Rust via prost-build. Mirrors the cargo + Bazel (cargo_build_script)
//! convention: `PROTOC` is supplied by Bazel's build_script_env, or found on
//! PATH under cargo. Proto sources are reached relative to CARGO_MANIFEST_DIR
//! = `credresolve`, i.e. `../proto/...`.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    prost_build::compile_protos(
        &["../proto/fastverk/v1/connection.proto"],
        &["../proto"],
    )?;
    Ok(())
}
