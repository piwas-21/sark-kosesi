//! Browser harness seed: separate test target, never compiled into API/worker binaries.
use commerce_infrastructure::MIGRATOR;
use sqlx::{Executor, PgPool};
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
    let (bytes, items, translations, version): (i64, i64, i64, String) = sqlx::query_as(
        "SELECT pg_database_size(current_database()),
                (SELECT count(*) FROM catalogue_items),
                (SELECT count(*) FROM catalogue_translations),
                current_setting('server_version')",
    )
    .fetch_one(&pool)
    .await
    .expect("fixture evidence");
    assert_eq!((items, translations), (1, 7));
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
