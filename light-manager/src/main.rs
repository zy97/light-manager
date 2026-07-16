use light_manager::{
    light_runtime::{build_light_service, log_startup, wait_for_shutdown},
    telemetry,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = telemetry::init_log();
    let light_service = build_light_service();
    log_startup(&light_service);
    wait_for_shutdown(&light_service).await;
    Ok(())
}
