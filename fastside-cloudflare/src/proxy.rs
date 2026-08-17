use std::{
    cell::RefCell,
    io,
    net::IpAddr,
    pin::Pin,
    rc::Rc,
    sync::{Arc, OnceLock},
    task::{Context, Poll},
    time::Duration,
};

use base64ct::{Base64, Encoding};
use bytes::Bytes;
use fastside::crawler::{
    CrawledInstanceStatus, CrawlerError, InstanceRequest, InstanceResponse,
    should_read_response_body,
};
use fastside_shared::{
    config::{CrawlerConfig, Proxy, ProxyAuth},
    request_headers::REQUEST_HEADERS,
    serde_types::{Instance, Service},
};
use futures::{
    future::{AbortHandle, Abortable, Either},
    pin_mut,
};
use http_body_util::{BodyExt, Empty};
use hyper::{
    Request,
    body::Incoming,
    client::conn::http1,
    header::{ACCEPT_ENCODING, CONNECTION, HOST, LOCATION, PROXY_AUTHORIZATION},
};
use hyper_util::rt::TokioIo;
use rustls::{ClientConfig, RootCertStore, time_provider::TimeProvider};
use rustls_pki_types::{ServerName, UnixTime};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio_rustls::TlsConnector;
use url::{Host, Url};
use worker::{Date, Delay, Socket, wasm_bindgen_futures::spawn_local};

const MAX_CONNECT_RESPONSE_SIZE: usize = 16 * 1024;
const MAX_REDIRECTS: usize = 10;

trait Io: AsyncRead + AsyncWrite + Unpin {}
impl<T: AsyncRead + AsyncWrite + Unpin> Io for T {}
type BoxedIo = Box<dyn Io>;

#[derive(Debug)]
struct WorkerTime;

impl TimeProvider for WorkerTime {
    fn current_time(&self) -> Option<UnixTime> {
        Some(UnixTime::since_unix_epoch(Duration::from_millis(
            Date::now().as_millis(),
        )))
    }
}

fn tls_config() -> Arc<ClientConfig> {
    static CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let roots = RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            let mut config = ClientConfig::builder_with_details(
                Arc::new(rustls::crypto::ring::default_provider()),
                Arc::new(WorkerTime),
            )
            .with_safe_default_protocol_versions()
            .expect("ring supports the default TLS versions")
            .with_root_certificates(roots)
            .with_no_client_auth();
            config.alpn_protocols = vec![b"http/1.1".to_vec()];
            Arc::new(config)
        })
        .clone()
}

async fn start_tls(stream: BoxedIo, hostname: &str) -> Result<BoxedIo, RequestFailure> {
    let server_name = ServerName::try_from(hostname.to_owned())
        .map_err(|error| RequestFailure::Request(error.to_string()))?;
    TlsConnector::from(tls_config())
        .connect(server_name, stream)
        .await
        .map(|stream| Box::new(stream) as BoxedIo)
        .map_err(|error| RequestFailure::Request(error.to_string()))
}

#[derive(Clone, Copy)]
enum ProxyProtocol {
    Http,
    Socks5,
}

struct ProxyEndpoint {
    protocol: ProxyProtocol,
    hostname: String,
    port: u16,
    tls: bool,
    auth: Option<ProxyAuth>,
}

impl ProxyEndpoint {
    fn parse(proxy: &Proxy) -> Result<Self, CrawlerError> {
        let url = Url::parse(&proxy.url)?;
        let (protocol, tls, default_port) = match url.scheme() {
            "http" => (ProxyProtocol::Http, false, 80),
            "https" => (ProxyProtocol::Http, true, 443),
            "socks5" | "socks5h" => (ProxyProtocol::Socks5, false, 1080),
            scheme => {
                return Err(CrawlerError::Request(format!(
                    "unsupported proxy scheme: {scheme}"
                )));
            }
        };
        let hostname = hostname(&url)
            .map_err(|_| CrawlerError::Request("proxy URL has no host".to_owned()))?;
        let auth = proxy.auth.clone().or_else(|| {
            (!url.username().is_empty() || url.password().is_some()).then(|| ProxyAuth {
                username: decode(url.username()),
                password: decode(url.password().unwrap_or_default()),
            })
        });
        Ok(Self {
            protocol,
            hostname,
            port: url.port().unwrap_or(default_port),
            tls,
            auth,
        })
    }
}

