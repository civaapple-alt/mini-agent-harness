use mini_agent_protocol::ToolError;

pub(crate) fn run<T, E>(
    thread_name: &'static str,
    label: &'static str,
    future: impl std::future::Future<Output = Result<T, E>> + Send + 'static,
) -> Result<T, E>
where
    T: Send + 'static,
    E: From<ToolError> + Send + 'static,
{
    let join = std::thread::Builder::new()
        .name(thread_name.into())
        .spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| {
                    E::from(ToolError(format!("cannot start {label} runtime: {error}")))
                })?
                .block_on(future)
        })
        .map_err(|error| E::from(ToolError(format!("cannot start {label} thread: {error}"))))?;
    join.join()
        .unwrap_or_else(|_| Err(E::from(ToolError(format!("{label} thread panicked")))))
}
