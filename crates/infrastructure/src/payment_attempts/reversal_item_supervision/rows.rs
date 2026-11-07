use super::*;
#[derive(sqlx::FromRow)]
struct Row {
    id: String,
    attempt_id: String,
    observation_id: String,
    page_id: String,
    ordinal: i32,
    child_id: String,
    enrollment_key: String,
    profile_id: String,
    payment_id: String,
    reversal_kind: String,
    credential_fingerprint: String,
    state: String,
    review_reason: Option<String>,
    receipt_id: Option<String>,
    generation: i32,
    failures: i32,
    created_at: i64,
    updated_at: i64,
    due_at: i64,
    lease_until: Option<i64>,
}
fn opaque(v: &str, prefix: &str) -> bool {
    v.starts_with(prefix)
        && v.len() > prefix.len()
        && v.len() <= 256
        && v.trim() == v
        && !v.chars().any(char::is_control)
}
fn decode(r: Row) -> Result<Job, RecoveryError> {
    let kind = match r.reversal_kind.as_str() {
        "refund" => ReversalKind::Refund,
        "chargeback" => ReversalKind::Chargeback,
        _ => return Err(RecoveryError::InvalidStoredData),
    };
    let j = Job {
        id: r.id,
        attempt_id: r.attempt_id,
        observation_id: r.observation_id,
        page_id: r.page_id,
        ordinal: r.ordinal,
        child_id: r.child_id,
        enrollment_key: r.enrollment_key,
        profile_id: r.profile_id,
        payment_id: r.payment_id,
        kind,
        credential_fingerprint: r.credential_fingerprint,
        state: r.state,
        review_reason: r.review_reason,
        receipt_id: r.receipt_id,
        generation: r.generation,
        failures: r.failures,
        created_at: r.created_at,
        updated_at: r.updated_at,
        due_at: r.due_at,
        lease_until: r.lease_until,
    };
    let max = 9_007_199_254_740_991;
    if !identifier(&j.id)
        || !identifier(&j.attempt_id)
        || !identifier(&j.observation_id)
        || !identifier(&j.page_id)
        || !(1..=10).contains(&j.ordinal)
        || !opaque(
            &j.child_id,
            if kind == ReversalKind::Refund {
                "re_"
            } else {
                "chb_"
            },
        )
        || input(&j.attempt_id, &j.enrollment_key).is_err()
        || !opaque(&j.profile_id, "pfl_")
        || !opaque(&j.payment_id, "tr_")
        || !fingerprint(&j.credential_fingerprint)
        || !(0..=3).contains(&j.generation)
        || !(0..=3).contains(&j.failures)
        || !(0..=max).contains(&j.created_at)
        || !(j.created_at..=max).contains(&j.updated_at)
        || !(j.created_at..=max).contains(&j.due_at)
        || j.receipt_id.as_ref().is_some_and(|v| !identifier(v))
        || !matches!(
            j.state.as_str(),
            "pending" | "leased" | "review" | "observed"
        )
        || (j.state == "leased") != j.lease_until.is_some()
        || (j.state == "review") != j.review_reason.is_some()
        || (j.state == "observed") != j.receipt_id.is_some()
        || j.review_reason.as_deref().is_some_and(|v| {
            !matches!(
                v,
                "uncertain" | "invalid" | "retry_exhausted" | "observation_limit"
            )
        })
        || j.lease_until.is_some_and(|v| {
            j.generation == 0 || j.updated_at.checked_add(60) != Some(v) || v > max
        })
        || (j.state == "pending"
            && (j.due_at < j.updated_at || j.generation >= 3 || j.failures >= 3))
        || (matches!(j.state.as_str(), "review" | "observed") && j.due_at != j.updated_at)
        || (j.failures == 3 && j.state != "review")
    {
        return Err(RecoveryError::InvalidStoredData);
    }
    Ok(j)
}
async fn read(
    tx: &mut Transaction<'_, Postgres>,
    id: Option<&str>,
    attempt: Option<&str>,
    key: Option<&str>,
) -> Result<Option<Job>, RecoveryError> {
    sqlx::query_as::<_,Row>("SELECT left(id,33) id,left(attempt_id,33) attempt_id,left(observation_id,33) observation_id,left(page_id,33) page_id,ordinal,left(child_id,257) child_id,left(enrollment_key,129) enrollment_key,left(profile_id,257) profile_id,left(payment_id,257) payment_id,left(reversal_kind,20) reversal_kind,left(credential_fingerprint,65) credential_fingerprint,left(state,20) state,left(review_reason,32) review_reason,left(receipt_id,33) receipt_id,generation,failures,created_at,updated_at,due_at,lease_until FROM reversal_item_jobs WHERE ($1::text IS NULL OR id=$1) AND ($2::text IS NULL OR attempt_id=$2) AND ($3::text IS NULL OR enrollment_key=$3)")
        .bind(id).bind(attempt).bind(key).fetch_optional(&mut **tx).await?.map(decode).transpose()
}
pub(super) async fn load(
    tx: &mut Transaction<'_, Postgres>,
    id: &str,
) -> Result<Option<Job>, RecoveryError> {
    read(tx, Some(id), None, None).await
}
pub(super) async fn by_key(
    tx: &mut Transaction<'_, Postgres>,
    attempt: &str,
    key: &str,
) -> Result<Option<Job>, RecoveryError> {
    read(tx, None, Some(attempt), Some(key)).await
}

#[cfg(test)]
#[path = "../../../tests/support/reversal_item_supervision_rows.rs"]
mod tests;
