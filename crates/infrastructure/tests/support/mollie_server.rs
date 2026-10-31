//! Bounded loopback Hyper instrument; no actual provider, credentials or shared resources.
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{Request as HttpRequest, Response, body::Incoming, service::service_fn};
use hyper_util::rt::TokioIo;
use serde_json::Value;
use std::{
    collections::VecDeque,
    net::TcpListener,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};
pub enum Reply {
    Json(u16, Value),
    Encoded(&'static str, Value),
    Raw {
        status: u16,
        body: String,
        mime: &'static str,
        chunked: bool,
    },
    Redirect(String),
    Delay(Duration),
    Gate(Arc<AtomicBool>),
    Close,
}
pub struct Request {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}
impl Request {
    pub fn header(&self, key: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }
}
struct State {
    stop: AtomicBool,
    replies: Mutex<VecDeque<Reply>>,
    requests: Mutex<Vec<Request>>,
}
pub struct Server {
    pub base: url::Url,
    state: Arc<State>,
    worker: Option<JoinHandle<()>>,
}
impl Server {
    pub fn start(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("owned loopback");
        listener.set_nonblocking(true).expect("bounded accept");
        let base = url::Url::parse(&format!(
            "http://{}/v2/",
            listener.local_addr().expect("address")
        ))
        .expect("local URL");
        let state = Arc::new(State {
            stop: AtomicBool::new(false),
            replies: Mutex::new(replies.into()),
            requests: Mutex::new(Vec::new()),
        });
        let shared = state.clone();
        let worker = thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread().enable_all().build().expect("instrument runtime").block_on(async move {
                let listener=tokio::net::TcpListener::from_std(listener).expect("instrument listener");
                loop {
                    if shared.stop.load(Ordering::SeqCst){break;}
                    tokio::select! {
                        _=tokio::time::sleep(Duration::from_millis(2))=>{},
                        accepted=listener.accept()=>{
                            let Ok((stream,_))=accepted else{break;};let state=shared.clone();
                            tokio::spawn(async move {
                                let service=service_fn(move|request|respond(request,state.clone()));
                                let _=hyper::server::conn::http1::Builder::new().keep_alive(false).serve_connection(TokioIo::new(stream),service).await;
                            });
                        }
                    }
                }
            });
        });
        Self {
            base,
            state,
            worker: Some(worker),
        }
    }
    pub fn count(&self) -> usize {
        self.state.requests.lock().expect("instrument").len()
    }
    pub fn inspect<T>(&self, f: impl FnOnce(&[Request]) -> T) -> T {
        f(&self.state.requests.lock().expect("instrument"))
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.state.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            worker.join().expect("instrument thread");
        }
    }
}
async fn respond(
    request: HttpRequest<Incoming>,
    state: Arc<State>,
) -> Result<Response<Full<Bytes>>, std::io::Error> {
    let (parts, body) = request.into_parts();
    let body = Limited::new(body, 16384)
        .collect()
        .await
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidData))?
        .to_bytes()
        .to_vec();
    state.requests.lock().expect("instrument").push(Request {
        method: parts.method.to_string(),
        path: parts.uri.to_string(),
        headers: parts
            .headers
            .iter()
            .filter_map(|(k, v)| Some((k.to_string(), v.to_str().ok()?.to_owned())))
            .collect(),
        body,
    });
    let reply = state
        .replies
        .lock()
        .expect("instrument script")
        .pop_front()
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::ConnectionReset))?;
    let (status, body, mime, chunked, location) = match reply {
        Reply::Encoded(encoding, value) => {
            return Response::builder()
                .status(200)
                .header("Content-Type", "application/json")
                .header("Content-Encoding", encoding)
                .header("Connection", "close")
                .body(Full::new(Bytes::from(value.to_string())))
                .map_err(|_| std::io::ErrorKind::InvalidData.into());
        }
        Reply::Json(status, v) => (status, v.to_string(), "application/hal+json", false, None),
        Reply::Raw {
            status,
            body,
            mime,
            chunked,
        } => (status, body, mime, chunked, None),
        Reply::Redirect(url) => (302, String::new(), "application/json", false, Some(url)),
        Reply::Delay(duration) => {
            tokio::time::sleep(duration).await;
            (200, "{}".into(), "application/json", false, None)
        }
        Reply::Gate(gate) => {
            while !gate.load(Ordering::SeqCst) {
                if state.stop.load(Ordering::SeqCst) {
                    return Err(std::io::ErrorKind::Interrupted.into());
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
            (200, "{}".into(), "application/json", false, None)
        }
        Reply::Close => return Err(std::io::ErrorKind::ConnectionReset.into()),
    };
    let mut response = Response::builder()
        .status(status)
        .header("Content-Type", mime)
        .header("Connection", "close");
    if chunked {
        response = response.header("Transfer-Encoding", "chunked");
    }
    if let Some(location) = location {
        response = response.header("Location", location);
    }
    response
        .body(Full::new(Bytes::from(body)))
        .map_err(|_| std::io::ErrorKind::InvalidData.into())
}
