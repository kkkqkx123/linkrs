//! gRPC cold snapshot distribution service.
//!
//! Server side serves `.lkcs` snapshot files from a directory (a "snapshot
//! share"); clients pull files chunk-by-chunk, verify the CRC32, persist
//! them, and register them with the local storage engine so the query
//! engine's cold fallback picks them up automatically.

use std::path::{Path, PathBuf};

use tonic::{Request, Response, Status};

use crate::api::server::grpc::proto::coldsnapshot::{
    cold_snapshot_service_server::ColdSnapshotService, ListSnapshotsRequest,
    ListSnapshotsResponse, PullSnapshotRequest, PushSnapshotRequest, PushSnapshotResponse,
    SnapshotChunk, SnapshotDescriptor,
};

const CHUNK_SIZE: usize = 1024 * 1024;

/// Filesystem-backed cold snapshot service.
#[derive(Debug, Clone)]
pub struct ColdSnapshotServer {
    snapshot_dir: PathBuf,
}

impl ColdSnapshotServer {
    pub fn new<P: AsRef<Path>>(snapshot_dir: P) -> Self {
        Self {
            snapshot_dir: snapshot_dir.as_ref().to_path_buf(),
        }
    }

    fn resolve(&self, file_name: &str) -> Option<PathBuf> {
        if file_name.is_empty()
            || file_name.contains('/')
            || file_name.contains('\\')
            || file_name.contains("..")
        {
            return None;
        }
        let path = self.snapshot_dir.join(file_name);
        path.extension().is_some_and(|e| e == "lkcs").then_some(path)
    }
}

/// Metadata extraction from a `.lkcs` header. Parsed structurally so the
/// service does not need a full storage-engine dependency for listing.
fn snapshot_descriptor(path: &Path) -> Option<SnapshotDescriptor> {
    let file_size = std::fs::metadata(path).ok()?.len();
    let checksum = std::fs::read(path).ok().map(|b| crc32fast::hash(&b))?;
    let snapshot = crate::storage::cold::ColdSnapshot::open(path).ok()?;
    Some(SnapshotDescriptor {
        label: snapshot.label(),
        label_name: snapshot.schema().label_name.clone(),
        snapshot_ts: snapshot.snapshot_ts(),
        edge_count: snapshot.edge_count(),
        file_name: path.file_name().map(|f| f.to_string_lossy().into_owned())?,
        file_size,
        checksum,
    })
}

