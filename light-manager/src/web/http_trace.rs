use crate::web::api_response::ApiResponse;
use axum::{
    Json,
    body::{Body, to_bytes},
    extract::Request,
    http::{HeaderMap, StatusCode, Uri, Version, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use opentelemetry::{Context, global, trace::TraceContextExt};
use opentelemetry_http::HeaderExtractor;
use std::time::Instant;
use tracing::{Instrument, error, field, info, info_span};
use tracing_opentelemetry::OpenTelemetrySpanExt;

pub async fn trace_http_request(request: Request, next: Next) -> Response {
    let request_info = HttpRequestLogInfo::from_request(&request);
    let parent_context = extract_trace_context(request.headers());
    let parent_trace_id = trace_id_from_context(&parent_context);
    let span = info_span!(
        "http_request",
        trace_id = field::Empty,
        protocol = %request_info.protocol,
        method = %request_info.method,
        scheme = %request_info.scheme,
        host = %request_info.host,
        pathbase = "",
        path = %request_info.path,
        requestpath = %request_info.path,
        querystring = %request_info.query_string,
        contenttype = %request_info.content_type,
        contentlength = request_info.content_length,
    );
    let _ = span.set_parent(parent_context);

    let trace_id = parent_trace_id.unwrap_or_else(|| {
        trace_id_from_context(&span.context()).unwrap_or_else(|| "unknown".to_string())
    });
    span.record("trace_id", field::display(&trace_id));

    async move {
        let started_at = Instant::now();
        info!(
            trace_id = %trace_id,
            protocol = %request_info.protocol,
            method = %request_info.method,
            scheme = %request_info.scheme,
            host = %request_info.host,
            pathbase = "",
            path = %request_info.path,
            requestpath = %request_info.path,
            querystring = %request_info.query_string,
            contenttype = %request_info.content_type,
            contentlength = request_info.content_length,
            "{}",
            request_info.start_message()
        );

        let response = next.run(request).await;
        let status_code = response.status().as_u16();
        let (parts, body) = response.into_parts();
        let response_content_type = header_value(&parts.headers, header::CONTENT_TYPE);
        let body_bytes = match to_bytes(body, usize::MAX).await {
            Ok(bytes) => bytes,
            Err(err) => {
                error!(
                    trace_id = %trace_id,
                    error = ?err,
                    "读取HTTP响应体失败 error={:?}",
                    err
                );
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiResponse::<()> {
                        success: false,
                        data: None,
                        message: "failed to read http response body".to_string(),
                    }),
                )
                    .into_response();
            }
        };
        let response_info = HttpResponseLogInfo::new(
            response_content_type,
            body_bytes.len() as u64,
            started_at.elapsed().as_millis(),
        );

        info!(
            trace_id = %trace_id,
            protocol = %request_info.protocol,
            method = %request_info.method,
            scheme = %request_info.scheme,
            host = %request_info.host,
            pathbase = "",
            path = %request_info.path,
            requestpath = %request_info.path,
            querystring = %request_info.query_string,
            statuscode = status_code,
            contentlength = response_info.content_length,
            contenttype = %response_info.content_type,
            elapsedmilliseconds = response_info.elapsed_ms,
            "{}",
            response_info.finish_message(&request_info, status_code)
        );

        Response::from_parts(parts, Body::from(body_bytes))
    }
    .instrument(span)
    .await
}

#[derive(Debug, PartialEq, Eq)]
struct HttpRequestLogInfo {
    protocol: String,
    method: String,
    scheme: String,
    host: String,
    path: String,
    query_string: String,
    content_type: String,
    content_length: u64,
}

impl HttpRequestLogInfo {
    fn from_request(request: &Request) -> Self {
        let headers = request.headers();
        let uri = request.uri();

        Self {
            protocol: protocol(request.version()),
            method: request.method().to_string(),
            scheme: scheme(uri, headers),
            host: host(uri, headers),
            path: uri.path().to_string(),
            query_string: query_string(uri),
            content_type: header_value(headers, header::CONTENT_TYPE),
            content_length: content_length(headers),
        }
    }

    fn start_message(&self) -> String {
        format!(
            "Request starting {} {} {}://{}{}{}{} - {} {}",
            self.protocol,
            self.method,
            self.scheme,
            self.host,
            "",
            self.path,
            self.query_string,
            self.content_type,
            self.content_length
        )
    }
}

#[derive(Debug, PartialEq, Eq)]
struct HttpResponseLogInfo {
    content_type: String,
    content_length: u64,
    elapsed_ms: u128,
}

