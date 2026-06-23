fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "grpc")]
    {
        let proto_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("proto")
            .join("graphdb.proto");

        let proto_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("proto");

        println!("cargo:rerun-if-changed={}", proto_path.display());

        tonic_build::configure()
            .build_server(true)
            .build_client(true)
            .out_dir(std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()))
            .compile_protos(
                &[proto_path.to_str().unwrap()],
                &[proto_dir.to_str().unwrap()],
            )?;
    }

    Ok(())
}
