//! Custom function registry handlers.

use tonic::{Request, Response, Status};

use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

use super::error::op_error_to_status;
use super::proto::*;
use super::service::LinkrsService;

impl<
        S: StorageClient
            + StorageSchemaContextOps
            + StorageSyncContextOps
            + StorageOperationContextOps
            + Clone
            + Send
            + Sync
            + 'static,
    > LinkrsService<S>
{
    pub(crate) async fn handle_register_function(
        &self,
        request: Request<RegisterFunctionRequest>,
    ) -> Result<Response<RegisterFunctionResponse>, Status> {
        use crate::http::handlers::function::register_udf_from_source;
        let req = request.into_inner();
        let registry = self.app_state.server.get_function_registry();
        let registered_name = register_udf_from_source(&registry, &req.name, &req.implementation)
            .map_err(op_error_to_status)?;
        Ok(Response::new(RegisterFunctionResponse {
            success: true,
            function_id: registered_name,
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_unregister_function(
        &self,
        request: Request<UnregisterFunctionRequest>,
    ) -> Result<Response<UnregisterFunctionResponse>, Status> {
        use crate::http::handlers::function::unregister_udf_by_name;
        let req = request.into_inner();
        let registry = self.app_state.server.get_function_registry();
        unregister_udf_by_name(&registry, &req.name).map_err(op_error_to_status)?;
        Ok(Response::new(UnregisterFunctionResponse {
            success: true,
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_list_functions(
        &self,
        _request: Request<ListFunctionsRequest>,
    ) -> Result<Response<ListFunctionsResponse>, Status> {
        let registry = self.app_state.server.get_function_registry();
        let registry_guard = registry.read();
        let functions = registry_guard
            .function_names()
            .into_iter()
            .map(|name| function_info_for(&registry_guard, name))
            .collect();
        Ok(Response::new(ListFunctionsResponse {
            functions,
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_get_function_info(
        &self,
        request: Request<GetFunctionInfoRequest>,
    ) -> Result<Response<GetFunctionInfoResponse>, Status> {
        let req = request.into_inner();
        let registry = self.app_state.server.get_function_registry();
        let registry_guard = registry.read();
        match registry_guard.contains(&req.name) {
            true => Ok(Response::new(GetFunctionInfoResponse {
                exists: true,
                function: Some(function_info_for(&registry_guard, &req.name)),
                error: String::new(),
            })),
            false => Ok(Response::new(GetFunctionInfoResponse {
                exists: false,
                function: None,
                error: String::new(),
            })),
        }
    }
}

/// Describe a registered function for the wire.
pub(crate) fn function_info_for(
    registry: &crate::query::executor::expression::functions::FunctionRegistry,
    name: &str,
) -> super::proto::FunctionInfo {
    let builtin = registry.get_builtin(name);
    let custom = registry.get_custom(name);
    let (function_type, description) = match (&builtin, &custom) {
        (Some(function), _) => ("builtin", function.description().to_string()),
        (None, Some(function)) => {
            let arity = if function.is_variadic {
                format!("variadic from {}", function.arity)
            } else {
                format!("arity {}", function.arity)
            };
            (
                "custom",
                if function.description.is_empty() {
                    arity
                } else {
                    format!("{} ({arity})", function.description)
                },
            )
        }
        (None, None) => ("unknown", String::new()),
    };
    super::proto::FunctionInfo {
        name: name.to_string(),
        function_type: function_type.to_string(),
        parameters: vec![],
        return_type: registry
            .get_return_type(name)
            .map(|t| format!("{t:?}"))
            .unwrap_or_else(|| "unknown".to_string()),
        description,
    }
}
