//! Explicit, isolated release-binary smoke; not a load or production test.
use commerce_application::request_context::Identifiers;
use commerce_infrastructure::{MIGRATOR, runtime::SecureIdentifiers};
use serde::Deserialize;
use sqlx::PgPool;
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[allow(dead_code)]
#[path = "../../infrastructure/tests/support/checkout_quote_fixture.rs"]
mod fixture;

const ORIGIN: &str = "https://guest.example.test";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Proof {
    database_url: String,
}

fn bound_url(database: &str, proof: &str) -> bool {
    let Ok(proof) = serde_json::from_str::<Proof>(proof) else {
        return false;
    };
    let Ok(url) = url::Url::parse(database) else {
        return false;
    };
    proof.database_url == database
        && url.scheme() == "postgresql"
        && url.host_str() == Some("127.0.0.1")
        && url.port().is_some_and(|port| port != 0)
        && url.username() == "p1_test"
        && url.password().is_some_and(|value| value.len() >= 32)
        && url.path() == "/postgres"
        && url.query().is_none()
        && url.fragment().is_none()
}

#[cfg(unix)]
fn owned_database() -> Option<String> {
    use std::os::unix::fs::PermissionsExt;
    let path = std::env::var_os("SARK_QUOTE_OWNED_DB_PROOF")?;
    let path = Path::new(&path);
    let parent = path.parent()?.file_name()?.to_str()?;
    if !path.is_absolute()
        || path.file_name()? != "owned-db.json"
        || !parent.starts_with("sark-quote-smoke-")
    {
        return None;
    }
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if !metadata.is_file()
        || metadata.len() > 4096
        || metadata.permissions().mode() & 0o777 != 0o600
    {
        return None;
    }
    let proof = std::fs::read_to_string(path).ok()?;
    let database = std::env::var("DATABASE_URL").ok()?;
    (std::env::var("APP_ENV").as_deref() == Ok("test") && bound_url(&database, &proof))
        .then_some(database)
}

#[cfg(not(unix))]
fn owned_database() -> Option<String> {
    None
}

#[test]
fn proof_rejects_mismatch_and_nonlocal_targets() {
    let database = "postgresql://p1_test:abcdefghijklmnopqrstuvwxyz012345@127.0.0.1:43210/postgres";
    let proof = serde_json::json!({"database_url": database}).to_string();
    assert!(bound_url(database, &proof));
    assert!(!bound_url(
        &database.replace("127.0.0.1", "example.test"),
        &proof
    ));
    let remote = database.replace("127.0.0.1", "example.test");
    assert!(!bound_url(
        &remote,
        &serde_json::json!({"database_url": remote}).to_string()
    ));
    assert!(!bound_url(&database.replace("43210", "43211"), &proof));
    assert!(!bound_url(database, "{}"));
    assert!(!bound_url(
        database,
        &serde_json::json!({"database_url": database, "extra": true}).to_string()
    ));
}

#[test]
fn missing_or_nonowned_proof_stops_before_database_access() {
    for proof in [None, Some("/tmp/owned-db.json")] {
        let mut child = Command::new(std::env::current_exe().expect("test binary"));
        child
            .args(["--ignored", "--exact", "owned_quote_http_smoke"])
            .env_clear()
            .env("APP_ENV", "test")
            .env(
                "DATABASE_URL",
                "postgresql://unowned.example.invalid/postgres",
            );
        if let Some(path) = proof {
            child.env("SARK_QUOTE_OWNED_DB_PROOF", path);
        }
        let result = bounded_output(&mut child).expect("proof negative control");
        assert!(!result.status.success());
        let text = String::from_utf8_lossy(&result.stdout);
        assert!(text.contains("owned disposable database proof required"));
        assert!(!text.contains("unowned.example.invalid"));
    }
}

