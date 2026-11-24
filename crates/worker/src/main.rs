//! Read-only probe by default; explicit local/test worker passes are bounded.
#![forbid(unsafe_code)]
use cli::{Mode, batch_limit, mode};
use commerce_infrastructure::runtime::SystemClock;
use commerce_infrastructure::{connect, readiness};
use commerce_worker::expire_batch;
mod callbacks;
mod cli;
mod paid_parent;
mod payments;
mod reversal_items;
mod reversal_pages;
mod staff_cancellation;
mod staff_handover;
mod staff_input;
mod staff_packing;
mod staff_packing_input;
mod staff_reversal_review;

#[tokio::main]
async fn main() -> Result<(), std::io::Error> {
    if !matches!(std::env::var("APP_ENV").as_deref(), Ok("local" | "test")) {
        return Err(std::io::Error::other("production is disabled"));
    }
    let mode = mode()?;
    let subscriber = tracing_subscriber::fmt().json();
    let staff_order_read = matches!(
        mode,
        Mode::ReviewCancellation(_)
            | Mode::ReviewCancellationInbox(_)
            | Mode::InspectCancellationPacking(_)
            | Mode::InspectHandoverEvidence(_)
    );
    if staff_order_read
        || matches!(
            mode,
            Mode::InspectPacking(_) | Mode::ReleasePacking { .. } | Mode::RecordPacking { .. }
        )
    {
        subscriber.with_writer(std::io::stderr).init();
    } else {
        subscriber.init();
    }
    // Read before any DB or provider configuration; never accept a token argument.
    let staff_token = if staff_order_read
        || matches!(
            mode,
            Mode::ReviewReversalItem(_)
                | Mode::ReviewReversalInbox(_)
                | Mode::InspectPacking(_)
                | Mode::ReleasePacking { .. }
                | Mode::RecordPacking { .. }
        ) {
        Some(staff_input::read_token()?)
    } else {
        None
    };
    let packing_input = match &mode {
        Mode::RecordPacking { path, .. } => Some(staff_packing_input::read(path)?),
        _ => None,
    };
    let limit = if mode == Mode::Expire {
        batch_limit()?
    } else {
        0
    };
    let database_url = std::env::var("DATABASE_URL")
        .map_err(|_| std::io::Error::other("DATABASE_URL required"))?;
    let pool = connect(&database_url)
        .await
        .map_err(|_| std::io::Error::other("database connection failed"))?;
    readiness(&pool)
        .await
        .map_err(|_| std::io::Error::other("database unavailable"))?;
    match mode {
        Mode::Expire => {
            let batch = expire_batch(&pool, &SystemClock, limit)
                .await
                .map_err(|_| std::io::Error::other("hold expiry unavailable"))?;
            tracing::info!(
                code = "hold_expiry_batch",
                expired = batch.expired,
                failed = batch.failed,
                limit
            );
            if batch.failed > 0 {
                return Err(std::io::Error::other("hold expiry requires retry"));
            }
        }
        Mode::Reconcile => payments::run(&pool).await?,
        Mode::InspectPaymentRecovery(attempt) => payments::inspect(&pool, &attempt).await?,
        Mode::InspectPaymentCustody(attempt) => payments::inspect_custody(&pool, &attempt).await?,
        Mode::Callbacks => callbacks::run(&pool).await?,
        Mode::ReversalPages => reversal_pages::run(&pool, None).await?,
        Mode::ReversalItems => reversal_items::run(&pool, None).await?,
        Mode::EnrollReversalItem(intent) => reversal_items::run(&pool, Some(intent)).await?,
        Mode::InspectReversalItem(id) => reversal_items::inspect(&pool, &id).await?,
        Mode::ReviewReversalItem(id) => {
            let token =
                staff_token.ok_or_else(|| std::io::Error::other("staff review unavailable"))?;
            staff_reversal_review::inspect(&pool, &token, &id).await?
        }
        Mode::ReviewReversalInbox(after) => {
            let token =
                staff_token.ok_or_else(|| std::io::Error::other("staff review unavailable"))?;
            staff_reversal_review::next(&pool, &token, after.as_ref()).await?
        }
        Mode::ReviewCancellation(order) => {
            let token = staff_token
                .as_ref()
                .ok_or_else(|| std::io::Error::other("staff cancellation review unavailable"))?;
            staff_cancellation::inspect(&pool, token, &order).await?
        }
        Mode::ReviewCancellationInbox(after) => {
            let token = staff_token
                .as_ref()
                .ok_or_else(|| std::io::Error::other("staff cancellation review unavailable"))?;
            staff_cancellation::next(&pool, token, after.as_deref()).await?
        }
        Mode::InspectCancellationPacking(order) => {
            let token = staff_token
                .as_ref()
                .ok_or_else(|| std::io::Error::other("staff cancellation review unavailable"))?;
            staff_cancellation::inspect_packing(&pool, token, &order).await?
        }
        Mode::InspectHandoverEvidence(order) => {
            let token = staff_token
                .as_ref()
                .ok_or_else(|| std::io::Error::other("staff handover inspection unavailable"))?;
            staff_handover::inspect(&pool, token, &order).await?
        }
        Mode::InspectPacking(order) => {
            let token = staff_token
                .ok_or_else(|| std::io::Error::other("packing inspection unavailable"))?;
            staff_packing::inspect(&pool, &token, &order).await?
        }
        Mode::RecordPacking { order, effect, .. } => {
            let token =
                staff_token.ok_or_else(|| std::io::Error::other("packing record unavailable"))?;
            let input =
                packing_input.ok_or_else(|| std::io::Error::other("packing record unavailable"))?;
            staff_packing::record(&pool, &token, &order, &effect, &input).await?
        }
        Mode::ReleasePacking {
            order,
            target,
            effect,
        } => {
            let token =
                staff_token.ok_or_else(|| std::io::Error::other("packing release unavailable"))?;
            staff_packing::release(&pool, &token, &order, &target, &effect).await?
        }
        Mode::EnrollReversal(intent) => reversal_pages::run(&pool, Some(intent)).await?,
        Mode::InspectReversal(id) => reversal_pages::inspect(&pool, &id).await?,
        Mode::InspectPaidParent(intent) => paid_parent::inspect(&pool, intent).await?,
        Mode::InspectPaidParentSet(attempt) => paid_parent::inspect_set(&pool, &attempt).await?,
        Mode::Probe => tracing::info!(code = "worker_dependencies_ready", effects_enabled = false),
    }
    Ok(())
}
