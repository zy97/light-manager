use crate::{
    app_config::{APP_CONFIG, Light, LightConfig},
    tcp_manager::{Pool, TcpManager},
};
use std::{collections::HashMap, net::SocketAddr, time::Duration};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LightStatus {
    Red,
    Green,
    RedFlash,
}

#[derive(Clone)]
pub struct LightRuntime {
    pub name: String,
    pub address: String,
    pub pool: Pool,
}

pub struct LightService {
    runtimes: HashMap<String, LightRuntime>,
    request_ip_map: HashMap<String, String>,
    port: u16,
}

pub fn build_light_service() -> LightService {
    LightService::new(&APP_CONFIG.light)
}

impl LightService {
    pub fn new(config: &LightConfig) -> Self {
        let port = config.port();
        let runtimes = config
            .lights
            .iter()
            .map(|light| {
                let runtime = build_light_runtime(light, port);
                (runtime.address.clone(), runtime)
            })
            .collect();

        Self {
            runtimes,
            request_ip_map: config.request_ip_map(),
            port,
        }
    }

    pub async fn light_by_request_ip(
        &self,
        status: LightStatus,
        request_ip: &str,
    ) -> Result<(), LightError> {
        let light_ip = self
            .request_ip_map
            .get(request_ip)
            .ok_or_else(|| LightError::UnknownRequestIp(request_ip.to_string()))?;

        self.light(status, light_ip).await
    }

    pub async fn light(&self, status: LightStatus, light_ip: &str) -> Result<(), LightError> {
        let address = normalize_light_address(light_ip, self.port);

        if let Some(runtime) = self.runtimes.get(&address) {
            send_status(runtime, status).await
        } else {
            let runtime = LightRuntime {
                name: light_ip.to_string(),
                address: address.clone(),
                pool: build_light_pool(&address),
            };
            send_status(&runtime, status).await
        }
    }

    pub async fn red_status_by_request_ip(&self, request_ip: &str) -> Result<bool, LightError> {
        let light_ip = self
            .request_ip_map
            .get(request_ip)
            .ok_or_else(|| LightError::UnknownRequestIp(request_ip.to_string()))?;

        self.red_status(light_ip).await
    }

    pub async fn red_status(&self, light_ip: &str) -> Result<bool, LightError> {
        let address = normalize_light_address(light_ip, self.port);

        if let Some(runtime) = self.runtimes.get(&address) {
            read_red_status(runtime).await
        } else {
            let runtime = LightRuntime {
                name: light_ip.to_string(),
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

fn build_light_runtime(light: &Light, port: u16) -> LightRuntime {
    let address = normalize_light_address(&light.address, port);
    LightRuntime {
        name: light.name.clone(),
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
        light_name = %runtime.name,
        light_addr = %runtime.address,
        command = ?command,
        "sending light command"
    );

    match timeout(IO_TIMEOUT, connection.tcp_stream.write_all(command)).await {
        Ok(Ok(())) => {
            if let Err(err) = connection.tcp_stream.flush().await {
                connection.status = false;
                return Err(LightError::Io(runtime.address.clone(), err));
            }

            info!(
                light_name = %runtime.name,
                light_addr = %runtime.address,
                command = ?command,
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

fn normalize_light_address(address: &str, default_port: u16) -> String {
    if address.parse::<SocketAddr>().is_ok() {
        address.to_string()
    } else {
        format!("{address}:{default_port}")
    }
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
                info!("{}:{}已关闭", runtime.name, runtime.address);
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
    use super::{
        GREEN_LIGHT_OFF, GREEN_LIGHT_ON, LightService, LightStatus, RED_FLASH_TIMES, RED_LIGHT_OFF,
        RED_LIGHT_ON, build_light_runtime, normalize_light_address, read_red_status, send_status,
    };
    use crate::app_config::{Light, LightConfig, RequestLightMap, default_request_light_maps};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        sync::mpsc,
    };

    #[test]
    fn normalize_light_address_appends_default_port_when_missing() {
        assert_eq!(
            normalize_light_address("192.168.70.151", 502),
            "192.168.70.151:502"
        );
        assert_eq!(
            normalize_light_address("192.168.70.151:1502", 502),
            "192.168.70.151:1502"
        );
    }

    #[test]
    fn default_request_ip_map_matches_csharp_mapping() {
        let config = LightConfig {
            default_port: 502,
            lights: Vec::new(),
            request_light_maps: default_request_light_maps(),
        };

        let service = LightService::new(&config);

        assert_eq!(
            service.request_ip_map.get("192.168.70.167"),
            Some(&"192.168.70.153".to_string())
        );
        assert_eq!(
            service.request_ip_map.get("::1"),
            Some(&"192.168.70.151".to_string())
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
    async fn light_by_request_ip_uses_configured_mapping() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(4);
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            for _ in 0..2 {
                let mut command = [0; 8];
                stream.read_exact(&mut command).await.unwrap();
                tx.send(command.to_vec()).await.unwrap();
            }
        });

        let config = LightConfig {
            default_port: 502,
            lights: vec![Light {
                name: "A".to_string(),
                address: addr.to_string(),
            }],
            request_light_maps: vec![RequestLightMap {
                request_ip: "127.0.0.1".to_string(),
                light_ip: addr.to_string(),
            }],
        };
        let service = LightService::new(&config);

        service
            .light_by_request_ip(LightStatus::Green, "127.0.0.1")
            .await
            .unwrap();

        assert_eq!(rx.recv().await.unwrap(), RED_LIGHT_OFF);
        assert_eq!(rx.recv().await.unwrap(), GREEN_LIGHT_ON);
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
            name: "test".to_string(),
            address: addr.to_string(),
        };

        (build_light_runtime(&light, 502), rx)
    }
}
