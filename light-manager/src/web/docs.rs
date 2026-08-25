use crate::web::server::ApiDoc;
use utoipa::OpenApi;

/// 返回自动生成的 OpenAPI 3.1.0 JSON,供 Scalar 等工具渲染。
pub async fn openapi_json() -> axum::Json<utoipa::openapi::OpenApi> {
    axum::Json(ApiDoc::openapi())
}