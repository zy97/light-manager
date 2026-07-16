use opentelemetry::global;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{
    Resource, logs::SdkLoggerProvider, propagation::TraceContextPropagator,
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
    let app_target = env!("CARGO_PKG_NAME");
    let resource = Resource::builder()
        .with_service_name(env!("CARGO_PKG_NAME"))
        .build();

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
    global::set_text_map_propagator(TraceContextPropagator::new());
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

    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_thread_names(true)
        .with_filter(filter_fn(move |metadata| {
            metadata.target().starts_with(app_target) && metadata.level() <= &Level::DEBUG
        }));

    let file_appender = tracing_appender::rolling::daily("logs", "app.log");
    let (non_blocking, file_guard) = tracing_appender::non_blocking(file_appender);
    let file_layer = fmt::layer()
        .with_thread_names(true)
        .with_ansi(false)
        .with_writer(non_blocking)
        .with_filter(filter_fn(move |metadata| {
            metadata.target().starts_with(app_target) && metadata.level() <= &Level::INFO
        }));

    let otel_trace_layer = tracing_opentelemetry::layer()
        .with_tracer(tracer)
        .with_filter(filter_fn(move |metadata| {
            metadata.target().starts_with(app_target) && metadata.level() <= &Level::DEBUG
        }));
    let otel_log_layer =
        OpenTelemetryTracingBridge::new(&logger_provider).with_filter(filter_fn(move |metadata| {
            metadata.target().starts_with(app_target) && metadata.level() <= &Level::DEBUG
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
