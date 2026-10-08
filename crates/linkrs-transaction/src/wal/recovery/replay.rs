//! Replay dispatch skeleton, redo deserialization helpers, and per-domain
//! replay branches (graph data, schema, index/system)

use postcard::from_bytes;
use serde::de::DeserializeOwned;

use crate::wal::{
    AddEdgePropRedo, AddVertexPropRedo, AlterSpaceCommentRedo, ClearSpaceRedo, CreateEdgeIndexRedo,
    CreateEdgeTypeRedo, CreateMacroRedo, CreateSpaceRedo, CreateTagIndexRedo, CreateTypeAliasRedo,
    CreateVertexTypeRedo, DeleteEdgePropRedo, DeleteEdgeRedo, DeleteEdgeTypeRedo,
    DeleteVertexPropRedo, DeleteVertexPropsRedo, DeleteVertexRedo, DeleteVertexTypeRedo,
    DropEdgeIndexRedo, DropMacroRedo, DropSpaceRedo, DropTagIndexRedo, DropTypeAliasRedo,
    InsertEdgeRedo, InsertVertexRedo, Lsn, ParsedWalEntry, RenameEdgePropRedo, RenameEdgeTypeRedo,
    RenameTagRedo, RenameVertexPropRedo, UpdateEdgePropRedo, UpdateSequenceRedo,
    UpdateVertexPropRedo, WalOpType,
};
use linkrs_core::types::Timestamp;
use linkrs_core::{StorageError, StorageResult};

use super::config::RecoveryStats;
use super::manager::RecoveryManager;
use super::RecoveryApplier;

macro_rules! recovery_arm_ref {
    ($applier:expr, $op:expr, $entry:expr, $payload:expr, $ts:expr, $stats:expr, $redo_type:ty, $replay_fn:ident) => {{
        match deserialize_redo::<$redo_type>($payload) {
            Ok(redo) => {
                $applier.$replay_fn(&redo, $ts)?;
                $stats.wal_entries_replayed += 1;
                $stats.last_lsn = $entry.lsn;
            }
            Err(e) => return Err(recovery_deserialize_error(&mut $stats, $entry.lsn, $op, e)),
        }
    }};
}

fn recovery_deserialize_error(
    stats: &mut RecoveryStats,
    lsn: Lsn,
    op_type: WalOpType,
    error: StorageError,
) -> StorageError {
    stats.errors_encountered += 1;
    StorageError::wal_error(format!(
        "Failed to deserialize {} at {}: {}",
        op_type, lsn, error
    ))
}

fn deserialize_redo<T: DeserializeOwned>(payload: &[u8]) -> StorageResult<T> {
    from_bytes(payload).map_err(|e| StorageError::deserialize_error(e.to_string()))
}

impl RecoveryManager {
    pub(super) fn replay_parsed_entries(
        &mut self,
        entries: &[ParsedWalEntry],
        applier: &dyn RecoveryApplier,
    ) -> StorageResult<()> {
        for entry in entries {
            if let Some(start_lsn) = self.config.start_lsn {
                if entry.lsn <= start_lsn {
                    continue;
                }
            }

            let op_type = match WalOpType::try_from(entry.header.op_type) {
                Ok(t) => t,
                Err(error) => {
                    self.stats.errors_encountered += 1;
                    return Err(StorageError::wal_error(format!(
                        "Invalid WAL operation at {}: {}",
                        entry.lsn, error
                    )));
                }
            };

            let ts = entry.header.timestamp;
            self.stats.max_timestamp = self.stats.max_timestamp.max(ts);

            match op_type {
                WalOpType::InsertVertex
                | WalOpType::InsertEdge
                | WalOpType::UpdateVertexProp
                | WalOpType::UpdateEdgeProp
                | WalOpType::DeleteVertex
                | WalOpType::DeleteEdge
                | WalOpType::DeleteVertexProps => {
                    self.replay_graph_op(entry, op_type, ts, applier)?;
                }
                WalOpType::CreateVertexType
                | WalOpType::CreateEdgeType
                | WalOpType::DeleteVertexType
                | WalOpType::DeleteEdgeType
                | WalOpType::CreateSpace
                | WalOpType::DropSpace
                | WalOpType::ClearSpace
                | WalOpType::AlterSpaceComment
                | WalOpType::AddVertexProp
                | WalOpType::AddEdgeProp
                | WalOpType::DeleteVertexProp
                | WalOpType::DeleteEdgeProp
                | WalOpType::RenameVertexProp
                | WalOpType::RenameEdgeProp
                | WalOpType::RenameTag
                | WalOpType::RenameEdgeType
                | WalOpType::CreateMacro
                | WalOpType::DropMacro
                | WalOpType::CreateTypeAlias
                | WalOpType::DropTypeAlias => {
                    self.replay_schema_op(entry, op_type, ts, applier)?;
                }
                WalOpType::CreateTagIndex
                | WalOpType::DropTagIndex
                | WalOpType::CreateEdgeIndex
                | WalOpType::DropEdgeIndex
                | WalOpType::Compact
                | WalOpType::UpdateSequence => {
                    self.replay_index_system_op(entry, op_type, ts, applier)?;
                }
                WalOpType::OutboxIntent
                | WalOpType::TransactionCommit
                | WalOpType::TransactionAbort => {}
            }
        }

        Ok(())
    }

