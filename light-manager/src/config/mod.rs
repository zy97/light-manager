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
    pub commands: LightCommandConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LightCommandConfig {
    pub basic: BasicLightCommandConfig,
    pub composite: CompositeLightCommandConfig,
    pub timing: LightTimingConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct BasicLightCommandConfig {
    pub green_light_off: String,
    pub green_light_on: String,
    pub red_light_on: String,
    pub red_light_off: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct CompositeLightCommandConfig {
    pub red: Vec<CompositeItemConfig>,
    pub green: Vec<CompositeItemConfig>,
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

#[cfg(test)]
mod tests {
    use super::{
        BasicLightCommandConfig, CompositeItemConfig, CompositeLightCommandConfig,
        LightCommandConfig, LightConfig, LightTimingConfig,
    };

    #[test]
    fn request_ip_map_is_built_from_each_light() {
        let config = LightConfig {
            lights: vec![
                super::Light {
                    address: "192.168.70.151:502".to_string(),
                    request_ips: vec!["::1".to_string(), "192.168.70.166".to_string()],
                },
                super::Light {
                    address: "192.168.70.153:502".to_string(),
                    request_ips: vec!["192.168.70.166".to_string()],
                },
            ],
            commands: test_command_config(),
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

    fn test_command_config() -> LightCommandConfig {
        LightCommandConfig {
            basic: BasicLightCommandConfig {
                green_light_off: "01 05 00 00 FF 00 8C 3A".to_string(),
                green_light_on: "01 05 00 00 00 00 CD CA".to_string(),
                red_light_on: "01 05 00 02 FF 00 2D FA".to_string(),
                red_light_off: "01 05 00 02 00 00 6C 0A".to_string(),
            },
            composite: CompositeLightCommandConfig {
                red: vec![command("green_light_off"), command("red_light_on")],
                green: vec![command("red_light_off"), command("green_light_on")],
                red_flash: vec![
                    CompositeItemConfig::Repeat {
                        repeat: 5,
                        steps: vec![],
                    },
                    command("red_light_off"),
                    command("green_light_on"),
                ],
            },
            timing: LightTimingConfig {
                io_timeout_ms: 1000,
            },
        }
    }

    fn command(command: &str) -> CompositeItemConfig {
        CompositeItemConfig::Command {
            command: command.to_string(),
        }
    }
}
