//! Read-only startup validation for the explicitly selected staging preview.
use commerce_infrastructure::MIGRATOR;
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

fn require_strict(ignore_missing: bool) -> Result<(), sqlx::Error> {
    if ignore_missing {
        return Err(sqlx::Error::Protocol(
            "incompatible migration history".into(),
        ));
    }
    Ok(())
}

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
    let incompatible = || sqlx::Error::Protocol("incompatible migration history".into());
    // A relaxed manifest permits arbitrarily many unknown records and cannot be
    // safely checked by this bounded, fully preapplied startup mode.
    require_strict(MIGRATOR.ignore_missing)?;
    let known: HashSet<_> = MIGRATOR.iter().map(|m| m.version).collect();
    let limit = i64::try_from(known.len())
        .map_err(|_| incompatible())?
        .checked_add(1)
        .ok_or_else(incompatible)?;
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET LOCAL lock_timeout = '250ms'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET LOCAL statement_timeout = '1s'")
        .execute(&mut *tx)
        .await?;
    let rows: Vec<(i64, bool, Option<Vec<u8>>)> = sqlx::query_as(
        "SELECT version, success, CASE WHEN octet_length(checksum)=48 THEN checksum END FROM public._sqlx_migrations ORDER BY version LIMIT $1",
    )
    .bind(limit)
    .fetch_all(&mut *tx)
    .await?;
    if rows.len() > known.len() {
        return Err(incompatible());
    }
    let mut applied = HashMap::new();
    for (version, success, checksum) in rows {
        let checksum = checksum.ok_or_else(incompatible)?;
        if !success || !known.contains(&version) || applied.insert(version, checksum).is_some() {
            return Err(incompatible());
        }
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

#[cfg(test)]
mod tests {
    #[test]
    fn preapplied_mode_rejects_relaxed_manifest_policy() {
        assert!(super::require_strict(false).is_ok());
        assert!(super::require_strict(true).is_err());
    }
}
