// Compile-time protobuf code generation for the gRPC layer.
//
// Source contracts live in the workspace `proto/` directory:
// - `graphdb.proto` for the client-facing GraphDB service.
//
// `tonic-build` regenerates the Rust bindings into OUT_DIR whenever a proto
// changes; `crate::grpc::proto` pulls them in via `tonic::include_proto!`.
// Nothing generated is checked in. Building with the `grpc` feature requires
// `protoc` on PATH.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "grpc")]
    {
        let manifest_dir = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
        let proto_dir = manifest_dir
            .parent()
            .and_then(|p| p.parent())
            .map(|p| p.join("proto"))
            .ok_or("graphdb-server is expected under <workspace>/crates")?;

        for file in ["graphdb.proto"] {
            println!("cargo:rerun-if-changed={}", proto_dir.join(file).display());
        }

        tonic_build::configure()
            .build_server(true)
            .build_client(true)
            .compile_protos(&[proto_dir.join("graphdb.proto")], &[proto_dir])?;
    }

    Ok(())
}
