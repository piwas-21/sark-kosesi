//! Durable authenticated reversal discovery hints; no financial or stock authority.
use super::evidence::{context, fresh_id, input, limits, now};
use super::*;
use crate::mollie::{ReversalCursor, ReversalKind, ReversalPage};
use commerce_application::request_context::{Clock, Identifiers, ObservedClock};
use sqlx::PgPool;
pub(crate) mod rows;

/// Historical authenticated page. Candidate identifiers still require exact GETs.
#[derive(Clone, PartialEq, Eq)]
pub struct ReversalPageReceipt {
    pub id: String,
    pub order_id: String,
    pub attempt_id: String,
    pub effect_key: String,
    pub payment_id: String,
    pub kind: ReversalKind,
    pub credential_fingerprint: String,
    pub scan_id: String,
    pub page_number: i32,
    pub previous_page_id: Option<String>,
    pub from_cursor: Option<String>,
    pub next_cursor: Option<String>,
    pub candidates: Vec<String>,
    pub recorded_at: i64,
    pub event_id: String,
    pub replayed: bool,
}
impl ReversalPageReceipt {
    fn matches(&self, previous: Option<&str>, v: &ReversalPage) -> bool {
        self.previous_page_id.as_deref() == previous
            && self.payment_id == v.payment_id()
            && self.kind == v.kind()
            && self.credential_fingerprint == v.credential_fingerprint()
            && self.from_cursor.as_deref() == v.requested_from()
            && self.next_cursor.as_deref() == v.next_from()
            && self.candidates == v.candidates()
    }
}
/// A proof can only be made by this module after validating the complete chain.
/// Sibling modules can inspect it but cannot construct one from raw cursor data.
pub(crate) struct CursorProof {
    attempt: AttemptSnapshot,
    receipt: ReversalPageReceipt,
    from: String,
}
impl CursorProof {
    pub(crate) fn attempt(&self) -> &AttemptSnapshot {
        &self.attempt
    }
    pub(crate) fn receipt(&self) -> &ReversalPageReceipt {
        &self.receipt
    }
    pub(crate) fn from(&self) -> &str {
        &self.from
    }
}

