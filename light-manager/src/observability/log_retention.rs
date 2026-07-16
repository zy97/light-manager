use crate::config::LoggingConfig;
use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};
use tokio::time::sleep;
use tracing::{error, info, warn};

pub const LOG_DIR: &str = "logs";
pub const LOG_FILE_PREFIX: &str = "app.log";

const SECONDS_PER_DAY: u64 = 24 * 60 * 60;

#[derive(Clone)]
struct LogRetentionSettings {
    log_dir: PathBuf,
    file_prefix: &'static str,
    retained_days: u64,
    cleanup_interval: Duration,
}

pub fn spawn_cleanup_task(config: LoggingConfig) {
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
