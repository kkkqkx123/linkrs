//! Binary serialization helpers for `EntityRef` values.

use graphdb_core::types::storage_ids::VertexId;
use graphdb_core::wal::EntityRef;

pub(crate) fn write_entity_ref<W: std::io::Write>(
    writer: &mut W,
    entity_ref: &Option<EntityRef>,
) -> std::io::Result<()> {
    match entity_ref {
        None => writer.write_all(&[0u8]),
        Some(EntityRef::Vertex(vid)) => {
            writer.write_all(&[1u8])?;
            let bytes = vid.as_bytes();
            let len = bytes.len().min(u8::MAX as usize) as u8;
            writer.write_all(&[len])?;
            writer.write_all(&bytes[..len as usize])
        }
        Some(EntityRef::Edge {
            src,
            dst,
            edge_type,
            ranking,
        }) => {
            writer.write_all(&[2u8])?;
            let src_bytes = src.as_bytes();
            let src_len = src_bytes.len().min(u8::MAX as usize) as u8;
            writer.write_all(&[src_len])?;
            writer.write_all(&src_bytes[..src_len as usize])?;
            let dst_bytes = dst.as_bytes();
            let dst_len = dst_bytes.len().min(u8::MAX as usize) as u8;
            writer.write_all(&[dst_len])?;
            writer.write_all(&dst_bytes[..dst_len as usize])?;
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
                let mut len = [0u8; 1];
                reader.read_exact(&mut len)?;
                let mut bytes = vec![0u8; len[0] as usize];
                reader.read_exact(&mut bytes)?;
                let vid = VertexId::from_bytes(bytes);
                Ok(Some(EntityRef::Vertex(vid)))
            }
            2 => {
                let mut len = [0u8; 1];
                reader.read_exact(&mut len)?;
                let mut src_bytes = vec![0u8; len[0] as usize];
                reader.read_exact(&mut src_bytes)?;
                let src = VertexId::from_bytes(src_bytes);

                reader.read_exact(&mut len)?;
                let mut dst_bytes = vec![0u8; len[0] as usize];
                reader.read_exact(&mut dst_bytes)?;
                let dst = VertexId::from_bytes(dst_bytes);

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
