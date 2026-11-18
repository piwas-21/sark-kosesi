//! One-candidate live discovery of recorded requests; no disposition or state transition.
use super::{
    identifier,
    inspection::{self, CancellationInspection, InspectionError},
};
use crate::staff_access::{SessionToken, lock_staff};
use commerce_application::{
    request_context::{Clock, ObservedClock},
    staff_access::Capability,
};
use sqlx::{PgPool, Postgres, Transaction};

/// Returns the next validated recorded request in bytewise order. Pass the
/// returned order ID as `after`; refresh from None to include earlier inserts.
/// This scans request records, not orphan events or completed dispositions.
pub async fn next_post_dispatch_request(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    after: Option<&str>,
) -> Result<Option<CancellationInspection>, InspectionError> {
    if after.is_some_and(|id| !identifier(id)) {
        return Err(InspectionError::InvalidRequest);
    }
    let observed = ObservedClock::new(clock, 0);
    let mut access = lock_staff(pool, token, &observed, Capability::CancellationReview)
        .await
        .map_err(inspection::staff)?;
    let result = next(&mut access.tx, after).await;
    inspection::finish(access, &observed, result)
}

async fn next(
    tx: &mut Transaction<'_, Postgres>,
    after: Option<&str>,
) -> Result<Option<CancellationInspection>, InspectionError> {
    let candidate: Option<String> = if let Some(after) = after {
        sqlx::query_scalar("SELECT left(order_id,33) FROM public.checkout_post_dispatch_requests WHERE order_id COLLATE \"C\">$1 COLLATE \"C\" ORDER BY order_id COLLATE \"C\" LIMIT 1")
            .bind(after).fetch_optional(&mut **tx).await?
    } else {
        sqlx::query_scalar("SELECT left(order_id,33) FROM public.checkout_post_dispatch_requests ORDER BY order_id COLLATE \"C\" LIMIT 1")
            .fetch_optional(&mut **tx).await?
    };
    let Some(order) = candidate else {
        return Ok(None);
    };
    if !identifier(&order) {
        return Err(InspectionError::InvalidStoredData);
    }
    let request = inspection::inspect_locked(tx, &order)
        .await?
        .ok_or(InspectionError::InvalidStoredData)?;
    Ok(Some(request))
}