/// Record only after authenticated list I/O. Look up the effect before retrying I/O.
pub async fn record(
    pool: &PgPool,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
    effect: &str,
    previous_page_id: Option<&str>,
    verified: &ReversalPage,
) -> Result<ReversalPageReceipt, PaymentStoreError> {
    let attempt_id = &verified.attempt().binding.id;
    input(attempt_id, effect)?;
    if previous_page_id.is_some_and(|id| !identifier(id)) {
        return Err(PaymentStoreError::InvalidRequest);
    }
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let (frozen, started) = context(&mut tx, attempt_id, true).await?;
    if verified.attempt() != frozen.snapshot() {
        return Err(PaymentStoreError::Conflict);
    }
    if let Some(receipt) = rows::load(&mut tx, &frozen, started, Some(effect), None).await? {
        if !receipt.matches(previous_page_id, verified) {
            return Err(PaymentStoreError::Conflict);
        }
        tx.rollback().await?;
        return Ok(receipt);
    }
    let initial = clock
        .unix_seconds()
        .map_err(|_| PaymentStoreError::ClockUnavailable)?;
    let clock = ObservedClock::new(clock, initial);
    let at = now(&clock)?;
    let (count, last_at) = rows::history(&mut tx, attempt_id).await?;
    if count >= 128 {
        return Err(PaymentStoreError::Unavailable);
    }
    if at < started || last_at.is_some_and(|last| at < last) {
        return Err(PaymentStoreError::ClockUnavailable);
    }
    let previous = match previous_page_id {
        Some(id) => Some(
            rows::load(&mut tx, &frozen, started, None, Some(id))
                .await?
                .ok_or(PaymentStoreError::NotFound)?,
        ),
        None => None,
    };
    if let Some(p) = &previous
        && rows::followed(&mut tx, &p.id).await?
    {
        return Err(PaymentStoreError::Conflict);
    }
    let id = fresh_id(ids)?;
    let receipt = ReversalPageReceipt {
        scan_id: previous
            .as_ref()
            .map_or_else(|| id.clone(), |p| p.scan_id.clone()),
        page_number: previous.as_ref().map_or(1, |p| p.page_number + 1),
        id,
        order_id: frozen.snapshot().binding.order_id.clone(),
        attempt_id: attempt_id.clone(),
        effect_key: effect.into(),
        payment_id: verified.payment_id().into(),
        kind: verified.kind(),
        credential_fingerprint: verified.credential_fingerprint().into(),
        previous_page_id: previous_page_id.map(str::to_owned),
        from_cursor: verified.requested_from().map(str::to_owned),
        next_cursor: verified.next_from().map(str::to_owned),
        candidates: verified.candidates().to_vec(),
        recorded_at: at,
        event_id: fresh_id(ids)?,
        replayed: false,
    };
    rows::validate(&receipt, &frozen, started).map_err(|_| PaymentStoreError::Conflict)?;
    if previous
        .as_ref()
        .is_some_and(|p| !rows::follows(&receipt, p))
    {
        return Err(PaymentStoreError::Conflict);
    }
    rows::chain(&mut tx, &frozen, started, &receipt).await?;
    rows::insert(&mut tx, &receipt).await?;
    sqlx::query("SET CONSTRAINTS require_complete_reversal_page IMMEDIATE")
        .execute(&mut *tx)
        .await?;
    let mut saved = rows::load(&mut tx, &frozen, started, Some(effect), None)
        .await?
        .ok_or(PaymentStoreError::InvalidStoredData)?;
    saved.replayed = false;
    if saved != receipt || !rows::current_transaction(&mut tx, &receipt.id).await? {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    now(&clock)?;
    tx.commit().await?;
    Ok(receipt)
}
/// Recover the committed receipt before making a provider request.
pub async fn lookup(
    pool: &PgPool,
    attempt_id: &str,
    effect: &str,
) -> Result<Option<ReversalPageReceipt>, PaymentStoreError> {
    input(attempt_id, effect)?;
    read(pool, attempt_id, Some(effect), None).await
}
/// Read validated historical evidence, including already-followed pages.
pub async fn read_page(
    pool: &PgPool,
    attempt_id: &str,
    page_id: &str,
) -> Result<ReversalPageReceipt, PaymentStoreError> {
    if !identifier(attempt_id) || !identifier(page_id) {
        return Err(PaymentStoreError::InvalidRequest);
    }
    read(pool, attempt_id, None, Some(page_id))
        .await?
        .ok_or(PaymentStoreError::NotFound)
}
async fn read(
    pool: &PgPool,
    attempt_id: &str,
    effect: Option<&str>,
    id: Option<&str>,
) -> Result<Option<ReversalPageReceipt>, PaymentStoreError> {
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let (frozen, started) = context(&mut tx, attempt_id, false).await?;
    let result = rows::load(&mut tx, &frozen, started, effect, id).await?;
    tx.rollback().await?;
    Ok(result)
}
/// Restore a continuation only for a validated, unfollowed, nonterminal page below the cap.
pub async fn resume(
    pool: &PgPool,
    attempt_id: &str,
    page_id: &str,
) -> Result<Option<ReversalCursor>, PaymentStoreError> {
    if !identifier(attempt_id) || !identifier(page_id) {
        return Err(PaymentStoreError::InvalidRequest);
    }
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let (frozen, started) = context(&mut tx, attempt_id, true).await?;
    let receipt = rows::load(&mut tx, &frozen, started, None, Some(page_id))
        .await?
        .ok_or(PaymentStoreError::NotFound)?;
    let (count, _) = rows::history(&mut tx, attempt_id).await?;
    let can_continue = count < 128
        && receipt.page_number < 32
        && receipt.next_cursor.is_some()
        && !rows::followed(&mut tx, page_id).await?;
    let cursor = if can_continue {
        receipt.next_cursor.clone().map(|from| {
            ReversalCursor::from_storage(CursorProof {
                attempt: frozen.snapshot().clone(),
                receipt,
                from,
            })
        })
    } else {
        None
    };
    tx.rollback().await?;
    Ok(cursor)
}
