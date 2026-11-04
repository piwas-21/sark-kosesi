use super::*;
use commerce_domain::{checkout_quotes::MAX_QUOTE_UNIX_SECONDS, quotes::MAX_MINOR_UNITS};

#[derive(sqlx::FromRow)]
struct Row {
    id: String,
    order_id: String,
    attempt_id: String,
    effect_key: String,
    payment_id: String,
    child_id: String,
    phase: String,
    amount_minor: i64,
    credential_fingerprint: String,
    source_created_seconds: i64,
    source_created_nanos: i32,
    source_reversed_seconds: Option<i64>,
    source_reversed_nanos: Option<i32>,
    recorded_at: i64,
    event_id: String,
    transaction_present: bool,
    event_present: bool,
}
fn phase_name(phase: ReversalPhase) -> &'static str {
    match phase {
        ReversalPhase::RefundQueued => "refund_queued",
        ReversalPhase::RefundPending => "refund_pending",
        ReversalPhase::RefundProcessing => "refund_processing",
        ReversalPhase::RefundRefunded => "refund_refunded",
        ReversalPhase::RefundFailed => "refund_failed",
        ReversalPhase::RefundCanceled => "refund_canceled",
        ReversalPhase::ChargebackReversalUnknown => "chargeback_reversal_unknown",
        ReversalPhase::ChargebackReceived => "chargeback_received",
        ReversalPhase::ChargebackReversed => "chargeback_reversed",
    }
}
fn parse_phase(value: &str) -> Option<ReversalPhase> {
    Some(match value {
        "refund_queued" => ReversalPhase::RefundQueued,
        "refund_pending" => ReversalPhase::RefundPending,
        "refund_processing" => ReversalPhase::RefundProcessing,
        "refund_refunded" => ReversalPhase::RefundRefunded,
        "refund_failed" => ReversalPhase::RefundFailed,
        "refund_canceled" => ReversalPhase::RefundCanceled,
        "chargeback_reversal_unknown" => ReversalPhase::ChargebackReversalUnknown,
        "chargeback_received" => ReversalPhase::ChargebackReceived,
        "chargeback_reversed" => ReversalPhase::ChargebackReversed,
        _ => return None,
    })
}
fn opaque(value: &str, prefix: &str) -> bool {
    value.starts_with(prefix)
        && value.len() > prefix.len()
        && value.len() <= 256
        && value.trim() == value
        && !value.chars().any(char::is_control)
}
fn timestamp(seconds: i64, nanos: i32) -> bool {
    (-62_167_219_200..=253_402_300_799).contains(&seconds) && (0..1_000_000_000).contains(&nanos)
}
pub(super) fn validate(
    r: &ReversalReceipt,
    frozen: &FrozenAttempt,
    started: i64,
) -> Result<(), PaymentStoreError> {
    let reversed = match (r.source_reversed_seconds, r.source_reversed_nanos) {
        (None, None) => None,
        (Some(s), Some(n)) if timestamp(s, n) => Some((s, n)),
        _ => return Err(PaymentStoreError::InvalidStoredData),
    };
    let chargeback = matches!(
        r.phase,
        ReversalPhase::ChargebackReversalUnknown
            | ReversalPhase::ChargebackReceived
            | ReversalPhase::ChargebackReversed
    );
    if !identifier(&r.id)
        || !identifier(&r.event_id)
        || !identifier(&r.order_id)
        || r.order_id != frozen.snapshot().binding.order_id
        || r.attempt_id != frozen.snapshot().binding.id
        || input(&r.attempt_id, &r.effect_key).is_err()
        || !opaque(&r.payment_id, "tr_")
        || !opaque(&r.child_id, if chargeback { "chb_" } else { "re_" })
        || !(1..=MAX_MINOR_UNITS).contains(&r.amount_minor)
        || r.credential_fingerprint.len() != 64
        || !r
            .credential_fingerprint
            .bytes()
            .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
        || !timestamp(r.source_created_seconds, r.source_created_nanos)
        || reversed.is_some_and(|t| t < (r.source_created_seconds, r.source_created_nanos))
        || matches!(r.phase, ReversalPhase::ChargebackReversed) != reversed.is_some()
        || !(started..=MAX_QUOTE_UNIX_SECONDS).contains(&r.recorded_at)
    {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    Ok(())
}
pub(super) async fn load(
    tx: &mut Transaction<'_, Postgres>,
    frozen: &FrozenAttempt,
    started: i64,
    effect: &str,
) -> Result<Option<ReversalReceipt>, PaymentStoreError> {
    let row = sqlx::query_as::<_, Row>(
        r#"SELECT left(o.id,33) AS id,
        left(o.order_id,33) AS order_id,left(o.attempt_id,33) AS attempt_id,
        left(o.effect_key,129) AS effect_key,left(o.payment_id,257) AS payment_id,
        left(o.child_id,257) AS child_id,left(o.phase,40) AS phase,o.amount_minor,
        left(o.credential_fingerprint,65) AS credential_fingerprint,
        o.source_created_seconds,o.source_created_nanos,o.source_reversed_seconds,
        o.source_reversed_nanos,o.recorded_at,left(o.event_id,33) AS event_id,
        o.transaction_id IS NOT NULL AS transaction_present,
        EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.id=o.event_id
          AND e.aggregate_id=o.order_id AND e.kind='payment.reversal_observed.v1'
          AND e.created_at=o.recorded_at
          AND e.payload=payment_reversal_observation_event(o)) AS event_present
        FROM payment_reversal_observations o WHERE o.attempt_id=$1 AND o.effect_key=$2"#,
    )
    .bind(&frozen.snapshot().binding.id)
    .bind(effect)
    .fetch_optional(&mut **tx)
    .await?;
    row.map(|r| {
        if !r.event_present || !r.transaction_present {
            return Err(PaymentStoreError::InvalidStoredData);
        }
        let receipt = ReversalReceipt {
            id: r.id,
            order_id: r.order_id,
            attempt_id: r.attempt_id,
            effect_key: r.effect_key,
            payment_id: r.payment_id,
            child_id: r.child_id,
            phase: parse_phase(&r.phase).ok_or(PaymentStoreError::InvalidStoredData)?,
            amount_minor: r.amount_minor,
            credential_fingerprint: r.credential_fingerprint,
            source_created_seconds: r.source_created_seconds,
            source_created_nanos: r.source_created_nanos,
            source_reversed_seconds: r.source_reversed_seconds,
            source_reversed_nanos: r.source_reversed_nanos,
            recorded_at: r.recorded_at,
            event_id: r.event_id,
            replayed: true,
        };
        validate(&receipt, frozen, started)?;
        Ok(receipt)
    })
    .transpose()
}
pub(super) async fn history(
    tx: &mut Transaction<'_, Postgres>,
    attempt_id: &str,
) -> Result<(i64, Option<i64>), PaymentStoreError> {
    Ok(sqlx::query_as("SELECT count(*),max(recorded_at) FROM (SELECT recorded_at FROM payment_reversal_observations WHERE attempt_id=$1 LIMIT 129) bounded")
        .bind(attempt_id).fetch_one(&mut **tx).await?)
}
pub(super) async fn insert(
    tx: &mut Transaction<'_, Postgres>,
    r: &ReversalReceipt,
) -> Result<(), PaymentStoreError> {
    sqlx::query("INSERT INTO payment_reversal_observations(id,attempt_id,order_id,effect_key,payment_id,child_id,phase,amount_minor,credential_fingerprint,source_created_seconds,source_created_nanos,source_reversed_seconds,source_reversed_nanos,recorded_at,event_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)")
        .bind(&r.id).bind(&r.attempt_id).bind(&r.order_id).bind(&r.effect_key)
        .bind(&r.payment_id).bind(&r.child_id).bind(phase_name(r.phase)).bind(r.amount_minor)
        .bind(&r.credential_fingerprint).bind(r.source_created_seconds).bind(r.source_created_nanos)
        .bind(r.source_reversed_seconds).bind(r.source_reversed_nanos).bind(r.recorded_at)
        .bind(&r.event_id).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) SELECT o.event_id,o.order_id,'payment.reversal_observed.v1',payment_reversal_observation_event(o),o.recorded_at FROM payment_reversal_observations o WHERE o.id=$1")
        .bind(&r.id).execute(&mut **tx).await?;
    Ok(())
}

#[cfg(test)]
#[path = "../../../tests/support/payment_reversal_row_unit.rs"]
mod tests;
