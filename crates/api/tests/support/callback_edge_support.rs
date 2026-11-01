use axum::Router;
use commerce_api::callback_edge::Config;
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Notify, oneshot},
    task::JoinHandle,
};

pub const TOKEN: &str = "fixture_edge_server_preview_12345678901234567890";
pub const PATH: &str = "/api/callbacks/mollie";
pub fn values(upstream: SocketAddr) -> HashMap<String, String> {
    HashMap::from([
        ("APP_ENV".into(), "test".into()),
        ("CALLBACK_EDGE_ENABLED".into(), "true".into()),
        ("CALLBACK_EDGE_BIND".into(), "127.0.0.1:0".into()),
        ("CALLBACK_EDGE_PROXY_IPS".into(), "127.0.0.1".into()),
        ("API_URL".into(), format!("http://{upstream}")),
        ("PREVIEW_TOKEN".into(), TOKEN.into()),
    ])
}
pub fn configured(values: &HashMap<String, String>) -> Config {
    Config::parse(values)
        .expect("valid edge configuration")
        .expect("explicitly enabled edge")
}
pub struct Server {
    pub address: SocketAddr,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}
impl Server {
    pub async fn start(router: Router) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("owned loopback listener");
        let address = listener.local_addr().expect("owned address");
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
            .expect("owned HTTP server");
        });
        Self {
            address,
            stop: Some(stop),
            task: Some(task),
        }
    }
    pub async fn stop(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(mut task) = self.task.take() {
            match tokio::time::timeout(Duration::from_secs(4), &mut task).await {
                Ok(result) => result.expect("owned HTTP server completed without panicking"),
                Err(_) => {
                    task.abort();
                    panic!("owned HTTP server did not shut down");
                }
            }
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
#[derive(Clone, Debug)]
pub struct Captured {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}
impl Captured {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}
#[derive(Clone)]
pub enum Behavior {
    Reply(Vec<u8>),
    ReplyShared(Arc<Mutex<Vec<u8>>>),
    Hold(Arc<Notify>),
    HeadersThenHold {
        release: Arc<Notify>,
        sent: Arc<Notify>,
    },
    Delay(Duration),
    Disconnect,
}
pub struct Stub {
    pub address: SocketAddr,
    pub captured: Arc<Mutex<Vec<Captured>>>,
    pub connections: Arc<AtomicUsize>,
    changed: Arc<Notify>,
    task: JoinHandle<()>,
}
impl Stub {
    pub async fn start(behavior: Behavior) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("owned upstream listener");
        let address = listener.local_addr().expect("upstream address");
        let captured = Arc::new(Mutex::new(Vec::new()));
        let changed = Arc::new(Notify::new());
        let connections = Arc::new(AtomicUsize::new(0));
        let (records, signal, count) = (captured.clone(), changed.clone(), connections.clone());
        let task = tokio::spawn(async move {
            let mut children = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (mut stream, _) = accepted.expect("upstream accepted");
                        count.fetch_add(1, Ordering::SeqCst);
                        let (records, signal, behavior) = (records.clone(), signal.clone(), behavior.clone());
                        children.spawn(async move {
                            let Some(request) = read_request(&mut stream).await else { return; };
                            records.lock().expect("captured request lock").push(request);
                            signal.notify_waiters();
                            let reply = match behavior {
                                Behavior::Reply(reply) => reply,
                                Behavior::ReplyShared(reply) => reply.lock().expect("mutable fixture response").clone(),
                                Behavior::Hold(release) => { release.notified().await; empty_200() }
                                Behavior::HeadersThenHold { release, sent } => {
                                    stream.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").await.expect("unfinished empty upstream response headers");
                                    sent.notify_one();
                                    release.notified().await;
                                    b"0\r\n\r\n".to_vec()
                                }
                                Behavior::Delay(delay) => { tokio::time::sleep(delay).await; empty_200() }
                                Behavior::Disconnect => return,
                            };
                            let _ = stream.write_all(&reply).await;
                            let _ = stream.shutdown().await;
                        });
                    }
                    _ = children.join_next(), if !children.is_empty() => {}
                }
            }
        });
        Self {
            address,
            captured,
            connections,
            changed,
            task,
        }
    }
    pub async fn wait_for(&self, count: usize) {
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let notification = self.changed.notified();
                if self.captured.lock().expect("captured lock").len() >= count {
                    return;
                }
                notification.await;
            }
        })
        .await
        .expect("literal upstream request count reached");
    }
    pub fn records(&self) -> Vec<Captured> {
        self.captured.lock().expect("captured lock").clone()
    }
}
impl Drop for Stub {
    fn drop(&mut self) {
        self.task.abort();
    }
}
pub fn empty_200() -> Vec<u8> {
    b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
}
async fn read_request(stream: &mut TcpStream) -> Option<Captured> {
    let mut input = Vec::new();
    let header_end = loop {
        let mut byte = [0; 1];
        if tokio::time::timeout(Duration::from_secs(3), stream.read_exact(&mut byte))
            .await
            .ok()?
            .is_err()
        {
            return None;
        }
        input.push(byte[0]);
        if input.ends_with(b"\r\n\r\n") {
            break input.len();
        }
        assert!(input.len() <= 16_384, "bounded upstream request headers");
    };
    let text = std::str::from_utf8(&input).expect("HTTP headers");
    let mut lines = text.split("\r\n");
    let mut request_line = lines.next().expect("request line").split_whitespace();
    let method = request_line.next().expect("method").to_owned();
    let path = request_line.next().expect("path").to_owned();
    let headers: Vec<_> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    let length: usize = headers
        .iter()
        .find(|(name, _)| name == "content-length")
        .map(|(_, value)| value.parse().expect("request content length"))
        .unwrap_or(0);
    assert!(length <= 1024, "upstream never gets an oversized body");
    input.resize(header_end + length, 0);
    stream
        .read_exact(&mut input[header_end..])
        .await
        .expect("complete upstream request body");
    Some(Captured {
        method,
        path,
        headers,
        body: input[header_end..].to_vec(),
    })
}
#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}
impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}
pub fn head(
    method: &str,
    path: &str,
    host: &str,
    content_type: Option<&str>,
    extras: &[(&str, &str)],
    length: usize,
) -> Vec<u8> {
    let mut bytes = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nContent-Length: {length}\r\nConnection: close\r\n").into_bytes();
    if let Some(content_type) = content_type {
        bytes.extend_from_slice(format!("Content-Type: {content_type}\r\n").as_bytes());
    }
    for (key, value) in extras {
        bytes.extend_from_slice(format!("{key}: {value}\r\n").as_bytes());
    }
    bytes.extend_from_slice(b"\r\n");
    bytes
}
pub async fn exchange(address: SocketAddr, bytes: Vec<u8>) -> Response {
    let mut stream = TcpStream::connect(address).await.expect("edge connection");
    stream.write_all(&bytes).await.expect("edge request");
    read_response(&mut stream).await
}
pub async fn post(address: SocketAddr, body: &[u8]) -> Response {
    let mut bytes = head(
        "POST",
        PATH,
        "private-edge.example.test",
        Some("application/x-www-form-urlencoded"),
        &[],
        body.len(),
    );
    bytes.extend_from_slice(body);
    exchange(address, bytes).await
}
pub async fn read_response(stream: &mut TcpStream) -> Response {
    let mut raw = Vec::new();
    tokio::time::timeout(Duration::from_secs(4), stream.read_to_end(&mut raw))
        .await
        .expect("literal total edge budget")
        .expect("edge response bytes");
    let header_end = raw
        .windows(4)
        .position(|v| v == b"\r\n\r\n")
        .expect("complete response headers")
        + 4;
    let text = std::str::from_utf8(&raw[..header_end]).expect("HTTP response headers");
    let mut lines = text.split("\r\n");
    let status = lines
        .next()
        .expect("status line")
        .split_whitespace()
        .nth(1)
        .expect("status")
        .parse()
        .expect("numeric HTTP status");
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    Response {
        status,
        headers,
        body: raw[header_end..].to_vec(),
    }
}
pub fn safe_failure(response: &Response) {
    assert_ne!(response.status, 200);
    assert!(response.body.len() <= 256, "static bounded error body");
    for name in [
        "set-cookie",
        "www-authenticate",
        "x-upstream-private",
        "x-request-id",
    ] {
        assert!(
            response.header(name).is_none(),
            "upstream header must not escape"
        );
    }
}
pub fn findings<'a>(text: &str, secrets: &'a [&str]) -> Vec<&'a str> {
    secrets
        .iter()
        .copied()
        .filter(|secret| text.contains(secret))
        .collect()
}
