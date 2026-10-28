use super::{
    Effect, HoldReceipt, HoldRequest, HoldWriteError, StoredHold, apply_component, limits,
};
use commerce_domain::holds::{HoldState, HoldWindow};
use sqlx::PgPool;

pub async fn reserve_bom(
    pool: &PgPool,
    request: HoldRequest<'_>,
) -> Result<HoldReceipt, HoldWriteError> {
    if request.id.len() != 32
        || !request
            .id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || request.owner_id.trim().is_empty()
        || request.owner_id.chars().count() > 128
        || !(1..=1000).contains(&request.units)
        || request.revision <= 0
        || request.sku.is_empty()
        || request.sku.len() > 65
    {
        return Err(HoldWriteError::InvalidRequest);
    }
    HoldWindow::new(request.created_at, request.expires_at)?;
    let bom = crate::bom::bill_of_materials(pool, request.sku, request.revision)
        .await?
        .ok_or(HoldWriteError::MissingBom)?;
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let inserted=sqlx::query("INSERT INTO stock_holds (id,owner_id,sku,revision,units,created_at,expires_at) VALUES ($1,$2,$3,$4,$5,$6,$7) ON CONFLICT DO NOTHING RETURNING id")
        .bind(request.id).bind(request.owner_id).bind(request.sku).bind(request.revision)
        .bind(i32::from(request.units)).bind(request.created_at).bind(request.expires_at)
        .fetch_optional(&mut *tx).await?;
    if inserted.is_none() {
        let stored = sqlx::query_as::<_, StoredHold>("SELECT * FROM stock_holds WHERE id=$1")
            .bind(request.id)
            .fetch_one(&mut *tx)
            .await?;
        if stored.owner_id != request.owner_id
            || stored.sku != request.sku
            || stored.revision != request.revision
            || stored.units != i32::from(request.units)
            || stored.created_at != request.created_at
            || stored.expires_at != request.expires_at
        {
            return Err(HoldWriteError::IdempotencyConflict);
        }
        let receipt = HoldReceipt {
            id: stored.id.clone(),
            state: stored.state()?,
            replayed: true,
        };
        tx.rollback().await?;
        return Ok(receipt);
    }
    // The immutable BOM reader orders component codes, defining lock order for
    // all reservations. No caller-supplied quantities or variant stock pool.
    for component in bom.components {
        let quantity = i64::from(component.quantity) * i64::from(request.units);
        apply_component(
            &mut tx,
            &component.code,
            quantity,
            request.id,
            request.owner_id,
            Effect::Reserve,
        )
        .await?;
        sqlx::query(
            "INSERT INTO stock_hold_lines (hold_id,component_code,quantity) VALUES ($1,$2,$3)",
        )
        .bind(request.id)
        .bind(component.code)
        .bind(quantity)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query("UPDATE stock_holds SET state='active' WHERE id=$1")
        .bind(request.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(HoldReceipt {
        id: request.id.to_owned(),
        state: HoldState::Active,
        replayed: false,
    })
}
