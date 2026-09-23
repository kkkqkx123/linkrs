//! Binary serialization helpers for `EntityRef` values.

use graphdb_core::types::storage_ids::{VertexId, VertexIdKind};
use graphdb_core::wal::EntityRef;

fn write_vertex_id<W: std::io::Write>(writer: &mut W, vid: &VertexId) -> std::io::Result<()> {
    let bytes = vid.as_bytes();
    let len = bytes.len().min(u8::MAX as usize) as u8;
    writer.write_all(&[vid.kind().as_u8()])?;
    writer.write_all(&[len])?;
    writer.write_all(&bytes[..len as usize])
}

fn read_vertex_id<R: std::io::Read>(reader: &mut R) -> std::io::Result<VertexId> {
    let mut kind = [0u8; 1];
    reader.read_exact(&mut kind)?;
    let kind = VertexIdKind::from_u8(kind[0]).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Unknown VertexId kind in EntityRef",
        )
    })?;
    let mut len = [0u8; 1];
    reader.read_exact(&mut len)?;
    let mut bytes = vec![0u8; len[0] as usize];
    reader.read_exact(&mut bytes)?;
    VertexId::from_typed_bytes(kind, &bytes)
        .map_err(|detail| std::io::Error::new(std::io::ErrorKind::InvalidData, detail))
}

pub(crate) fn write_entity_ref<W: std::io::Write>(
    writer: &mut W,
    entity_ref: &Option<EntityRef>,
) -> std::io::Result<()> {
    match entity_ref {
        None => writer.write_all(&[0u8]),
        Some(EntityRef::Vertex(vid)) => {
            writer.write_all(&[1u8])?;
            write_vertex_id(writer, vid)
        }
        Some(EntityRef::Edge {
            src,
            dst,
            edge_type,
            ranking,
        }) => {
            writer.write_all(&[2u8])?;
            write_vertex_id(writer, src)?;
            write_vertex_id(writer, dst)?;
            writer.write_all(&edge_type.to_le_bytes())?;
            writer.write_all(&ranking.to_le_bytes())
        }
    }
}

pub(crate) struct EntityRefReader;

impl EntityRefReader {
    pub(crate) fn read<R: std::io::Read>(reader: &mut R) -> std::io::Result<Option<EntityRef>> {
        let mut tag = [0u8; 1];
        reader.read_exact(&mut tag)?;
        match tag[0] {
            0 => Ok(None),
            1 => {
                let vid = read_vertex_id(reader)?;
                Ok(Some(EntityRef::Vertex(vid)))
            }
            2 => {
                let src = read_vertex_id(reader)?;

                let dst = read_vertex_id(reader)?;

                let mut edge_type_bytes = [0u8; 4];
                reader.read_exact(&mut edge_type_bytes)?;
                let edge_type = u32::from_le_bytes(edge_type_bytes);

                let mut ranking_bytes = [0u8; 8];
                reader.read_exact(&mut ranking_bytes)?;
                let ranking = i64::from_le_bytes(ranking_bytes);

                Ok(Some(EntityRef::Edge {
                    src,
                    dst,
                    edge_type,
                    ranking,
                }))
            }
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Unknown EntityRef tag",
            )),
        }
    }
}
