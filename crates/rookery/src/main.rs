mod config;
mod northbound;

use std::sync::Arc;
use std::time::Duration;

use rookery_discovery::Discovery;
use rookery_fleet::Fleet;
use rookery_instance_live::LiveClientProvider;
use rookery_web::AppState;

use config::Config;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Before the config is read, so a fault while parsing it has somewhere to
    // be recorded. The guard must be held for the whole of main — dropping it
    // silently stops the log file being written.
    let _diag = diag::init(
        diag::Options::new("rookery", "ROOKERY", env!("CARGO_PKG_VERSION"))
            .with_default_filter("rookery=info,rookery_fleet=info,rookery_instance_live=info"),
    )?;

    if std::env::args().any(|a| a == "--collect-diagnostics") {
        println!("{}", diag::collect_diagnostics()?.display());
        return Ok(());
    }

    let config_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "config/rookery.toml".to_string());
    let config = Config::load(&config_path)?;
    tracing::info!(?config, "loaded config");
    diag::set_config(&config);

    let registry = Arc::new(rookery_core::Registry::load_or_new(
        config.registry_path.clone().into(),
    )?);
    tracing::info!(instances = registry.list().len(), "registry loaded");

    let provider = Arc::new(LiveClientProvider::new().await?);
    if let Some(addr) = provider.sender().local_addr() {
        tracing::info!(%addr, "southbound OSC socket");
    }

    let fleet = Arc::new(Fleet::new(registry, provider));
    fleet.spawn_poller(Duration::from_millis(config.poll_interval_ms));

    let northbound = match &config.osc_bind {
        Some(bind) => {
            let addr = northbound::spawn(bind, config.osc_prefix.clone(), fleet.clone()).await?;
            tracing::warn!(
                %addr,
                prefix = %config.osc_prefix,
                "northbound OSC listening — this port has NO authentication and can retarget \
                 every instance in the fleet"
            );
            Some(addr.to_string())
        }
        None => {
            tracing::info!("northbound OSC is off (set osc_bind to enable it)");
            None
        }
    };

    let state = AppState {
        fleet,
        discovery: Arc::new(Discovery::new()?),
        northbound,
        northbound_prefix: config.osc_prefix.clone(),
    };

    let app = rookery_web::app(state);
    let listener = tokio::net::TcpListener::bind(&config.bind).await?;
    tracing::info!(bind = %config.bind, "rookery listening");
    axum::serve(listener, app).await?;
    Ok(())
}
