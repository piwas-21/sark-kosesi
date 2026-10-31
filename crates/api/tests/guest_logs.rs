mod guest_support;
use axum::http::StatusCode;
use commerce_infrastructure::MIGRATOR;
use guest_support::*;
use sqlx::PgPool;
use std::sync::Arc;
use tower::ServiceExt;
#[derive(Clone)]
struct LogSink(Arc<std::sync::Mutex<Vec<u8>>>);
impl std::io::Write for LogSink {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .map_err(|_| std::io::Error::other("fixture lock"))?
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogSink {
    type Writer = Self;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn trace_level_transport_and_database_errors_never_log_cookie_csrf_or_body(pool: PgPool) {
    use tracing::instrument::WithSubscriber;
    let sink = LogSink(Arc::new(std::sync::Mutex::new(vec![])));
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .without_time()
        .with_writer(sink.clone())
        .finish();
    let (cookie, csrf) = async {
        tracing::warn!(code = "guest_log_positive_control");
        let (app, _) = app(&pool);
        let (cookie, csrf) = issue(&app, None).await;
        let response = app
            .clone()
            .oneshot(request(
                "PUT",
                "/api/guest/basket",
                Some(&cookie),
                Some(&csrf),
                &update("log-negative-control", 0, 1),
            ))
            .await
            .expect("write");
        assert_eq!(response.status(), StatusCode::OK);
        let response = app
            .clone()
            .oneshot(request(
                "POST",
                "/api/guest/session",
                Some("__Host-piwas-guest=malformed-secret-marker"),
                None,
                "{\"email\":\"private-body-marker@example.test\"}",
            ))
            .await
            .expect("invalid");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let response = app.clone().oneshot(request(
            "POST", "/api/guest/nl/quotes", Some(&cookie), Some(&csrf),
            r#"{"expected_basket_revision":1,"effect_key":"quote-log-private-effect","country":"NL","postcode":"private-postcode-marker"}"#,
        )).await.expect("quote failure");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let response = app.clone().oneshot(request(
            "POST", "/api/guest/nl/quotes", Some(&cookie), Some(&csrf),
            r#"{"expected_basket_revision":1,"effect_key":"quote-log-private-effect","country":"NL","postcode":"1234 ZZ"}"#,
        )).await.expect("unready quote");
        assert_eq!(response.status(), StatusCode::CONFLICT);
        pool.close().await;
        let response = app
            .oneshot(request("GET", "/api/guest/basket", Some(&cookie), None, ""))
            .await
            .expect("database error");
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        (cookie, csrf)
    }
    .with_subscriber(subscriber)
    .await;
    let log = String::from_utf8(sink.0.lock().expect("logs").clone()).expect("UTF8");
    assert!(log.contains("guest_log_positive_control"));
    assert!(log.contains("session response state"), "{log}");
    assert!(log.contains("request_unavailable"));
    for secret in [
        cookie.split('=').nth(1).expect("token"),
        &csrf,
        TOKEN,
        "malformed-secret-marker",
        "private-body-marker",
        "log-negative-control",
        "quote-log-private-effect",
        "private-postcode-marker",
        "1234 ZZ",
    ] {
        assert!(!log.contains(secret), "sensitive marker leaked");
    }
}
