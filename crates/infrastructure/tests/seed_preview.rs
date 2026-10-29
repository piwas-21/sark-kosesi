//! Browser harness seed: separate test target, never compiled into API/worker binaries.
use commerce_infrastructure::MIGRATOR;
use sqlx::{Executor, PgPool};
#[path = "support/browser_catalogue.rs"]
mod browser_catalogue;
#[tokio::test]
async fn seed_owned_browser_database() {
    assert_eq!(std::env::var("APP_ENV").as_deref(), Ok("test"));
    let pool = PgPool::connect(&std::env::var("DATABASE_URL").expect("owned test DB"))
        .await
        .expect("connect");
    MIGRATOR.run(&pool).await.expect("current schema");
    pool.execute(include_str!("../../../tests/fixtures/catalogue.sql"))
        .await
        .expect("seed once");
    browser_catalogue::seed(&pool).await;
    if let Ok(token) = std::env::var("PREVIEW_TOKEN") {
        use sha2::{Digest, Sha256};
        pool.execute("CREATE TABLE browser_fixture_guard(token_hash bytea NOT NULL)")
            .await
            .expect("test-only effect guard");
        sqlx::query("INSERT INTO browser_fixture_guard VALUES($1)")
            .bind(Sha256::digest(token).to_vec())
            .execute(&pool)
            .await
            .expect("owned fixture guard");
    }
    let (bytes, items, translations, version): (i64, i64, i64, String) = sqlx::query_as(
        "SELECT pg_database_size(current_database()),
                (SELECT count(*) FROM catalogue_items),
                (SELECT count(*) FROM catalogue_translations),
                current_setting('server_version')",
    )
    .fetch_one(&pool)
    .await
    .expect("fixture evidence");
    assert_eq!((items, translations), (5, 7));
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.local/evidence");
    std::fs::create_dir_all(&directory).expect("evidence directory");
    let evidence = serde_json::json!({
        "databaseBytes": bytes,
        "catalogueItems": items,
        "translations": translations,
        "postgresVersion": version
    });
    std::fs::write(
        directory.join("postgres-fixture.json"),
        serde_json::to_vec_pretty(&evidence).expect("evidence JSON"),
    )
    .expect("write evidence");
}
