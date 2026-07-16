use crate::{
    config::{APP_CONFIG, Light, LightConfig},
    light::protocol::{LightProtocol, LightStatus, RuntimeStep, format_command},
    light::tcp_manager::{Pool, TcpManager},
};
use std::{collections::HashMap, time::Duration};
use tokio::{
    io::AsyncWriteExt,
    time::{sleep, timeout},
};
use tracing::{debug, info, warn};

#[derive(Clone)]
pub struct LightRuntime {
    pub address: String,
    pub pool: Pool,
}

pub struct LightService {
    lights: Vec<Light>,
    runtimes: HashMap<String, LightRuntime>,
    request_ip_map: HashMap<String, Vec<String>>,
    protocol: LightProtocol,
}

pub fn build_light_service() -> LightService {
    LightService::new(&APP_CONFIG.light)
}

impl LightService {
    pub fn new(config: &LightConfig) -> Self {
        let runtimes = config
            .lights
            .iter()
            .map(|light| {
                let runtime = build_light_runtime(light);
                (runtime.address.clone(), runtime)
            })
            .collect();

        Self {
            lights: config.lights.clone(),
            runtimes,
            request_ip_map: config.request_ip_map(),
            protocol: LightProtocol::from_config(&config.commands)
                .expect("invalid light command config"),
        }
    }

    pub async fn light_by_request_ip(
        &self,
        status: LightStatus,
        request_ip: &str,
    ) -> Result<(), LightError> {
        let light_addresses = self
            .request_ip_map
            .get(request_ip)
            .ok_or_else(|| LightError::UnknownRequestIp(request_ip.to_string()))?;

        self.light(status, &light_addresses[0]).await
    }

    pub async fn light(&self, status: LightStatus, light_ip: &str) -> Result<(), LightError> {
        let address = light_ip.to_string();

        if let Some(runtime) = self.runtimes.get(&address) {
            send_status(runtime, &self.protocol, status).await
        } else {
            let runtime = LightRuntime {
                address: address.clone(),
                pool: build_light_pool(&address),
            };
            send_status(&runtime, &self.protocol, status).await
        }
    }

    pub fn configured_light_count(&self) -> usize {
        self.runtimes.len()
    }
}

fn build_light_runtime(light: &Light) -> LightRuntime {
    let address = light.address.clone();
    LightRuntime {
        pool: build_light_pool(&address),
        address,
    }
}

fn build_light_pool(address: &str) -> Pool {
    Pool::builder(TcpManager {
        addr: address.to_string(),
    })
    .max_size(1)
    .build()
    .expect("failed to build light pool")
}

async fn send_status(
    runtime: &LightRuntime,
    protocol: &LightProtocol,
    status: LightStatus,
) -> Result<(), LightError> {
    match status {
        LightStatus::Red => red_light_on(runtime, protocol).await,
        LightStatus::Green => green_light_on(runtime, protocol).await,
        LightStatus::RedFlash => red_light_flash(runtime, protocol).await,
    }
}

async fn red_light_on(runtime: &LightRuntime, protocol: &LightProtocol) -> Result<(), LightError> {
    send_sequence(runtime, &protocol.red, protocol.io_timeout).await
}

async fn green_light_on(
    runtime: &LightRuntime,
    protocol: &LightProtocol,
) -> Result<(), LightError> {
    send_sequence(runtime, &protocol.green, protocol.io_timeout).await
}

async fn red_light_flash(
    runtime: &LightRuntime,
    protocol: &LightProtocol,
) -> Result<(), LightError> {
    send_runtime_steps(runtime, &protocol.red_flash, protocol.io_timeout).await
}

async fn send_sequence(
    runtime: &LightRuntime,
    commands: &[Vec<u8>],
    io_timeout: Duration,
) -> Result<(), LightError> {
    for command in commands {
        send(runtime, command, io_timeout).await?;
    }

    Ok(())
}