    fn replay_graph_op(
        &mut self,
        entry: &ParsedWalEntry,
        op_type: WalOpType,
        ts: Timestamp,
        applier: &dyn RecoveryApplier,
    ) -> StorageResult<()> {
        let payload = &entry.payload;
        match op_type {
            WalOpType::InsertVertex => {
                let redo: InsertVertexRedo = deserialize_redo(payload)?;
                applier.replay_insert_vertex(redo.label, redo.vid, &redo.properties, ts)?;
                self.stats.wal_entries_replayed += 1;
                self.stats.last_lsn = entry.lsn;
            }
            WalOpType::InsertEdge => {
                recovery_arm_ref!(
                    applier,
                    op_type,
                    entry,
                    payload,
                    ts,
                    self.stats,
                    InsertEdgeRedo,
                    replay_insert_edge
                )
            }
            WalOpType::UpdateVertexProp => {
                let redo: UpdateVertexPropRedo = deserialize_redo(payload)?;
                applier.replay_update_vertex_prop(
                    redo.label,
                    redo.vid,
                    &redo.prop_name,
                    &redo.value,
                    ts,
                )?;
                self.stats.wal_entries_replayed += 1;
                self.stats.last_lsn = entry.lsn;
            }
            WalOpType::UpdateEdgeProp => {
                recovery_arm_ref!(
                    applier,
                    op_type,
                    entry,
                    payload,
                    ts,
                    self.stats,
                    UpdateEdgePropRedo,
                    replay_update_edge_prop
                )
            }
            WalOpType::DeleteVertex => {
                let redo: DeleteVertexRedo = deserialize_redo(payload)?;
                applier.replay_delete_vertex(redo.label, redo.vid, ts)?;
                self.stats.wal_entries_replayed += 1;
                self.stats.last_lsn = entry.lsn;
            }
            WalOpType::DeleteEdge => {
                recovery_arm_ref!(
                    applier,
                    op_type,
                    entry,
                    payload,
                    ts,
                    self.stats,
                    DeleteEdgeRedo,
                    replay_delete_edge
                )
            }
            WalOpType::DeleteVertexProps => {
                recovery_arm_ref!(
                    applier,
                    op_type,
                    entry,
                    payload,
                    ts,
                    self.stats,
                    DeleteVertexPropsRedo,
                    replay_delete_vertex_props
                )
            }
            other => unreachable!("graph data ops are routed here only: {other}"),
        }
        Ok(())
    }