struct Server {
    child: Child,
    address: SocketAddr,
}
fn bounded_output(command: &mut Command) -> Option<std::process::Output> {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    child.wait_with_output().ok()
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Server {
    fn owns_listener(&mut self) -> bool {
        if !matches!(self.child.try_wait(), Ok(None)) {
            return false;
        }
        let command = if cfg!(target_os = "macos") {
            "/usr/sbin/lsof"
        } else {
            "/usr/bin/lsof"
        };
        let output = bounded_output(Command::new(command).args([
            "-nP",
            "-a",
            "-p",
            &self.child.id().to_string(),
            "-iTCP",
            "-sTCP:LISTEN",
            "-Fn",
        ]));
        let Some(output) = output else {
            return false;
        };
        if !output.status.success() || output.stdout.len() > 4096 {
            return false;
        }
        let Ok(text) = std::str::from_utf8(&output.stdout) else {
            return false;
        };
        let expected = format!("n{}", self.address);
        text.lines()
            .filter(|line| line.starts_with('n'))
            .eq([expected.as_str()])
    }
    fn request(
        &mut self,
        token: &str,
        method: &str,
        path: &str,
        cookie: Option<&str>,
        csrf: Option<&str>,
        body: &str,
    ) -> (u16, Vec<(String, String)>, String) {
        assert!(self.owns_listener(), "owned API listener unavailable");
        let mut stream = TcpStream::connect_timeout(&self.address, Duration::from_secs(2))
            .expect("owned API connection");
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .expect("read deadline");
        stream
            .set_write_timeout(Some(Duration::from_secs(3)))
            .expect("write deadline");
        let mut request = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\nOrigin: {ORIGIN}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
            body.len()
        );
        if let Some(cookie) = cookie {
            request.push_str(&format!("Cookie: {cookie}\r\n"));
        }
        if let Some(csrf) = csrf {
            request.push_str(&format!("x-csrf-token: {csrf}\r\n"));
        }
        request.push_str("\r\n");
        request.push_str(body);
        stream.write_all(request.as_bytes()).expect("request write");
        let mut response = String::new();
        stream
            .take(16_385)
            .read_to_string(&mut response)
            .expect("bounded response");
        assert!(response.len() <= 16_384, "response exceeds bound");
        let (head, body) = response.split_once("\r\n\r\n").expect("complete response");
        let mut lines = head.lines();
        let status = lines
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|value| value.parse().ok())
            .expect("status");
        let headers: Vec<_> = lines
            .map(|line| line.split_once(':').expect("header"))
            .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_owned()))
            .collect();
        let lengths: Vec<_> = headers
            .iter()
            .filter(|(name, _)| name == "content-length")
            .map(|(_, value)| value.parse::<usize>().ok())
            .collect();
        assert_eq!(lengths, [Some(body.len())], "complete response body");
        (status, headers, body.to_owned())
    }
}

fn token() -> String {
    let mut output = String::new();
    for _ in 0..2 {
        for byte in SecureIdentifiers.next().expect("fixture entropy") {
            use std::fmt::Write as _;
            write!(&mut output, "{byte:02x}").expect("hex");
        }
    }
    output
}

fn require_basket(value: &serde_json::Value) {
    assert!(
        value == &serde_json::json!({"revision":1,"replayed":false}),
        "invalid basket receipt"
    );
}

fn wrong_csrf(value: &str) -> String {
    assert!(
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "invalid session proof"
    );
    let mut bytes = value.as_bytes().to_vec();
    bytes[0] = if bytes[0] == b'0' { b'1' } else { b'0' };
    String::from_utf8(bytes).expect("ASCII session proof")
}

fn require_quote(value: &serde_json::Value) {
    assert!(
        value["replayed"] == false
            && value["quote"]["indicative"] == true
            && value["quote"]["locale"] == "nl"
            && value["quote"]["currency"] == "EUR"
            && value["quote"]["lines"]
                .as_array()
                .is_some_and(|lines| lines.len() == 1)
            && value["quote"]["lines"][0]["sku"] == "SET-FILLED"
            && value["quote"]["lines"][0]["quantity"] == 3
            && value["quote"]["total"]
                == serde_json::json!({"gross_minor":38245,"net_minor":31607,"vat_minor":6638}),
        "invalid quote response"
    );
}

