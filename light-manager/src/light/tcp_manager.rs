use deadpool::managed::{self, RecycleError};
use std::fmt;
use std::{net::SocketAddr, time::Duration};
use tokio::{
    io::AsyncWriteExt,
    net::TcpStream,
    time::{sleep, timeout},
};
use tracing::{error, info};

pub type Pool = managed::Pool<TcpManager>;

const CONNECT_TIMEOUT: Duration = Duration::from_millis(1000);
const RECONNECT_DELAY: Duration = Duration::from_millis(500);
const MAX_CONNECT_ATTEMPTS: usize = 3;

#[derive(Clone, Debug)]
pub struct TcpManager {
    pub addr: String,
}

pub struct ManagedConnection {
    pub addr: String,
    pub tcp_stream: TcpStream,
    pub status: bool,
    disconnected_logged: bool,
}

#[derive(Debug)]
pub enum Error {
    InvalidAddress(String),
    ConnectFailed(String, String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidAddress(addr) => write!(f, "invalid socket address: {addr}"),
            Error::ConnectFailed(addr, err) => {
                write!(f, "failed to connect socket address {addr}: {err}")
            }
        }
    }
}

impl std::error::Error for Error {}

impl managed::Manager for TcpManager {
    type Type = ManagedConnection;
    type Error = Error;

    async fn create(&self) -> Result<ManagedConnection, Error> {
        let socket_addr = self.parse_socket_addr()?;

        let mut last_error = None;
        for attempt in 1..=MAX_CONNECT_ATTEMPTS {
            match timeout(CONNECT_TIMEOUT, TcpStream::connect(socket_addr)).await {
                Ok(Ok(tcp_stream)) => {
                    info!(
                        light_addr = %self.addr,
                        "灯控TCP连接成功 light_addr={}",
                        self.addr
                    );
                    return Ok(ManagedConnection {
                        addr: self.addr.clone(),
                        tcp_stream,
                        status: true,
                        disconnected_logged: false,
                    });
                }
                Ok(Err(err)) => {
                    let message = err.to_string();
                    error!(
                        light_addr = %self.addr,
                        attempt,
                        max_attempts = MAX_CONNECT_ATTEMPTS,
                        error = %message,
                        "灯控TCP连接失败 light_addr={} attempt={} max_attempts={} error={}",
                        self.addr,
                        attempt,
                        MAX_CONNECT_ATTEMPTS,
                        message
                    );
                    last_error = Some(message);
                }
                Err(_) => {
                    error!(
                        light_addr = %self.addr,
                        attempt,
                        max_attempts = MAX_CONNECT_ATTEMPTS,
                        "灯控TCP连接超时 light_addr={} attempt={} max_attempts={}",
                        self.addr,
                        attempt,
                        MAX_CONNECT_ATTEMPTS
                    );
                    last_error = Some("timeout".to_string());
                }
            }

            if attempt < MAX_CONNECT_ATTEMPTS {
                sleep(RECONNECT_DELAY).await;
            }
        }

        let last_error = last_error.unwrap_or_else(|| "unknown error".to_string());
        error!(
            light_addr = %self.addr,
            max_attempts = MAX_CONNECT_ATTEMPTS,
            error = %last_error,
            "灯控TCP重试后仍连接失败 light_addr={} max_attempts={} error={}",
            self.addr,
            MAX_CONNECT_ATTEMPTS,
            last_error
        );

        Err(Error::ConnectFailed(self.addr.clone(), last_error))
    }

    async fn recycle(
        &self,
        conn: &mut ManagedConnection,
        _: &managed::Metrics,
    ) -> managed::RecycleResult<Error> {
        if conn.status {
            Ok(())
        } else {
            conn.log_disconnect("unhealthy");
            if let Err(err) = conn.tcp_stream.shutdown().await {
                error!(
                    light_addr = %self.addr,
                    error = ?err,
                    "关闭tcp连接失败 light_addr={} error={:?}",
                    self.addr,
                    err
                );
            }
            Err(RecycleError::Message("can't recycle".into()))
        }
    }

    fn detach(&self, obj: &mut Self::Type) {
        obj.log_disconnect("detached");
    }
}

impl TcpManager {
    fn parse_socket_addr(&self) -> Result<SocketAddr, Error> {
        self.addr
            .parse::<SocketAddr>()
            .map_err(|_| Error::InvalidAddress(self.addr.clone()))
    }
}

impl ManagedConnection {
    fn log_disconnect(&mut self, reason: &'static str) {
        if !self.disconnected_logged {
            info!(
                light_addr = %self.addr,
                reason,
                "灯控TCP连接已断开 light_addr={} reason={}",
                self.addr,
                reason
            );
            self.disconnected_logged = true;
        }
    }
}

impl Drop for ManagedConnection {
    fn drop(&mut self) {
        self.log_disconnect("dropped");
    }
}

#[cfg(test)]
mod tests {
    use super::{Error, TcpManager};
    use deadpool::managed::Manager;
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn create_returns_error_after_connect_attempts_fail() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);

        let manager = TcpManager {
            addr: addr.to_string(),
        };

        let err = match manager.create().await {
            Ok(_) => panic!("expected connect failure"),
            Err(err) => err,
        };

        assert!(matches!(err, Error::ConnectFailed(_, _)));
    }
}
