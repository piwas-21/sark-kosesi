use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use commerce_api::{AppState, config::Config, contract, router};
use commerce_infrastructure::MIGRATOR;
use sqlx::{Executor, PgPool};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Semaphore;
use tower::ServiceExt;

const TOKEN: &str = "fixture_preview_key_12345678901234567890";
fn state(pool: PgPool, capacity: usize) -> AppState {
    AppState {
        pool,
        preview_token: Arc::from(TOKEN),
        capacity: Arc::new(Semaphore::new(capacity)),
    }
}
fn request(path: &str, token: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().uri(path);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    builder.body(Body::empty()).expect("request fixture")
}
#[test]
fn config_is_validated_and_never_enables_production() {
    let valid = HashMap::from([
        ("APP_ENV".into(), "test".into()),
        (
            "DATABASE_URL".into(),
            "postgresql://localhost/isolated".into(),
        ),
        ("PREVIEW_TOKEN".into(), TOKEN.into()),
        ("API_BIND".into(), "127.0.0.1:4567".into()),
    ]);
    assert!(Config::parse(&valid).is_ok());
    for (key, value) in [
        ("APP_ENV", "production"),
        ("APP_ENV", ""),
        ("DATABASE_URL", "https://db"),
        ("PREVIEW_TOKEN", "short"),
        ("PREVIEW_TOKEN", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa!"),
        ("API_BIND", "0.0.0.0:4567"),
        ("API_BIND", "invalid"),
    ] {
        let mut values = valid.clone();
        values.insert(key.into(), value.into());
        assert!(Config::parse(&values).is_err(), "{key} must fail");
    }
    for key in valid.keys() {
        let mut values = valid.clone();
        values.remove(key);
        assert!(Config::parse(&values).is_err());
    }
}
#[test]
fn generated_contract_has_real_response_and_security() {
    let value: serde_json::Value =
        serde_json::from_str(&contract::openapi().expect("generated")).expect("JSON");
    assert_eq!(
        value["paths"]["/api/catalogue/{locale}"]["get"]["responses"]["200"]["content"]["application/json"]
            ["schema"]["$ref"],
        "#/components/schemas/Catalogue"
    );
    assert_eq!(
        value["components"]["securitySchemes"]["preview"]["scheme"],
        "bearer"
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn authenticated_read_has_real_data_and_no_cache_leak(pool: PgPool) {
    pool.execute(include_str!("../../../tests/fixtures/catalogue.sql"))
        .await
        .expect("seed");
    let app = router(state(pool.clone(), 32));
    for token in [
        None,
        Some("wrong"),
        Some("fixture_preview_key_12345678901234567891"),
    ] {
        let response = app
            .clone()
            .oneshot(request("/api/catalogue/nl", token))
            .await
            .expect("HTTP");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(response.headers()["cache-control"], "private, no-store");
    }
    for locale in ["nl", "de", "fr", "en", "ar", "tr", "ku"] {
        let response = app
            .clone()
            .oneshot(request(&format!("/api/catalogue/{locale}"), Some(TOKEN)))
            .await
            .expect("HTTP");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["x-robots-tag"], "noindex, nofollow");
        let value: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 16384).await.expect("body"))
                .expect("JSON");
        assert_eq!(value["items"][0]["minor_units"], 12345);
        assert_eq!(value["items"][0]["sku"], "DEMO-COURTYARD-COVERS");
        assert_eq!(value["production_enabled"], false);
    }
    let response = app
        .clone()
        .oneshot(request("/api/catalogue/kmr", Some(TOKEN)))
        .await
        .expect("HTTP");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    for path in ["/health/live", "/health/ready"] {
        assert_eq!(
            app.clone()
                .oneshot(request(path, None))
                .await
                .expect("health")
                .status(),
            StatusCode::NO_CONTENT
        );
    }
    pool.close().await;
    for path in ["/api/catalogue/nl", "/health/ready"] {
        let response = app
            .clone()
            .oneshot(request(path, Some(TOKEN)))
            .await
            .expect("unavailable");
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = to_bytes(response.into_body(), 16384)
            .await
            .expect("bounded");
        let text = String::from_utf8(body.to_vec()).expect("UTF8");
        assert!(!text.contains("postgres"));
        assert!(!text.contains(TOKEN));
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn overload_fails_closed(pool: PgPool) {
    let app = router(state(pool, 0));
    assert_eq!(
        app.oneshot(request("/health/live", None))
            .await
            .expect("HTTP")
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[test]
fn api_binary_contract_and_disabled_production() {
    let binary = env!("CARGO_BIN_EXE_commerce-api");
    let output = std::process::Command::new(binary)
        .arg("openapi")
        .output()
        .expect("contract command");
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("OpenAPI");
    assert_eq!(value["openapi"], "3.1.0");
    let disabled = std::process::Command::new(binary)
        .env("APP_ENV", "production")
        .output()
        .expect("disabled command");
    assert!(!disabled.status.success());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn api_binary_serves_and_stops_gracefully(pool: PgPool) {
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        process::{Command, Stdio},
        time::Duration,
    };
    let listener = TcpListener::bind("127.0.0.1:0").expect("reserve port");
    let address = listener.local_addr().expect("address");
    drop(listener);
    use sqlx::ConnectOptions;
    let database_url = pool.connect_options().to_url_lossy().to_string();
    let child = Command::new(env!("CARGO_BIN_EXE_commerce-api"))
        .env("APP_ENV", "test")
        .env("DATABASE_URL", database_url)
        .env("PREVIEW_TOKEN", TOKEN)
        .env("API_BIND", address.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("API");
    struct OwnedProcess(std::process::Child);
    impl Drop for OwnedProcess {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut owned = OwnedProcess(child);
    let child = &mut owned.0;
    let mut connected = None;
    for _ in 0..100 {
        if let Ok(stream) = TcpStream::connect(address) {
            connected = Some(stream);
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    if let Some(mut stream) = connected {
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("timeout");
        stream
            .write_all(
                b"GET /health/ready HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
            )
            .expect("request");
        let mut response = String::new();
        stream.read_to_string(&mut response).expect("response");
        assert!(response.contains("204 No Content"));
    } else {
        let _ = child.kill();
        panic!("API failed to listen");
    }
    Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .expect("shutdown");
    assert!(child.wait().expect("stopped").success());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn request_dependency_timeout_is_redacted_and_recovers(pool: PgPool) {
    let mut transaction = pool.begin().await.expect("transaction");
    sqlx::query("LOCK TABLE catalogue_items IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *transaction)
        .await
        .expect("hold lock");
    let app = router(state(pool.clone(), 32));
    let response = app
        .clone()
        .oneshot(request("/api/catalogue/nl", Some(TOKEN)))
        .await
        .expect("bounded request");
    assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
    assert_eq!(response.headers()["cache-control"], "private, no-store");
    let value: serde_json::Value = serde_json::from_slice(
        &to_bytes(response.into_body(), 16384)
            .await
            .expect("bounded body"),
    )
    .expect("JSON");
    assert_eq!(value, serde_json::json!({"code":"dependency_timeout"}));
    transaction.rollback().await.expect("release lock");
    assert_eq!(
        app.oneshot(request("/api/catalogue/nl", Some(TOKEN)))
            .await
            .expect("recovered")
            .status(),
        StatusCode::OK
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn steady_users_and_burst_fit_the_real_request_bound(pool: PgPool) {
    let mut transaction = pool.begin().await.expect("transaction");
    sqlx::query("LOCK TABLE catalogue_items IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *transaction)
        .await
        .expect("hold every read pending");
    let app = router(state(pool.clone(), commerce_api::PREVIEW_REQUEST_CAPACITY));
    let mut requests = tokio::task::JoinSet::new();
    // Independent load-fixture requirement: 25 draining users plus 50 arrivals.
    for _ in 0..75 {
        let app = app.clone();
        requests.spawn(async move { app.oneshot(request("/api/catalogue/nl", Some(TOKEN))).await });
    }
    while let Some(response) = requests.join_next().await {
        let response = response.expect("owned task").expect("HTTP");
        // The deliberate DB lock reaches the request deadline, not admission failure.
        assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
        let value: serde_json::Value = serde_json::from_slice(
            &to_bytes(response.into_body(), 16384)
                .await
                .expect("bounded body"),
        )
        .expect("JSON");
        assert_eq!(value, serde_json::json!({"code":"dependency_timeout"}));
    }
    transaction.rollback().await.expect("release lock");
}
