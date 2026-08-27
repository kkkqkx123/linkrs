use graphdb_core::types::VertexId;
use graphdb_core::types::{PropertyDef, SpaceInfo, TagInfo};
use graphdb_core::vertex_edge_path::{Tag, Vertex};
use graphdb_core::DataType;
use graphdb_core::Value;
use graphdb_storage::{
    GraphStorage, PropertyGraphConfig, StorageReader, StorageSchemaOps, StorageWriter,
};
use std::collections::HashMap;

#[test]
fn debug_double_property() {
    let mut storage = GraphStorage::new_with_config(PropertyGraphConfig::test()).unwrap();
    storage
        .create_space(&mut SpaceInfo::new("s".to_string()))
        .unwrap();
    let tag = TagInfo::new("T".to_string()).with_properties(vec![
        PropertyDef::new("d".to_string(), DataType::Double),
        PropertyDef::new("f".to_string(), DataType::Float),
    ]);
    storage.create_tag("s", &tag).unwrap();
    let v = Vertex::new_with_properties(
        VertexId::from_int64(1),
        vec![Tag::new(
            "T".to_string(),
            HashMap::from([
                ("d".to_string(), Value::Double(std::f64::consts::E)),
                ("f".to_string(), Value::Float(std::f32::consts::PI)),
            ]),
        )],
        HashMap::new(),
    );
    storage.insert_vertex("s", v).unwrap();
    let read = storage
        .get_vertex("s", &VertexId::from_int64(1))
        .unwrap()
        .unwrap();
    for t in read.tags() {
        println!("DBG props={:?}", t.properties);
    }
}
