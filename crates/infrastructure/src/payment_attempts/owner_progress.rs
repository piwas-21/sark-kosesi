//! Gross payment evidence for an exact owner read under the original attempt claim.
use super::*;

pub(crate) enum GrossPaymentSource {
    NoDispatch {
        prepared_at: Option<i64>,
    },
    Dispatch {
        source_at: i64,
        state: Option<recognition::State>,
    },
}

/// The caller holds the basket and has validated the exact order/account and
/// claimed its optional original attempt.
pub(crate) async fn owner_gross_progress_locked(
    tx: &mut Transaction<'_, Postgres>,
    order: &commerce_domain::checkout_orders::OrderIdentity,
    account: Option<i64>,
    attempt: Option<&str>,
) -> Result<GrossPaymentSource, PaymentStoreError> {
    let frozen = match attempt {
        Some(id) => Some(
            stored::by_id(
                tx,
                &order.basket_id,
                id,
                checkout_orders::OrderAccess::Owned(account),
            )
            .await?,
        ),
        None => None,
    };
    // The claimed ID comes from the exact order row and by_id validates its
    // frozen binding against that same row and account.
    let binding = frozen.as_ref().map(|frozen| &frozen.snapshot().binding);
    let prepared_at = binding.map(|binding| binding.created_at);
    prepared_event(tx, &order.id, binding).await?;
    let mut source = GrossPaymentSource::NoDispatch { prepared_at };
    if let Some(frozen) = frozen.as_ref() {
        let attempt_id = &frozen.snapshot().binding.id;
        match evidence::started(tx, frozen).await {
            Ok((id, started_at)) => {
                owner_dispatch::reconciliation_event(tx, &order.id, attempt_id, &id, started_at)
                    .await?;
                let (_, recognition) =
                    Box::pin(recognition::current_in_transaction(tx, attempt_id)).await?;
                source = GrossPaymentSource::Dispatch {
                    source_at: recognition.as_ref().map_or(started_at, |r| r.recorded_at),
                    state: recognition.map(|r| r.state),
                };
            }
            Err(PaymentStoreError::Unavailable) => {}
            Err(other) => return Err(other),
        }
    }
    match source {
        GrossPaymentSource::Dispatch { state: Some(_), .. } => {}
        _ => {
            let no_dispatch = matches!(source, GrossPaymentSource::NoDispatch { .. });
            let inconsistent: bool = sqlx::query_scalar(
                "SELECT ($2::boolean AND EXISTS(SELECT 1 FROM public.commerce_outbox
                   WHERE aggregate_id=$1 AND kind='payment.reconciliation_required.v1'))
                 OR EXISTS(SELECT 1 FROM public.payment_recognitions WHERE order_id=$1)
                 OR EXISTS(SELECT 1 FROM public.payment_recognition_current WHERE order_id=$1)
                 OR EXISTS(SELECT 1 FROM public.commerce_outbox
                   WHERE aggregate_id=$1 AND kind='payment.recognized.v1')",
            )
            .bind(&order.id)
            .bind(no_dispatch)
            .fetch_one(&mut **tx)
            .await?;
            if inconsistent {
                return Err(PaymentStoreError::InvalidStoredData);
            }
        }
    }
    Ok(source)
}

async fn prepared_event(
    tx: &mut Transaction<'_, Postgres>,
    order: &str,
    binding: Option<&AttemptBinding>,
) -> Result<(), PaymentStoreError> {
    let events: Vec<bool> = sqlx::query_scalar(
        "SELECT COALESCE(id ~ '^[a-f0-9]{32}$' AND created_at=$3
         AND payload=jsonb_build_object('schema_version',1,'order_id',$1::text,'attempt_id',$2::text),false)
         FROM public.commerce_outbox WHERE aggregate_id=$1 AND kind='payment.attempt_prepared.v1' LIMIT 2",
    )
    .bind(order)
    .bind(binding.map(|binding| binding.id.as_str()))
    .bind(binding.map(|binding| binding.created_at))
    .fetch_all(&mut **tx)
    .await?;
    match (binding, events.as_slice()) {
        (None, []) | (Some(_), [true]) => Ok(()),
        _ => Err(PaymentStoreError::InvalidStoredData),
    }
}
