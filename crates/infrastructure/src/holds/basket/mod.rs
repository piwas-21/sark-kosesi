//! Trusted atomic basket reservations. No customer, order or provider authorization.
use super::{Effect, HoldReceipt, HoldWriteError, apply_component, limits};
use commerce_application::request_context::Clock;
use commerce_domain::{
    BillOfMaterials,
    catalogue_authoring::merchant_code,
    holds::{HoldAction, HoldState, HoldWindow},
    inventory::StockBalance,
};
use sqlx::{PgPool, Postgres, Transaction};
use std::collections::{BTreeMap, BTreeSet};
mod history;
mod stock;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BasketHoldLine {
    pub id: String,
    pub sku: String,
    pub revision: i32,
    pub units: u16,
}
pub struct BasketHoldRequest<'a> {
    pub id: &'a str,
    /// Immutable server order identity; never a client-selected customer or role.
    pub owner_id: &'a str,
    pub created_at: i64,
    pub expires_at: i64,
    pub lines: &'a [BasketHoldLine],
}
#[derive(Debug, PartialEq, Eq)]
pub struct BasketHoldReceipt {
    pub id: String,
    pub holds: Vec<HoldReceipt>,
    pub replayed: bool,
}
fn identifier(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn valid_line(line: &BasketHoldLine) -> bool {
    identifier(&line.id)
        && merchant_code(&line.sku)
        && line.revision > 0
        && (1..=1000).contains(&line.units)
}
fn selection(request: &BasketHoldRequest<'_>) -> Result<Vec<BasketHoldLine>, HoldWriteError> {
    let mut skus = BTreeSet::new();
    let mut ids = BTreeSet::new();
    if !identifier(request.id)
        || !identifier(request.owner_id)
        || request.lines.is_empty()
        || request.lines.len() > 20
        || !request
            .lines
            .iter()
            .all(|line| valid_line(line) && skus.insert(&line.sku) && ids.insert(&line.id))
    {
        return Err(HoldWriteError::InvalidRequest);
    }
    HoldWindow::new(request.created_at, request.expires_at)?;
    let mut lines = request.lines.to_vec();
    lines.sort_by(|a, b| a.sku.cmp(&b.sku));
    Ok(lines)
}
fn now(clock: &dyn Clock) -> Result<i64, HoldWriteError> {
    i64::try_from(
        clock
            .unix_seconds()
            .map_err(|_| HoldWriteError::ClockUnavailable)?,
    )
    .map_err(|_| HoldWriteError::ClockUnavailable)
}
fn live(request: &BasketHoldRequest<'_>, started: i64, at: i64) -> Result<(), HoldWriteError> {
    if at < started {
        return Err(HoldWriteError::ClockUnavailable);
    }
    // This pure policy check proves a newly active hold is still consumable. It writes no state.
    HoldWindow::new(request.created_at, request.expires_at)?.transition(
        HoldState::Active,
        HoldAction::Consume,
        at,
    )?;
    Ok(())
}
/// All manifests, component movements and child holds commit together.
/// Later checkout must call the transaction entry alongside its order/outbox writes.
pub async fn reserve_basket(
    pool: &PgPool,
    clock: &dyn Clock,
    request: BasketHoldRequest<'_>,
) -> Result<BasketHoldReceipt, HoldWriteError> {
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
        .execute(&mut *tx)
        .await?;
    limits(&mut tx).await?;
    let receipt = reserve_in_transaction(&mut tx, clock, request).await?;
    if receipt.replayed {
        tx.rollback().await?;
    } else {
        tx.commit().await?;
    }
    Ok(receipt)
}
pub(crate) async fn reserve_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    clock: &dyn Clock,
    request: BasketHoldRequest<'_>,
) -> Result<BasketHoldReceipt, HoldWriteError> {
    let lines = selection(&request)?;
    let started = now(clock)?;
    if started < request.created_at {
        return Err(commerce_domain::holds::HoldError::InvalidClock.into());
    }
    let inserted:Option<String>=sqlx::query_scalar("INSERT INTO basket_hold_groups(id,owner_id,created_at,expires_at,members_count) VALUES($1,$2,$3,$4,$5) ON CONFLICT DO NOTHING RETURNING id")
  .bind(request.id).bind(request.owner_id).bind(request.created_at).bind(request.expires_at).bind(i32::try_from(lines.len()).map_err(|_|HoldWriteError::InvalidRequest)?).fetch_optional(&mut **tx).await?;
    if inserted.is_none() {
        return history::replay(tx, clock, &request, &lines, started).await;
    }
    let admitted = now(clock)?;
    live(&request, started, admitted)?;
    let plans = prepare(tx, &lines).await?;
    claim_children(tx, &request, &plans).await?;
    stock::claim(tx, &plans).await?;
    apply(tx, &request, &plans).await?;
    sqlx::query("UPDATE basket_hold_groups SET sealed=true WHERE id=$1")
        .bind(request.id)
        .execute(&mut **tx)
        .await?;
    live(&request, admitted, now(clock)?)?;
    Ok(BasketHoldReceipt {
        id: request.id.into(),
        holds: lines
            .into_iter()
            .map(|line| HoldReceipt {
                id: line.id,
                state: HoldState::Active,
                replayed: false,
            })
            .collect(),
        replayed: false,
    })
}

