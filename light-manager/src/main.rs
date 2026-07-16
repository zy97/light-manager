use light_manager::{
    config::APP_CONFIG,
    light::{build_light_service, log_startup},
    observability::{log_retention, telemetry},
    web,
};
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = telemetry::init_log();
    log_retention::spawn_cleanup_task(APP_CONFIG.logging.clone());
    let light_service = Arc::new(build_light_service());
    log_startup(&light_service);
    web::serve(APP_CONFIG.server.listen_addr.clone(), light_service).await?;
    Ok(())
}
