//! Disposable browser fixture. Synthetic facts do not establish merchant readiness.
use commerce_infrastructure::MIGRATOR;
use sha2::{Digest, Sha256};
use sqlx::{Executor, PgPool};
use std::time::{SystemTime, UNIX_EPOCH};

#[allow(dead_code)]
#[path = "support/checkout_quote_fixture.rs"]
mod fixture;

fn inputs() -> (String, String, String) {
    assert_eq!(std::env::var("APP_ENV").as_deref(), Ok("test"));
    let url = std::env::var("DATABASE_URL").expect("fixture database");
    let name = std::env::var("GUEST_BROWSER_DATABASE_NAME").expect("fixture name");
    let marker = std::env::var("GUEST_BROWSER_DATABASE_MARKER").expect("fixture marker");
    let suffix = name
        .strip_prefix("sark_guest_browser_")
        .expect("fixture prefix");
    assert!(
        suffix.len() == 32
            && suffix
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
    assert!(
        marker.len() == 64
            && marker
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
    let options: sqlx::postgres::PgConnectOptions = url.parse().expect("database URL");
    assert_eq!(options.get_host(), "127.0.0.1");
    assert_eq!(options.get_username(), "p1_test");
    assert_eq!(options.get_database(), Some(name.as_str()));
    (url, name, marker)
}

async fn marker_matches(pool: &PgPool, marker: &str) -> bool {
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM guest_browser_fixture_guard WHERE marker_hash=$1")
            .bind(Sha256::digest(marker.as_bytes()).to_vec())
            .fetch_one(pool)
            .await
            .expect("fixture ownership guard");
    count == 1
}

async fn guarded_pool() -> PgPool {
    let (url, name, marker) = inputs();
    let pool = PgPool::connect(&url).await.expect("fixture database");
    let current: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .expect("database identity");
    assert_eq!(current, name);
    assert!(
        marker_matches(&pool, &marker).await,
        "fixture marker mismatch"
    );
    pool
}

#[tokio::test]
#[ignore = "manual browser harness creates and owns a fresh named database"]
async fn seed_guest_browser_database() {
    let (url, name, marker) = inputs();
    let cluster_url = std::env::var("GUEST_BROWSER_CLUSTER_URL").expect("disposable cluster");
    let cluster: sqlx::postgres::PgConnectOptions = cluster_url.parse().expect("cluster URL");
    let target: sqlx::postgres::PgConnectOptions = url.parse().expect("fixture URL");
    assert_eq!(cluster.get_host(), "127.0.0.1");
    assert_eq!(cluster.get_username(), "p1_test");
    assert_eq!(cluster.get_database(), Some("postgres"));
    assert_eq!(cluster.get_port(), target.get_port());
    let parent = PgPool::connect(&cluster_url)
        .await
        .expect("disposable cluster");
    // The validated random identifier is never supplied as SQL syntax unchecked.
    let create_database = format!("CREATE DATABASE {name}");
    sqlx::query(sqlx::AssertSqlSafe(create_database))
        .execute(&parent)
        .await
        .expect("exclusive fixture database creation");
    parent.close().await;
    let pool = PgPool::connect(&url).await.expect("new fixture database");
    let tables: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_class WHERE relnamespace='public'::regnamespace AND relkind IN ('r','p')")
        .fetch_one(&pool).await.expect("empty schema");
    assert_eq!(tables, 0, "fixture requires a fresh disposable database");
    pool.execute("CREATE TABLE guest_browser_fixture_guard(marker_hash bytea NOT NULL)")
        .await
        .expect("fixture guard");
    sqlx::query("INSERT INTO guest_browser_fixture_guard VALUES($1)")
        .bind(Sha256::digest(marker.as_bytes()).to_vec())
        .execute(&pool)
        .await
        .expect("fixture marker");
    pool.close().await;
    let pool = guarded_pool().await;
    MIGRATOR.run(&pool).await.expect("migrations");
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_secs();
    fixture::seed_for_load(&pool, now).await;
}

#[tokio::test]
#[ignore = "manual browser harness verifies its owned database after Chromium"]
async fn verify_guest_browser_effects() {
    let pool = guarded_pool().await;
    let (_, _, marker) = inputs();
    let foreign = if marker.starts_with('0') {
        "1".repeat(64)
    } else {
        "0".repeat(64)
    };
    assert!(
        !marker_matches(&pool, &foreign).await,
        "foreign marker must fail"
    );
    let effects: (i64, i64, i64, i64, i64, i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM checkout_orders),(SELECT count(*) FROM checkout_order_cancellations),(SELECT count(*) FROM commerce_outbox WHERE kind='checkout.order_created.v1'),(SELECT count(*) FROM commerce_outbox WHERE kind='checkout.order_canceled.v1'),(SELECT count(*) FROM payment_attempts),(SELECT count(*) FROM stock_movements),(SELECT count(*) FROM basket_hold_groups),(SELECT count(*) FROM guest_quote_snapshots)")
        .fetch_one(&pool).await.expect("effects");
    assert_eq!(effects, (1, 1, 1, 1, 0, 2, 1, 1));
    let unexpected: i64 = sqlx::query_scalar("SELECT count(*) FROM stock_movements WHERE action <> 'reserve' OR on_hand_delta <> 0 OR reserved_delta <= 0")
        .fetch_one(&pool).await.expect("reservation-only movements");
    assert_eq!(unexpected, 0);
}