fn decode(value: &str) -> String {
    String::from_utf8_lossy(&urlencoding::decode_binary(value.as_bytes())).into_owned()
}

enum RequestFailure {
    Request(String),
    Redirect,
}

#[derive(Clone)]
struct SharedSocket(Rc<RefCell<Option<Socket>>>);

fn close_socket(mut socket: Socket) {
    spawn_local(async move {
        let _ = socket.close().await;
    });
}

struct OpeningSocket(Option<Socket>);

impl OpeningSocket {
    async fn opened(&self) -> Result<(), RequestFailure> {
        self.0
            .as_ref()
            .expect("socket is opening")
            .opened()
            .await
            .map(|_| ())
            .map_err(|error| RequestFailure::Request(error.to_string()))
    }

    fn share(mut self) -> SharedSocket {
        SharedSocket::new(self.0.take().expect("socket is open"))
    }
}

impl Drop for OpeningSocket {
    fn drop(&mut self) {
        if let Some(socket) = self.0.take() {
            close_socket(socket);
        }
    }
}

impl SharedSocket {
    fn new(socket: Socket) -> Self {
        Self(Rc::new(RefCell::new(Some(socket))))
    }

    fn close(&self) {
        let Some(socket) = self.0.borrow_mut().take() else {
            return;
        };
        close_socket(socket);
    }

    fn poll<T>(
        &self,
        operation: impl FnOnce(Pin<&mut Socket>) -> Poll<io::Result<T>>,
    ) -> Poll<io::Result<T>> {
        match self.0.borrow_mut().as_mut() {
            Some(socket) => operation(Pin::new(socket)),
            None => Poll::Ready(Err(io::Error::other("socket is closed"))),
        }
    }
}

impl Drop for SharedSocket {
    fn drop(&mut self) {
        if Rc::strong_count(&self.0) == 1 {
            self.close();
        }
    }
}

impl AsyncRead for SharedSocket {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        self.poll(|socket| socket.poll_read(context, buffer))
    }
}

impl AsyncWrite for SharedSocket {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.poll(|socket| socket.poll_write(context, buffer))
    }

    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll(|socket| socket.poll_flush(context))
    }

    fn poll_shutdown(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll(|socket| socket.poll_shutdown(context))
    }
}

struct ConnectionGuard {
    connection: AbortHandle,
    socket: SharedSocket,
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.connection.abort();
        self.socket.close();
    }
}

struct PendingResponse {
    response: hyper::Response<Incoming>,
    _connection: ConnectionGuard,
}

fn target_port(url: &Url) -> Result<u16, RequestFailure> {
    url.port_or_known_default()
        .ok_or_else(|| RequestFailure::Request("target URL has no port".to_owned()))
}

fn hostname(url: &Url) -> Result<String, RequestFailure> {
    match url.host() {
        Some(Host::Domain(host)) => Ok(host.to_owned()),
        Some(Host::Ipv4(address)) => Ok(address.to_string()),
        Some(Host::Ipv6(address)) => Ok(address.to_string()),
        None => Err(RequestFailure::Request("target URL has no host".to_owned())),
    }
}

fn host_text(url: &Url) -> Result<String, RequestFailure> {
    match url.host() {
        Some(Host::Ipv6(address)) => Ok(format!("[{address}]")),
        Some(address) => Ok(address.to_string()),
        None => Err(RequestFailure::Request("target URL has no host".to_owned())),
    }
}

fn authority(url: &Url) -> Result<String, RequestFailure> {
    Ok(format!("{}:{}", host_text(url)?, target_port(url)?))
}