impl HttpResponseLogInfo {
    fn new(content_type: String, content_length: u64, elapsed_ms: u128) -> Self {
        Self {
            content_type,
            content_length,
            elapsed_ms,
        }
    }

    fn finish_message(&self, request: &HttpRequestLogInfo, status_code: u16) -> String {
        format!(
            "Request finished {} {} {}://{}{}{}{} - {} {} {} {}ms",
            request.protocol,
            request.method,
            request.scheme,
            request.host,
            "",
            request.path,
            request.query_string,
            status_code,
            self.content_length,
            self.content_type,
            self.elapsed_ms
        )
    }
}

fn protocol(version: Version) -> String {
    format!("{version:?}")
}

fn scheme(uri: &Uri, headers: &HeaderMap) -> String {
    uri.scheme_str()
        .or_else(|| header_str(headers, "x-forwarded-proto"))
        .or_else(|| header_str(headers, "x-scheme"))
        .unwrap_or("http")
        .to_string()
}

fn host(uri: &Uri, headers: &HeaderMap) -> String {
    uri.authority()
        .map(|authority| authority.as_str())
        .or_else(|| header_str(headers, header::HOST.as_str()))
        .unwrap_or("")
        .to_string()
}

fn query_string(uri: &Uri) -> String {
    uri.query()
        .map(|query| format!("?{query}"))
        .unwrap_or_default()
}

fn content_length(headers: &HeaderMap) -> u64 {
    header_str(headers, header::CONTENT_LENGTH.as_str())
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0)
}

fn header_value(headers: &HeaderMap, name: header::HeaderName) -> String {
    header_str(headers, name.as_str()).unwrap_or("").to_string()
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

fn extract_trace_context(headers: &HeaderMap) -> Context {
    global::get_text_map_propagator(|propagator| propagator.extract(&HeaderExtractor(headers)))
}

fn trace_id_from_context(context: &Context) -> Option<String> {
    let span_context = context.span().span_context().clone();

    if span_context.is_valid() {
        Some(span_context.trace_id().to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{HttpRequestLogInfo, HttpResponseLogInfo, trace_id_from_context};
    use axum::body::Body;
    use axum::http::{HeaderMap, HeaderValue, Request};
    use opentelemetry::propagation::TextMapPropagator;
    use opentelemetry_http::HeaderExtractor;
    use opentelemetry_sdk::propagation::TraceContextPropagator;

    #[test]
    fn reads_trace_id_from_traceparent_header() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "traceparent",
            HeaderValue::from_static("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"),
        );
        let propagator = TraceContextPropagator::new();
        let context = propagator.extract(&HeaderExtractor(&headers));

        assert_eq!(
            trace_id_from_context(&context),
            Some("4bf92f3577b34da6a3ce929d0e0e4736".to_string())
        );
    }

    #[test]
    fn builds_http_request_log_info_like_aspnet_core() {
        let request = Request::builder()
            .method("POST")
            .uri("http://10.11.60.143:9001/User/Login")
            .header("content-type", "application/json")
            .header("content-length", "43")
            .body(Body::empty())
            .unwrap();

        assert_eq!(
            HttpRequestLogInfo::from_request(&request),
            HttpRequestLogInfo {
                protocol: "HTTP/1.1".to_string(),
                method: "POST".to_string(),
                scheme: "http".to_string(),
                host: "10.11.60.143:9001".to_string(),
                path: "/User/Login".to_string(),
                query_string: String::new(),
                content_type: "application/json".to_string(),
                content_length: 43,
            }
        );
    }

    #[test]
    fn renders_http_request_log_fields_in_body() {
        let request = Request::builder()
            .method("POST")
            .uri("http://10.11.60.143:9001/User/Login?source=scanner")
            .header("content-type", "application/json")
            .header("content-length", "43")
            .body(Body::empty())
            .unwrap();

        assert_eq!(
            HttpRequestLogInfo::from_request(&request).start_message(),
            "Request starting HTTP/1.1 POST http://10.11.60.143:9001/User/Login?source=scanner - application/json 43"
        );
    }

    #[test]
    fn renders_http_response_log_fields_in_body() {
        let request = Request::builder()
            .method("POST")
            .uri("http://10.11.60.143:9001/User/Login?source=scanner")
            .header("content-type", "application/json")
            .header("content-length", "43")
            .body(Body::empty())
            .unwrap();
        let request_info = HttpRequestLogInfo::from_request(&request);
        let response_info = HttpResponseLogInfo::new("application/json".to_string(), 52, 5);

        assert_eq!(
            response_info.finish_message(&request_info, 200),
            "Request finished HTTP/1.1 POST http://10.11.60.143:9001/User/Login?source=scanner - 200 52 application/json 5ms"
        );
    }
}
