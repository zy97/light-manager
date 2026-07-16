use deadpool::managed::{self, RecycleError};
use std::fmt;
use std::{net::SocketAddr, time::Duration};
use tokio::{
    io::AsyncWriteExt,
    net::TcpStream,
    time::{sleep, timeout},
};
use tracing::{debug, error};

pub type Pool = managed::Pool<TcpManager>;

const CONNECT_TIMEOUT: Duration = Duration::from_millis(1000);
const RECONNECT_DELAY: Duration = Duration::from_millis(500);

#[derive(Clone, Debug)]
pub struct TcpManager {
    pub addr: String,
}

pub struct ManagedConnection {
    pub tcp_stream: TcpStream,
    pub status: bool,
}

#[derive(Debug)]
pub enum Error {
    InvalidAddress(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidAddress(addr) => write!(f, "invalid socket address: {addr}"),
        }
    }
}

impl std::error::Error for Error {}

impl managed::Manager for TcpManager {
    type Type = ManagedConnection;
    type Error = Error;

    async fn create(&self) -> Result<ManagedConnection, Error> {
        let socket_addr = self.parse_socket_addr()?;

        loop {
            match timeout(CONNECT_TIMEOUT, TcpStream::connect(socket_addr)).await {
                Ok(Ok(tcp_stream)) => {
                    debug!("连接tcp:{},成功", self.addr);
                    return Ok(ManagedConnection {
                        tcp_stream,
                        status: true,
                    });
                }
                Ok(Err(err)) => {
                    error!("连接tcp:{}，失败: {:?}", self.addr, err);
                }
                Err(_) => {
                    error!("连接tcp:{}，超时", self.addr);
                }
            }

            sleep(RECONNECT_DELAY).await;
        }
    }

    async fn recycle(
        &self,
        conn: &mut ManagedConnection,
        _: &managed::Metrics,
    ) -> managed::RecycleResult<Error> {
        if conn.status {
            Ok(())
        } else {
            if let Err(err) = conn.tcp_stream.shutdown().await {
                error!("关闭tcp连接失败:{} {:?}", self.addr, err);
            }
            debug!("断开连接成功！");
            Err(RecycleError::Message("can't recycle".into()))
        }
    }

    fn detach(&self, _obj: &mut Self::Type) {}
}

impl TcpManager {
    fn parse_socket_addr(&self) -> Result<SocketAddr, Error> {
        self.addr
            .parse::<SocketAddr>()
            .map_err(|_| Error::InvalidAddress(self.addr.clone()))
    }
}