async fn send_runtime_steps(
    runtime: &LightRuntime,
    steps: &[RuntimeStep],
    io_timeout: Duration,
) -> Result<(), LightError> {
    for step in steps {
        match step {
            RuntimeStep::Command(command) => {
                send(runtime, command, io_timeout).await?;
            }
            RuntimeStep::Repeat { repeat, steps } => {
                for _ in 0..*repeat {
                    for step in steps {
                        send(runtime, &step.command, io_timeout).await?;
                        if let Some(delay) = step.delay {
                            sleep(delay).await;
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

async fn send(
    runtime: &LightRuntime,
    command: &[u8],
    io_timeout: Duration,
) -> Result<(), LightError> {
    let mut connection = runtime
        .pool
        .get()
        .await
        .map_err(|err| LightError::ConnectionPool(runtime.address.clone(), err.to_string()))?;

    debug!(
        light_addr = %runtime.address,
        command = %format_command(command),
        "正在发送灯控命令 light_addr={} command={}",
        runtime.address,
        format_command(command)
    );

    match timeout(io_timeout, connection.tcp_stream.write_all(command)).await {
        Ok(Ok(())) => {
            if let Err(err) = connection.tcp_stream.flush().await {
                connection.status = false;
                return Err(LightError::Io(runtime.address.clone(), err));
            }

            info!(
                light_addr = %runtime.address,
                command = %format_command(command),
                "灯控命令发送成功 light_addr={} command={}",
                runtime.address,
                format_command(command)
            );
            Ok(())
        }
        Ok(Err(err)) => {
            connection.status = false;
            Err(LightError::Io(runtime.address.clone(), err))
        }
        Err(_) => {
            connection.status = false;
            Err(LightError::Timeout(runtime.address.clone()))
        }
    }
}

pub fn log_startup(service: &LightService) {
    info!(
        lights = ?service.lights,
        "灯控管理器已启动 lights={:?}",
        service.lights
    );

    if service.configured_light_count() == 0 {
        warn!("未配置固定灯控设备，仍可通过传入灯 IP 动态发送命令");
    }
}

#[derive(Debug)]
pub enum LightError {
    UnknownRequestIp(String),
    ConnectionPool(String, String),
    Io(String, std::io::Error),
    Timeout(String),
}

impl std::fmt::Display for LightError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LightError::UnknownRequestIp(ip) => write!(f, "unknown request ip: {ip}"),
            LightError::ConnectionPool(addr, err) => {
                write!(f, "failed to acquire light connection {addr}: {err}")
            }
            LightError::Io(addr, err) => write!(f, "light tcp io error {addr}: {err}"),
            LightError::Timeout(addr) => write!(f, "light tcp operation timeout: {addr}"),
        }
    }
}

impl std::error::Error for LightError {}

#[cfg(test)]
mod tests {
    use super::{LightRuntime, LightService, build_light_pool, build_light_runtime, send_status};
    use crate::config::{
        BasicLightCommandConfig, CompositeItemConfig, CompositeLightCommandConfig,
        CompositeStepConfig, Light, LightCommandConfig, LightConfig, LightTimingConfig,
    };
    use crate::light::protocol::{LightProtocol, LightStatus};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        sync::mpsc,
    };

    #[test]
    fn request_ip_map_matches_light_scoped_config() {
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
            commands: test_command_config(),
        };

        let service = LightService::new(&config);

        assert_eq!(
            service.request_ip_map.get("192.168.70.166"),
            Some(&vec![
                "192.168.70.151:502".to_string(),
                "192.168.70.153:502".to_string()
            ])
        );
        assert_eq!(
            service.request_ip_map.get("::1"),
            Some(&vec!["192.168.70.151:502".to_string()])
        );
    }

    #[test]
    fn startup_log_data_includes_lights() {
        let config = LightConfig {
            lights: vec![
                Light {
                    address: "192.168.70.153:502".to_string(),
                    request_ips: vec!["192.168.70.166".to_string()],
                },
                Light {
                    address: "192.168.70.151:502".to_string(),
                    request_ips: vec!["::1".to_string(), "192.168.70.166".to_string()],
                },
            ],
            commands: test_command_config(),
        };

        let service = LightService::new(&config);

        assert_eq!(service.lights, config.lights);
    }

    #[tokio::test]
    async fn sends_red_light_commands_in_order() {
        let protocol = default_protocol();
        let (runtime, mut received) = spawn_light_server(&[&[], &[]]).await;

        send_status(&runtime, &protocol, LightStatus::Red)
            .await
            .unwrap();

        assert_eq!(received.recv().await.unwrap(), protocol.red[0]);
        assert_eq!(received.recv().await.unwrap(), protocol.red[1]);
    }

    #[tokio::test]
    async fn sends_green_light_commands_in_order() {
        let protocol = default_protocol();
        let (runtime, mut received) = spawn_light_server(&[&[], &[]]).await;

        send_status(&runtime, &protocol, LightStatus::Green)
            .await
            .unwrap();

        assert_eq!(received.recv().await.unwrap(), protocol.green[0]);
        assert_eq!(received.recv().await.unwrap(), protocol.green[1]);
    }

    #[tokio::test]
    async fn sends_red_flash_sequence_then_green() {
        let protocol = default_protocol();
        let responses = vec![&[][..]; 12];
        let (runtime, mut received) = spawn_light_server(&responses).await;

        send_status(&runtime, &protocol, LightStatus::RedFlash)
            .await
            .unwrap();

        for _ in 0..5 {
            assert_eq!(received.recv().await.unwrap(), protocol.red[1]);
            assert_eq!(received.recv().await.unwrap(), protocol.green[0]);
        }
        assert_eq!(received.recv().await.unwrap(), protocol.green[0]);
        assert_eq!(received.recv().await.unwrap(), protocol.green[1]);
    }

    #[tokio::test]
    async fn light_by_request_ip_uses_first_mapped_light() {
        let (runtime_a, mut received_a) = spawn_light_server(&[&[], &[]]).await;
        let runtime_b = LightRuntime {
            address: "127.0.0.1:1".to_string(),
            pool: build_light_pool("127.0.0.1:1"),
        };

        let config = LightConfig {
            lights: vec![
                Light {
                    address: runtime_a.address.clone(),
                    request_ips: vec!["127.0.0.1".to_string()],
                },
                Light {
                    address: runtime_b.address.clone(),
                    request_ips: vec!["127.0.0.1".to_string()],
                },
            ],
            commands: test_command_config(),
        };
        let service = LightService::new(&config);

        service
            .light_by_request_ip(LightStatus::Green, "127.0.0.1")
            .await
            .unwrap();

        let protocol = default_protocol();
        assert_eq!(received_a.recv().await.unwrap(), protocol.green[0]);
        assert_eq!(received_a.recv().await.unwrap(), protocol.green[1]);
    }

    async fn spawn_light_server(
        responses: &[&[u8]],
    ) -> (super::LightRuntime, mpsc::Receiver<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let responses: Vec<Vec<u8>> = responses.iter().map(|response| response.to_vec()).collect();
        let (tx, rx) = mpsc::channel::<Vec<u8>>(responses.len().max(1));

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            for response in responses {
                let mut command = [0; 8];
                stream.read_exact(&mut command).await.unwrap();
                tx.send(command.to_vec()).await.unwrap();
                if !response.is_empty() {
                    stream.write_all(&response).await.unwrap();
                }
            }
        });

        let light = Light {
            address: addr.to_string(),
            request_ips: Vec::new(),
        };

        (build_light_runtime(&light), rx)
    }

    fn default_protocol() -> LightProtocol {
        LightProtocol::from_config(&test_command_config()).unwrap()
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
