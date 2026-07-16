use crate::observability::log_retention::{LOG_DIR, LOG_FILE_PREFIX};
use opentelemetry::{
    KeyValue, global, propagation::TextMapCompositePropagator, trace::TracerProvider as _,
};
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{
    Resource,
    logs::SdkLoggerProvider,
    propagation::{BaggagePropagator, TraceContextPropagator},
    trace::SdkTracerProvider,
};
use tracing::Level;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{
    Layer, filter::filter_fn, fmt, layer::SubscriberExt, util::SubscriberInitExt,
};

pub struct TelemetryGuard {
    _file_guard: WorkerGuard,
    tracer_provider: SdkTracerProvider,
    logger_provider: SdkLoggerProvider,
}

pub fn init_log() -> TelemetryGuard {
    let endpoint = "http://127.0.0.1:4317";
    let app_target = env!("CARGO_PKG_NAME").replace('-', "_");
    let resource = build_resource(&deployment_environment(), &service_instance_id());

    let trace_exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .with_timeout(std::time::Duration::from_secs(3))
        .build()
        .expect("无法创建 OTLP span 导出器");

    let tracer_provider = SdkTracerProvider::builder()
        .with_batch_exporter(trace_exporter)
        .with_resource(resource.clone())
        .build();
    global::set_tracer_provider(tracer_provider.clone());
    global::set_text_map_propagator(build_text_map_propagator());
    let tracer = tracer_provider.tracer("tracing-otel");

    let log_exporter = opentelemetry_otlp::LogExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .with_timeout(std::time::Duration::from_secs(3))
        .build()
        .expect("无法创建 OTLP log 导出器");

    let logger_provider = SdkLoggerProvider::builder()
        .with_batch_exporter(log_exporter)
        .with_resource(resource)
        .build();

    let fmt_target = app_target.clone();
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_thread_names(true)
        .with_writer(std::io::stdout)
        .with_ansi(true)
        .with_filter(filter_fn(move |metadata| {
            metadata.target().starts_with(&fmt_target) && metadata.level() <= &Level::INFO
        }));

    let file_appender = tracing_appender::rolling::daily(LOG_DIR, LOG_FILE_PREFIX);
    let (non_blocking, file_guard) = tracing_appender::non_blocking(file_appender);
    let file_target = app_target.clone();
    let file_layer = fmt::layer()
        .with_thread_names(true)
        .with_ansi(false)
        .with_writer(non_blocking)
        .with_filter(filter_fn(move |metadata| {
            metadata.target().starts_with(&file_target) && metadata.level() <= &Level::INFO
        }));

    let otel_trace_target = app_target.clone();
    let otel_trace_layer = tracing_opentelemetry::layer()
        .with_tracer(tracer)
        .with_filter(filter_fn(move |metadata| {
            metadata.target().starts_with(&otel_trace_target) && metadata.level() <= &Level::DEBUG
        }));
    let otel_log_layer =
        OpenTelemetryTracingBridge::new(&logger_provider).with_filter(filter_fn(move |metadata| {
            metadata.target().starts_with(&app_target) && metadata.level() <= &Level::DEBUG
        }));

    tracing_subscriber::registry()
        .with(fmt_layer)
        .with(file_layer)
        .with(otel_trace_layer)
        .with(otel_log_layer)
        .init();

    TelemetryGuard {
        _file_guard: file_guard,
        tracer_provider,
        logger_provider,
    }
}

fn build_resource(deployment_environment: &str, service_instance_id: &str) -> Resource {
    Resource::builder()
        .with_service_name(env!("CARGO_PKG_NAME"))
        .with_attributes([
            KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
            KeyValue::new(
                "deployment.environment.name",
                deployment_environment.to_string(),
            ),
            KeyValue::new("service.instance.id", service_instance_id.to_string()),
        ])
        .build()
}

fn build_text_map_propagator() -> TextMapCompositePropagator {
    TextMapCompositePropagator::new(vec![
        Box::new(TraceContextPropagator::new()),
        Box::new(BaggagePropagator::new()),
    ])
}

fn deployment_environment() -> String {
    std::env::var("OTEL_DEPLOYMENT_ENVIRONMENT")
        .or_else(|_| std::env::var("APP_ENV"))
        .unwrap_or_else(|_| "development".to_string())
}

fn service_instance_id() -> String {
    std::env::var("SERVICE_INSTANCE_ID")
        .or_else(|_| std::env::var("HOSTNAME"))
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        if let Err(err) = self.tracer_provider.shutdown() {
            eprintln!("TracerProvider shutdown error: {err:?}");
        }
        if let Err(err) = self.logger_provider.shutdown() {
            eprintln!("LoggerProvider shutdown error: {err:?}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{build_resource, build_text_map_propagator};
    use opentelemetry::{
        Key, StringValue, Value, baggage::BaggageExt, propagation::TextMapPropagator,
        trace::TraceContextExt,
    };
    use std::collections::HashMap;

    #[test]
    fn resource_includes_service_identity_attributes() {
        let resource = build_resource("test", "light-manager-1");

        assert_eq!(
            resource.get(&Key::new("service.name")),
            Some(Value::from(env!("CARGO_PKG_NAME")))
        );
        assert_eq!(
            resource.get(&Key::new("service.version")),
            Some(Value::from(env!("CARGO_PKG_VERSION")))
        );
        assert_eq!(
            resource.get(&Key::new("deployment.environment.name")),
            Some(Value::from("test"))
        );
        assert_eq!(
            resource.get(&Key::new("service.instance.id")),
            Some(Value::from("light-manager-1"))
        );
    }

    #[test]
    fn composite_propagator_extracts_trace_context_and_baggage() {
        let mut carrier = HashMap::new();
        carrier.insert(
            "traceparent".to_string(),
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".to_string(),
        );
        carrier.insert("baggage".to_string(), "device.id=light-01".to_string());

        let context = build_text_map_propagator().extract(&carrier);

        assert_eq!(
            context.span().span_context().trace_id().to_string(),
            "4bf92f3577b34da6a3ce929d0e0e4736"
        );
        assert_eq!(
            context.baggage().get("device.id"),
            Some(&StringValue::from("light-01"))
        );
    }
}
