//! Read-only probe by default; explicit local/test worker passes are bounded.
#![forbid(unsafe_code)]
use commerce_infrastructure::payment_attempts::reversal_item_supervision::inbox::Cursor;
use commerce_infrastructure::runtime::SystemClock;
use commerce_infrastructure::{connect, readiness};
use commerce_worker::expire_batch;
mod callbacks;
mod paid_parent;
mod payments;
mod reversal_items;
mod reversal_pages;
mod staff_input;
mod staff_packing;
mod staff_reversal_review;

#[derive(PartialEq, Eq)]
enum Mode {
    Probe,
    Expire,
    Reconcile,
    InspectPaymentRecovery(String),
    InspectPaymentCustody(String),
    Callbacks,
    ReversalPages,
    ReversalItems,
    EnrollReversalItem(reversal_items::Enrollment),
    InspectReversalItem(String),
    ReviewReversalItem(String),
    InspectPacking(String),
    ReviewReversalInbox(Option<Cursor>),
    EnrollReversal(reversal_pages::Enrollment),
    InspectReversal(String),
    InspectPaidParent(paid_parent::Inspection),
    InspectPaidParentSet(String),
}

fn parse_mode(args: &[String]) -> Result<Mode, std::io::Error> {
    match args {
        [] => Ok(Mode::Probe),
        [argument] if argument == "--expire-holds" => Ok(Mode::Expire),
        [argument] if argument == "--reconcile-payments" => Ok(Mode::Reconcile),
        [argument, attempt]
            if argument == "--inspect-payment-recovery" && reversal_pages::identifier(attempt) =>
        {
            Ok(Mode::InspectPaymentRecovery(attempt.clone()))
        }
        [argument, attempt]
            if argument == "--inspect-payment-custody" && reversal_pages::identifier(attempt) =>
        {
            Ok(Mode::InspectPaymentCustody(attempt.clone()))
        }
        [argument] if argument == "--reconcile-callbacks" => Ok(Mode::Callbacks),
        [argument] if argument == "--reconcile-reversal-pages" => Ok(Mode::ReversalPages),
        [argument] if argument == "--reconcile-reversal-items" => Ok(Mode::ReversalItems),
        [argument, attempt, observation, page, ordinal, effect]
            if argument == "--enroll-reversal-item" =>
        {
            reversal_items::Enrollment::parse(attempt, observation, page, ordinal, effect)
                .map(Mode::EnrollReversalItem)
        }
        [argument, job]
            if argument == "--inspect-reversal-item" && reversal_pages::identifier(job) =>
        {
            Ok(Mode::InspectReversalItem(job.clone()))
        }
        [argument, job]
            if argument == "--review-reversal-item" && reversal_pages::identifier(job) =>
        {
            Ok(Mode::ReviewReversalItem(job.clone()))
        }
        [argument, order]
            if argument == "--inspect-packing"
                && commerce_domain::order_stock::identifier(order) =>
        {
            Ok(Mode::InspectPacking(order.clone()))
        }
        [argument] if argument == "--review-reversal-inbox" => Ok(Mode::ReviewReversalInbox(None)),
        [argument, timestamp, job] if argument == "--review-reversal-inbox" => {
            let time = timestamp
                .parse::<i64>()
                .ok()
                .filter(|time| time.to_string() == *timestamp)
                .ok_or_else(|| std::io::Error::other("unsupported worker command"))?;
            let cursor = Cursor::new(time, job)
                .map_err(|_| std::io::Error::other("unsupported worker command"))?;
            Ok(Mode::ReviewReversalInbox(Some(cursor)))
        }
        [argument, attempt, observation, kind, effect] if argument == "--enroll-reversal-scan" => {
            reversal_pages::Enrollment::parse(attempt, observation, kind, effect)
                .map(Mode::EnrollReversal)
        }
        [argument, job]
            if argument == "--inspect-reversal-scan" && reversal_pages::identifier(job) =>
        {
            Ok(Mode::InspectReversal(job.clone()))
        }
        [argument, attempt, effect] if argument == "--inspect-paid-parent" => {
            paid_parent::Inspection::parse(attempt, effect).map(Mode::InspectPaidParent)
        }
        [argument, attempt]
            if argument == "--inspect-paid-parent-set" && reversal_pages::identifier(attempt) =>
        {
            Ok(Mode::InspectPaidParentSet(attempt.clone()))
        }
        _ => Err(std::io::Error::other("unsupported worker command")),
    }
}
fn mode() -> Result<Mode, std::io::Error> {
    let args = std::env::args_os()
        .skip(1)
        .map(|arg| {
            arg.into_string()
                .map_err(|_| std::io::Error::other("unsupported worker command"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    parse_mode(&args)
}
fn batch_limit() -> Result<u16, std::io::Error> {
    let raw = match std::env::var("HOLD_EXPIRY_BATCH") {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => return Ok(25),
        Err(_) => return Err(std::io::Error::other("invalid HOLD_EXPIRY_BATCH")),
    };
    raw.parse::<u16>()
        .ok()
        .filter(|limit| (1..=100).contains(limit))
        .ok_or_else(|| std::io::Error::other("invalid HOLD_EXPIRY_BATCH"))
}

#[tokio::main]
async fn main() -> Result<(), std::io::Error> {
    if !matches!(std::env::var("APP_ENV").as_deref(), Ok("local" | "test")) {
        return Err(std::io::Error::other("production is disabled"));
    }
    let mode = mode()?;
    let subscriber = tracing_subscriber::fmt().json();
    if matches!(mode, Mode::InspectPacking(_)) {
        subscriber.with_writer(std::io::stderr).init();
    } else {
        subscriber.init();
    }
    // Read before any DB or provider configuration; never accept a token argument.
    let staff_token = if matches!(
        mode,
        Mode::ReviewReversalItem(_) | Mode::ReviewReversalInbox(_) | Mode::InspectPacking(_)
    ) {
        Some(staff_input::read_token()?)
    } else {
        None
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
        Mode::InspectPacking(order) => {
            let token = staff_token
                .ok_or_else(|| std::io::Error::other("packing inspection unavailable"))?;
            staff_packing::inspect(&pool, &token, &order).await?
        }
        Mode::EnrollReversal(intent) => reversal_pages::run(&pool, Some(intent)).await?,
        Mode::InspectReversal(id) => reversal_pages::inspect(&pool, &id).await?,
        Mode::InspectPaidParent(intent) => paid_parent::inspect(&pool, intent).await?,
        Mode::InspectPaidParentSet(attempt) => paid_parent::inspect_set(&pool, &attempt).await?,
        Mode::Probe => tracing::info!(code = "worker_dependencies_ready", effects_enabled = false),
    }
    Ok(())
}

#[cfg(test)]
#[path = "../tests/support/command_modes.rs"]
mod command_modes_tests;
