use crate::executor::base::ExecutionContext;
use crate::executor::build_error::PlanBuildError;
use crate::executor::streaming::operators::spec::{
    DdlSpec, UserManageCommand
};

fn user_manage_to_command(
    node: &crate::planning::plan::core::nodes::management::manage_node_enums::UserManageNode,
) -> UserManageCommand {
    use crate::executor::streaming::operators::spec::UserManageCommand;
    use crate::planning::plan::core::nodes::management::manage_node_enums::UserManageNode::*;
    match node {
        Create(n) => UserManageCommand::Create {
            username: n.username().to_string(),
            password: n.password().to_string(),
            if_not_exists: n.if_not_exists(),
        },
        Alter(n) => UserManageCommand::Alter {
            username: n.username().to_string(),
            new_password: n.new_password().cloned(),
            is_locked: n.is_locked(),
            history_limit: n.password_history_depth(),
        },
        Drop(n) => UserManageCommand::Drop {
            username: n.username().to_string(),
            if_exists: n.if_exists(),
        },
        ChangePassword(n) => UserManageCommand::ChangePassword {
            password_info: n.password_info().clone(),
        },
        GrantRole(n) => UserManageCommand::GrantRole {
            username: n.username().to_string(),
            space_name: n.space_name().to_string(),
            role: n.role(),
        },
        RevokeRole(n) => UserManageCommand::RevokeRole {
            username: n.username().to_string(),
            space_name: n.space_name().to_string(),
            role: n.role(),
        },
        ShowUsers(_) => UserManageCommand::ShowUsers,
        ShowRoles(n) => UserManageCommand::ShowRoles {
            space_name: n.space_name().to_string(),
        },
        DescribeUser(n) => UserManageCommand::DescribeUser {
            username: n.username().to_string(),
        },
    }
}

pub(in crate::executor::streaming::plan::arena_builder) fn build_user_manage_spec(
    node: &crate::planning::plan::core::nodes::management::manage_node_enums::UserManageNode,
    _exec_ctx: &ExecutionContext,
) -> Result<DdlSpec, PlanBuildError> {
    Ok(DdlSpec::UserManage {
        command: user_manage_to_command(node),
    })
}

pub(in crate::executor::streaming::plan::arena_builder) fn build_macro_manage_spec(
    node: &crate::planning::plan::core::nodes::management::manage_node_enums::MacroManageNode,
    _exec_ctx: &ExecutionContext,
) -> Result<DdlSpec, PlanBuildError> {
    use crate::executor::streaming::operators::spec::MacroManageCommand;
    use crate::planning::plan::core::nodes::management::manage_node_enums::MacroManageNode::*;
    let command = match node {
        Create(n) => MacroManageCommand::Create {
            macro_name: n.macro_name().to_string(),
            params: n.info().params.clone(),
            body: n.info().body.clone(),
            if_not_exists: n.info().if_not_exists,
        },
        Drop(n) => MacroManageCommand::Drop {
            macro_name: n.macro_name().to_string(),
            if_exists: n.if_exists(),
        },
    };
    Ok(DdlSpec::MacroManage { command })
}

pub(in crate::executor::streaming::plan::arena_builder) fn build_type_manage_spec(
    node: &crate::planning::plan::core::nodes::management::manage_node_enums::TypeManageNode,
    _exec_ctx: &ExecutionContext,
) -> Result<DdlSpec, PlanBuildError> {
    use crate::executor::streaming::operators::spec::TypeManageCommand;
    use crate::planning::plan::core::nodes::management::manage_node_enums::TypeManageNode::*;
    let command = match node {
        Create(n) => TypeManageCommand::Create {
            type_name: n.alias_name().to_string(),
            underlying_type: n.info().underlying_type.clone(),
            if_not_exists: n.info().if_not_exists,
        },
        Drop(n) => TypeManageCommand::Drop {
            type_name: n.alias_name().to_string(),
            if_exists: n.if_exists(),
        },
    };
    Ok(DdlSpec::TypeManage { command })
}

pub(in crate::executor::streaming::plan::arena_builder) fn build_sequence_manage_spec(
    node: &crate::planning::plan::core::nodes::management::manage_node_enums::SequenceManageNode,
    _exec_ctx: &ExecutionContext,
) -> Result<DdlSpec, PlanBuildError> {
    use crate::executor::streaming::operators::spec::SequenceManageCommand;
    use crate::planning::plan::core::nodes::management::manage_node_enums::SequenceManageNode::*;
    let command = match node {
        Create(n) => SequenceManageCommand::Create {
            seq_name: n.seq_name().to_string(),
            start: n.info().start,
            increment: n.info().increment,
            min_value: n.info().min_value,
            max_value: n.info().max_value,
            cycle: n.info().cycle.unwrap_or(false),
            if_not_exists: n.info().if_not_exists,
        },
        Alter(n) => SequenceManageCommand::Alter {
            seq_name: n.seq_name().to_string(),
            increment: n.info().increment,
            min_value: n.info().min_value,
            max_value: n.info().max_value,
            cycle: n.info().cycle,
        },
        Drop(n) => SequenceManageCommand::Drop {
            seq_name: n.seq_name().to_string(),
            if_exists: n.if_exists(),
        },
    };
    Ok(DdlSpec::SequenceManage { command })
}

// ── Fulltext spec builders ────────────────────────────────────────────────────
