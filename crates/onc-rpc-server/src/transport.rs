use crate::{AsyncServer, Server};
use bytes::BytesMut;
use onc_rpc_runtime::{RuntimeError, WireError, try_decode_message_from_buffer, write_rpc_message};
use std::sync::Arc;
use thiserror::Error;
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

#[derive(Debug, Error)]
pub enum ServerTransportError {
    #[error("i/o failure: {0}")]
    Io(String),
    #[error("wire failure: {0}")]
    Wire(WireError),
    #[error("server dispatch failure: {0}")]
    Server(#[from] crate::ServerError),
    #[error("runtime transport failure: {0}")]
    Runtime(#[from] RuntimeError),
}

pub struct TokioServerTransport {
    listener: TcpListener,
    server: Arc<Server>,
}

pub struct TokioAsyncServerTransport {
    listener: TcpListener,
    server: Arc<AsyncServer>,
}

impl TokioServerTransport {
    pub async fn bind(server: Server) -> Result<Self, ServerTransportError> {
        let listener = TcpListener::bind(server.config().bind_addr)
            .await
            .map_err(|err| ServerTransportError::Io(err.to_string()))?;
        Ok(Self {
            listener,
            server: Arc::new(server),
        })
    }

    pub fn local_addr(&self) -> Result<std::net::SocketAddr, ServerTransportError> {
        self.listener
            .local_addr()
            .map_err(|err| ServerTransportError::Io(err.to_string()))
    }

    pub async fn accept_once(&self) -> Result<(), ServerTransportError> {
        let (stream, _) = self
            .listener
            .accept()
            .await
            .map_err(|err| ServerTransportError::Io(err.to_string()))?;
        serve_sync_connection(stream, self.server.clone()).await
    }

    pub async fn serve(self) -> Result<(), ServerTransportError> {
        loop {
            let (stream, _) = self
                .listener
                .accept()
                .await
                .map_err(|err| ServerTransportError::Io(err.to_string()))?;
            let server = self.server.clone();
            tokio::spawn(async move {
                let _ = serve_sync_connection(stream, server).await;
            });
        }
    }
}

impl TokioAsyncServerTransport {
    pub async fn bind(server: AsyncServer) -> Result<Self, ServerTransportError> {
        let listener = TcpListener::bind(server.config().bind_addr)
            .await
            .map_err(|err| ServerTransportError::Io(err.to_string()))?;
        Ok(Self {
            listener,
            server: Arc::new(server),
        })
    }

    pub fn local_addr(&self) -> Result<std::net::SocketAddr, ServerTransportError> {
        self.listener
            .local_addr()
            .map_err(|err| ServerTransportError::Io(err.to_string()))
    }

    pub async fn accept_once(&self) -> Result<(), ServerTransportError> {
        let (stream, _) = self
            .listener
            .accept()
            .await
            .map_err(|err| ServerTransportError::Io(err.to_string()))?;
        serve_async_connection(stream, self.server.clone()).await
    }

    pub async fn serve(self) -> Result<(), ServerTransportError> {
        loop {
            let (stream, _) = self
                .listener
                .accept()
                .await
                .map_err(|err| ServerTransportError::Io(err.to_string()))?;
            let server = self.server.clone();
            tokio::spawn(async move {
                let _ = serve_async_connection(stream, server).await;
            });
        }
    }
}

async fn serve_sync_connection(
    stream: TcpStream,
    server: Arc<Server>,
) -> Result<(), ServerTransportError> {
    let (mut reader, writer) = stream.into_split();
    let writer = Arc::new(Mutex::new(writer));
    let mut buffer = BytesMut::with_capacity(8192);

    loop {
        while let Some(message) =
            try_decode_message_from_buffer(&mut buffer).map_err(map_runtime_error)?
        {
            let reply = server.handle_message(message)?;
            let writer = writer.clone();
            tokio::spawn(async move {
                let mut writer = writer.lock().await;
                let _ = write_rpc_message(&mut *writer, &reply).await;
            });
        }

        let read = reader
            .read_buf(&mut buffer)
            .await
            .map_err(|err| ServerTransportError::Io(err.to_string()))?;
        if read == 0 {
            return Ok(());
        }
    }
}

async fn serve_async_connection(
    stream: TcpStream,
    server: Arc<AsyncServer>,
) -> Result<(), ServerTransportError> {
    let (mut reader, writer) = stream.into_split();
    let writer = Arc::new(Mutex::new(writer));
    let mut buffer = BytesMut::with_capacity(8192);

    loop {
        while let Some(message) =
            try_decode_message_from_buffer(&mut buffer).map_err(map_runtime_error)?
        {
            let server = server.clone();
            let writer = writer.clone();
            tokio::spawn(async move {
                if let Ok(reply) = server.handle_message(message).await {
                    let mut writer = writer.lock().await;
                    let _ = write_rpc_message(&mut *writer, &reply).await;
                }
            });
        }

        let read = reader
            .read_buf(&mut buffer)
            .await
            .map_err(|err| ServerTransportError::Io(err.to_string()))?;
        if read == 0 {
            return Ok(());
        }
    }
}

fn map_runtime_error(error: RuntimeError) -> ServerTransportError {
    match error {
        RuntimeError::Wire(err) => ServerTransportError::Wire(err),
        other => ServerTransportError::Runtime(other),
    }
}
