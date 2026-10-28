//! Trusted reservation primitives. Session authorization/provider recovery follow.

mod create;
mod transition;
pub use create::reserve_bom;
pub use transition::transition_hold;

use commerce_domain::{
    holds::{HoldAction, HoldError, HoldState},
    inventory::{StockBalance, StockError},
};
use sqlx::{Postgres, Transaction};

#[derive(Debug)]
pub enum HoldWriteError {
    InvalidRequest,
    MissingBom,
    NotFound,
    Ownership,
    IdempotencyConflict,
    Stock(StockError),
    Policy(HoldError),
    Database(sqlx::Error),
}
impl From<sqlx::Error> for HoldWriteError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}
impl From<StockError> for HoldWriteError {
    fn from(error: StockError) -> Self {
        Self::Stock(error)
    }
}
impl From<HoldError> for HoldWriteError {
    fn from(error: HoldError) -> Self {
        Self::Policy(error)
    }
}

pub struct HoldRequest<'a> {
    pub id: &'a str,
    pub owner_id: &'a str,
    pub sku: &'a str,
    pub revision: i32,
    pub units: u16,
    pub created_at: i64,
    pub expires_at: i64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HoldReceipt {
    pub id: String,
    pub state: HoldState,
    pub replayed: bool,
}

/// Expiry is worker-only; release and consumption require the server's owner ID.
pub enum HoldCommand<'a> {
    Release {
        owner_id: &'a str,
        actor_id: &'a str,
    },
    Consume {
        owner_id: &'a str,
        actor_id: &'a str,
    },
    Expire {
        actor_id: &'a str,
    },
}
impl HoldCommand<'_> {
    fn action(&self) -> HoldAction {
        match self {
            Self::Release { .. } => HoldAction::Release,
            Self::Consume { .. } => HoldAction::Consume,
            Self::Expire { .. } => HoldAction::Expire,
        }
    }
    fn owner(&self) -> Option<&str> {
        match self {
            Self::Release { owner_id, .. } | Self::Consume { owner_id, .. } => Some(owner_id),
            Self::Expire { .. } => None,
        }
    }
    fn actor(&self) -> &str {
        match self {
            Self::Release { actor_id, .. }
            | Self::Consume { actor_id, .. }
            | Self::Expire { actor_id } => actor_id,
        }
    }
}

#[derive(sqlx::FromRow)]
struct StoredHold {
    id: String,
    owner_id: String,
    sku: String,
    revision: i32,
    units: i32,
    created_at: i64,
    expires_at: i64,
    state: String,
}
impl StoredHold {
    fn state(&self) -> Result<HoldState, HoldWriteError> {
        match self.state.as_str() {
            "active" => Ok(HoldState::Active),
            "released" => Ok(HoldState::Released),
            "consumed" => Ok(HoldState::Consumed),
            "expired" => Ok(HoldState::Expired),
            _ => Err(sqlx::Error::Protocol("Invalid stored hold state".into()).into()),
        }
    }
}

pub(super) async fn limits(tx: &mut Transaction<'_, Postgres>) -> Result<(), sqlx::Error> {
    sqlx::query("SET LOCAL lock_timeout='1s'")
        .execute(&mut **tx)
        .await?;
    sqlx::query("SET LOCAL statement_timeout='2s'")
        .execute(&mut **tx)
        .await?;
    Ok(())
}

#[derive(Clone, Copy)]
enum Effect {
    Reserve,
    Release,
    Consume,
    Expire,
}
impl Effect {
    fn label(self) -> &'static str {
        match self {
            Self::Reserve => "reserve",
            Self::Release => "release-hold",
            Self::Consume => "consume-hold",
            Self::Expire => "expire-hold",
        }
    }
    fn apply(self, before: StockBalance, quantity: i64) -> Result<StockBalance, StockError> {
        match self {
            Self::Reserve => before.reserve(quantity),
            Self::Release | Self::Expire => before.release(quantity),
            Self::Consume => before.consume(quantity),
        }
    }
}

#[derive(sqlx::FromRow)]
struct BalanceRow {
    on_hand: i64,
    quarantine: i64,
    reserved: i64,
}

async fn apply_component(
    tx: &mut Transaction<'_, Postgres>,
    component: &str,
    quantity: i64,
    hold: &str,
    owner: &str,
    effect: Effect,
) -> Result<(), HoldWriteError> {
    sqlx::query("INSERT INTO stock_balances (component_code) VALUES ($1) ON CONFLICT DO NOTHING")
        .bind(component)
        .execute(&mut **tx)
        .await?;
    let row = sqlx::query_as::<_, BalanceRow>(
        "SELECT on_hand,quarantine,reserved FROM stock_balances WHERE component_code=$1 FOR UPDATE",
    )
    .bind(component)
    .fetch_one(&mut **tx)
    .await?;
    let before = StockBalance::new(row.on_hand, row.quarantine, row.reserved)?;
    let after = effect.apply(before, quantity)?;
    sqlx::query(
        "UPDATE stock_balances SET on_hand=$2,quarantine=$3,reserved=$4 WHERE component_code=$1",
    )
    .bind(component)
    .bind(after.on_hand())
    .bind(after.quarantine())
    .bind(after.reserved())
    .execute(&mut **tx)
    .await?;
    let effect_key = format!("hold:{hold}:{}:{component}", effect.label());
    sqlx::query("INSERT INTO stock_movements (effect_key,component_code,action,quantity,actor,reason,on_hand_delta,quarantine_delta,reserved_delta,on_hand_after,quarantine_after,reserved_after,hold_id) VALUES ($1,$2,$3,$4,$5,'sealed BOM hold transition',$6,0,$7,$8,$9,$10,$11)")
        .bind(effect_key).bind(component).bind(effect.label()).bind(quantity).bind(owner)
        .bind(after.on_hand()-before.on_hand()).bind(after.reserved()-before.reserved())
        .bind(after.on_hand()).bind(after.quarantine()).bind(after.reserved()).bind(hold).execute(&mut **tx).await?;
    Ok(())
}
