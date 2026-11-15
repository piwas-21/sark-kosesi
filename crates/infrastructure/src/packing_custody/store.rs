use super::{Error, Record};
use sqlx::{Postgres, Transaction};

pub async fn lock_key(tx: &mut Transaction<'_, Postgres>, key: &str) -> Result<(), Error> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("packing-custody:{key}"))
        .execute(&mut **tx)
        .await?;
    Ok(())
}
pub async fn lookup(
    tx: &mut Transaction<'_, Postgres>,
    key: &str,
) -> Result<Option<(String, String)>, Error> {
    Ok(
        sqlx::query_as("SELECT order_id,attempt_id FROM packing_custody WHERE effect_key=$1")
            .bind(key)
            .fetch_optional(&mut **tx)
            .await?,
    )
}
pub async fn history(
    tx: &mut Transaction<'_, Postgres>,
    order: &str,
) -> Result<Vec<Record>, Error> {
    let exact: bool = sqlx::query_scalar("SELECT packing_custody_history_exact($1)")
        .bind(order)
        .fetch_one(&mut **tx)
        .await?;
    if !exact {
        return Err(Error::InvalidStoredData);
    }
    let rows = sqlx::query_as::<_, Record>("SELECT id,order_id,attempt_id,recognition_id,allocation_kind,allocation_id,actor_id,effect_key,generation,action,target_id,units,packages,recorded_at,event_id FROM packing_custody WHERE order_id=$1 ORDER BY generation LIMIT 65")
        .bind(order).fetch_all(&mut **tx).await?;
    if rows.len() > 64 {
        return Err(Error::InvalidStoredData);
    }
    Ok(rows)
}
pub async fn insert(tx: &mut Transaction<'_, Postgres>, r: &Record) -> Result<(), Error> {
    let inserted=sqlx::query("INSERT INTO packing_custody(id,order_id,attempt_id,recognition_id,allocation_kind,allocation_id,actor_id,effect_key,generation,action,target_id,units,packages,recorded_at,event_id,transaction_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,pg_current_xact_id())")
        .bind(&r.id).bind(&r.order_id).bind(&r.attempt_id).bind(&r.recognition_id).bind(&r.allocation_kind).bind(&r.allocation_id).bind(&r.actor_id).bind(&r.effect_key).bind(r.generation).bind(&r.action).bind(&r.target_id).bind(&r.units).bind(&r.packages).bind(r.recorded_at).bind(&r.event_id).execute(&mut **tx).await?;
    if inserted.rows_affected() != 1 {
        return Err(Error::InvalidStoredData);
    }
    let inserted=sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) SELECT event_id,order_id,'order.packing_custody_recorded.v1',packing_custody_event(p),recorded_at FROM packing_custody p WHERE id=$1")
        .bind(&r.id).execute(&mut **tx).await?;
    if inserted.rows_affected() != 1 {
        return Err(Error::InvalidStoredData);
    }
    sqlx::query("SET CONSTRAINTS require_complete_packing_custody IMMEDIATE")
        .execute(&mut **tx)
        .await?;
    Ok(())
}
