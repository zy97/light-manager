use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Serialize)]
pub struct ApiResponse<T>
where
    T: Serialize,
{
    pub success: bool,
    pub data: Option<T>,
    pub message: String,
}

/// 健康检查等返回字符串 data 的响应 schema。
#[derive(Debug, Serialize, ToSchema)]
pub struct ApiResponseString {
    pub success: bool,
    pub data: Option<String>,
    pub message: String,
}

/// 控制信号灯等返回空 data 的响应 schema。
#[derive(Debug, Serialize, ToSchema)]
pub struct ApiResponseNull {
    pub success: bool,
    pub data: Option<()>,
    pub message: String,
}