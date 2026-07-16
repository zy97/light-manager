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

#[derive(Debug, Deserialize, Clone)]
pub struct Light {
    pub address: String,
    #[serde(default)]
    pub request_ips: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LightConfig {
    #[serde(default)]
    pub lights: Vec<Light>,
}

#[derive(Debug, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub server: ServerConfig,
    pub light: LightConfig,
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
    use super::{Light, LightConfig};

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
