use crate::core::types::{EdgeId, Timestamp};
use crate::core::{StorageError, StorageResult, Value};
use crate::storage::edge::edge_table::core::EdgeTableConfig;
use crate::storage::edge::{
    CsrBase, CsrVariant, EdgeRecord, EdgeSchema, MutableCsrTrait, Nbr,
};

pub struct SimpleEdgeStore {
    pub label: crate::core::types::LabelId,
    pub label_name: String,
    pub src_label: crate::core::types::LabelId,
    pub dst_label: crate::core::types::LabelId,
    pub schema: EdgeSchema,
    pub out_csr: CsrVariant,
    pub in_csr: CsrVariant,
    properties: SimplePropertyTable,
    pub is_open: bool,
    pub next_edge_id: EdgeId,
    pub config: EdgeTableConfig,
    pub stats_manager: Option<std::sync::Arc<crate::core::stats::StatsManager>>,
}

impl std::fmt::Debug for SimpleEdgeStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SimpleEdgeStore")
            .field("label", &self.label)
            .field("label_name", &self.label_name)
            .field("out_csr", &self.out_csr)
            .field("in_csr", &self.in_csr)
            .field("is_open", &self.is_open)
            .field("next_edge_id", &self.next_edge_id)
            .finish()
    }
}

impl SimpleEdgeStore {
    pub fn new(schema: EdgeSchema) -> StorageResult<Self> {
        Self::with_config(schema, EdgeTableConfig::default())
    }

    pub fn with_config(schema: EdgeSchema, config: EdgeTableConfig) -> StorageResult<Self> {
        let label = schema.label_id;
        let label_name = schema.label_name.clone();
        let src_label = schema.src_label;
        let dst_label = schema.dst_label;

        let out_csr = CsrVariant::from_strategy_with_overflow(
            schema.oe_strategy,
            config.initial_vertex_capacity,
            config.initial_edge_capacity,
            config.overflow_chunk_edges,
        )?;
        let in_csr = CsrVariant::from_strategy_with_overflow(
            schema.ie_strategy,
            config.initial_vertex_capacity,
            config.initial_edge_capacity,
            config.overflow_chunk_edges,
        )?;

        let properties = SimplePropertyTable::with_schema(&schema);

        Ok(Self {
            label,
            label_name,
            src_label,
            dst_label,
            schema,
            out_csr,
            in_csr,
            properties,
            is_open: true,
            next_edge_id: EdgeId::new(0),
            config,
            stats_manager: None,
        })
    }

    pub fn label(&self) -> crate::core::types::LabelId {
        self.label
    }

    pub fn src_label(&self) -> crate::core::types::LabelId {
        self.src_label
    }

    pub fn dst_label(&self) -> crate::core::types::LabelId {
        self.dst_label
    }

    pub fn schema(&self) -> &EdgeSchema {
        &self.schema
    }

    pub fn schema_mut(&mut self) -> &mut EdgeSchema {
        &mut self.schema
    }

    pub fn set_stats_manager(&mut self, mgr: std::sync::Arc<crate::core::stats::StatsManager>) {
        self.stats_manager = Some(mgr);
    }

