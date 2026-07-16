use light_manager::{
    app_config::APP_CONFIG,
    light_runtime::{build_light_service, log_startup},
    telemetry, web_server,
};
use std::sync::Arc;
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = telemetry::init_log();
    let light_service = Arc::new(build_light_service());
    log_startup(&light_service);
    info!(
        listen_addr = %APP_CONFIG.server.listen_addr,
        "starting light manager http server"
    );
    web_server::serve(APP_CONFIG.server.listen_addr.clone(), light_service).await?;
    Ok(())
}
