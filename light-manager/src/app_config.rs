use config::{Config, ConfigError};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::LazyLock;
use tracing::debug;

pub static APP_CONFIG: LazyLock<AppConfig> = LazyLock::new(|| {
    let config = load_config().expect("failed to load application config");
    debug!("加载配置成功：{:#?}", config);
    config
});

#[derive(Debug, Deserialize, Clone)]
pub struct ServerConfig {
    #[serde(default = "default_listen_addr")]
    pub listen_addr: String,
}

#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
pub struct Light {
    pub address: String,
    #[serde(default)]
    pub request_ips: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LightConfig {
    #[serde(default)]
    pub lights: Vec<Light>,
    #[serde(default)]
    pub commands: LightCommandConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LightCommandConfig {
    #[serde(default)]
    pub basic: BasicLightCommandConfig,
    #[serde(default)]
    pub composite: CompositeLightCommandConfig,
    #[serde(default)]
    pub timing: LightTimingConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct BasicLightCommandConfig {
    #[serde(default = "default_green_light_off")]
    pub green_light_off: String,
    #[serde(default = "default_green_light_on")]
    pub green_light_on: String,
    #[serde(default = "default_red_light_on")]
    pub red_light_on: String,
    #[serde(default = "default_red_light_off")]
    pub red_light_off: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct CompositeLightCommandConfig {
    #[serde(default = "default_red_sequence")]
    pub red: Vec<CompositeItemConfig>,
    #[serde(default = "default_green_sequence")]
    pub green: Vec<CompositeItemConfig>,
    #[serde(default = "default_red_flash_sequence")]
    pub red_flash: Vec<CompositeItemConfig>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum CompositeItemConfig {
    Command {
        command: String,
    },
    Repeat {
        repeat: usize,
        steps: Vec<CompositeStepConfig>,
    },
}

#[derive(Debug, Deserialize, Clone)]
pub struct CompositeStepConfig {
    pub command: String,
    #[serde(default)]
    pub delay_ms: Option<u64>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LightTimingConfig {
    #[serde(default = "default_io_timeout_ms")]
    pub io_timeout_ms: u64,
}

#[derive(Debug, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
    pub light: LightConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LoggingConfig {
    #[serde(default = "default_log_retained_days")]
    pub retained_days: u64,
    #[serde(default = "default_log_cleanup_interval_hours")]
    pub cleanup_interval_hours: u64,
}

pub fn load_config() -> Result<AppConfig, ConfigError> {
    let settings = Config::builder()
        .add_source(config::File::with_name("./config"))
        .build()?;
    settings.try_deserialize::<AppConfig>()
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            listen_addr: default_listen_addr(),
        }
    }
}

fn default_listen_addr() -> String {
    "0.0.0.0:3000".to_string()
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            retained_days: default_log_retained_days(),
            cleanup_interval_hours: default_log_cleanup_interval_hours(),
        }
    }
}

fn default_log_retained_days() -> u64 {
    30
}

fn default_log_cleanup_interval_hours() -> u64 {
    24
}

impl LightConfig {
    pub fn request_ip_map(&self) -> HashMap<String, Vec<String>> {
        let mut map = HashMap::new();

        for light in &self.lights {
            for request_ip in &light.request_ips {
                map.entry(request_ip.clone())
                    .or_insert_with(Vec::new)
                    .push(light.address.clone());
            }
        }

        map
    }
}

impl Default for LightCommandConfig {
    fn default() -> Self {
        Self {
            basic: BasicLightCommandConfig::default(),
            composite: CompositeLightCommandConfig::default(),
            timing: LightTimingConfig::default(),
        }
    }
}

impl Default for BasicLightCommandConfig {
    fn default() -> Self {
        Self {
            green_light_off: default_green_light_off(),
            green_light_on: default_green_light_on(),
            red_light_on: default_red_light_on(),
            red_light_off: default_red_light_off(),
        }
    }
}

impl Default for CompositeLightCommandConfig {
    fn default() -> Self {
        Self {
            red: default_red_sequence(),
            green: default_green_sequence(),
            red_flash: default_red_flash_sequence(),
        }
    }
}

impl Default for LightTimingConfig {
    fn default() -> Self {
        Self {
            io_timeout_ms: default_io_timeout_ms(),
        }
    }
}

fn default_green_light_off() -> String {
    "01 05 00 00 FF 00 8C 3A".to_string()
}

fn default_green_light_on() -> String {
    "01 05 00 00 00 00 CD CA".to_string()
}

fn default_red_light_on() -> String {
    "01 05 00 02 FF 00 2D FA".to_string()
}

fn default_red_light_off() -> String {
    "01 05 00 02 00 00 6C 0A".to_string()
}

fn default_red_sequence() -> Vec<CompositeItemConfig> {
    vec![
        CompositeItemConfig::command("green_light_off"),
        CompositeItemConfig::command("red_light_on"),
    ]
}

fn default_green_sequence() -> Vec<CompositeItemConfig> {
    vec![
        CompositeItemConfig::command("red_light_off"),
        CompositeItemConfig::command("green_light_on"),
    ]
}

fn default_red_flash_sequence() -> Vec<CompositeItemConfig> {
    vec![
        CompositeItemConfig::Repeat {
            repeat: 5,
            steps: vec![
                CompositeStepConfig {
                    command: "red_light_on".to_string(),
                    delay_ms: Some(500),
                },
                CompositeStepConfig {
                    command: "red_light_off".to_string(),
                    delay_ms: Some(200),
                },
            ],
        },
        CompositeItemConfig::command("red_light_off"),
        CompositeItemConfig::command("green_light_on"),
    ]
}

fn default_io_timeout_ms() -> u64 {
    1000
}

impl CompositeItemConfig {
    fn command(command: impl Into<String>) -> Self {
        Self::Command {
            command: command.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Light, LightCommandConfig, LightConfig};

    #[test]
    fn request_ip_map_is_built_from_each_light() {
        let config = LightConfig {
            lights: vec![
                Light {
                    address: "192.168.70.151:502".to_string(),
                    request_ips: vec!["::1".to_string(), "192.168.70.166".to_string()],
                },
                Light {
                    address: "192.168.70.153:502".to_string(),
                    request_ips: vec!["192.168.70.166".to_string()],
                },
            ],
            commands: LightCommandConfig::default(),
        };

        let map = config.request_ip_map();

        assert_eq!(
            map.get("::1"),
            Some(&vec!["192.168.70.151:502".to_string()])
        );
        assert_eq!(
            map.get("192.168.70.166"),
            Some(&vec![
                "192.168.70.151:502".to_string(),
                "192.168.70.153:502".to_string()
            ])
        );
    }
}