#[test]
fn invalid_http_json_diagnostics_do_not_echo_private_values() {
    let value = serde_json::json!({
        "revision": "private-token-marker",
        "replayed": "private-cookie-marker",
        "quote": {"total": "private-csrf-marker"}
    });
    for check in [require_basket, require_quote] {
        let failure = std::panic::catch_unwind(|| check(&value)).expect_err("invalid response");
        let diagnostic = failure
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| failure.downcast_ref::<String>().map(String::as_str))
            .expect("static diagnostic");
        assert!(!diagnostic.contains("private-token-marker"));
        assert!(!diagnostic.contains("private-cookie-marker"));
        assert!(!diagnostic.contains("private-csrf-marker"));
    }
}

#[test]
fn invalid_session_proofs_fail_without_echoing_private_values() {
    for value in ["", "🔒private-csrf-marker"] {
        let failure = std::panic::catch_unwind(|| wrong_csrf(value)).expect_err("invalid proof");
        let diagnostic = failure
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| failure.downcast_ref::<String>().map(String::as_str))
            .expect("static diagnostic");
        assert_eq!(diagnostic, "invalid session proof");
        assert!(!diagnostic.contains("private-csrf-marker"));
    }
    for value in ["0".repeat(64), "f".repeat(64)] {
        let invalid = wrong_csrf(&value);
        assert_ne!(invalid, value);
        assert_eq!(invalid.len(), 64);
    }
}

