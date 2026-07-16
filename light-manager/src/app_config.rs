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
pub struct Light {
    pub name: String,
    pub address: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RequestLightMap {
    pub request_ip: String,
    pub light_ip: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LightConfig {
    #[serde(default)]
    pub default_port: u16,
    #[serde(default)]
    pub lights: Vec<Light>,
    #[serde(default = "default_request_light_maps")]
    pub request_light_maps: Vec<RequestLightMap>,
}

#[derive(Debug, Deserialize)]
pub struct AppConfig {
    pub light: LightConfig,
}

pub fn load_config() -> Result<AppConfig, ConfigError> {
    let settings = Config::builder()
        .add_source(config::File::with_name("./config"))
        .build()?;
    settings.try_deserialize::<AppConfig>()
}

pub fn default_request_light_maps() -> Vec<RequestLightMap> {
    [
        ("::1", "192.168.70.151"),
        ("192.168.70.166", "192.168.70.151"),
        ("192.168.70.167", "192.168.70.153"),
        ("192.168.70.168", "192.168.70.155"),
        ("192.168.70.169", "192.168.70.157"),
    ]
    .into_iter()
    .map(|(request_ip, light_ip)| RequestLightMap {
        request_ip: request_ip.to_string(),
        light_ip: light_ip.to_string(),
    })
    .collect()
}

impl LightConfig {
    pub fn port(&self) -> u16 {
        if self.default_port == 0 {
            502
        } else {
            self.default_port
        }
    }

    pub fn request_ip_map(&self) -> HashMap<String, String> {
        self.request_light_maps
            .iter()
            .map(|item| (item.request_ip.clone(), item.light_ip.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::LightConfig;

    #[test]
    fn port_defaults_to_modbus_tcp_port() {
        let config = LightConfig {
            default_port: 0,
            lights: Vec::new(),
            request_light_maps: Vec::new(),
        };

        assert_eq!(config.port(), 502);
    }
}
