use crate::{
    app_config::{APP_CONFIG, Light, LightConfig},
    tcp_manager::{Pool, TcpManager},
};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    signal,
    time::{sleep, timeout},
};
use tracing::{debug, info, warn};

const GREEN_LIGHT_OFF: &[u8] = &[0x01, 0x05, 0x00, 0x00, 0xFF, 0x00, 0x8C, 0x3A];
const GREEN_LIGHT_ON: &[u8] = &[0x01, 0x05, 0x00, 0x00, 0x00, 0x00, 0xCD, 0xCA];
const RED_LIGHT_ON: &[u8] = &[0x01, 0x05, 0x00, 0x02, 0xFF, 0x00, 0x2D, 0xFA];
const RED_LIGHT_OFF: &[u8] = &[0x01, 0x05, 0x00, 0x02, 0x00, 0x00, 0x6C, 0x0A];
const READ_STATUS: &[u8] = &[0x01, 0x01, 0x00, 0x00, 0x00, 0x08, 0x3D, 0xCC];

const IO_TIMEOUT: Duration = Duration::from_millis(1000);
const RED_FLASH_ON_DELAY: Duration = Duration::from_millis(500);
const RED_FLASH_OFF_DELAY: Duration = Duration::from_millis(200);
const RED_FLASH_TIMES: usize = 5;

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
    runtimes: HashMap<String, LightRuntime>,
    request_ip_map: HashMap<String, Vec<String>>,
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
            runtimes,
            request_ip_map: config.request_ip_map(),
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
            send_status(runtime, status).await
        } else {
            let runtime = LightRuntime {
                address: address.clone(),
                pool: build_light_pool(&address),
            };
            send_status(&runtime, status).await
        }
    }

    pub async fn red_status_by_request_ip(&self, request_ip: &str) -> Result<bool, LightError> {
        let light_addresses = self
            .request_ip_map
            .get(request_ip)
            .ok_or_else(|| LightError::UnknownRequestIp(request_ip.to_string()))?;

        if light_addresses.len() != 1 {
            return Err(LightError::MultipleLightsForRequestIp(
                request_ip.to_string(),
            ));
        }

        self.red_status(&light_addresses[0]).await
    }

    pub async fn red_status(&self, light_ip: &str) -> Result<bool, LightError> {
        let address = light_ip.to_string();

        if let Some(runtime) = self.runtimes.get(&address) {
            read_red_status(runtime).await
        } else {
            let runtime = LightRuntime {
                address: address.clone(),
                pool: build_light_pool(&address),
            };
            read_red_status(&runtime).await
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

async fn send_status(runtime: &LightRuntime, status: LightStatus) -> Result<(), LightError> {
    match status {
        LightStatus::Red => red_light_on(runtime).await,
        LightStatus::Green => green_light_on(runtime).await,
        LightStatus::RedFlash => red_light_flash(runtime).await,
    }
}

async fn red_light_on(runtime: &LightRuntime) -> Result<(), LightError> {
    send(runtime, GREEN_LIGHT_OFF).await?;
    send(runtime, RED_LIGHT_ON).await
}

async fn green_light_on(runtime: &LightRuntime) -> Result<(), LightError> {
    send(runtime, RED_LIGHT_OFF).await?;
    send(runtime, GREEN_LIGHT_ON).await
}

async fn red_light_flash(runtime: &LightRuntime) -> Result<(), LightError> {
    send(runtime, GREEN_LIGHT_OFF).await?;

    for _ in 0..RED_FLASH_TIMES {
        send(runtime, RED_LIGHT_ON).await?;
        sleep(RED_FLASH_ON_DELAY).await;
        send(runtime, RED_LIGHT_OFF).await?;
        sleep(RED_FLASH_OFF_DELAY).await;
    }

    green_light_on(runtime).await
}

async fn send(runtime: &LightRuntime, command: &[u8]) -> Result<(), LightError> {
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

    match timeout(IO_TIMEOUT, connection.tcp_stream.write_all(command)).await {
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

async fn read_red_status(runtime: &LightRuntime) -> Result<bool, LightError> {
    let mut connection = runtime
        .pool
        .get()
        .await
        .map_err(|err| LightError::ConnectionPool(runtime.address.clone(), err.to_string()))?;

    timeout(IO_TIMEOUT, connection.tcp_stream.write_all(READ_STATUS))
        .await
        .map_err(|_| LightError::Timeout(runtime.address.clone()))?
        .map_err(|err| {
            connection.status = false;
            LightError::Io(runtime.address.clone(), err)
        })?;

    let mut buffer = [0; 1024];
    let read_len = timeout(IO_TIMEOUT, connection.tcp_stream.read(&mut buffer))
        .await
        .map_err(|_| LightError::Timeout(runtime.address.clone()))?
        .map_err(|err| {
            connection.status = false;
            LightError::Io(runtime.address.clone(), err)
        })?;

    Ok(read_len > 3 && buffer[3] == 15)
}

fn format_command(command: &[u8]) -> String {
    command
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
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
    info!(
        light_count = service.configured_light_count(),
        mapped_request_ip_count = service.request_ip_map.len(),
        "light manager started"
    );

    if service.configured_light_count() == 0 {
        warn!("未配置固定灯控设备，仍可通过传入灯 IP 动态发送命令");
    }
}

#[derive(Debug)]
pub enum LightError {
    UnknownRequestIp(String),
    MultipleLightsForRequestIp(String),
    ConnectionPool(String, String),
    Io(String, std::io::Error),
    Timeout(String),
}

impl std::fmt::Display for LightError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LightError::UnknownRequestIp(ip) => write!(f, "unknown request ip: {ip}"),
            LightError::MultipleLightsForRequestIp(ip) => {
                write!(
                    f,
                    "request ip maps to multiple lights, light_ip is required: {ip}"
                )
            }
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
        GREEN_LIGHT_OFF, GREEN_LIGHT_ON, LightRuntime, LightService, LightStatus, RED_FLASH_TIMES,
        RED_LIGHT_OFF, RED_LIGHT_ON, build_light_pool, build_light_runtime, format_command,
        read_red_status, send_status,
    };
    use crate::app_config::{Light, LightConfig};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        sync::mpsc,
    };

    #[test]
    fn formats_command_as_hex_bytes() {
        assert_eq!(format_command(GREEN_LIGHT_OFF), "01 05 00 00 FF 00 8C 3A");
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

    #[tokio::test]
    async fn red_status_reads_fourth_byte_like_csharp() {
        let (runtime, mut received) = spawn_light_server(&[&[0x01, 0x02, 0x03, 0x0F]]).await;

        assert!(read_red_status(&runtime).await.unwrap());
        assert_eq!(received.recv().await.unwrap(), super::READ_STATUS);
    }

    #[tokio::test]
    async fn sends_red_light_commands_in_order() {
        let (runtime, mut received) = spawn_light_server(&[&[], &[]]).await;

        send_status(&runtime, LightStatus::Red).await.unwrap();

        assert_eq!(received.recv().await.unwrap(), GREEN_LIGHT_OFF);
        assert_eq!(received.recv().await.unwrap(), RED_LIGHT_ON);
    }

    #[tokio::test]
    async fn sends_green_light_commands_in_order() {
        let (runtime, mut received) = spawn_light_server(&[&[], &[]]).await;

        send_status(&runtime, LightStatus::Green).await.unwrap();

        assert_eq!(received.recv().await.unwrap(), RED_LIGHT_OFF);
        assert_eq!(received.recv().await.unwrap(), GREEN_LIGHT_ON);
    }

    #[tokio::test]
    async fn sends_red_flash_sequence_then_green() {
        let responses = vec![&[][..]; 3 + RED_FLASH_TIMES * 2];
        let (runtime, mut received) = spawn_light_server(&responses).await;

        send_status(&runtime, LightStatus::RedFlash).await.unwrap();

        assert_eq!(received.recv().await.unwrap(), GREEN_LIGHT_OFF);
        for _ in 0..RED_FLASH_TIMES {
            assert_eq!(received.recv().await.unwrap(), RED_LIGHT_ON);
            assert_eq!(received.recv().await.unwrap(), RED_LIGHT_OFF);
        }
        assert_eq!(received.recv().await.unwrap(), RED_LIGHT_OFF);
        assert_eq!(received.recv().await.unwrap(), GREEN_LIGHT_ON);
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
        };
        let service = LightService::new(&config);

        service
            .light_by_request_ip(LightStatus::Green, "127.0.0.1")
            .await
            .unwrap();

        assert_eq!(received_a.recv().await.unwrap(), RED_LIGHT_OFF);
        assert_eq!(received_a.recv().await.unwrap(), GREEN_LIGHT_ON);
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
}