#[tokio::test]
#[ignore = "run only through scripts/quote_http_smoke.py against its owned native cluster"]
async fn owned_quote_http_smoke() {
    let database = owned_database().expect("owned disposable database proof required");
    let pool = PgPool::connect(&database).await.expect("isolated database");
    let empty: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public' AND c.relkind IN ('r','p','v','m','S','f')")
        .fetch_one(&pool).await.expect("empty fixture check");
    assert_eq!(empty, 0, "fresh database required");
    MIGRATOR.run(&pool).await.expect("isolated migrations");
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_secs();
    fixture::seed_for_load(&pool, now).await;
    let before = effects(&pool).await;
    assert_eq!(before.0, 0, "fresh quote ledger required");
    assert_eq!(
        before.1,
        vec![
            ("SEAT-COVER".into(), 100, 0, 0),
            ("SEAT-FOAM".into(), 100, 0, 0)
        ]
    );
    assert_eq!(before.2, (0, 0, 0, 0, 0, 0, 0));
    let address = TcpListener::bind("127.0.0.1:0")
        .expect("private port")
        .local_addr()
        .expect("address");
    let bearer = token();
    let child = Command::new(env!("CARGO_BIN_EXE_commerce-api"))
        .env_clear()
        .env("APP_ENV", "test")
        .env("DATABASE_URL", &database)
        .env("PREVIEW_TOKEN", &bearer)
        .env("API_BIND", address.to_string())
        .env("GUEST_ORIGIN", ORIGIN)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("release API");
    let mut server = Server { child, address };
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !server.owns_listener() {
        assert!(
            matches!(server.child.try_wait(), Ok(None)),
            "release API exited"
        );
        assert!(
            std::time::Instant::now() < deadline,
            "owned listener unavailable"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let (status, headers, body) =
        server.request(&bearer, "POST", "/api/guest/session", None, None, "{}");
    assert_eq!(status, 200);
    let cookie = headers
        .iter()
        .find(|(name, _)| name == "set-cookie")
        .map(|(_, value)| value.as_str())
        .expect("session cookie");
    assert!(
        cookie.contains("Secure")
            && cookie.contains("HttpOnly")
            && cookie.contains("SameSite=Lax")
            && cookie.contains("Path=/")
            && !cookie.contains("Domain=")
    );
    let cookie = cookie.split(';').next().expect("cookie pair");
    assert!(cookie.starts_with("__Host-piwas-guest="));
    let session: serde_json::Value = serde_json::from_str(&body).expect("session JSON");
    assert!(session["revision"] == 0, "invalid session revision");
    let csrf = session["csrf_token"].as_str().expect("session proof");
    let invalid_csrf = wrong_csrf(csrf);
    let basket = r#"{"expected_revision":0,"effect_key":"smoke-basket","lines":[{"sku":"SET-FILLED","quantity":3}]}"#;
    let (status, _, body) = server.request(
        &bearer,
        "PUT",
        "/api/guest/basket",
        Some(cookie),
        Some(csrf),
        basket,
    );
    assert_eq!(status, 200);
    let receipt: serde_json::Value = serde_json::from_str(&body).expect("basket JSON");
    require_basket(&receipt);
    let quote = r#"{"expected_basket_revision":1,"effect_key":"smoke-quote","country":"NL","postcode":"1234 AB"}"#;
    let (denied, _, _) = server.request(
        &bearer,
        "POST",
        "/api/guest/nl/quotes",
        Some(cookie),
        Some(&invalid_csrf),
        quote,
    );
    assert_eq!(denied, 403);
    assert_eq!(effects(&pool).await.0, 0);
    let (status, _, body) = server.request(
        &bearer,
        "POST",
        "/api/guest/nl/quotes",
        Some(cookie),
        Some(csrf),
        quote,
    );
    assert_eq!(status, 200);
    let value: serde_json::Value = serde_json::from_str(&body).expect("quote JSON");
    require_quote(&value);
    for private in ["basket_id", "policy_id", "1234 AB", "csrf", "evidence"] {
        assert!(!body.contains(private), "private response field");
    }
    let after = effects(&pool).await;
    assert_eq!(after.0, 1);
    assert_eq!(after.1, before.1);
    assert_eq!(after.2, before.2);
    let (status, _, body) = server.request(
        &bearer,
        "POST",
        "/api/guest/nl/quotes",
        Some(cookie),
        Some(csrf),
        quote,
    );
    assert_eq!(status, 200);
    let replay: serde_json::Value = serde_json::from_str(&body).expect("replay JSON");
    assert!(replay["replayed"] == true, "invalid replay receipt");
    assert!(replay["quote"] == value["quote"], "replayed quote changed");
    assert_eq!(effects(&pool).await, after);
    sqlx::query("UPDATE stock_balances SET on_hand=on_hand-1 WHERE component_code='SEAT-COVER'")
        .execute(&pool)
        .await
        .expect("isolated detector control");
    assert_ne!(effects(&pool).await, after);
}

type Effects = (
    i64,
    Vec<(String, i64, i64, i64)>,
    (i64, i64, i64, i64, i64, i64, i64),
);
async fn effects(pool: &PgPool) -> Effects {
    let count = sqlx::query_scalar("SELECT count(*) FROM guest_quote_snapshots")
        .fetch_one(pool)
        .await
        .expect("quote count");
    let balances = sqlx::query_as("SELECT component_code,on_hand,quarantine,reserved FROM stock_balances ORDER BY component_code").fetch_all(pool).await.expect("balances");
    let ledgers = sqlx::query_as("SELECT (SELECT count(*) FROM stock_holds),(SELECT count(*) FROM stock_hold_lines),(SELECT count(*) FROM basket_hold_groups),(SELECT count(*) FROM stock_movements),(SELECT count(*) FROM checkout_orders),(SELECT count(*) FROM payment_attempts),(SELECT count(*) FROM commerce_outbox)").fetch_one(pool).await.expect("ledgers");
    (count, balances, ledgers)
}
