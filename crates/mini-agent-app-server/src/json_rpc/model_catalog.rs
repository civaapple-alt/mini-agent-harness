use super::*;

impl<M> AppServerConnection<M>
where
    M: Model + Send + 'static,
{
    pub(super) async fn handle_model_catalog_manage(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        let params = match request.decode_params::<ModelCatalogManageParams>() {
            Ok(params) => params,
            Err(error) => return response_error(request.id, error),
        };
        let store = match mini_agent_host::ModelCatalogStore::machine_default() {
            Ok(store) => store,
            Err(error) => return response_error(request.id, JsonRpcError::server_error(error)),
        };
        let result = match params.operation {
            ModelCatalogOperation::Get => store.view(),
            ModelCatalogOperation::UpsertProvider => {
                let Some(provider) = params.provider else {
                    return response_error(
                        request.id,
                        JsonRpcError::invalid_params("provider is required for upsert_provider"),
                    );
                };
                let provider = match parse_value::<mini_agent_host::ProviderProfile>(provider) {
                    Ok(provider) => provider,
                    Err(error) => return response_error(request.id, error),
                };
                store.upsert_provider(provider, params.api_key)
            }
            ModelCatalogOperation::DeleteProvider => {
                let Some(provider_id) = params.provider_id else {
                    return response_error(
                        request.id,
                        JsonRpcError::invalid_params("providerId is required for delete_provider"),
                    );
                };
                store.delete_provider(&provider_id)
            }
            ModelCatalogOperation::UpsertModel => {
                let (Some(provider_id), Some(model)) = (params.provider_id, params.model) else {
                    return response_error(
                        request.id,
                        JsonRpcError::invalid_params(
                            "providerId and model are required for upsert_model",
                        ),
                    );
                };
                let model = match parse_value::<mini_agent_host::ModelProfile>(model) {
                    Ok(model) => model,
                    Err(error) => return response_error(request.id, error),
                };
                store.upsert_model(&provider_id, model, params.previous_model_id.as_deref())
            }
            ModelCatalogOperation::DeleteModel => {
                let (Some(provider_id), Some(model_id)) = (params.provider_id, params.model_id)
                else {
                    return response_error(
                        request.id,
                        JsonRpcError::invalid_params(
                            "providerId and modelId are required for delete_model",
                        ),
                    );
                };
                store.delete_model(&provider_id, &model_id)
            }
            ModelCatalogOperation::SetDefaults => {
                let (Some(default_model), Some(verifier_default_model)) =
                    (params.default_model, params.verifier_default_model)
                else {
                    return response_error(
                        request.id,
                        JsonRpcError::invalid_params(
                            "defaultModel and verifierDefaultModel are required; use null to clear",
                        ),
                    );
                };
                store.set_defaults(default_model, verifier_default_model)
            }
            ModelCatalogOperation::SetProjectDefault => {
                let (Some(project_id), Some(project_default)) =
                    (params.project_id, params.project_default)
                else {
                    return response_error(
                        request.id,
                        JsonRpcError::invalid_params(
                            "projectId and projectDefault are required; use null to clear",
                        ),
                    );
                };
                store.set_project_default(project_id, project_default)
            }
        };
        match result {
            Ok(catalog) => match project_catalog(catalog) {
                Ok(catalog) => response_value(request.id, ModelCatalogManageResult { catalog }),
                Err(error) => response_error(request.id, JsonRpcError::server_error(error)),
            },
            Err(error) => response_error(request.id, JsonRpcError::server_error(error)),
        }
    }
}

fn parse_value<T: serde::de::DeserializeOwned>(
    value: impl serde::Serialize,
) -> Result<T, JsonRpcError> {
    let value = serde_json::to_value(value)
        .map_err(|error| JsonRpcError::invalid_params(error.to_string()))?;
    serde_json::from_value(value).map_err(|error| JsonRpcError::invalid_params(error.to_string()))
}

fn project_catalog(
    catalog: mini_agent_host::ModelCatalogView,
) -> Result<ProtocolModelCatalogView, String> {
    let value = serde_json::to_value(catalog).map_err(|error| error.to_string())?;
    serde_json::from_value(value).map_err(|error| error.to_string())
}
