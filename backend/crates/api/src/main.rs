use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let config = pcc_api::Config::from_env()?;
    tracing::info!(
        bind = %config.bind_addr,
        dry_run = config.gates.dry_run,
        enable_mainnet_transfers = config.gates.enable_mainnet_transfers,
        "starting pcc-api"
    );
    if !config.gates.dry_run || config.gates.enable_mainnet_transfers {
        tracing::warn!("LIVE EXECUTION GATES ARE OPEN — every value-moving operation still requires a per-transaction approval reference");
    }

    let listener = tokio::net::TcpListener::bind(config.bind_addr).await?;
    let app = pcc_api::router(config);
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
