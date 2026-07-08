fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "qdrant-grpc")]
    {
        println!("cargo:rerun-if-changed=proto/");

        let manifest_dir = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
        let proto_dir = manifest_dir.join("proto");

        tonic_build::configure()
            .build_server(false)
            .build_client(true)
            .compile_protos(
                &[
                    proto_dir.join("collections_service.proto"),
                    proto_dir.join("points_service.proto"),
                ],
                &[proto_dir],
            )?;
    }

    Ok(())
}