struct Plan {
    pub line: BasketHoldLine,
    pub bom: BillOfMaterials,
}
async fn prepare(
    tx: &mut Transaction<'_, Postgres>,
    lines: &[BasketHoldLine],
) -> Result<Vec<Plan>, HoldWriteError> {
    let mut plans = Vec::with_capacity(lines.len());
    for line in lines {
        let bom = crate::bom::load(&mut **tx, &line.sku, line.revision)
            .await?
            .ok_or(HoldWriteError::MissingBom)?;
        plans.push(Plan {
            line: line.clone(),
            bom,
        });
    }
    Ok(plans)
}
async fn claim_children(
    tx: &mut Transaction<'_, Postgres>,
    request: &BasketHoldRequest<'_>,
    plans: &[Plan],
) -> Result<(), HoldWriteError> {
    let mut ordered: Vec<_> = plans.iter().collect();
    ordered.sort_by(|a, b| a.line.id.cmp(&b.line.id));
    for plan in ordered {
        // Fresh insertion only: a collision cannot attach legacy/other group stock to this command.
        sqlx::query("INSERT INTO stock_holds(id,owner_id,sku,revision,units,created_at,expires_at,basket_group_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8)")
   .bind(&plan.line.id).bind(request.owner_id).bind(&plan.line.sku).bind(plan.line.revision).bind(i32::from(plan.line.units)).bind(request.created_at).bind(request.expires_at).bind(request.id).execute(&mut **tx).await?;
    }
    Ok(())
}
async fn apply(
    tx: &mut Transaction<'_, Postgres>,
    request: &BasketHoldRequest<'_>,
    plans: &[Plan],
) -> Result<(), HoldWriteError> {
    for plan in plans {
        for component in &plan.bom.components {
            let quantity = i64::from(component.quantity)
                .checked_mul(i64::from(plan.line.units))
                .ok_or(HoldWriteError::InvalidRequest)?;
            apply_component(
                tx,
                &component.code,
                quantity,
                &plan.line.id,
                request.owner_id,
                Effect::Reserve,
            )
            .await?;
            sqlx::query(
                "INSERT INTO stock_hold_lines(hold_id,component_code,quantity) VALUES($1,$2,$3)",
            )
            .bind(&plan.line.id)
            .bind(&component.code)
            .bind(quantity)
            .execute(&mut **tx)
            .await?;
        }
        sqlx::query("UPDATE stock_holds SET state='active' WHERE id=$1")
            .bind(&plan.line.id)
            .execute(&mut **tx)
            .await?;
        sqlx::query("INSERT INTO basket_hold_members(group_id,hold_id,sku,revision,units) VALUES($1,$2,$3,$4,$5)").bind(request.id).bind(&plan.line.id).bind(&plan.line.sku).bind(plan.line.revision).bind(i32::from(plan.line.units)).execute(&mut **tx).await?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../tests/support/basket_transaction.rs"]
mod transaction_tests;