    fn replay_schema_op(
        &mut self,
        entry: &ParsedWalEntry,
        op_type: WalOpType,
        ts: Timestamp,
        applier: &dyn RecoveryApplier,
    ) -> StorageResult<()> {
        let payload = &entry.payload;
        match op_type {
            WalOpType::CreateVertexType => {
                recovery_arm_ref!(
                    applier,
                    op_type,
                    entry,
                    payload,
                    ts,
                    self.stats,
                    CreateVertexTypeRedo,
                    replay_create_vertex_type
                )
            }
            WalOpType::CreateEdgeType => {
                recovery_arm_ref!(
                    applier,
                    op_type,
                    entry,
                    payload,
                    ts,
                    self.stats,
                    CreateEdgeTypeRedo,
                    replay_create_edge_type
                )
            }
            WalOpType::DeleteVertexType => {
                recovery_arm_ref!(
                    applier,
                    op_type,
                    entry,
                    payload,
                    ts,
                    self.stats,
                    DeleteVertexTypeRedo,
                    replay_delete_vertex_type
                )
            }
            WalOpType::DeleteEdgeType => {
                recovery_arm_ref!(
                    applier,
                    op_type,
                    entry,
                    payload,
                    ts,
                    self.stats,
                    DeleteEdgeTypeRedo,
                    replay_delete_edge_type
                )
            }
            WalOpType::CreateSpace => {
                recovery_arm_ref!(
                    applier,
                    op_type,
                    entry,
                    payload,
                    ts,
                    self.stats,
                    CreateSpaceRedo,
                    replay_create_space
                )
            }
            WalOpType::DropSpace => {
                recovery_arm_ref!(
                    applier,
                    op_type,
                    entry,
                    payload,
                    ts,
                    self.stats,
                    DropSpaceRedo,
                    replay_drop_space
                )
            }
            WalOpType::ClearSpace => {
                recovery_arm_ref!(
                    applier,
                    op_type,
                    entry,
                    payload,
                    ts,
                    self.stats,
                    ClearSpaceRedo,
                    replay_clear_space
                )
            }
            WalOpType::AlterSpaceComment => {
                recovery_arm_ref!(
                    applier,
                    op_type,
                    entry,
                    payload,
                    ts,
                    self.stats,
                    AlterSpaceCommentRedo,
                    replay_alter_space_comment
                )
            }
            WalOpType::AddVertexProp => {
                recovery_arm_ref!(
                    applier,
                    op_type,
                    entry,
                    payload,
                    ts,
                    self.stats,
                    AddVertexPropRedo,
                    replay_add_vertex_prop
                )
            }
            WalOpType::AddEdgeProp => {
                recovery_arm_ref!(
                    applier,
                    op_type,
                    entry,
                    payload,
                    ts,
                    self.stats,
                    AddEdgePropRedo,
                    replay_add_edge_prop
                )
            }
            WalOpType::DeleteVertexProp => {
                recovery_arm_ref!(
                    applier,
                    op_type,
                    entry,
                    payload,
                    ts,
                    self.stats,
                    DeleteVertexPropRedo,
                    replay_delete_vertex_prop
                )
            }
            WalOpType::DeleteEdgeProp => {
                recovery_arm_ref!(
                    applier,
                    op_type,
                    entry,
                    payload,
                    ts,
                    self.stats,
                    DeleteEdgePropRedo,
                    replay_delete_edge_prop
                )
            }
            WalOpType::RenameVertexProp => {
                recovery_arm_ref!(
                    applier,
                    op_type,
                    entry,
                    payload,
                    ts,
                    self.stats,
                    RenameVertexPropRedo,
                    replay_rename_vertex_prop
                )
            }
            WalOpType::RenameEdgeProp => {
                recovery_arm_ref!(
                    applier,
                    op_type,
                    entry,
                    payload,
                    ts,
                    self.stats,
                    RenameEdgePropRedo,
                    replay_rename_edge_prop
                )
            }
            WalOpType::RenameTag => {
                recovery_arm_ref!(
                    applier,
                    op_type,
                    entry,
                    payload,
                    ts,
                    self.stats,
                    RenameTagRedo,
                    replay_rename_tag
                )
            }
            WalOpType::RenameEdgeType => {
                recovery_arm_ref!(
                    applier,
                    op_type,
                    entry,
                    payload,
                    ts,
                    self.stats,
                    RenameEdgeTypeRedo,
                    replay_rename_edge_type
                )
            }
            WalOpType::CreateMacro => {
                recovery_arm_ref!(
                    applier,
                    op_type,
                    entry,
                    payload,
                    ts,
                    self.stats,
                    CreateMacroRedo,
                    replay_create_macro
                )
            }
            WalOpType::DropMacro => {
                recovery_arm_ref!(
                    applier,
                    op_type,
                    entry,
                    payload,
                    ts,
                    self.stats,
                    DropMacroRedo,
                    replay_drop_macro
                )
            }
            WalOpType::CreateTypeAlias => {
                recovery_arm_ref!(
                    applier,
                    op_type,
                    entry,
                    payload,
                    ts,
                    self.stats,
                    CreateTypeAliasRedo,
                    replay_create_type_alias
                )
            }
            WalOpType::DropTypeAlias => {
                recovery_arm_ref!(
                    applier,
                    op_type,
                    entry,
                    payload,
                    ts,
                    self.stats,
                    DropTypeAliasRedo,
                    replay_drop_type_alias
                )
            }
            other => unreachable!("schema ops are routed here only: {other}"),
        }
        Ok(())
    }

    fn replay_index_system_op(
        &mut self,
        entry: &ParsedWalEntry,
        op_type: WalOpType,
        ts: Timestamp,
        applier: &dyn RecoveryApplier,
    ) -> StorageResult<()> {
        let payload = &entry.payload;
        match op_type {
            WalOpType::CreateTagIndex => {
                recovery_arm_ref!(
                    applier,
                    op_type,
                    entry,
                    payload,
                    ts,
                    self.stats,
                    CreateTagIndexRedo,
                    replay_create_tag_index
                )
            }
            WalOpType::DropTagIndex => {
                recovery_arm_ref!(
                    applier,
                    op_type,
                    entry,
                    payload,
                    ts,
                    self.stats,
                    DropTagIndexRedo,
                    replay_drop_tag_index
                )
            }
            WalOpType::CreateEdgeIndex => {
                recovery_arm_ref!(
                    applier,
                    op_type,
                    entry,
                    payload,
                    ts,
                    self.stats,
                    CreateEdgeIndexRedo,
                    replay_create_edge_index
                )
            }
            WalOpType::DropEdgeIndex => {
                recovery_arm_ref!(
                    applier,
                    op_type,
                    entry,
                    payload,
                    ts,
                    self.stats,
                    DropEdgeIndexRedo,
                    replay_drop_edge_index
                )
            }
            WalOpType::Compact => {
                applier.replay_compact(ts)?;
                self.stats.wal_entries_replayed += 1;
            }
            WalOpType::UpdateSequence => {
                recovery_arm_ref!(
                    applier,
                    op_type,
                    entry,
                    payload,
                    ts,
                    self.stats,
                    UpdateSequenceRedo,
                    replay_update_sequence
                )
            }
            other => unreachable!("index/system ops are routed here only: {other}"),
        }
        Ok(())
    }
}
