//! Child-only environment controls and graceful shutdown retain genuine LLVM data.
use super::*;
use std::{
    io::Read,
    net::TcpListener as StdListener,
    process::{Child, Command, Output, Stdio},
    sync::Mutex,
};

fn command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_commerce-callback-edge"));
    command.env_clear();
    if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
        command.env("LLVM_PROFILE_FILE", profile);
    }
    command
}
fn run(values: &HashMap<String, String>) -> Output {
    command()
        .envs(values)
        .output()
        .expect("owned short-lived edge command")
}
fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}
struct Binary {
    child: Option<Child>,
    address: SocketAddr,
}
impl Binary {
    async fn start(upstream: SocketAddr, proxy_trap: SocketAddr) -> Self {
        let reservation = StdListener::bind("127.0.0.1:0").expect("reserve owned edge address");
        let address = reservation.local_addr().expect("reserved address");
        drop(reservation);
        let mut settings = values(upstream);
        settings.insert("CALLBACK_EDGE_BIND".into(), address.to_string());
        let mut command = command();
        command.envs(settings);
        for key in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ] {
            command.env(key, format!("http://{proxy_trap}"));
        }
        let child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("owned edge child");
        let mut binary = Self {
            child: Some(child),
            address,
        };
        for _ in 0..100 {
            if TcpStream::connect(address).await.is_ok() {
                return binary;
            }
            assert!(
                binary
                    .child
                    .as_mut()
                    .expect("owned child")
                    .try_wait()
                    .expect("child status")
                    .is_none(),
                "enabled edge exited before listening"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("enabled edge did not listen");
    }
    async fn stop(mut self) -> String {
        let mut child = self.child.take().expect("owned edge process");
        assert!(
            Command::new("/bin/kill")
                .args(["-INT", &child.id().to_string()])
                .status()
                .expect("graceful signal to owned process")
                .success()
        );
        let mut result = None;
        for _ in 0..150 {
            result = child.try_wait().expect("owned child status");
            if result.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let Some(result) = result else {
            let _ = child.kill();
            let _ = child.wait();
            panic!("edge did not finish graceful shutdown");
        };
        assert!(
            result.success(),
            "edge gracefully exits and flushes native profiles"
        );
        let mut log = String::new();
        child
            .stdout
            .take()
            .expect("stdout pipe")
            .read_to_string(&mut log)
            .expect("stdout log");
        child
            .stderr
            .take()
            .expect("stderr pipe")
            .read_to_string(&mut log)
            .expect("stderr log");
        log
    }
}
impl Drop for Binary {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[test]
fn binary_disabled_invalid_nonunicode_and_busy_bind_paths_keep_diagnostics_static() {
    let disabled = run(&HashMap::new());
    assert!(disabled.status.success());
    assert!(text(&disabled).contains("callback_edge_disabled"));
    let mut settings = values("127.0.0.1:45671".parse().expect("loopback"));
    settings.insert("CALLBACK_EDGE_ENABLED".into(), "false".into());
    settings.insert("API_URL".into(), "private-disabled-upstream-marker".into());
    let disabled = run(&settings);
    assert!(disabled.status.success());
    assert!(!text(&disabled).contains("private-disabled-upstream-marker"));
    for (key, value) in [
        ("APP_ENV", "production"),
        ("CALLBACK_EDGE_ENABLED", "private-flag-marker"),
    ] {
        let mut invalid = settings.clone();
        invalid.insert(key.into(), value.into());
        let output = run(&invalid);
        assert!(!output.status.success());
        assert!(!text(&output).contains(TOKEN));
        assert!(!text(&output).contains("private-flag-marker"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let private = std::ffi::OsString::from_vec(b"private-nonunicode-marker\xff".to_vec());
        let output = command()
            .env("APP_ENV", "test")
            .env("CALLBACK_EDGE_ENABLED", "true")
            .env("PREVIEW_TOKEN", private)
            .output()
            .expect("non-Unicode child-only setting");
        assert!(!output.status.success());
        assert!(text(&output).contains("non-Unicode callback edge configuration"));
        assert!(!text(&output).contains("private-nonunicode-marker"));
    }
    let reservation = StdListener::bind("127.0.0.1:0").expect("owned busy bind");
    let mut busy = values("127.0.0.1:45671".parse().expect("upstream"));
    busy.insert(
        "CALLBACK_EDGE_BIND".into(),
        reservation.local_addr().expect("busy address").to_string(),
    );
    let output = run(&busy);
    assert!(!output.status.success());
    assert!(text(&output).contains("callback edge bind unavailable"));
    assert!(!text(&output).contains(TOKEN));
}

#[tokio::test]
async fn actual_binary_ignores_proxy_environment_and_never_logs_request_or_upstream_details() {
    let reply = Arc::new(Mutex::new(empty_200()));
    let upstream = Stub::start(Behavior::ReplyShared(reply.clone())).await;
    let trap = Stub::start(Behavior::Reply(empty_200())).await;
    let binary = Binary::start(upstream.address, trap.address).await;
    let body = b"id=tr_BINARY_PRIVATE";
    let mut raw = head(
        "POST",
        &format!("{PATH}?private=binary-query-marker"),
        "binary-host-marker.invalid",
        Some("application/x-www-form-urlencoded"),
        &[
            ("Authorization", "Bearer binary-caller-auth-marker"),
            ("Cookie", "private=binary-cookie-marker"),
            ("X-Private", "binary-header-marker"),
        ],
        body.len(),
    );
    raw.extend_from_slice(body);
    let response = exchange(binary.address, raw).await;
    assert_eq!(response.status, 200);
    assert!(response.body.is_empty());
    assert_eq!(upstream.records().len(), 1);
    assert!(
        trap.records().is_empty(),
        "environment HTTP proxies must never receive server credentials or callback bytes"
    );
    let private = b"upstream-private-body-marker";
    let mut error=format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nX-Upstream-Private: binary-private-upstream-header\r\nConnection: close\r\n\r\n",private.len()).into_bytes();
    error.extend_from_slice(private);
    *reply.lock().expect("mutable upstream response") = error;
    let response = post(binary.address, b"id=tr_BINARY_REJECTED_PRIVATE").await;
    assert_eq!(response.status, 503);
    safe_failure(&response);
    assert_eq!(response.body, b"{\"code\":\"callback_edge_unavailable\"}");
    assert_eq!(upstream.records().len(), 2);
    assert!(trap.records().is_empty());
    let log = binary.stop().await;
    assert!(
        log.contains("callback_edge_listening"),
        "actual startup event calibrates process log capture"
    );
    for line in log.lines() {
        serde_json::from_str::<serde_json::Value>(line).expect("JSON process log");
    }
    let secrets = [
        TOKEN,
        "tr_BINARY_PRIVATE",
        "tr_BINARY_REJECTED_PRIVATE",
        "binary-query-marker",
        "binary-host-marker",
        "binary-caller-auth-marker",
        "binary-cookie-marker",
        "binary-header-marker",
        "upstream-private-body-marker",
        "binary-private-upstream-header",
    ];
    assert_eq!(
        findings("probe binary-caller-auth-marker detected", &secrets),
        vec!["binary-caller-auth-marker"],
        "positive sensitive-string detector control"
    );
    assert!(
        findings(&log, &secrets).is_empty(),
        "private detail leaked through actual child output"
    );
}