    pub fn insert_edge(
        &mut self,
        src: u32,
        dst: u32,
        rank: i64,
        property_values: &[(String, Value)],
    ) -> StorageResult<()> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }

        let mut converted_values = Vec::with_capacity(property_values.len());
        for (name, value) in property_values {
            let prop_idx = self
                .schema
                .properties
                .iter()
                .position(|p| p.name == *name)
                .ok_or_else(|| StorageError::column_not_found(name.clone()))?;
            let prop_def = &self.schema.properties[prop_idx];
            if value.data_type() != prop_def.data_type {
                let converted = value.try_cast_to(&prop_def.data_type)?;
                converted_values.push((name.clone(), converted));
            } else {
                converted_values.push((name.clone(), value.clone()));
            }
        }

        let prop_offset = if !converted_values.is_empty() {
            self.properties.insert(&converted_values)?
        } else {
            0
        };

        let dst_key = encode_edge_endpoint(dst, rank);
        let src_key = encode_edge_endpoint(src, rank);

        let edge_id = self.next_edge_id.fetch_add();

        if let Err(e) = self
            .out_csr
            .insert_edge(src, dst_key, edge_id, prop_offset, 0)
        {
            if prop_offset > 0 {
                self.properties.delete(prop_offset);
            }
            return Err(e);
        }

        if let Err(e) = self
            .in_csr
            .insert_edge(dst, src_key, edge_id, prop_offset, 0)
        {
            let _ = self.out_csr.delete_edge(src, edge_id, 0);
            if prop_offset > 0 {
                self.properties.delete(prop_offset);
            }
            return Err(e);
        }

        Ok(())
    }

    pub fn delete_edge(&mut self, src: u32, dst: u32, rank: i64) -> StorageResult<bool> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }

        let dst_key = encode_edge_endpoint(dst, rank);

        if let Some(nbr) = self.out_csr.get_edge(src, dst_key, Timestamp::MAX) {
            let edge_id = nbr.edge_id;
            self.out_csr.delete_edge(src, edge_id, 0);
            self.in_csr
                .delete_edge_by_dst(dst, src_key_for(src), 0);
            return Ok(true);
        }

        Ok(false)
    }

    pub fn get_edge(&self, src: u32, dst: u32, rank: i64) -> Option<EdgeRecord> {
        let dst_key = encode_edge_endpoint(dst, rank);
        let nbr = self
            .out_csr
            .get_edge(src, dst_key, Timestamp::MAX)?;
        Some(nbr_to_record(src, &nbr, &self.properties))
    }

    pub fn out_edges(&self, src: u32) -> Vec<EdgeRecord> {
        self.out_csr
            .iter(Timestamp::MAX)
            .filter(|(s, _)| s.as_int64().unwrap_or(0) as u32 == src)
            .map(|(s, nbr)| {
                let src_u32 = s.as_int64().unwrap_or(0) as u32;
                nbr_to_record(src_u32, &nbr, &self.properties)
            })
            .collect()
    }

    pub fn in_edges(&self, dst: u32) -> Vec<EdgeRecord> {
        self.in_csr
            .iter(Timestamp::MAX)
            .filter(|(d, _)| d.as_int64().unwrap_or(0) as u32 == dst)
            .map(|(_, nbr)| {
                let (src_id, _) = decode_edge_endpoint(nbr.neighbor);
                let src_u32 = src_id.as_int64().unwrap_or(0) as u32;
                nbr_to_record(src_u32, &nbr, &self.properties)
            })
            .collect()
    }

    pub fn scan(&self) -> Vec<EdgeRecord> {
        self.out_csr
            .iter(Timestamp::MAX)
            .map(|(src, nbr)| {
                let src_u32 = src.as_int64().unwrap_or(0) as u32;
                nbr_to_record(src_u32, &nbr, &self.properties)
            })
            .collect()
    }

    pub fn edge_count(&self) -> u64 {
        self.out_csr.edge_count()
    }

    pub fn add_property(
        &mut self,
        name: String,
        data_type: crate::core::DataType,
        nullable: bool,
    ) -> StorageResult<()> {
        use crate::storage::types::StoragePropertyDef;
        if self.schema.properties.iter().any(|p| p.name == name) {
            return Err(StorageError::column_already_exists(name));
        }
        let prop_def = StoragePropertyDef::new(name, data_type);
        self.schema.properties.push(prop_def);
        Ok(())
    }

    pub fn remove_property(&mut self, name: &str) -> StorageResult<()> {
        let index = self
            .schema
            .properties
            .iter()
            .position(|p| p.name == name)
            .ok_or_else(|| StorageError::column_not_found(name.to_string()))?;
        self.schema.properties.remove(index);
        Ok(())
    }

    pub fn rename_property(&mut self, old_name: &str, new_name: &str) -> StorageResult<()> {
        let prop = self
            .schema
            .properties
            .iter_mut()
            .find(|p| p.name == old_name)
            .ok_or_else(|| StorageError::column_not_found(old_name.to_string()))?;
        prop.name = new_name.to_string();
        Ok(())
    }

    pub fn update_edge_property(
        &mut self,
        src: u32,
        dst: u32,
        rank: i64,
        prop_name: &str,
        value: &Value,
    ) -> StorageResult<bool> {
        let dst_key = encode_edge_endpoint(dst, rank);
        let nbr = self
            .out_csr
            .get_edge(src, dst_key, Timestamp::MAX)
            .ok_or_else(|| StorageError::invalid_operation(format!("edge not found: {} -> {}@{}", src, dst, rank)))?;
        self.properties
            .update(nbr.prop_offset, prop_name, value.clone())
    }

    pub fn delete_edge_by_offset(
        &mut self,
        src: u32,
        dst: u32,
        rank: i64,
        oe_offset: i32,
        ie_offset: i32,
    ) -> StorageResult<bool> {
        let dst_key = encode_edge_endpoint(dst, rank);
        let nbr = self
            .out_csr
            .get_edge(src, dst_key, Timestamp::MAX)
            .ok_or_else(|| StorageError::invalid_operation(format!("edge not found: {} -> {}@{}", src, dst, rank)))?;

        self.out_csr.delete_edge_by_offset(src, oe_offset, 0);
        self.in_csr
            .delete_edge_by_offset(dst, ie_offset, 0);

        if nbr.prop_offset > 0 {
            self.properties.delete(nbr.prop_offset);
        }

        Ok(true)
    }

    pub fn revert_delete_edge_by_offset(
        &mut self,
        src: u32,
        dst: u32,
        rank: i64,
        _oe_offset: i32,
        _ie_offset: i32,
        _ts: Timestamp,
    ) -> StorageResult<bool> {
        self.insert_edge(src, dst, rank, &[]).map(|_| true)
    }

    pub fn memory_size(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.out_csr.used_memory_size()
            + self.in_csr.used_memory_size()
            + self.properties.memory_size()
            + self.schema.properties.iter().map(|p| p.name.len()).sum::<usize>()
    }

    pub fn rebuild_schema_change_from_redo(
        &mut self,
        _details: crate::storage::schema::ChangeDetails,
    ) -> StorageResult<()> {
        Ok(())
    }
}