fn host_header(url: &Url) -> Result<String, RequestFailure> {
    let host = host_text(url)?;
    Ok(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host,
    })
}

fn path_and_query(url: &Url) -> String {
    match url.query() {
        Some(query) => format!("{}?{query}", url.path()),
        None => url.path().to_owned(),
    }
}

fn absolute_target(url: &Url) -> Result<String, RequestFailure> {
    Ok(format!(
        "{}://{}{}",
        url.scheme(),
        host_header(url)?,
        path_and_query(url)
    ))
}

fn basic_auth(auth: &ProxyAuth) -> String {
    format!(
        "Basic {}",
        Base64::encode_string(format!("{}:{}", auth.username, auth.password).as_bytes())
    )
}

async fn http_connect(
    stream: &mut BoxedIo,
    endpoint: &ProxyEndpoint,
    target: &Url,
) -> Result<(), RequestFailure> {
    let authority = authority(target)?;
    let auth = endpoint
        .auth
        .as_ref()
        .map(|auth| format!("Proxy-Authorization: {}\r\n", basic_auth(auth)))
        .unwrap_or_default();
    stream
        .write_all(
            format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n{auth}\r\n").as_bytes(),
        )
        .await
        .map_err(|error| RequestFailure::Request(error.to_string()))?;
    stream
        .flush()
        .await
        .map_err(|error| RequestFailure::Request(error.to_string()))?;

    let mut response = Vec::new();
    while !response.ends_with(b"\r\n\r\n") {
        if response.len() == MAX_CONNECT_RESPONSE_SIZE {
            return Err(RequestFailure::Request(
                "proxy CONNECT response is too large".to_owned(),
            ));
        }
        let mut byte = [0];
        stream
            .read_exact(&mut byte)
            .await
            .map_err(|error| RequestFailure::Request(error.to_string()))?;
        response.push(byte[0]);
    }
    let status = std::str::from_utf8(&response)
        .ok()
        .and_then(|response| response.lines().next())
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|status| status.parse::<u16>().ok())
        .ok_or_else(|| RequestFailure::Request("invalid proxy CONNECT response".to_owned()))?;
    if !(200..300).contains(&status) {
        return Err(RequestFailure::Request(format!(
            "proxy CONNECT returned HTTP {status}"
        )));
    }
    Ok(())
}

