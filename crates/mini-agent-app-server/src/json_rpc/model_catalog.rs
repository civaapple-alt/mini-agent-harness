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
        if params.operation == ModelCatalogOperation::TestConnection {
            let (Some(provider_id), Some(model_id)) = (params.provider_id, params.model_id) else {
                return response_error(
                    request.id,
                    JsonRpcError::invalid_params(
                        "providerId and modelId are required for test_connection",
                    ),
                );
            };
            let selection = mini_agent_protocol::ModelSelection::new(provider_id, model_id);
            let status = match store.test_connection(&selection).await {
                Ok(status) => status,
                Err(error) => {
                    return response_error(request.id, JsonRpcError::server_error(error));
                }
            };
            let catalog = match store.view().and_then(project_catalog) {
                Ok(catalog) => catalog,
                Err(error) => {
                    return response_error(request.id, JsonRpcError::server_error(error));
                }
            };
            return response_value(
                request.id,
                ModelCatalogManageResult {
                    catalog,
                    connection_test: Some(protocol_connection_test(status)),
                },
            );
        }
        let result = match params.operation {
            ModelCatalogOperation::Get => store.view(),
            ModelCatalogOperation::TestConnection => unreachable!("handled above"),
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
                store.set_defaults_with_reasoning(
                    default_model,
                    params.default_reasoning_selection.unwrap_or_default(),
                    verifier_default_model,
                )
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
                Ok(catalog) => response_value(
                    request.id,
                    ModelCatalogManageResult {
                        catalog,
                        connection_test: None,
                    },
                ),
                Err(error) => response_error(request.id, JsonRpcError::server_error(error)),
            },
            Err(error) => response_error(request.id, JsonRpcError::server_error(error)),
        }
    }
}

fn protocol_connection_test(
    status: mini_agent_host::ModelConnectionTestStatus,
) -> mini_agent_app_server_protocol::ModelConnectionTestResult {
    use mini_agent_app_server_protocol::ModelConnectionTestStatus as ProtocolStatus;
    use mini_agent_host::ModelConnectionTestStatus as HostStatus;
    let (status, message) = match status {
        HostStatus::Succeeded => (ProtocolStatus::Succeeded, "Connection succeeded."),
        HostStatus::InvalidCredentials => (
            ProtocolStatus::InvalidCredentials,
            "The provider rejected the API key.",
        ),
        HostStatus::ProviderRejected => (
            ProtocolStatus::ProviderRejected,
            "The provider rejected the connection test request.",
        ),
        HostStatus::TimedOut => (ProtocolStatus::TimedOut, "The provider request timed out."),
        HostStatus::Unreachable => (
            ProtocolStatus::Unreachable,
            "The provider could not be reached.",
        ),
        HostStatus::InvalidResponse => (
            ProtocolStatus::InvalidResponse,
            "The provider returned an incomplete or invalid response.",
        ),
        HostStatus::Failed => (ProtocolStatus::Failed, "The connection test failed."),
    };
    mini_agent_app_server_protocol::ModelConnectionTestResult {
        status,
        message: message.to_string(),
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

#[cfg(test)]
mod tests {
    use super::project_catalog;

    #[test]
    fn provider_search_support_survives_protocol_projection() {
        let catalog = mini_agent_host::ModelCatalogView {
            providers: vec![mini_agent_host::ProviderView {
                profile: mini_agent_host::ProviderProfile {
                    id: "provider".to_string(),
                    name: "Provider".to_string(),
                    kind: mini_agent_host::ProviderKind::Custom,
                    base_url: "https://example.test/v1".to_string(),
                    enabled: true,
                    web_search: Some(true),
                    models: Vec::new(),
                },
                api_key_configured: true,
                web_search_support: mini_agent_host::ProviderWebSearchSupport::Supported,
                web_search_enabled: true,
            }],
            default_model: None,
            default_reasoning_selection: Default::default(),
            verifier_default_model: None,
            project_defaults: Default::default(),
        };
        let projected = project_catalog(catalog).unwrap();
        let provider = &projected.providers[0];
        assert_eq!(
            provider.web_search_support,
            mini_agent_app_server_protocol::ModelProviderWebSearchSupport::Supported
        );
        assert!(provider.web_search_enabled);
    }
}
