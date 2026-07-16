use light_manager::{
    app_config::APP_CONFIG,
    light_runtime::{build_light_service, log_startup},
    telemetry, web_server,
};
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = telemetry::init_log();
    telemetry::spawn_log_retention_task(APP_CONFIG.logging.clone());
    let light_service = Arc::new(build_light_service());
    log_startup(&light_service);
    web_server::serve(APP_CONFIG.server.listen_addr.clone(), light_service).await?;
    Ok(())
}
