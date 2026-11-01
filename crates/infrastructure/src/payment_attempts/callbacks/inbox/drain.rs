//! Independent receipt-event draining; never fence fetched facts by latest lease.
use super::super::super::{identities, observations};
use super::*;
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Disposition {
    Observed,
    PaidObserved,
    Ambiguous,
    Unmatched,
    CustodyInvalid,
}
impl Disposition {
    fn parse(raw: &str) -> Result<Self, PaymentStoreError> {
        match raw {
            "observed" => Ok(Self::Observed),
            "paid_observed" => Ok(Self::PaidObserved),
            "ambiguous" => Ok(Self::Ambiguous),
            "unmatched" => Ok(Self::Unmatched),
            "custody_invalid" => Ok(Self::CustodyInvalid),
            _ => Err(PaymentStoreError::InvalidStoredData),
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::Observed => "observed",
            Self::PaidObserved => "paid_observed",
            Self::Ambiguous => "ambiguous",
            Self::Unmatched => "unmatched",
            Self::CustodyInvalid => "custody_invalid",
        }
    }
}
#[derive(sqlx::FromRow)]
struct ProcessingRow {
    event_id: Option<String>,
    processed_at: i64,
    outcome: String,
    observation_id: Option<String>,
    resolution_id: Option<String>,
}
impl ProcessingRow {
    fn validate_financial(
        &self,
        op: &str,
        receipt: &commerce_domain::payment_attempts::callbacks::LookupSnapshot,
        observation: &commerce_domain::payment_attempts::observations::ObservationSnapshot,
        resolution: &identities::Resolution,
        outcome: Disposition,
    ) -> Result<(), PaymentStoreError> {
        if observation.effect_key != format!("callback-lookup-{op}")
            || observation.intent.payment != receipt.payment
            || observation.intent.credential_fingerprint != receipt.credential_fingerprint
            || observation.recorded_at < receipt.recorded_at
            || resolution.recorded_at < receipt.recorded_at
            || Some(resolution.id.as_str()) != self.resolution_id.as_deref()
            || resolution.through_observation < observation.revision
            || self.processed_at < resolution.recorded_at
            || classify(resolution) != outcome
        {
            return Err(PaymentStoreError::InvalidStoredData);
        }
        Ok(())
    }
}
pub(crate) async fn read_disposition(
    pool: &PgPool,
    op: &str,
) -> Result<Option<Disposition>, PaymentStoreError> {
    if quarantine::read(pool, op).await? {
        return Ok(Some(Disposition::CustodyInvalid));
    }
    let row=sqlx::query_as::<_,ProcessingRow>("SELECT left(event_id,33) AS event_id,processed_at,left(outcome,32) AS outcome,left(observation_id,33) AS observation_id,left(resolution_id,33) AS resolution_id FROM callback_receipt_processing WHERE operation_id=$1")
        .bind(op).fetch_optional(pool).await?;
    let Some(row) = row else { return Ok(None) };
    let source = processing_source::read(pool, op).await?;
    let outcome = Disposition::parse(&row.outcome)?;
    if row.event_id != source.event_id
        || !(source.created_at..=9_007_199_254_740_991).contains(&row.processed_at)
    {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    // A redacted exception proves only original intake/generation identity. It
    // deliberately cannot restore a payment capability from malformed custody.
    if outcome == Disposition::CustodyInvalid {
        if row.observation_id.is_some() || row.resolution_id.is_some() {
            return Err(PaymentStoreError::InvalidStoredData);
        }
        return Ok(Some(outcome));
    }
    if row.event_id.is_none() {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    let receipt = lookup::read_lookup(pool, op)
        .await?
        .ok_or(PaymentStoreError::InvalidStoredData)?;
    if row.processed_at < receipt.snapshot.recorded_at {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    if matches!(
        outcome,
        Disposition::Observed | Disposition::PaidObserved | Disposition::Ambiguous
    ) {
        let observation_id = row
            .observation_id
            .as_deref()
            .filter(|v| identifier(v))
            .ok_or(PaymentStoreError::InvalidStoredData)?;
        row.resolution_id
            .as_deref()
            .filter(|v| identifier(v))
            .ok_or(PaymentStoreError::InvalidStoredData)?;
        let attempt = &receipt.snapshot.payment.attempt_id;
        let observation = observations::read_observation(pool, attempt, observation_id)
            .await
            .map_err(quarantine::proof_error)?;
        let resolution = identities::read_recorded(pool, attempt, &format!("callback-inbox-{op}"))
            .await
            .map_err(quarantine::proof_error)?
            .ok_or(PaymentStoreError::InvalidStoredData)?;
        row.validate_financial(op, &receipt.snapshot, &observation, &resolution, outcome)?;
    } else if row.observation_id.is_some() || row.resolution_id.is_some() {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    Ok(Some(outcome))
}
fn classify(resolution: &identities::Resolution) -> Disposition {
    if resolution.summary.provider_ids.len() > 1 {
        Disposition::Ambiguous
    } else if !resolution.summary.paid_provider_ids.is_empty() {
        Disposition::PaidObserved
    } else {
        Disposition::Observed
    }
}
pub(crate) async fn process_operation(
    pool: &PgPool,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
    op: &str,
) -> Result<Disposition, PaymentStoreError> {
    if !identifier(op) {
        return Err(PaymentStoreError::InvalidRequest);
    }
    if let Some(outcome) = quarantine::checked_disposition(pool, clock, op).await? {
        return Ok(outcome);
    }
    let receipt = match lookup::read_lookup(pool, op).await {
        Ok(Some(receipt)) => receipt,
        Err(PaymentStoreError::InvalidStoredData) => {
            return persist(
                pool,
                clock,
                op,
                Disposition::CustodyInvalid,
                None,
                None,
                None,
            )
            .await;
        }
        Ok(None) => return Err(PaymentStoreError::Unavailable),
        Err(error) => return Err(error),
    };
    let initial = clock
        .unix_seconds()
        .map_err(|_| PaymentStoreError::ClockUnavailable)?;
    let clock = ObservedClock::new(clock, initial);
    let clock: &dyn Clock = &clock;
    if now(clock)? < receipt.snapshot.recorded_at {
        return Err(PaymentStoreError::ClockUnavailable);
    }
    let (outcome, observation_id, resolution_id) =
        match lookup::process_lookup(pool, clock, ids, op).await {
            Ok(observation) => {
                let resolution = match identities::resolve(
                    pool,
                    clock,
                    ids,
                    &observation.snapshot.attempt_id,
                    &format!("callback-inbox-{op}"),
                )
                .await
                {
                    Ok(resolution) => resolution,
                    Err(PaymentStoreError::InvalidStoredData) => {
                        return persist(
                            pool,
                            clock,
                            op,
                            Disposition::CustodyInvalid,
                            None,
                            None,
                            Some(receipt.snapshot.recorded_at),
                        )
                        .await;
                    }
                    Err(error) => return Err(error),
                };
                if resolution.resolution.recorded_at < receipt.snapshot.recorded_at
                    || resolution.resolution.through_observation < observation.snapshot.revision
                {
                    return persist(
                        pool,
                        clock,
                        op,
                        Disposition::CustodyInvalid,
                        None,
                        None,
                        Some(receipt.snapshot.recorded_at),
                    )
                    .await;
                }
                (
                    classify(&resolution.resolution),
                    Some(observation.snapshot.id),
                    Some(resolution.resolution.id),
                )
            }
            Err(
                PaymentStoreError::NotFound
                | PaymentStoreError::Conflict
                | PaymentStoreError::Unavailable,
            ) => (Disposition::Unmatched, None, None),
            Err(PaymentStoreError::InvalidStoredData) => (Disposition::CustodyInvalid, None, None),
            Err(error) => return Err(error),
        };
    persist(
        pool,
        clock,
        op,
        outcome,
        observation_id,
        resolution_id,
        Some(receipt.snapshot.recorded_at),
    )
    .await
}
async fn persist(
    pool: &PgPool,
    clock: &dyn Clock,
    op: &str,
    outcome: Disposition,
    observation_id: Option<String>,
    resolution_id: Option<String>,
    recorded_at: Option<i64>,
) -> Result<Disposition, PaymentStoreError> {
    let source = processing_source::read(pool, op).await?;
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let at = now(clock)?;
    if at < source.created_at || recorded_at.is_some_and(|v| at < v) {
        return Err(PaymentStoreError::ClockUnavailable);
    }
    sqlx::query("INSERT INTO callback_receipt_processing(operation_id,event_id,processed_at,outcome,observation_id,resolution_id) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(operation_id) DO NOTHING")
        .bind(op).bind(source.event_id).bind(at).bind(outcome.name()).bind(observation_id).bind(resolution_id).execute(&mut *tx).await?;
    if now(clock)? < at {
        return Err(PaymentStoreError::ClockUnavailable);
    }
    tx.commit().await?;
    read_disposition(pool, op)
        .await?
        .ok_or(PaymentStoreError::InvalidStoredData)
}
pub(crate) async fn drain_receipts(
    pool: &PgPool,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
    profile: &str,
    limit: u16,
) -> Result<u16, PaymentStoreError> {
    active::audit(pool).await?;
    if !(1..=MAX_BATCH).contains(&limit) {
        return Err(PaymentStoreError::InvalidRequest);
    }
    // No predicate on the current job state or generation: older in-flight
    // responses remain visible even after a newer OPEN or terminal review stop.
    let operations:Vec<String>=sqlx::query_scalar("SELECT left(g.operation_id,33) FROM callback_inbox_generations g JOIN callback_inbox_active a ON a.intake_id=g.intake_id JOIN callback_intakes i ON i.id=g.intake_id JOIN callback_lookup_receipts r ON r.operation_id=g.operation_id WHERE a.profile_id=$1 AND NOT EXISTS(SELECT 1 FROM callback_receipt_processing p WHERE p.operation_id=g.operation_id) ORDER BY r.recorded_at,g.operation_id LIMIT $2")
        .bind(profile).bind(i64::from(limit)).fetch_all(pool).await?;
    let mut drained = 0;
    for op in operations {
        process_operation(pool, clock, ids, &op).await?;
        drained += 1;
    }
    Ok(drained)
}

#[cfg(test)]
#[path = "../../../../tests/support/callback_inbox_processing_proofs.rs"]
pub(crate) mod proof_tests;