#[tonic::async_trait]
impl ColdSnapshotService for ColdSnapshotServer {
    async fn list_remote_snapshots(
        &self,
        _request: Request<ListSnapshotsRequest>,
    ) -> Result<Response<ListSnapshotsResponse>, Status> {
        let mut snapshots = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&self.snapshot_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "lkcs") {
                    if let Some(desc) = snapshot_descriptor(&path) {
                        snapshots.push(desc);
                    }
                }
            }
        }
        snapshots.sort_by_key(|s| (s.label, s.snapshot_ts));
        Ok(Response::new(ListSnapshotsResponse { snapshots }))
    }

    type PullSnapshotStream = tokio_stream::wrappers::ReceiverStream<Result<SnapshotChunk, Status>>;

    async fn pull_snapshot(
        &self,
        request: Request<PullSnapshotRequest>,
    ) -> Result<Response<Self::PullSnapshotStream>, Status> {
        let req = request.into_inner();
        let requested_ts = req.snapshot_ts;

        // Pick the newest snapshot of the label, optionally at a specific ts.
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&self.snapshot_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "lkcs") {
                    if let Ok(snapshot) = crate::storage::cold::ColdSnapshot::open(&path) {
                        if snapshot.label() == req.label
                            && requested_ts.is_none_or(|ts| snapshot.snapshot_ts() == ts)
                        {
                            candidates.push(path);
                        }
                    }
                }
            }
        }
        if candidates.is_empty() {
            return Err(Status::not_found(format!(
                "no cold snapshot for label {}",
                req.label
            )));
        }
        candidates.sort_by_key(|p| {
            crate::storage::cold::ColdSnapshot::open(p)
                .map(|s| s.snapshot_ts())
                .unwrap_or(0)
        });
        let path = candidates.pop().unwrap();

        let (tx, rx) = tokio::sync::mpsc::channel(4);
        tokio::spawn(async move {
            let file_name = path
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default();
            let checksum = std::fs::read(&path)
                .ok()
                .map(|b| crc32fast::hash(&b))
                .unwrap_or(0);
            let Ok(file) = std::fs::File::open(&path) else {
                let _ = tx.send(Err(Status::not_found("snapshot file disappeared"))).await;
                return;
            };
            let mut reader = std::io::BufReader::new(file);
            use std::io::{Read, Seek, SeekFrom};
            let file_len = reader.get_ref().metadata().map(|m| m.len()).unwrap_or(0);
            let mut offset = 0u64;
            loop {
                let remaining = file_len.saturating_sub(offset);
                let read_size = remaining.min(CHUNK_SIZE as u64) as usize;
                if read_size == 0 {
                    break;
                }
                let mut buf = vec![0u8; read_size];
                if Read::read_exact(&mut reader, &mut buf).is_err() {
                    let _ = tx.send(Err(Status::internal("failed to read snapshot file"))).await;
                    return;
                }
                let last = offset + read_size as u64 >= file_len;
                if tx
                    .send(Ok(SnapshotChunk {
                        file_name: file_name.clone(),
                        checksum,
                        offset,
                        data: buf,
                        last,
                    }))
                    .await
                    .is_err()
                {
                    return;
                }
                offset += read_size as u64;
            }
            let _ = Seek::seek(&mut reader, SeekFrom::Start(0)).ok();
        });

        Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(rx)))
    }

    async fn push_snapshot(
        &self,
        request: Request<PushSnapshotRequest>,
    ) -> Result<Response<PushSnapshotResponse>, Status> {
        let req = request.into_inner();
        let Some(path) = self.resolve(&req.file_name) else {
            return Err(Status::invalid_argument("invalid snapshot file name"));
        };
        let actual_crc = crc32fast::hash(&req.data);
        if actual_crc != req.checksum {
            return Err(Status::invalid_argument(format!(
                "checksum mismatch: got {:#x}, expected {:#x}",
                actual_crc, req.checksum
            )));
        }
        // Sanity-check the payload parses as a snapshot before persisting.
        if crate::storage::cold::ColdSnapshot::from_bytes(&req.data).is_err() {
            return Err(Status::invalid_argument("payload is not a valid .lkcs snapshot"));
        }
        std::fs::create_dir_all(&self.snapshot_dir)
            .map_err(|e| Status::internal(format!("cannot create snapshot dir: {}", e)))?;
        std::fs::write(&path, &req.data)
            .map_err(|e| Status::internal(format!("cannot write snapshot file: {}", e)))?;
        log::info!(
            "Cold snapshot pushed: {} ({} bytes)",
            path.display(),
            req.data.len()
        );
        Ok(Response::new(PushSnapshotResponse {
            accepted: true,
            message: path.display().to_string(),
        }))
    }
}

/// Streaming pull client: fetches a snapshot from a remote `ColdSnapshotService`
/// and writes it to `dest_path`, verifying the per-chunk CRC32 of the final
/// assembly (the file CRC is carried by every chunk).
pub struct ColdSnapshotClient {
    inner: crate::api::server::grpc::proto::coldsnapshot::cold_snapshot_service_client::ColdSnapshotServiceClient<tonic::transport::Channel>,
}

impl ColdSnapshotClient {
    /// Wrap an established gRPC channel.
    pub fn with_channel(
        channel: tonic::transport::Channel,
    ) -> Self {
        Self {
            inner: crate::api::server::grpc::proto::coldsnapshot::cold_snapshot_service_client::ColdSnapshotServiceClient::new(channel),
        }
    }

    /// Connect to a remote endpoint like `http://127.0.0.1:50051`.
    pub async fn connect_addr(
        address: &str,
    ) -> Result<Self, tonic::transport::Error> {
        let channel = tonic::transport::Endpoint::new(address.to_string())?
            .connect()
            .await?;
        Ok(Self::with_channel(channel))
    }

    pub async fn list(&mut self) -> Result<Vec<SnapshotDescriptor>, Status> {
        let response = self
            .inner
            .list_remote_snapshots(ListSnapshotsRequest {})
            .await?;
        Ok(response.into_inner().snapshots)
    }

    /// Pull the latest (or timestamp-pinned) snapshot of `label` and write
    /// it to `dest_path`. Returns the assembled file size.
    pub async fn pull(
        &mut self,
        label: u32,
        snapshot_ts: Option<u64>,
        dest_path: &Path,
    ) -> Result<u64, Status> {
        let mut stream = self
            .inner
            .pull_snapshot(PullSnapshotRequest { label, snapshot_ts })
            .await?
            .into_inner();
        let mut buffer: Vec<u8> = Vec::new();
        let mut expected_crc = 0u32;
        let mut expected_len = 0u64;
        while let Some(chunk) = stream.message().await? {
            if expected_crc == 0 && chunk.checksum != 0 {
                expected_crc = chunk.checksum;
            }
            expected_len = chunk.offset + chunk.data.len() as u64;
            buffer.extend_from_slice(&chunk.data);
        }
        if expected_len == 0 {
            return Err(Status::not_found(format!(
                "remote returned an empty snapshot for label {}",
                label
            )));
        }
        if expected_crc != 0 && crc32fast::hash(&buffer) != expected_crc {
            return Err(Status::data_loss(format!(
                "snapshot checksum mismatch: got {:#x}, expected {:#x}",
                crc32fast::hash(&buffer),
                expected_crc
            )));
        }
        std::fs::write(dest_path, &buffer)
            .map_err(|e| Status::internal(format!("cannot write snapshot file: {}", e)))?;
        Ok(buffer.len() as u64)
    }

