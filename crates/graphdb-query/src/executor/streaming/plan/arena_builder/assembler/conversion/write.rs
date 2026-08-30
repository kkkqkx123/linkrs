use std::sync::Arc;

use super::super::super::super::super::operators::spec::{
    ApplySpec, BlockingSpec, DdlSpec, JoinSpec, SetSpec, SourceSpec, TxnSpec,
};
use super::super::super::super::super::subquery::SubqueryRunnerSpec;
use super::super::super::super::properties::{PhysicalProperties, SPILL_DEFAULT_THRESHOLD};
use super::super::super::super::types::{
    FragmentId, FragmentKind, FragmentSpec, InputContract, LogicalNodeId, OperatorKindSpec,
    PhysicalOperatorId, PhysicalOperatorIdAllocator, PhysicalOperatorSpec, StateOwnership,
};
use super::super::super::metadata::{
    estimate_source_cardinality, source_explain_name, source_output_layout,
};
use super::super::super::specs::*;
use super::super::fragment_ops::FragmentCtx;
use super::super::{ArenaFragmentAllocator, ArenaPlanAssembler};
use crate::executor::base::ExecutionContext;
use crate::executor::build_error::PlanBuildError;
use crate::executor::streaming::plan::PhysicalPlanBuildContext;
use crate::executor::streaming::plan::PhysicalPlanBuilder;
use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::planning::plan::core::nodes::base::plan_node_traits::{
    MultipleInputNode, SingleInputNode,
};

