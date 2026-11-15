//! Read-only startup validation for the explicitly selected staging preview.
use commerce_infrastructure::MIGRATOR;
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::{collections::HashMap, time::Duration};

pub(super) async fn connect(url: &str) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(8)
        .acquire_timeout(Duration::from_millis(250))
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("SET search_path = pg_catalog, public, pg_temp")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(url)
        .await
}

pub(super) async fn validate(pool: &PgPool) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .execute(&mut *tx)
        .await?;
    let rows: Vec<(i64, bool, Vec<u8>)> = sqlx::query_as(
        "SELECT version, success, checksum FROM public._sqlx_migrations ORDER BY version",
    )
    .fetch_all(&mut *tx)
    .await?;
    let incompatible = || sqlx::Error::Protocol("incompatible migration history".into());
    let mut applied = HashMap::new();
    for (version, success, checksum) in rows {
        if !success || applied.insert(version, checksum).is_some() {
            return Err(incompatible());
        }
    }
    if !MIGRATOR.ignore_missing
        && applied
            .keys()
            .any(|version| !MIGRATOR.iter().any(|m| m.version == *version))
    {
        return Err(incompatible());
    }
    for migration in MIGRATOR
        .iter()
        .filter(|m| !m.migration_type.is_down_migration())
    {
        if applied.get(&migration.version).map(Vec::as_slice) != Some(migration.checksum.as_ref()) {
            return Err(incompatible());
        }
    }
    tx.rollback().await?;
    Ok(())
}
