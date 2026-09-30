use super::*;
use mini_agent_app_server_protocol::{
    WebSearchSettingsReadParams, WebSearchSettingsResult, WebSearchSettingsUpdateParams,
    WebSearchSettingsView as ProtocolWebSearchSettingsView,
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
