use crate::{
    light::{LightError, LightService, LightStatus},
    web::{api_response::ApiResponse, http_trace::trace_http_request},
};
use axum::{
    Json, Router,
    extract::{ConnectInfo, State},
    http::StatusCode,
    middleware,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use std::{net::SocketAddr, sync::Arc};
use tokio::{net::TcpListener, signal};
use tracing::{error, info};

#[derive(Clone)]
pub struct AppState {
    light_service: Arc<LightService>,
}

#[derive(Debug, Deserialize)]
pub struct ControlLightRequest {
    pub status: LightStatus,
    pub light_ip: Option<String>,
    pub request_ip: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LightTarget {
    LightIp(String),
    RequestIp(String),
}

pub fn build_router(light_service: Arc<LightService>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/api/lights/control", post(control_light))
        .layer(middleware::from_fn(trace_http_request))
        .with_state(AppState { light_service })
}

pub async fn serve(
    listen_addr: String,
    light_service: Arc<LightService>,
) -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind(&listen_addr).await?;
    let local_addr = listener.local_addr()?;
    info!(
        listen_addr = %local_addr,
        "灯控HTTP服务已监听 listen_addr={}",
        local_addr
    );

    axum::serve(
        listener,
        build_router(light_service).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;

    Ok(())
}

async fn health() -> Json<ApiResponse<&'static str>> {
    Json(ApiResponse {
        success: true,
        data: Some("ok"),
        message: "ok".to_string(),
    })
}

async fn control_light(
    State(state): State<AppState>,
    ConnectInfo(remote_addr): ConnectInfo<SocketAddr>,
    Json(request): Json<ControlLightRequest>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let target = resolve_target(request.light_ip, request.request_ip, remote_addr);

    match &target {
        LightTarget::LightIp(light_ip) => {
            state.light_service.light(request.status, light_ip).await?
        }
        LightTarget::RequestIp(request_ip) => {
            state
                .light_service
                .light_by_request_ip(request.status, request_ip)
                .await?
        }
    };

    Ok(Json(ApiResponse {
        success: true,
        data: None,
        message: "ok".to_string(),
    }))
}

fn resolve_target(
    light_ip: Option<String>,
    request_ip: Option<String>,
    remote_addr: SocketAddr,
) -> LightTarget {
    if let Some(light_ip) = light_ip.filter(|ip| !ip.trim().is_empty()) {
        LightTarget::LightIp(light_ip)
    } else if let Some(request_ip) = request_ip.filter(|ip| !ip.trim().is_empty()) {
        LightTarget::RequestIp(request_ip)
    } else {
        LightTarget::RequestIp(remote_addr.ip().to_string())
    }
}

async fn shutdown_signal() {
    if let Err(err) = signal::ctrl_c().await {
        error!(
            error = ?err,
            "监听关闭信号失败 error={:?}",
            err
        );
    }
}

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    message: String,
}

impl From<LightError> for ApiError {
    fn from(error: LightError) -> Self {
        let status = match error {
            LightError::UnknownRequestIp(_) => StatusCode::BAD_REQUEST,
            LightError::ConnectionPool(_, _) | LightError::Io(_, _) | LightError::Timeout(_) => {
                StatusCode::BAD_GATEWAY
            }
        };

        Self {
            status,
            message: error.to_string(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = Json(ApiResponse::<()> {
            success: false,
            data: None,
            message: self.message,
        });

        (self.status, body).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::{LightTarget, resolve_target};
    use std::net::SocketAddr;

    #[test]
    fn explicit_light_ip_wins_over_request_ip() {
        let remote_addr: SocketAddr = "127.0.0.1:12345".parse().unwrap();

        let target = resolve_target(
            Some("192.168.70.151".to_string()),
            Some("192.168.70.166".to_string()),
            remote_addr,
        );

        assert!(matches!(
            target,
            LightTarget::LightIp(ip) if ip == "192.168.70.151"
        ));
    }

    #[test]
    fn falls_back_to_remote_ip_when_no_target_is_provided() {
        let remote_addr: SocketAddr = "127.0.0.1:12345".parse().unwrap();

        let target = resolve_target(None, None, remote_addr);

        assert!(matches!(
            target,
            LightTarget::RequestIp(ip) if ip == "127.0.0.1"
        ));
    }
}
