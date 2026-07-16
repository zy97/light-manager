use crate::app_config::LoggingConfig;
use opentelemetry::global;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{
    Resource, logs::SdkLoggerProvider, propagation::TraceContextPropagator,
    trace::SdkTracerProvider,
};
use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};
use tokio::time::sleep;
use tracing::{Level, error, info, warn};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{
    Layer, filter::filter_fn, fmt, layer::SubscriberExt, util::SubscriberInitExt,
};

const LOG_DIR: &str = "logs";
const LOG_FILE_PREFIX: &str = "app.log";
const SECONDS_PER_DAY: u64 = 24 * 60 * 60;

pub struct TelemetryGuard {
    _file_guard: WorkerGuard,
    tracer_provider: SdkTracerProvider,
    logger_provider: SdkLoggerProvider,
}

#[derive(Clone)]
struct LogRetentionSettings {
    log_dir: PathBuf,
    file_prefix: &'static str,
    retained_days: u64,
    cleanup_interval: Duration,
}

pub fn init_log() -> TelemetryGuard {
    let endpoint = "http://127.0.0.1:4317";
    let app_target = env!("CARGO_PKG_NAME").replace('-', "_");
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

    let fmt_target = app_target.clone();
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_thread_names(true)
        .with_writer(std::io::stdout)
        .with_ansi(true)
        .with_filter(filter_fn(move |metadata| {
            metadata.target().starts_with(&fmt_target) && metadata.level() <= &Level::INFO
        }));

    let file_appender = tracing_appender::rolling::daily("logs", "app.log");
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

pub fn spawn_log_retention_task(config: LoggingConfig) {
    if config.retained_days == 0 {
        warn!(
            retained_days = config.retained_days,
            "本地日志清理已禁用 retained_days={}", config.retained_days
        );
        return;
    }

    let cleanup_interval_hours = config.cleanup_interval_hours.max(1);
    let settings = LogRetentionSettings {
        log_dir: PathBuf::from(LOG_DIR),
        file_prefix: LOG_FILE_PREFIX,
        retained_days: config.retained_days,
        cleanup_interval: Duration::from_secs(cleanup_interval_hours.saturating_mul(60 * 60)),
    };

    info!(
        log_dir = %settings.log_dir.display(),
        retained_days = settings.retained_days,
        cleanup_interval_hours,
        "本地日志清理任务已启动 log_dir={} retained_days={} cleanup_interval_hours={}",
        settings.log_dir.display(),
        settings.retained_days,
        cleanup_interval_hours
    );

    tokio::spawn(async move {
        loop {
            cleanup_old_log_files(&settings);
            sleep(settings.cleanup_interval).await;
        }
    });
}

fn cleanup_old_log_files(settings: &LogRetentionSettings) {
    match delete_expired_log_files(settings, SystemTime::now()) {
        Ok(deleted_count) => {
            info!(
                log_dir = %settings.log_dir.display(),
                retained_days = settings.retained_days,
                deleted_count,
                "本地日志清理完成 log_dir={} retained_days={} deleted_count={}",
                settings.log_dir.display(),
                settings.retained_days,
                deleted_count
            );
        }
        Err(err) => {
            error!(
                log_dir = %settings.log_dir.display(),
                error = ?err,
                "本地日志清理失败 log_dir={} error={:?}",
                settings.log_dir.display(),
                err
            );
        }
    }
}

fn delete_expired_log_files(settings: &LogRetentionSettings, now: SystemTime) -> io::Result<usize> {
    let entries = match fs::read_dir(&settings.log_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(err) => return Err(err),
    };

    let mut deleted_count = 0;
    for entry_result in entries {
        let entry = match entry_result {
            Ok(entry) => entry,
            Err(err) => {
                warn!(
                    log_dir = %settings.log_dir.display(),
                    error = ?err,
                    "读取日志目录项失败 log_dir={} error={:?}",
                    settings.log_dir.display(),
                    err
                );
                continue;
            }
        };
        let path = entry.path();

        if !is_managed_log_file(&path, settings.file_prefix) {
            continue;
        }

        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(err) => {
                warn!(
                    path = %path.display(),
                    error = ?err,
                    "读取日志文件元数据失败 path={} error={:?}",
                    path.display(),
                    err
                );
                continue;
            }
        };

        if !metadata.is_file() {
            continue;
        }

        let modified = match metadata.modified() {
            Ok(modified) => modified,
            Err(err) => {
                warn!(
                    path = %path.display(),
                    error = ?err,
                    "读取日志文件修改时间失败 path={} error={:?}",
                    path.display(),
                    err
                );
                continue;
            }
        };

        if !is_expired(modified, now, settings.retained_days) {
            continue;
        }

        match fs::remove_file(&path) {
            Ok(()) => {
                deleted_count += 1;
                info!(
                    path = %path.display(),
                    retained_days = settings.retained_days,
                    "已删除过期日志文件 path={} retained_days={}",
                    path.display(),
                    settings.retained_days
                );
            }
            Err(err) => {
                warn!(
                    path = %path.display(),
                    error = ?err,
                    "删除过期日志文件失败 path={} error={:?}",
                    path.display(),
                    err
                );
            }
        }
    }

    Ok(deleted_count)
}

fn is_managed_log_file(path: &Path, file_prefix: &str) -> bool {
    path.file_name()
        .and_then(|file_name| file_name.to_str())
        .map(|file_name| file_name.starts_with(file_prefix))
        .unwrap_or(false)
}

fn is_expired(modified: SystemTime, now: SystemTime, retained_days: u64) -> bool {
    if retained_days == 0 {
        return false;
    }

    let retained = Duration::from_secs(retained_days.saturating_mul(SECONDS_PER_DAY));
    now.duration_since(modified)
        .map(|age| age > retained)
        .unwrap_or(false)
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
    use super::{is_expired, is_managed_log_file};
    use std::{
        path::Path,
        time::{Duration, SystemTime},
    };

    #[test]
    fn detects_expired_log_files_by_modified_time() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(40 * 24 * 60 * 60);
        let old_file = now - Duration::from_secs(31 * 24 * 60 * 60);
        let recent_file = now - Duration::from_secs(29 * 24 * 60 * 60);

        assert!(is_expired(old_file, now, 30));
        assert!(!is_expired(recent_file, now, 30));
        assert!(!is_expired(old_file, now, 0));
    }

    #[test]
    fn only_manages_app_log_files() {
        assert!(is_managed_log_file(Path::new("logs/app.log"), "app.log"));
        assert!(is_managed_log_file(
            Path::new("logs/app.log.2026-07-16"),
            "app.log"
        ));
        assert!(!is_managed_log_file(Path::new("logs/other.log"), "app.log"));
    }
}