async fn socks5_connect(
    stream: &mut BoxedIo,
    endpoint: &ProxyEndpoint,
    target: &Url,
) -> Result<(), RequestFailure> {
    let method = if endpoint.auth.is_some() { 2 } else { 0 };
    stream
        .write_all(&[5, 1, method])
        .await
        .map_err(|error| RequestFailure::Request(error.to_string()))?;
    let mut selection = [0; 2];
    stream
        .read_exact(&mut selection)
        .await
        .map_err(|error| RequestFailure::Request(error.to_string()))?;
    if selection != [5, method] {
        return Err(RequestFailure::Request(
            "SOCKS5 proxy rejected the authentication method".to_owned(),
        ));
    }
    if let Some(auth) = &endpoint.auth {
        let username = auth.username.as_bytes();
        let password = auth.password.as_bytes();
        let username_len = u8::try_from(username.len())
            .map_err(|_| RequestFailure::Request("SOCKS5 username is too long".to_owned()))?;
        let password_len = u8::try_from(password.len())
            .map_err(|_| RequestFailure::Request("SOCKS5 password is too long".to_owned()))?;
        let mut request = Vec::with_capacity(username.len() + password.len() + 3);
        request.extend_from_slice(&[1, username_len]);
        request.extend_from_slice(username);
        request.push(password_len);
        request.extend_from_slice(password);
        stream
            .write_all(&request)
            .await
            .map_err(|error| RequestFailure::Request(error.to_string()))?;
        let mut response = [0; 2];
        stream
            .read_exact(&mut response)
            .await
            .map_err(|error| RequestFailure::Request(error.to_string()))?;
        if response != [1, 0] {
            return Err(RequestFailure::Request(
                "SOCKS5 authentication failed".to_owned(),
            ));
        }
    }

    let target_host = hostname(target)?;
    let mut request = Vec::with_capacity(target_host.len() + 7);
    request.extend_from_slice(&[5, 1, 0]);
    if let Ok(address) = target_host.parse::<IpAddr>() {
        match address {
            IpAddr::V4(address) => {
                request.push(1);
                request.extend_from_slice(&address.octets());
            }
            IpAddr::V6(address) => {
                request.push(4);
                request.extend_from_slice(&address.octets());
            }
        }
    } else {
        let host_len = u8::try_from(target_host.len())
            .map_err(|_| RequestFailure::Request("target hostname is too long".to_owned()))?;
        request.extend_from_slice(&[3, host_len]);
        request.extend_from_slice(target_host.as_bytes());
    }
    request.extend_from_slice(&target_port(target)?.to_be_bytes());
    stream
        .write_all(&request)
        .await
        .map_err(|error| RequestFailure::Request(error.to_string()))?;

    let mut response = [0; 4];
    stream
        .read_exact(&mut response)
        .await
        .map_err(|error| RequestFailure::Request(error.to_string()))?;
    if response[0] != 5 || response[1] != 0 {
        return Err(RequestFailure::Request(format!(
            "SOCKS5 proxy connection failed with code {}",
            response[1]
        )));
    }
    let address_len = match response[3] {
        1 => 4,
        4 => 16,
        3 => {
            let mut length = [0];
            stream
                .read_exact(&mut length)
                .await
                .map_err(|error| RequestFailure::Request(error.to_string()))?;
            usize::from(length[0])
        }
        _ => {
            return Err(RequestFailure::Request(
                "invalid SOCKS5 address type".to_owned(),
            ));
        }
    };
    let mut address_and_port = vec![0; address_len + 2];
    stream
        .read_exact(&mut address_and_port)
        .await
        .map_err(|error| RequestFailure::Request(error.to_string()))?;
    Ok(())
}

async fn connect(
    endpoint: &ProxyEndpoint,
    target: &Url,
) -> Result<(BoxedIo, bool, SharedSocket), RequestFailure> {
    if !matches!(target.scheme(), "http" | "https") {
        return Err(RequestFailure::Redirect);
    }
    let socket = OpeningSocket(Some(
        Socket::builder()
            .connect(&endpoint.hostname, endpoint.port)
            .map_err(|error| RequestFailure::Request(error.to_string()))?,
    ));
    socket.opened().await?;
    let socket = socket.share();
    let mut stream: BoxedIo = Box::new(socket.clone());
    if endpoint.tls {
        stream = start_tls(stream, &endpoint.hostname).await?;
    }

    let absolute_form = match endpoint.protocol {
        ProxyProtocol::Http if target.scheme() == "http" => true,
        ProxyProtocol::Http => {
            http_connect(&mut stream, endpoint, target).await?;
            false
        }
        ProxyProtocol::Socks5 => {
            socks5_connect(&mut stream, endpoint, target).await?;
            false
        }
    };
    if target.scheme() == "https" {
        stream = start_tls(stream, &hostname(target)?).await?;
    }
    Ok((stream, absolute_form, socket))
}