impl ArenaPlanAssembler {
    pub(crate) fn convert_write_node(
        node: &PlanNodeEnum,
        operators: &mut Vec<PhysicalOperatorSpec>,
        fragments: &mut Vec<FragmentSpec>,
        op_alloc: &mut PhysicalOperatorIdAllocator,
        frag_alloc: &mut ArenaFragmentAllocator,
        exec_ctx: &ExecutionContext,
    ) -> Result<(FragmentId, PhysicalOperatorId), PlanBuildError> {
        match node {
            PlanNodeEnum::InsertVertices(iv_node) => {
                let (child_fid, _) = Self::push_source_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    node.id(),
                    build_standalone_write_source(node)?,
                );
                let spec = build_insert_vertices_spec(iv_node, exec_ctx)?;
                Self::push_sink_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }
            PlanNodeEnum::InsertEdges(ie_node) => {
                let (child_fid, _) = Self::push_source_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    node.id(),
                    build_standalone_write_source(node)?,
                );
                let spec = build_insert_edges_spec(ie_node, exec_ctx)?;
                Self::push_sink_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }
            PlanNodeEnum::DeleteVertices(dv_node) => {
                let (child_fid, _) = Self::push_source_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    node.id(),
                    build_standalone_write_source(node)?,
                );
                let spec = build_delete_vertices_spec(dv_node, exec_ctx)?;
                Self::push_sink_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }
            PlanNodeEnum::DeleteEdges(de_node) => {
                let (child_fid, _) = Self::push_source_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    node.id(),
                    build_standalone_write_source(node)?,
                );
                let spec = build_delete_edges_spec(de_node, exec_ctx)?;
                Self::push_sink_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }
            PlanNodeEnum::DeleteTags(dt_node) => {
                let (child_fid, _) = Self::push_source_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    node.id(),
                    build_standalone_write_source(node)?,
                );
                let spec = build_delete_tags_spec(dt_node, exec_ctx)?;
                Self::push_sink_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }
            PlanNodeEnum::PipeDeleteVertices(pdv_node) => {
                let (child_fid, _) = Self::convert_node(
                    pdv_node.input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let spec = build_pipe_delete_vertices_spec(pdv_node, exec_ctx)?;
                Self::push_sink_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }
            PlanNodeEnum::PipeDeleteEdges(pde_node) => {
                let (child_fid, _) = Self::convert_node(
                    pde_node.input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let spec = build_pipe_delete_edges_spec(pde_node, exec_ctx)?;
                Self::push_sink_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }
            PlanNodeEnum::Update(u_node) => {
                let (child_fid, _) = Self::push_source_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    node.id(),
                    build_standalone_write_source(node)?,
                );
                let spec = build_update_spec(u_node, exec_ctx)?;
                Self::push_sink_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }
            PlanNodeEnum::UpdateVertices(uv_node) => {
                let (child_fid, _) = Self::push_source_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    node.id(),
                    build_standalone_write_source(node)?,
                );
                let spec = build_update_vertices_spec(uv_node, exec_ctx)?;
                Self::push_sink_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }
            PlanNodeEnum::UpdateEdges(ue_node) => {
                let (child_fid, _) = Self::push_source_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    node.id(),
                    build_standalone_write_source(node)?,
                );
                let spec = build_update_edges_spec(ue_node, exec_ctx)?;
                Self::push_sink_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }
            PlanNodeEnum::CopyFrom(copy_node) => {
                let (child_fid, _) = Self::push_source_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    node.id(),
                    build_standalone_write_source(node)?,
                );
                let spec = build_copy_from_spec(copy_node, exec_ctx)?;
                Self::push_sink_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }
            PlanNodeEnum::CopyTo(copy_node) => {
                let (child_fid, _) = Self::push_source_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    node.id(),
                    build_standalone_write_source(node)?,
                );
                let spec = build_copy_to_spec(copy_node, exec_ctx)?;
                Self::push_sink_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }

            // ── DDL nodes ───────────────────────────────────────────────────────
            PlanNodeEnum::SpaceManage(sm_node) => {
                let spec = build_space_manage_spec(sm_node, exec_ctx)?;
                Self::push_ddl_op(operators, fragments, op_alloc, frag_alloc, node.id(), spec)
            }
            PlanNodeEnum::TagManage(tm_node) => {
                let spec = build_tag_manage_spec(tm_node, exec_ctx)?;
                Self::push_ddl_op(operators, fragments, op_alloc, frag_alloc, node.id(), spec)
            }
            PlanNodeEnum::EdgeManage(em_node) => {
                let spec = build_edge_manage_spec(em_node, exec_ctx)?;
                Self::push_ddl_op(operators, fragments, op_alloc, frag_alloc, node.id(), spec)
            }
            PlanNodeEnum::IndexManage(im_node) => {
                let spec = build_index_manage_spec(im_node, exec_ctx)?;
                Self::push_ddl_op(operators, fragments, op_alloc, frag_alloc, node.id(), spec)
            }
            PlanNodeEnum::DeleteIndex(di_node) => {
                let spec = build_delete_index_spec(di_node, exec_ctx)?;
                Self::push_ddl_op(operators, fragments, op_alloc, frag_alloc, node.id(), spec)
            }
            PlanNodeEnum::UserManage(um_node) => {
                let spec = build_user_manage_spec(um_node, exec_ctx)?;
                Self::push_ddl_op(operators, fragments, op_alloc, frag_alloc, node.id(), spec)
            }
            PlanNodeEnum::ShowStats(_) => Self::push_ddl_op(
                operators,
                fragments,
                op_alloc,
                frag_alloc,
                node.id(),
                DdlSpec::ShowStats {
                    space_name: exec_ctx.space_name.clone().unwrap_or_default(),
                },
            ),
            PlanNodeEnum::ShowConfigs(_) => Self::push_ddl_op(
                operators,
                fragments,
                op_alloc,
                frag_alloc,
                node.id(),
                DdlSpec::ShowConfigs {
                    space_name: exec_ctx.space_name.clone().unwrap_or_default(),
                },
            ),
            PlanNodeEnum::ShowQueries(_) => Self::push_ddl_op(
                operators,
                fragments,
                op_alloc,
                frag_alloc,
                node.id(),
                DdlSpec::ShowQueries {
                    space_name: exec_ctx.space_name.clone().unwrap_or_default(),
                },
            ),
            PlanNodeEnum::ShowSessions(_) => Self::push_ddl_op(
                operators,
                fragments,
                op_alloc,
                frag_alloc,
                node.id(),
                DdlSpec::ShowSessions {
                    space_name: exec_ctx.space_name.clone().unwrap_or_default(),
                },
            ),

            // ── Fulltext nodes ──────────────────────────────────────────────────
            PlanNodeEnum::FulltextManage(fm_node) => {
                let (child_fid, _) = Self::push_source_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    node.id(),
                    SourceSpec::Start,
                );
                let spec = build_fulltext_manage_spec(fm_node, exec_ctx)?;
                Self::push_fulltext_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }
            PlanNodeEnum::FulltextSearch(fs_node) => {
                let (child_fid, _) = Self::push_source_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    node.id(),
                    SourceSpec::Start,
                );
                let spec = build_fulltext_search_spec(fs_node, exec_ctx)?;
                Self::push_fulltext_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }
            PlanNodeEnum::FulltextLookup(fl_node) => {
                let (child_fid, _) = Self::push_source_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    node.id(),
                    SourceSpec::Start,
                );
                let spec = build_fulltext_lookup_spec(fl_node, exec_ctx)?;
                Self::push_fulltext_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }
            PlanNodeEnum::MatchFulltext(mf_node) => {
                let (child_fid, _) = Self::push_source_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    node.id(),
                    SourceSpec::Start,
                );
                let spec = build_match_fulltext_spec(mf_node, exec_ctx)?;
                Self::push_fulltext_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }

            // ── Vector nodes ────────────────────────────────────────────────────
            PlanNodeEnum::VectorManage(vm_node) => {
                let (child_fid, _) = Self::push_source_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    node.id(),
                    SourceSpec::Start,
                );
                let spec = build_vector_manage_spec(vm_node, exec_ctx)?;
                Self::push_vector_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }
            #[cfg(feature = "vector")]
            PlanNodeEnum::VectorSearch(vs_node) => {
                let (child_fid, _) = Self::push_source_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    node.id(),
                    SourceSpec::Start,
                );
                let spec = build_vector_search_spec(vs_node, exec_ctx)?;
                Self::push_vector_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }
            #[cfg(feature = "vector")]
            PlanNodeEnum::VectorLookup(vl_node) => {
                let (child_fid, _) = Self::push_source_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    node.id(),
                    SourceSpec::Start,
                );
                let spec = build_vector_lookup_spec(vl_node, exec_ctx)?;
                Self::push_vector_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }
            #[cfg(feature = "vector")]
            PlanNodeEnum::VectorMatch(vm_node) => {
                let (child_fid, _) = Self::push_source_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    node.id(),
                    SourceSpec::Start,
                );
                let spec = build_vector_match_spec(vm_node, exec_ctx)?;
                Self::push_vector_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }

            // ── Transaction nodes ───────────────────────────────────────────────
            PlanNodeEnum::BeginTransaction(_) => Self::push_txn_op(
                operators,
                fragments,
                op_alloc,
                frag_alloc,
                node.id(),
                TxnSpec::BeginTransaction,
            ),
            PlanNodeEnum::Commit(_) => Self::push_txn_op(
                operators,
                fragments,
                op_alloc,
                frag_alloc,
                node.id(),
                TxnSpec::Commit,
            ),
            PlanNodeEnum::Rollback(rollback_node) => {
                let spec = match rollback_node.savepoint() {
                    Some(name) => TxnSpec::RollbackToSavepoint {
                        name: name.to_string(),
                    },
                    None => TxnSpec::Rollback,
                };
                Self::push_txn_op(operators, fragments, op_alloc, frag_alloc, node.id(), spec)
            }
            PlanNodeEnum::Savepoint(savepoint_node) => Self::push_txn_op(
                operators,
                fragments,
                op_alloc,
                frag_alloc,
                node.id(),
                TxnSpec::Savepoint {
                    name: savepoint_node.savepoint().to_string(),
                },
            ),
            PlanNodeEnum::ReleaseSavepoint(release_node) => Self::push_txn_op(
                operators,
                fragments,
                op_alloc,
                frag_alloc,
                node.id(),
                TxnSpec::ReleaseSavepoint {
                    name: release_node.savepoint().to_string(),
                },
            ),

            // ── Unsupported ─────────────────────────────────────────────────────
            PlanNodeEnum::Loop(_) => Err(PlanBuildError::unsupported(
                node.name(),
                node.id(),
                "Loop plan nodes are not supported",
            )),
            PlanNodeEnum::PassThrough(_) => Err(PlanBuildError::unsupported(
                node.name(),
                node.id(),
                "PassThrough plan nodes are not supported",
            )),
            PlanNodeEnum::Select(_) => Err(PlanBuildError::unsupported(
                node.name(),
                node.id(),
                "Select plan nodes are not supported",
            )),
            PlanNodeEnum::AppendVertices(append_node) => {
                let (child_fid, _) = Self::convert_node(
                    append_node.inputs().first().ok_or_else(|| {
                        PlanBuildError::missing_value(
                            "AppendVertices",
                            node.id(),
                            "input",
                            "AppendVertices requires an input",
                        )
                    })?,
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let spec = build_append_vertices_spec(append_node, exec_ctx)?;
                Self::push_unary_op(operators, fragments, op_alloc, child_fid, node.id(), spec)
            }
            _ => unreachable!("convert_write_node called with non-write node"),
        }
    }
}
