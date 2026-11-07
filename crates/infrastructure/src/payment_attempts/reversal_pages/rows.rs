use super::*;
use commerce_domain::checkout_quotes::MAX_QUOTE_UNIX_SECONDS;
use std::collections::HashSet;

#[derive(sqlx::FromRow)]
struct Row {
    id: String,
    order_id: String,
    attempt_id: String,
    effect_key: String,
    payment_id: String,
    reversal_kind: String,
    credential_fingerprint: String,
    scan_id: String,
    page_number: i32,
    previous_page_id: Option<String>,
    from_cursor: Option<String>,
    next_cursor: Option<String>,
    candidates: Option<Vec<String>>,
    recorded_at: i64,
    event_id: String,
    transaction_present: bool,
    event_present: bool,
    child_count: i64,
}
fn kind_name(kind: ReversalKind) -> &'static str {
    match kind {
        ReversalKind::Refund => "refund",
        ReversalKind::Chargeback => "chargeback",
    }
}
fn parse_kind(value: &str) -> Option<ReversalKind> {
    match value {
        "refund" => Some(ReversalKind::Refund),
        "chargeback" => Some(ReversalKind::Chargeback),
        _ => None,
    }
}
fn opaque(value: &str, prefix: &str) -> bool {
    value.starts_with(prefix)
        && value.len() > prefix.len()
        && value.len() <= 256
        && value.trim() == value
        && !value.chars().any(char::is_control)
}
pub(super) fn validate(
    r: &ReversalPageReceipt,
    frozen: &FrozenAttempt,
    started: i64,
) -> Result<(), PaymentStoreError> {
    let prefix = match r.kind {
        ReversalKind::Refund => "re_",
        ReversalKind::Chargeback => "chb_",
    };
    let mut unique = HashSet::new();
    if !identifier(&r.id)
        || !identifier(&r.event_id)
        || !identifier(&r.order_id)
        || r.order_id != frozen.snapshot().binding.order_id
        || r.attempt_id != frozen.snapshot().binding.id
        || input(&r.attempt_id, &r.effect_key).is_err()
        || !identifier(&r.scan_id)
        || !opaque(&r.payment_id, "tr_")
        || !(1..=32).contains(&r.page_number)
        || r.credential_fingerprint.len() != 64
        || !r
            .credential_fingerprint
            .bytes()
            .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
        || !(started..=MAX_QUOTE_UNIX_SECONDS).contains(&r.recorded_at)
        || r.candidates.len() > 10
        || r.candidates
            .iter()
            .any(|id| !opaque(id, prefix) || !unique.insert(id))
        || r.from_cursor.as_ref().is_some_and(|v| !opaque(v, prefix))
        || r.next_cursor.as_ref().is_some_and(|v| !opaque(v, prefix))
        || (r.from_cursor.is_some() && r.from_cursor == r.next_cursor)
        || (r.page_number == 1
            && (r.previous_page_id.is_some() || r.from_cursor.is_some() || r.scan_id != r.id))
        || (r.page_number > 1
            && (r.previous_page_id.as_ref().is_none_or(|id| !identifier(id))
                || r.from_cursor.is_none()
                || r.scan_id == r.id))
    {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    Ok(())
}
pub(super) fn follows(r: &ReversalPageReceipt, previous: &ReversalPageReceipt) -> bool {
    r.previous_page_id.as_deref() == Some(previous.id.as_str())
        && r.scan_id == previous.scan_id
        && r.page_number == previous.page_number + 1
        && r.payment_id == previous.payment_id
        && r.kind == previous.kind
        && r.credential_fingerprint == previous.credential_fingerprint
        && previous.next_cursor.is_some()
        && r.from_cursor == previous.next_cursor
        && r.recorded_at >= previous.recorded_at
}
async fn one(
    tx: &mut Transaction<'_, Postgres>,
    frozen: &FrozenAttempt,
    started: i64,
    effect: Option<&str>,
    id: Option<&str>,
) -> Result<Option<ReversalPageReceipt>, PaymentStoreError> {
    let row = sqlx::query_as::<_, Row>(r#"SELECT left(p.id,33) AS id,left(p.order_id,33) AS order_id,
        left(p.attempt_id,33) AS attempt_id,left(p.effect_key,129) AS effect_key,
        left(p.payment_id,257) AS payment_id,left(p.reversal_kind,20) AS reversal_kind,
        left(p.credential_fingerprint,65) AS credential_fingerprint,left(p.scan_id,33) AS scan_id,
        p.page_number,left(p.previous_page_id,33) AS previous_page_id,
        left(p.from_cursor,257) AS from_cursor,left(p.next_cursor,257) AS next_cursor,
        CASE WHEN cardinality(p.candidates)<=10 AND octet_length(array_to_json(p.candidates)::text)<=8192
          THEN p.candidates END AS candidates,
        p.recorded_at,left(p.event_id,33) AS event_id,p.transaction_id IS NOT NULL AS transaction_present,
        EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.id=p.event_id AND e.aggregate_id=p.order_id
          AND e.kind='payment.reversal_page_recorded.v1' AND e.created_at=p.recorded_at
          AND e.payload=payment_reversal_page_event(p)) AS event_present,
        (SELECT count(*) FROM (SELECT 1 FROM payment_reversal_pages c WHERE c.previous_page_id=p.id LIMIT 2) bounded_children) AS child_count
        FROM payment_reversal_pages p WHERE p.attempt_id=$1
          AND ($2::text IS NULL OR p.effect_key=$2) AND ($3::text IS NULL OR p.id=$3)"#)
        .bind(&frozen.snapshot().binding.id).bind(effect).bind(id).fetch_optional(&mut **tx).await?;
    row.map(|r| r.decode(frozen, started)).transpose()
}
impl Row {
    fn decode(
        self,
        frozen: &FrozenAttempt,
        started: i64,
    ) -> Result<ReversalPageReceipt, PaymentStoreError> {
        if !self.event_present || !self.transaction_present || self.child_count > 1 {
            return Err(PaymentStoreError::InvalidStoredData);
        }
        let receipt = ReversalPageReceipt {
            id: self.id,
            order_id: self.order_id,
            attempt_id: self.attempt_id,
            effect_key: self.effect_key,
            payment_id: self.payment_id,
            kind: parse_kind(&self.reversal_kind).ok_or(PaymentStoreError::InvalidStoredData)?,
            credential_fingerprint: self.credential_fingerprint,
            scan_id: self.scan_id,
            page_number: self.page_number,
            previous_page_id: self.previous_page_id,
            from_cursor: self.from_cursor,
            next_cursor: self.next_cursor,
            candidates: self
                .candidates
                .ok_or(PaymentStoreError::InvalidStoredData)?,
            recorded_at: self.recorded_at,
            event_id: self.event_id,
            replayed: true,
        };
        validate(&receipt, frozen, started)?;
        Ok(receipt)
    }
}

pub(crate) async fn load(
    tx: &mut Transaction<'_, Postgres>,
    frozen: &FrozenAttempt,
    started: i64,
    effect: Option<&str>,
    id: Option<&str>,
) -> Result<Option<ReversalPageReceipt>, PaymentStoreError> {
    let result = one(tx, frozen, started, effect, id).await?;
    if let Some(receipt) = &result {
        chain(tx, frozen, started, receipt).await?;
    }
    Ok(result)
}
pub(super) async fn chain(
    tx: &mut Transaction<'_, Postgres>,
    frozen: &FrozenAttempt,
    started: i64,
    receipt: &ReversalPageReceipt,
) -> Result<(), PaymentStoreError> {
    let mut current = receipt.clone();
    let mut cursors = HashSet::new();
    if let Some(next) = &current.next_cursor {
        cursors.insert(next.clone());
    }
    for _ in 0..32 {
        if let Some(from) = &current.from_cursor
            && !cursors.insert(from.clone())
        {
            return Err(PaymentStoreError::InvalidStoredData);
        }
        let Some(id) = &current.previous_page_id else {
            return Ok(());
        };
        let previous = one(tx, frozen, started, None, Some(id))
            .await?
            .ok_or(PaymentStoreError::InvalidStoredData)?;
        if !follows(&current, &previous) {
            return Err(PaymentStoreError::InvalidStoredData);
        }
        current = previous;
    }
    Err(PaymentStoreError::InvalidStoredData)
}
pub(crate) async fn followed(
    tx: &mut Transaction<'_, Postgres>,
    id: &str,
) -> Result<bool, PaymentStoreError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM payment_reversal_pages WHERE previous_page_id=$1)",
    )
    .bind(id)
    .fetch_one(&mut **tx)
    .await?)
}
pub(crate) async fn history(
    tx: &mut Transaction<'_, Postgres>,
    attempt_id: &str,
) -> Result<(i64, Option<i64>), PaymentStoreError> {
    Ok(sqlx::query_as("SELECT count(*),max(recorded_at) FROM (SELECT recorded_at FROM payment_reversal_pages WHERE attempt_id=$1 LIMIT 129) bounded")
        .bind(attempt_id).fetch_one(&mut **tx).await?)
}
pub(super) async fn current_transaction(
    tx: &mut Transaction<'_, Postgres>,
    id: &str,
) -> Result<bool, PaymentStoreError> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM payment_reversal_pages WHERE id=$1 AND transaction_id=pg_current_xact_id())")
        .bind(id).fetch_one(&mut **tx).await?)
}
pub(super) async fn insert(
    tx: &mut Transaction<'_, Postgres>,
    r: &ReversalPageReceipt,
) -> Result<(), PaymentStoreError> {
    sqlx::query("INSERT INTO payment_reversal_pages(id,order_id,attempt_id,effect_key,payment_id,reversal_kind,credential_fingerprint,scan_id,page_number,previous_page_id,from_cursor,next_cursor,candidates,recorded_at,event_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)")
        .bind(&r.id).bind(&r.order_id).bind(&r.attempt_id).bind(&r.effect_key).bind(&r.payment_id)
        .bind(kind_name(r.kind)).bind(&r.credential_fingerprint).bind(&r.scan_id).bind(r.page_number)
        .bind(&r.previous_page_id).bind(&r.from_cursor).bind(&r.next_cursor).bind(&r.candidates)
        .bind(r.recorded_at).bind(&r.event_id).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) SELECT p.event_id,p.order_id,'payment.reversal_page_recorded.v1',payment_reversal_page_event(p),p.recorded_at FROM payment_reversal_pages p WHERE p.id=$1")
        .bind(&r.id).execute(&mut **tx).await?;
    Ok(())
}

#[cfg(test)]
#[path = "../../../tests/support/mollie_reversal_page_rows.rs"]
mod tests;
