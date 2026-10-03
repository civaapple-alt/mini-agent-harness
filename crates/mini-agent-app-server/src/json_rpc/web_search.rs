use super::*;
use mini_agent_app_server_protocol::{
    WebSearchSettingsReadParams, WebSearchSettingsResult, WebSearchSettingsUpdateParams,
    WebSearchSettingsView as ProtocolWebSearchSettingsView, WebSearchTestParams,
    WebSearchTestResult,
};

impl<M> AppServerConnection<M>
where
    M: Model + Send + 'static,
{
    pub(super) async fn handle_web_search_settings_read(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        if let Err(error) = request.decode_params::<WebSearchSettingsReadParams>() {
            return response_error(request.id, error);
        }
        let result = mini_agent_host::WebSearchSettingsStore::machine_default()
            .and_then(|store| store.view())
            .map(project_settings);
        match result {
            Ok(settings) => response_value(request.id, WebSearchSettingsResult { settings }),
            Err(error) => response_error(request.id, JsonRpcError::server_error(error)),
        }
    }

    pub(super) async fn handle_web_search_settings_update(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        let params = match request.decode_params::<WebSearchSettingsUpdateParams>() {
            Ok(params) => params,
            Err(error) => return response_error(request.id, error),
        };
        let result = mini_agent_host::WebSearchSettingsStore::machine_default().and_then(|store| {
            store.update(
                &params.provider,
                params.deepseek_api_key.as_deref(),
                params.exa_api_key.as_deref(),
                params.kimi_api_key.as_deref(),
            )
        });
        match result {
            Ok(settings) => response_value(
                request.id,
                WebSearchSettingsResult {
                    settings: project_settings(settings),
                },
            ),
            Err(error) => response_error(request.id, JsonRpcError::invalid_params(error)),
        }
    }

    pub(super) async fn handle_web_search_test(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        let params = match request.decode_params::<WebSearchTestParams>() {
            Ok(params) => params,
            Err(error) => return response_error(request.id, error),
        };
        let query = params.query.trim().to_string();
        if query.is_empty() || query.len() > 2_000 {
            return response_error(
                request.id,
                JsonRpcError::invalid_params("query must contain 1 to 2000 bytes"),
            );
        }
        let expected_query = query.clone();
        let provider = params.provider.map(|provider| provider.as_str());

        let result = tokio::task::spawn_blocking(move || {
            mini_agent_host::WebSearchSettingsStore::machine_default().and_then(|store| {
                match provider {
                    Some(provider) => store.test_search_for_provider(provider, &query),
                    None => store.test_search(&query),
                }
            })
        })
        .await;
        match result {
            Ok(Ok(encoded)) => match serde_json::from_str::<WebSearchTestResult>(&encoded) {
                Ok(result)
                    if result.query == expected_query
                        && result.results.len() <= 3
                        && result.result_count as usize == result.results.len() =>
                {
                    response_value(request.id, result)
                }
                _ => response_error(
                    request.id,
                    JsonRpcError::server_error("web search test returned an invalid response"),
                ),
            },
            Ok(Err(error)) => response_error(request.id, JsonRpcError::server_error(error)),
            Err(_) => response_error(
                request.id,
                JsonRpcError::server_error("web search test task failed"),
            ),
        }
    }
}

fn project_settings(
    settings: mini_agent_host::WebSearchSettingsView,
) -> ProtocolWebSearchSettingsView {
    ProtocolWebSearchSettingsView {
        provider: settings.provider,
        deepseek_api_key_configured: settings.deepseek_api_key_configured,
        exa_api_key_configured: settings.exa_api_key_configured,
        kimi_api_key_configured: settings.kimi_api_key_configured,
    }
}
