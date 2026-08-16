fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "grpc")]
    {
        let proto_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("proto");

        let graphdb_proto = proto_dir.join("graphdb.proto");
        let cold_snapshot_proto = proto_dir.join("cold_snapshot.proto");

        println!("cargo:rerun-if-changed={}", graphdb_proto.display());
        println!("cargo:rerun-if-changed={}", cold_snapshot_proto.display());

        tonic_build::configure()
            .build_server(true)
            .build_client(true)
            .out_dir(std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()))
            .compile_protos(
                &[
                    graphdb_proto.to_str().unwrap(),
                    cold_snapshot_proto.to_str().unwrap(),
                ],
                &[proto_dir.to_str().unwrap()],
            )?;
    }

    Ok(())
}
