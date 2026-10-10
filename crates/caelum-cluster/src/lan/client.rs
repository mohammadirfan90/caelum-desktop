//! Client side of the crew LAN (used by the peer node for replication and by secondary stations):
//! mutual-TLS connection with server-certificate pinning, plain HTTP/1.1 requests and WebSockets.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::http::{header, HeaderValue, Method, Request, StatusCode};
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper_util::rt::TokioIo;
use rustls::pki_types::ServerName;
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;
use tokio_rustls::TlsConnector;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::WebSocketStream;

use super::server::ClusterStatus;
use super::{fingerprint, Fingerprint};
use crate::fence::Fence;
use crate::wire::MAX_FRAME;

/// Connection attempts give up after this long.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// A client failure.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// TCP or TLS failure.
    #[error("connect: {0}")]
    Connect(String),
    /// The server's certificate is not the pinned one.
    #[error("server certificate does not match the provisioned station")]
    PinMismatch,
    /// HTTP failure.
    #[error("http: {0}")]
    Http(String),
    /// WebSocket failure.
    #[error("websocket: {0}")]
    WebSocket(String),
}

/// A mutual-TLS connection to a LAN server.
pub type LanStream = TlsStream<TcpStream>;

/// Connects with mutual TLS and, if `pin` is given, checks the server certificate's fingerprint.
pub async fn connect(
    config: Arc<rustls::ClientConfig>,
    addr: SocketAddr,
    server_name: &str,
    pin: Option<&Fingerprint>,
) -> Result<LanStream, ClientError> {
    let name = ServerName::try_from(server_name.to_owned())
        .map_err(|e| ClientError::Connect(e.to_string()))?;
    let attempt = async {
        let tcp = TcpStream::connect(addr)
            .await
            .map_err(|e| ClientError::Connect(e.to_string()))?;
        let _ = tcp.set_nodelay(true);
        TlsConnector::from(config)
            .connect(name, tcp)
            .await
            .map_err(|e| ClientError::Connect(e.to_string()))
    };
    let stream = tokio::time::timeout(CONNECT_TIMEOUT, attempt)
        .await
        .map_err(|_| ClientError::Connect("timed out".into()))??;
    if let Some(pin) = pin {
        let presented = stream
            .get_ref()
            .1
            .peer_certificates()
            .and_then(|c| c.first())
            .map(|c| fingerprint(c));
        if presented.as_ref() != Some(pin) {
            return Err(ClientError::PinMismatch);
        }
    }
    Ok(stream)
}

/// One HTTP/1.1 request over `stream`; returns the status and body.
pub async fn request(
    stream: LanStream,
    method: Method,
    path: &str,
    token: Option<&str>,
    body: Option<Vec<u8>>,
) -> Result<(StatusCode, Vec<u8>), ClientError> {
    let http = |e: &dyn std::fmt::Display| ClientError::Http(e.to_string());
    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(|e| http(&e))?;
    let driver = tokio::spawn(conn);
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, "caelum");
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    if body.is_some() {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
    }
    let request = builder
        .body(Full::new(Bytes::from(body.unwrap_or_default())))
        .map_err(|e| http(&e))?;
    let response = sender.send_request(request).await.map_err(|e| http(&e))?;
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .map_err(|e| http(&e))?
        .to_bytes();
    driver.abort();
    Ok((status, bytes.to_vec()))
}

/// Opens a WebSocket at `path` over `stream`.
pub async fn websocket(
    stream: LanStream,
    path: &str,
    token: Option<&str>,
) -> Result<WebSocketStream<LanStream>, ClientError> {
    let ws = |e: &dyn std::fmt::Display| ClientError::WebSocket(e.to_string());
    let mut request = format!("wss://caelum{path}")
        .into_client_request()
        .map_err(|e| ws(&e))?;
    if let Some(token) = token {
        let value = HeaderValue::from_str(&format!("Bearer {token}")).map_err(|e| ws(&e))?;
        request.headers_mut().insert(header::AUTHORIZATION, value);
    }
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_FRAME))
        .max_frame_size(Some(MAX_FRAME));
    let (socket, _) = tokio_tungstenite::client_async_with_config(request, stream, Some(config))
        .await
        .map_err(|e| ws(&e))?;
    Ok(socket)
}

/// Picks the node to use from the statuses of the nodes that answered: the primary with the
/// highest fence (a stale primary is never chosen over a newer one).
pub fn choose_primary(statuses: &[ClusterStatus]) -> Option<&ClusterStatus> {
    statuses
        .iter()
        .filter(|s| s.role == crate::failover::InstanceRole::Primary)
        .filter_map(|s| s.primary.as_ref().map(|f| (f, s)))
        .max_by(|a, b| a.0.cmp(b.0))
        .map(|(_, s)| s)
}

/// The fence a status names as primary, if it is primary itself.
pub fn primary_fence(status: &ClusterStatus) -> Option<&Fence> {
    (status.role == crate::failover::InstanceRole::Primary)
        .then_some(status.primary.as_ref())
        .flatten()
}
