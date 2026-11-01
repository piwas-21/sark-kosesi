//! Disabled by default. No database or provider configuration enters this process.
#![forbid(unsafe_code)]
use commerce_api::callback_edge::{Config, router};
#[tokio::main]
async fn main() -> Result<(), std::io::Error> {
    tracing_subscriber::fmt().json().init();
    let config = Config::from_env().map_err(std::io::Error::other)?;
    let Some(config) = config else {
        tracing::info!(code = "callback_edge_disabled");
        return Ok(());
    };
    let listener = tokio::net::TcpListener::bind(config.bind())
        .await
        .map_err(|_| std::io::Error::other("callback edge bind unavailable"))?;
    tracing::info!(code = "callback_edge_listening");
    axum::serve(
        listener,
        router(config).into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await
    .map_err(|_| std::io::Error::other("callback edge server unavailable"))
}