async fn send(endpoint: &ProxyEndpoint, target: &Url) -> Result<PendingResponse, RequestFailure> {
    let (stream, absolute_form, socket) = connect(endpoint, target).await?;
    let (mut sender, connection) = http1::handshake(TokioIo::new(stream))
        .await
        .map_err(|error| RequestFailure::Request(error.to_string()))?;
    let (abort, registration) = AbortHandle::new_pair();
    spawn_local(async move {
        let _ = Abortable::new(connection, registration).await;
    });
    let guard = ConnectionGuard {
        connection: abort,
        socket,
    };

    let uri = if absolute_form {
        absolute_target(target)?
    } else {
        path_and_query(target)
    };
    let mut request = Request::builder()
        .method("GET")
        .uri(uri)
        .header(HOST, host_header(target)?)
        .header(CONNECTION, "close")
        .header(ACCEPT_ENCODING, "identity");
    for (name, value) in REQUEST_HEADERS {
        request = request.header(name, value);
    }
    if absolute_form && let Some(auth) = &endpoint.auth {
        request = request.header(PROXY_AUTHORIZATION, basic_auth(auth));
    }
    let request = request
        .body(Empty::<Bytes>::new())
        .map_err(|error| RequestFailure::Request(error.to_string()))?;
    let response = sender
        .send_request(request)
        .await
        .map_err(|error| RequestFailure::Request(error.to_string()))?;
    Ok(PendingResponse {
        response,
        _connection: guard,
    })
}

fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

async fn follow_redirects(
    endpoint: &ProxyEndpoint,
    service: &Service,
    mut target: Url,
) -> Result<PendingResponse, RequestFailure> {
    for redirects in 0..=MAX_REDIRECTS {
        let response = send(endpoint, &target).await?;
        if !service.follow_redirects || !is_redirect(response.response.status().as_u16()) {
            return Ok(response);
        }
        let Some(location) = response.response.headers().get(LOCATION) else {
            return Ok(response);
        };
        if redirects == MAX_REDIRECTS {
            return Err(RequestFailure::Redirect);
        }
        let location = location.to_str().map_err(|_| RequestFailure::Redirect)?;
        target = target
            .join(location)
            .map_err(|_| RequestFailure::Redirect)?;
        target.set_fragment(None);
        if !matches!(target.scheme(), "http" | "https") {
            return Err(RequestFailure::Redirect);
        }
    }
    unreachable!()
}

pub async fn request(
    proxy: &Proxy,
    config: &CrawlerConfig,
    service: &Service,
    instance: &Instance,
    test_url: Url,
) -> Result<InstanceRequest, CrawlerError> {
    let endpoint = ProxyEndpoint::parse(proxy)?;
    let timeout =
        config.get_domain_timeout(instance.url.host_str().expect("instance URL has a host"));
    let start = Date::now().as_millis();
    let response = follow_redirects(&endpoint, service, test_url);
    let delay = Delay::from(timeout);
    pin_mut!(response, delay);
    let pending = match futures::future::select(response, delay).await {
        Either::Left((Ok(response), _)) => response,
        Either::Left((Err(RequestFailure::Redirect), _)) => {
            return Ok(InstanceRequest::Failed(
                CrawledInstanceStatus::RedirectPolicyError,
            ));
        }
        Either::Left((Err(RequestFailure::Request(error)), _)) => {
            worker::console_error!("Proxy request failed: {error}");
            return Ok(InstanceRequest::Failed(CrawledInstanceStatus::RequestError));
        }
        Either::Right(((), _)) => {
            return Ok(InstanceRequest::Failed(CrawledInstanceStatus::TimedOut));
        }
    };
    let duration = Duration::from_millis(Date::now().as_millis().saturating_sub(start));
    let status_code = pending.response.status().as_u16();
    let body = if should_read_response_body(service, instance, status_code) {
        let body = pending.response.into_body().collect();
        let delay = Delay::from(timeout);
        pin_mut!(body, delay);
        match futures::future::select(body, delay).await {
            Either::Left((Ok(body), _)) => {
                Some(String::from_utf8_lossy(&body.to_bytes()).into_owned())
            }
            Either::Left((Err(error), _)) => {
                return Err(CrawlerError::Request(error.to_string()));
            }
            Either::Right(((), _)) => {
                return Err(CrawlerError::Request("response body timed out".to_owned()));
            }
        }
    } else {
        None
    };
    Ok(InstanceRequest::Response(InstanceResponse {
        status_code,
        duration,
        body,
    }))
}
