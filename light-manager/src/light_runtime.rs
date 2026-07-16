use crate::{
    app_config::{
        APP_CONFIG, BasicLightCommandConfig, CompositeItemConfig, Light, LightCommandConfig,
        LightConfig,
    },
    tcp_manager::{Pool, TcpManager},
};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, time::Duration};
use tokio::{
    io::AsyncWriteExt,
    signal,
    time::{sleep, timeout},
};
use tracing::{debug, info, warn};

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LightStatus {
    Red,
    Green,
    RedFlash,
}

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

#[derive(Clone)]
struct LightProtocol {
    red: Vec<Vec<u8>>,
    green: Vec<Vec<u8>>,
    red_flash: Vec<RuntimeStep>,
    io_timeout: Duration,
}

#[derive(Clone)]
enum RuntimeStep {
    Command(Vec<u8>),
    Repeat {
        repeat: usize,
        steps: Vec<RuntimeRepeatStep>,
    },
}

#[derive(Clone)]
struct RuntimeRepeatStep {
    command: Vec<u8>,
    delay: Option<Duration>,
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
        "sending light command"
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
                "light command sent"
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

fn format_command(command: &[u8]) -> String {
    command
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

impl LightProtocol {
    fn from_config(config: &LightCommandConfig) -> Result<Self, LightError> {
        let basic_commands = parse_basic_commands(&config.basic)?;

        Ok(Self {
            red: resolve_sequence(&config.composite.red, &basic_commands)?,
            green: resolve_sequence(&config.composite.green, &basic_commands)?,
            red_flash: resolve_runtime_steps(&config.composite.red_flash, &basic_commands)?,
            io_timeout: Duration::from_millis(config.timing.io_timeout_ms),
        })
    }
}

fn parse_basic_commands(
    config: &BasicLightCommandConfig,
) -> Result<HashMap<String, Vec<u8>>, LightError> {
    let mut commands = HashMap::new();
    commands.insert(
        "green_light_off".to_string(),
        parse_command(&config.green_light_off)?,
    );
    commands.insert(
        "green_light_on".to_string(),
        parse_command(&config.green_light_on)?,
    );
    commands.insert(
        "red_light_on".to_string(),
        parse_command(&config.red_light_on)?,
    );
    commands.insert(
        "red_light_off".to_string(),
        parse_command(&config.red_light_off)?,
    );
    Ok(commands)
}

fn resolve_sequence(
    command_names: &[CompositeItemConfig],
    basic_commands: &HashMap<String, Vec<u8>>,
) -> Result<Vec<Vec<u8>>, LightError> {
    command_names
        .iter()
        .map(|command_name| match command_name {
            CompositeItemConfig::Command { command } => resolve_command(command, basic_commands),
            CompositeItemConfig::Repeat { .. } => Err(LightError::InvalidCommand(
                "repeat blocks are not allowed in simple sequences".to_string(),
            )),
        })
        .collect()
}

fn resolve_runtime_steps(
    steps: &[CompositeItemConfig],
    basic_commands: &HashMap<String, Vec<u8>>,
) -> Result<Vec<RuntimeStep>, LightError> {
    steps
        .iter()
        .map(|step| match step {
            CompositeItemConfig::Command { command } => Ok(RuntimeStep::Command(resolve_command(
                command,
                basic_commands,
            )?)),
            CompositeItemConfig::Repeat { repeat, steps } => Ok(RuntimeStep::Repeat {
                repeat: *repeat,
                steps: steps
                    .iter()
                    .map(|step| {
                        Ok(RuntimeRepeatStep {
                            command: resolve_command(&step.command, basic_commands)?,
                            delay: step.delay_ms.map(Duration::from_millis),
                        })
                    })
                    .collect::<Result<Vec<_>, LightError>>()?,
            }),
        })
        .collect()
}

fn resolve_command(
    command_name: &str,
    basic_commands: &HashMap<String, Vec<u8>>,
) -> Result<Vec<u8>, LightError> {
    basic_commands
        .get(command_name)
        .cloned()
        .ok_or_else(|| LightError::UnknownCommand(command_name.to_string()))
}

fn parse_command(command: &str) -> Result<Vec<u8>, LightError> {
    let mut bytes = Vec::new();

    for part in command
        .split(|ch: char| ch.is_ascii_whitespace() || ch == ',' || ch == '-')
        .filter(|part| !part.is_empty())
    {
        let hex = part
            .strip_prefix("0x")
            .or_else(|| part.strip_prefix("0X"))
            .unwrap_or(part);

        if hex.len() > 2 {
            if hex.len() % 2 != 0 {
                return Err(LightError::InvalidCommand(command.to_string()));
            }

            for index in (0..hex.len()).step_by(2) {
                bytes.push(
                    u8::from_str_radix(&hex[index..index + 2], 16)
                        .map_err(|_| LightError::InvalidCommand(command.to_string()))?,
                );
            }
        } else {
            bytes.push(
                u8::from_str_radix(hex, 16)
                    .map_err(|_| LightError::InvalidCommand(command.to_string()))?,
            );
        }
    }

    if bytes.is_empty() {
        return Err(LightError::InvalidCommand(command.to_string()));
    }

    Ok(bytes)
}

pub async fn wait_for_shutdown(service: &LightService) {
    match signal::ctrl_c().await {
        Ok(()) => {
            info!(
                light_count = service.configured_light_count(),
                "shutdown signal received, closing light pools"
            );
            for runtime in service.runtimes.values() {
                runtime.pool.close();
                info!("{}已关闭", runtime.address);
            }
            info!(
                light_count = service.configured_light_count(),
                "light manager stopped"
            );
        }
        Err(err) => {
            eprintln!("Unable to listen for shutdown signal: {}", err);
        }
    }
}

pub fn log_startup(service: &LightService) {
    info!(lights = ?service.lights, "light manager started");

    if service.configured_light_count() == 0 {
        warn!("未配置固定灯控设备，仍可通过传入灯 IP 动态发送命令");
    }
}

#[derive(Debug)]
pub enum LightError {
    InvalidCommand(String),
    UnknownCommand(String),
    UnknownRequestIp(String),
    ConnectionPool(String, String),
    Io(String, std::io::Error),
    Timeout(String),
}

impl std::fmt::Display for LightError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LightError::InvalidCommand(command) => {
                write!(f, "invalid light command config: {command}")
            }
            LightError::UnknownCommand(command_name) => {
                write!(f, "unknown light command name: {command_name}")
            }
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
    use super::{
        LightProtocol, LightRuntime, LightService, LightStatus, build_light_pool,
        build_light_runtime, format_command, parse_command, send_status,
    };
    use crate::app_config::{Light, LightCommandConfig, LightConfig};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        sync::mpsc,
    };

    #[test]
    fn formats_command_as_hex_bytes() {
        assert_eq!(
            format_command(&[0x01, 0x05, 0x00, 0x00, 0xFF, 0x00, 0x8C, 0x3A]),
            "01 05 00 00 FF 00 8C 3A"
        );
    }

    #[test]
    fn parses_configured_command_formats() {
        assert_eq!(
            parse_command("01 05 00 00 FF 00 8C 3A").unwrap(),
            vec![0x01, 0x05, 0x00, 0x00, 0xFF, 0x00, 0x8C, 0x3A]
        );
        assert_eq!(
            parse_command("01050000FF008C3A").unwrap(),
            vec![0x01, 0x05, 0x00, 0x00, 0xFF, 0x00, 0x8C, 0x3A]
        );
    }

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
            commands: LightCommandConfig::default(),
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
            commands: LightCommandConfig::default(),
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
            commands: LightCommandConfig::default(),
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
        LightProtocol::from_config(&LightCommandConfig::default()).unwrap()
    }
}
