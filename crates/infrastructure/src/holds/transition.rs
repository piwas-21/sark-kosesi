use super::{
    Effect, HoldCommand, HoldReceipt, HoldWriteError, StoredHold, apply_component, limits,
};
use commerce_domain::holds::{HoldAction, HoldWindow};
use sqlx::{PgPool, Postgres, Transaction};

#[derive(sqlx::FromRow)]
struct HoldLine {
    component_code: String,
    quantity: i64,
}

pub async fn transition_hold(
    pool: &PgPool,
    id: &str,
    command: HoldCommand<'_>,
    now: i64,
) -> Result<HoldReceipt, HoldWriteError> {
    if command.actor().trim().is_empty() || command.actor().chars().count() > 128 {
        return Err(HoldWriteError::InvalidRequest);
    }
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let stored =
        sqlx::query_as::<_, StoredHold>("SELECT * FROM stock_holds WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(HoldWriteError::NotFound)?;
    let receipt = transition_locked(&mut tx, &stored, &command, now).await?;
    if receipt.replayed {
        tx.rollback().await?;
    } else {
        tx.commit().await?;
    }
    Ok(receipt)
}

/// The caller owns the hold row lock and transaction; expiry uses the same policy.
pub(super) async fn transition_locked(
    tx: &mut Transaction<'_, Postgres>,
    stored: &StoredHold,
    command: &HoldCommand<'_>,
    now: i64,
) -> Result<HoldReceipt, HoldWriteError> {
    let id = stored.id.as_str();
    if command
        .owner()
        .is_some_and(|owner| owner != stored.owner_id)
    {
        return Err(HoldWriteError::Ownership);
    }
    let state = stored.state()?;
    let target = HoldWindow::new(stored.created_at, stored.expires_at)?.transition(
        state,
        command.action(),
        now,
    )?;
    if target == state {
        return Ok(HoldReceipt {
            id: stored.id.clone(),
            state,
            replayed: true,
        });
    }
    let (label, effect) = match command.action() {
        HoldAction::Release => ("released", Effect::Release),
        HoldAction::Consume => ("consumed", Effect::Consume),
        HoldAction::Expire => ("expired", Effect::Expire),
    };
    let lines=sqlx::query_as::<_,HoldLine>("SELECT component_code,quantity FROM stock_hold_lines WHERE hold_id=$1 ORDER BY component_code")
        .bind(id).fetch_all(&mut **tx).await?;
    if lines.is_empty() {
        return Err(sqlx::Error::Protocol("Empty stored hold".into()).into());
    }
    for line in lines {
        apply_component(
            tx,
            &line.component_code,
            line.quantity,
            id,
            command.actor(),
            effect,
        )
        .await?;
    }
    sqlx::query("UPDATE stock_holds SET state=$2 WHERE id=$1")
        .bind(id)
        .bind(label)
        .execute(&mut **tx)
        .await?;
    sqlx::query("UPDATE stock_hold_expiry_failures SET resolved_at=$2 WHERE hold_id=$1 AND resolved_at IS NULL")
        .bind(id).bind(now).execute(&mut **tx).await?;
    Ok(HoldReceipt {
        id: stored.id.clone(),
        state: target,
        replayed: false,
    })
}
