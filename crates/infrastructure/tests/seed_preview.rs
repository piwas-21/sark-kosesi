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
}