fn nbr_to_record(src: u32, nbr: &Nbr, properties: &SimplePropertyTable) -> EdgeRecord {
    let (dst_vid, rank) = decode_edge_endpoint(nbr.neighbor);
    let prop_values = properties.get(nbr.prop_offset).unwrap_or_default();
    EdgeRecord {
        src_vid: crate::core::types::VertexId::from_int64(src as i64),
        dst_vid,
        rank,
        properties: prop_values,
    }
}

fn encode_edge_endpoint(endpoint: u32, rank: i64) -> crate::core::types::VertexId {
    let mut data = Vec::with_capacity(16);
    data.extend_from_slice(&(endpoint as i64).to_be_bytes());
    data.extend_from_slice(&rank.to_be_bytes());
    crate::core::types::VertexId::from_bytes(data)
}

fn src_key_for(src: u32) -> crate::core::types::VertexId {
    encode_edge_endpoint(src, 0)
}

fn decode_edge_endpoint(key: crate::core::types::VertexId) -> (crate::core::types::VertexId, i64) {
    let bytes = key.as_bytes();
    if bytes.len() != 16 {
        log::warn!(
            "decode_edge_endpoint: unexpected key length {}, expected 16",
            bytes.len()
        );
    }
    let mut buf = [0u8; 16];
    let copy_len = bytes.len().min(16);
    buf[..copy_len].copy_from_slice(&bytes[..copy_len]);
    let mut endpoint_bytes = [0u8; 8];
    endpoint_bytes.copy_from_slice(&buf[..8]);
    let mut rank_bytes = [0u8; 8];
    rank_bytes.copy_from_slice(&buf[8..16]);
    (
        crate::core::types::VertexId::from_int64(i64::from_be_bytes(endpoint_bytes)),
        i64::from_be_bytes(rank_bytes),
    )
}

struct SimplePropertyTable {
    records: Vec<Option<Vec<u8>>>,
    free_list: Vec<u32>,
}

impl SimplePropertyTable {
    fn with_schema(schema: &EdgeSchema) -> Self {
        let _ = schema;
        Self {
            records: Vec::new(),
            free_list: Vec::new(),
        }
    }

    fn insert(&mut self, values: &[(String, Value)]) -> StorageResult<u32> {
        let serialized = serialize_values(values);

        let offset = if let Some(free_idx) = self.free_list.pop() {
            let row_idx = free_idx as usize;
            self.records[row_idx] = Some(serialized);
            free_idx + 1
        } else {
            let row_idx = self.records.len();
            self.records.push(Some(serialized));
            (row_idx + 1) as u32
        };

        Ok(offset)
    }

    fn get(&self, offset: u32) -> Option<Vec<(String, Value)>> {
        if offset == 0 {
            return None;
        }
        let row_idx = (offset - 1) as usize;
        let data = self.records.get(row_idx)?.as_ref()?;
        Some(deserialize_values(data))
    }

    fn update(&mut self, offset: u32, prop_name: &str, value: Value) -> StorageResult<bool> {
        if offset == 0 {
            return Ok(false);
        }
        let row_idx = (offset - 1) as usize;
        let record = self.records.get_mut(row_idx).ok_or_else(|| {
            StorageError::invalid_offset(offset)
        })?;
        let data = record.as_mut().ok_or_else(|| {
            StorageError::invalid_offset(offset)
        })?;

        let mut values = deserialize_values(data);
        if let Some(pos) = values.iter().position(|(n, _)| n == prop_name) {
            values[pos].1 = value;
        } else {
            values.push((prop_name.to_string(), value));
        }
        *data = serialize_values(&values);
        Ok(true)
    }

