use super::*;
use commerce_image_codec::Kind;
use std::{
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
};

static NEXT_SOCKET: AtomicU64 = AtomicU64::new(0);
const VALIDATE: Operation = Operation::Validate {
    kind: Kind::Png,
    width: 2,
    height: 3,
};
const INPUT: &[u8] = b"test frame";

struct Socket(PathBuf);

impl Socket {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!(
            "sark-dec-{}-{}.sock",
            std::process::id(),
            NEXT_SOCKET.fetch_add(1, Ordering::Relaxed)
        )))
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn endpoint(&self) -> DecoderEndpoint {
        let (peer, _other) = UnixStream::pair().expect("own peer");
        DecoderEndpoint {
            socket: self.0.clone(),
            uid: peer.peer_cred().expect("effective identity").uid(),
        }
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

async fn read_request(stream: &mut UnixStream) {
    let mut request = [0; 30];
    stream.read_exact(&mut request).await.expect("request");
    assert_eq!(
        &request[..20],
        &[
            b'I', b'C', b'0', b'1', 2, 2, 0, 0, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0, 10
        ]
    );
    assert_eq!(&request[20..], INPUT);
    assert_eq!(
        stream
            .try_read(&mut [0])
            .expect_err("write half stays open")
            .kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[test]
fn permits_only_canonical_short_socket_paths() {
    assert!(DecoderEndpoint::parse("/decoder-socket/decoder.sock").is_ok());
    for path in [
        "",
        "relative.sock",
        "/",
        "/decoder-socket/./decoder.sock",
        "/decoder-socket/../decoder.sock",
        "/decoder-socket//decoder.sock",
        "/decoder-socket/",
        "/not a socket",
        "/decoder-\0.sock",
    ] {
        assert!(DecoderEndpoint::parse(path).is_err(), "{path:?}");
    }
    assert!(DecoderEndpoint::parse(&format!("/{}", "x".repeat(100))).is_err());
}

#[tokio::test]
async fn exchange_requires_bound_peer_and_closed_bounded_reply() {
    for (response, expected_valid) in [
        (b"ICR1\0\0\0\0".to_vec(), true),
        (b"ICR1\0\0\0\0extra".to_vec(), false),
        (vec![0; wire::MAX_RESPONSE_BYTES + 1], false),
    ] {
        let socket = Socket::new();
        let listener = UnixListener::bind(socket.path()).expect("decoder socket");
        let endpoint = socket.endpoint();
        let (reply, ()) = tokio::join!(endpoint.exchange(VALIDATE, INPUT), async {
            let (mut stream, _) = listener.accept().await.expect("client");
            read_request(&mut stream).await;
            stream.write_all(&response).await.expect("response");
        });
        if expected_valid {
            assert_eq!(reply.expect("processed"), Response::Valid);
        } else {
            assert!(matches!(reply, Err(ImageError::ProcessingUnavailable)));
        }
    }
}

#[tokio::test]
async fn wrong_peer_receives_no_bytes_and_missing_transport_fails_closed() {
    let socket = Socket::new();
    let endpoint = socket.endpoint();
    assert!(matches!(
        endpoint.exchange(VALIDATE, INPUT).await,
        Err(ImageError::ProcessingUnavailable)
    ));
    assert!(matches!(
        endpoint.exchange(VALIDATE, b"").await,
        Err(ImageError::ProcessingUnavailable)
    ));
    let listener = UnixListener::bind(socket.path()).expect("decoder socket");
    let wrong_peer = DecoderEndpoint {
        uid: endpoint.uid ^ 1,
        ..endpoint
    };
    let (reply, ()) = tokio::join!(wrong_peer.exchange(VALIDATE, INPUT), async {
        let (mut stream, _) = listener.accept().await.expect("client");
        assert_eq!(stream.read(&mut [0]).await.expect("closed"), 0);
    });
    assert!(matches!(reply, Err(ImageError::ProcessingUnavailable)));
}

#[tokio::test]
async fn timeout_closes_held_request() {
    let socket = Socket::new();
    let listener = UnixListener::bind(socket.path()).expect("decoder socket");
    let endpoint = socket.endpoint();
    let (reply, ()) = tokio::join!(
        endpoint.with_deadline(VALIDATE, INPUT, Duration::from_millis(100)),
        async {
            let (mut stream, _) = listener.accept().await.expect("client");
            read_request(&mut stream).await;
            assert_eq!(stream.read(&mut [0]).await.expect("cancelled"), 0);
        }
    );
    assert!(matches!(reply, Err(ImageError::ProcessingUnavailable)));
}