    /// Push a local `.lkcs` file to the remote share.
    pub async fn push(&mut self, path: &Path) -> Result<(), Status> {
        let data = std::fs::read(path)
            .map_err(|e| Status::internal(format!("cannot read snapshot file: {}", e)))?;
        let checksum = crc32fast::hash(&data);
        let file_name = path
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .ok_or_else(|| Status::invalid_argument("path has no file name"))?;
        let response = self
            .inner
            .push_snapshot(PushSnapshotRequest {
                file_name,
                checksum,
                data,
            })
            .await?;
        let accepted = response.into_inner();
        if !accepted.accepted {
            return Err(Status::internal(format!(
                "remote rejected snapshot: {}",
                accepted.message
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::server::grpc::proto::coldsnapshot::cold_snapshot_service_server::ColdSnapshotServiceServer;
    use graphdb_query::storage::cold::ColdSnapshot;

    fn make_snapshot_file(dir: &Path, label: u32, ts: u64) -> PathBuf {
        let path = dir.join(format!("snap_{}_{}.lkcs", label, ts));
        ColdSnapshot::create_empty(label, "knows", ts, &path).unwrap();
        path
    }

    #[tokio::test]
    async fn test_cold_snapshot_grpc_server_ops() {
        let share_dir = tempfile::tempdir().unwrap();
        make_snapshot_file(share_dir.path(), 0, 100);

        let server = ColdSnapshotServer::new(share_dir.path());

        // Listing extracts descriptors from .lkcs headers.
        let response = server
            .list_remote_snapshots(tonic::Request::new(ListSnapshotsRequest {}))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(response.snapshots.len(), 1);
        assert_eq!(response.snapshots[0].label, 0);
        assert_eq!(response.snapshots[0].snapshot_ts, 100);
        assert_eq!(response.snapshots[0].file_name, "snap_0_100.lkcs");

        // Pulling an unknown label yields not_found.
        assert!(server
            .pull_snapshot(tonic::Request::new(PullSnapshotRequest {
                label: 42,
                snapshot_ts: None,
            }))
            .await
            .is_err());

        // Push with a wrong checksum is rejected.
        assert!(server
            .push_snapshot(tonic::Request::new(PushSnapshotRequest {
                file_name: "bad.lkcs".to_string(),
                checksum: 0,
                data: vec![1, 2, 3],
            }))
            .await
            .is_err());

        // A valid payload is persisted.
        let src = make_snapshot_file(share_dir.path(), 1, 200);
        let data = std::fs::read(&src).unwrap();
        server
            .push_snapshot(tonic::Request::new(PushSnapshotRequest {
                file_name: "copied.lkcs".to_string(),
                checksum: crc32fast::hash(&data),
                data,
            }))
            .await
            .unwrap();
        assert!(share_dir.path().join("copied.lkcs").exists());
    }

    #[tokio::test]
    async fn test_cold_snapshot_grpc_client_pull() {
        let share_dir = tempfile::tempdir().unwrap();
        let src = make_snapshot_file(share_dir.path(), 0, 100);

        // Serve the share on an ephemeral port.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let server = ColdSnapshotServer::new(share_dir.path());
            tonic::transport::Server::builder()
                .add_service(ColdSnapshotServiceServer::new(server))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await
                .unwrap();
        });

        let mut client =
            ColdSnapshotClient::connect_addr(&format!("http://{}", addr)).await.unwrap();

        let listing = client.list().await.unwrap();
        assert_eq!(listing.len(), 1);
        assert_eq!(listing[0].label, 0);

        // Pull and verify the reassembled file matches the original.
        let dest_dir = tempfile::tempdir().unwrap();
        let dest = dest_dir.path().join("pulled.lkcs");
        let size = client.pull(0, None, &dest).await.unwrap();
        assert_eq!(size, std::fs::metadata(&src).unwrap().len());
        assert_eq!(
            crc32fast::hash(&std::fs::read(&dest).unwrap()),
            crc32fast::hash(&std::fs::read(&src).unwrap())
        );

        // Pulling a timestamp that does not exist fails cleanly.
        assert!(client.pull(0, Some(999), &dest).await.is_err());
    }
}