    fn delete(&mut self, offset: u32) -> bool {
        if offset == 0 {
            return false;
        }
        let row_idx = (offset - 1) as usize;
        if row_idx < self.records.len() && self.records[row_idx].is_some() {
            self.records[row_idx] = None;
            self.free_list.push(offset - 1);
            true
        } else {
            false
        }
    }

    fn memory_size(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.records.capacity() * std::mem::size_of::<Option<Vec<u8>>>()
            + self
                .records
                .iter()
                .flatten()
                .map(|d| d.capacity())
                .sum::<usize>()
            + self.free_list.capacity() * std::mem::size_of::<u32>()
    }
}

fn serialize_values(values: &[(String, Value)]) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend((values.len() as u32).to_le_bytes());
    for (name, value) in values {
        let name_bytes = name.as_bytes();
        buf.extend((name_bytes.len() as u32).to_le_bytes());
        buf.extend(name_bytes);
        let value_bytes = value_to_bytes(value);
        buf.extend((value_bytes.len() as u32).to_le_bytes());
        buf.extend(&value_bytes);
    }
    buf
}

fn deserialize_values(data: &[u8]) -> Vec<(String, Value)> {
    if data.len() < 4 {
        return Vec::new();
    }
    let count = u32::from_le_bytes(data[0..4].try_into().unwrap()) as usize;
    let mut offset = 4;
    let mut result = Vec::with_capacity(count);
    for _ in 0..count {
        if offset + 4 > data.len() {
            break;
        }
        let name_len = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;
        if offset + name_len > data.len() {
            break;
        }
        let name = String::from_utf8(data[offset..offset + name_len].to_vec()).unwrap_or_default();
        offset += name_len;
        if offset + 4 > data.len() {
            break;
        }
        let val_len = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;
        if offset + val_len > data.len() {
            break;
        }
        let val = bytes_to_value(&data[offset..offset + val_len]).unwrap_or_else(|| Value::Null(Default::default()));
        offset += val_len;
        result.push((name, val));
    }
    result
}

fn value_to_bytes(value: &Value) -> Vec<u8> {
    match value {
        Value::Null(_) => vec![0],
        Value::Bool(b) => {
            let mut buf = vec![1];
            buf.push(if *b { 1 } else { 0 });
            buf
        }
        Value::SmallInt(i) => {
            let mut buf = vec![2];
            buf.extend(i.to_le_bytes());
            buf
        }
        Value::Int(i) => {
            let mut buf = vec![3];
            buf.extend(i.to_le_bytes());
            buf
        }
        Value::BigInt(i) => {
            let mut buf = vec![4];
            buf.extend(i.to_le_bytes());
            buf
        }
        Value::Float(f) => {
            let mut buf = vec![5];
            buf.extend(f.to_le_bytes());
            buf
        }
        Value::Double(f) => {
            let mut buf = vec![6];
            buf.extend(f.to_le_bytes());
            buf
        }
        Value::String(s) => {
            let mut buf = vec![7];
            buf.extend((s.len() as u32).to_le_bytes());
            buf.extend(s.as_bytes());
            buf
        }
        _ => {
            let s = format!("{:?}", value);
            let mut buf = vec![8];
            buf.extend((s.len() as u32).to_le_bytes());
            buf.extend(s.as_bytes());
            buf
        }
    }
}

fn bytes_to_value(data: &[u8]) -> Option<Value> {
    if data.is_empty() {
        return None;
    }
    match data[0] {
        0 => Some(Value::Null(Default::default())),
        1 => Some(Value::Bool(data.len() > 1 && data[1] != 0)),
        2 => {
            if data.len() < 3 {
                return None;
            }
            Some(Value::SmallInt(i16::from_le_bytes(data[1..3].try_into().ok()?)))
        }
        3 => {
            if data.len() < 5 {
                return None;
            }
            Some(Value::Int(i32::from_le_bytes(data[1..5].try_into().ok()?)))
        }
        4 => {
            if data.len() < 9 {
                return None;
            }
            Some(Value::BigInt(i64::from_le_bytes(data[1..9].try_into().ok()?)))
        }
        5 => {
            if data.len() < 5 {
                return None;
            }
            Some(Value::Float(f32::from_le_bytes(data[1..5].try_into().ok()?)))
        }
        6 => {
            if data.len() < 9 {
                return None;
            }
            Some(Value::Double(f64::from_le_bytes(data[1..9].try_into().ok()?)))
        }
        7 => {
            if data.len() < 5 {
                return None;
            }
            let len = u32::from_le_bytes(data[1..5].try_into().ok()?) as usize;
            if data.len() < 5 + len {
                return None;
            }
            Some(Value::String(
                String::from_utf8(data[5..5 + len].to_vec()).ok()?.into(),
            ))
        }
        _ => None,
    }
}
