//! Real HTTP chunking and process logs exercise the binary's configured route.
use super::*;
use std::{
    io::{Read, Write},
    net::{Shutdown, SocketAddr, TcpListener, TcpStream},
    process::{Child, Command, Stdio},
    time::Duration,
};

struct Server {
    child: Child,
    address: SocketAddr,
}
impl Server {
    fn start(pool: &PgPool) -> Self {
        use sqlx::ConnectOptions;
        let reservation = TcpListener::bind("127.0.0.1:0").expect("reserve private port");
        let address = reservation.local_addr().expect("private address");
        drop(reservation);
        let child = Command::new(env!("CARGO_BIN_EXE_commerce-api"))
            .env("APP_ENV", "test")
            .env(
                "DATABASE_URL",
                pool.connect_options().to_url_lossy().to_string(),
            )
            .env("PREVIEW_TOKEN", TOKEN)
            .env("API_BIND", address.to_string())
            .env("CALLBACK_INTAKE_PROFILE_ID", PROFILE)
            .env_remove("GUEST_ORIGIN")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("private callback binary");
        let mut server = Self { child, address };
        let ready_until = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            match server.child.try_wait() {
                Ok(Some(status)) => panic!("configured API exited before readiness: {status}"),
                Ok(None) => {}
                Err(_) => panic!("configured API readiness status unavailable"),
            }
            if TcpStream::connect(address).is_ok() {
                return server;
            }
            if std::time::Instant::now() >= ready_until {
                panic!("configured API did not become ready within startup deadline");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn exchange(&self, content_type: &str, chunks: &[&[u8]]) -> String {
        let mut stream = TcpStream::connect(self.address).expect("owned HTTP connection");
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .expect("read deadline");
        stream
            .set_write_timeout(Some(Duration::from_secs(3)))
            .expect("write deadline");
        let headers = format!(
            "POST {PATH}?private=transport-private-query HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {TOKEN}\r\nCookie: __Host-secret=transport-private-cookie\r\nContent-Type: {content_type}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
        );
        stream
            .write_all(headers.as_bytes())
            .expect("chunked headers");
        for chunk in chunks {
            write!(stream, "{:x}\r\n", chunk.len()).expect("chunk length");
            stream.write_all(chunk).expect("chunk bytes");
            stream.write_all(b"\r\n").expect("chunk end");
        }
        stream.write_all(b"0\r\n\r\n").expect("chunk terminator");
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .expect("complete HTTP response");
        response
    }
    fn incomplete_body(&self, close_write: bool) -> String {
        let mut stream = TcpStream::connect(self.address).expect("owned body-failure connection");
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .expect("body read deadline");
        stream.write_all(format!(
            "POST {PATH} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {TOKEN}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: 100\r\nConnection: close\r\n\r\nid=tr_INCOMPLETE_PRIVATE"
        ).as_bytes()).expect("incomplete body");
        if close_write {
            stream
                .shutdown(Shutdown::Write)
                .expect("actual premature EOF");
        }
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .expect("bounded body failure response");
        response
    }
    fn stop(mut self) -> String {
        assert!(
            Command::new("kill")
                .args(["-INT", &self.child.id().to_string()])
                .status()
                .expect("graceful signal")
                .success()
        );
        assert!(self.child.wait().expect("API stopped").success());
        let mut output = String::new();
        self.child
            .stdout
            .take()
            .expect("stdout")
            .read_to_string(&mut output)
            .expect("stdout log");
        self.child
            .stderr
            .take()
            .expect("stderr")
            .read_to_string(&mut output)
            .expect("stderr log");
        output
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn status(response: &str, expected: &str) {
    assert!(
        response.starts_with(expected),
        "unexpected static HTTP response: {response}"
    );
    assert!(
        response
            .to_ascii_lowercase()
            .contains("cache-control: private, no-store")
    );
    assert!(
        response
            .to_ascii_lowercase()
            .contains("x-robots-tag: noindex, nofollow")
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn chunked_transport_obeys_limits_and_binary_logs_never_retain_request_or_database_secrets(
    pool: PgPool,
) {
    let server = Server::start(&pool);
    let media = "application/x-www-form-urlencoded";
    let response = server.exchange(media, &[b"id=tr_CHUNKED_PRIVATE"]);
    status(&response, "HTTP/1.1 200 OK");
    assert!(
        response.ends_with("\r\n\r\n"),
        "success acknowledgement has no response body"
    );
    let response = server.exchange(media, &[b"%6", b"9d=%74%7", b"2%5fSPLIT_ESCAPE"]);
    status(&response, "HTTP/1.1 200 OK");
    assert_eq!(custody(&pool).await, (2, 2, 2, 2));
    for (length, expected) in [
        (1024, "HTTP/1.1 400 Bad Request"),
        (1025, "HTTP/1.1 413 Payload Too Large"),
    ] {
        let body = format!("id=tr_{}", "x".repeat(length - 6));
        assert_eq!(body.len(), length);
        let response = server.exchange(media, &[&body.as_bytes()[..511], &body.as_bytes()[511..]]);
        status(&response, expected);
    }
    let response = server.exchange("application/json", &[b"{\"id\":\"tr_NEXTGEN_PRIVATE\"}"]);
    status(&response, "HTTP/1.1 415 Unsupported Media Type");
    assert_eq!(custody(&pool).await, (2, 2, 2, 2));
    let response = server.incomplete_body(true);
    status(&response, "HTTP/1.1 400 Bad Request");
    assert!(response.contains("invalid_callback_body"));
    let response = server.incomplete_body(false);
    status(&response, "HTTP/1.1 504 Gateway Timeout");
    assert!(response.contains("dependency_timeout"));
    assert_eq!(custody(&pool).await, (2, 2, 2, 2));
    pool.execute("CREATE FUNCTION callback_private_failure_fixture() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'transport-private-database-marker'; END $$; CREATE TRIGGER callback_private_failure_fixture BEFORE INSERT ON callback_intakes FOR EACH ROW EXECUTE FUNCTION callback_private_failure_fixture()")
        .await.expect("actual private database exception");
    let response = server.exchange(media, &[b"id=tr_DATABASE_PRIVATE"]);
    status(&response, "HTTP/1.1 503 Service Unavailable");
    pool.execute("DROP TRIGGER callback_private_failure_fixture ON callback_intakes; DROP FUNCTION callback_private_failure_fixture()")
        .await.expect("restore healthy database");
    assert_eq!(custody(&pool).await, (2, 2, 2, 2));
    let response = server.exchange(media, &[b"id=tr_AFTER_FAILURE"]);
    status(&response, "HTTP/1.1 200 OK");
    assert_eq!(custody(&pool).await, (3, 3, 3, 3));
    no_financial_effects(&pool).await;
    let log = server.stop();
    assert!(
        log.contains("request_unavailable"),
        "actual failure event proves log capture is active"
    );
    let secrets = [
        TOKEN,
        "transport-private-cookie",
        "transport-private-query",
        "transport-private-database-marker",
        "tr_CHUNKED_PRIVATE",
        "tr_SPLIT_ESCAPE",
        "tr_NEXTGEN_PRIVATE",
        "tr_DATABASE_PRIVATE",
        "tr_INCOMPLETE_PRIVATE",
        "id=tr_AFTER_FAILURE",
    ];
    assert_eq!(
        findings("prefix transport-private-cookie suffix", &secrets),
        vec!["transport-private-cookie"],
        "positive detector control catches a synthetic sensitive marker"
    );
    assert!(
        findings(&log, &secrets).is_empty(),
        "request or database detail leaked into actual binary output"
    );
}
