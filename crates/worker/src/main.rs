//! Read-only probe by default; explicit local/test worker passes are bounded.
#![forbid(unsafe_code)]
use commerce_infrastructure::runtime::SystemClock;
use commerce_infrastructure::{connect, readiness};
use commerce_worker::expire_batch;
mod callbacks;
mod payments;
mod reversal_pages;

#[derive(PartialEq, Eq)]
enum Mode {
    Probe,
    Expire,
    Reconcile,
    Callbacks,
    ReversalPages,
    EnrollReversal(reversal_pages::Enrollment),
    InspectReversal(String),
}

fn parse_mode(args: &[String]) -> Result<Mode, std::io::Error> {
    match args {
        [] => Ok(Mode::Probe),
        [argument] if argument == "--expire-holds" => Ok(Mode::Expire),
        [argument] if argument == "--reconcile-payments" => Ok(Mode::Reconcile),
        [argument] if argument == "--reconcile-callbacks" => Ok(Mode::Callbacks),
        [argument] if argument == "--reconcile-reversal-pages" => Ok(Mode::ReversalPages),
        [argument, attempt, observation, kind, effect] if argument == "--enroll-reversal-scan" => {
            reversal_pages::Enrollment::parse(attempt, observation, kind, effect)
                .map(Mode::EnrollReversal)
        }
        [argument, job]
            if argument == "--inspect-reversal-scan" && reversal_pages::identifier(job) =>
        {
            Ok(Mode::InspectReversal(job.clone()))
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
    tracing_subscriber::fmt().json().init();
    if !matches!(std::env::var("APP_ENV").as_deref(), Ok("local" | "test")) {
        return Err(std::io::Error::other("production is disabled"));
    }
    let mode = mode()?;
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
        Mode::Callbacks => callbacks::run(&pool).await?,
        Mode::ReversalPages => reversal_pages::run(&pool, None).await?,
        Mode::EnrollReversal(intent) => reversal_pages::run(&pool, Some(intent)).await?,
        Mode::InspectReversal(id) => reversal_pages::inspect(&pool, &id).await?,
        Mode::Probe => tracing::info!(code = "worker_dependencies_ready", effects_enabled = false),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).into()).collect()
    }
    #[test]
    fn reversal_commands_require_exact_valid_arguments() {
        let id = "0123456789abcdef0123456789abcdef";
        assert!(matches!(parse_mode(&args(&[])), Ok(Mode::Probe)));
        assert!(matches!(
            parse_mode(&args(&["--reconcile-reversal-pages"])),
            Ok(Mode::ReversalPages)
        ));
        for kind in ["refund", "chargeback"] {
            assert!(matches!(
                parse_mode(&args(&[
                    "--enroll-reversal-scan",
                    id,
                    id,
                    kind,
                    "effect-123"
                ])),
                Ok(Mode::EnrollReversal(_))
            ));
        }
        assert!(matches!(
            parse_mode(&args(&["--inspect-reversal-scan", id])),
            Ok(Mode::InspectReversal(_))
        ));
        for command in [
            vec!["--reconcile-reversal-pages", "extra"],
            vec!["--enroll-reversal-scan", id, id, "refund"],
            vec!["--enroll-reversal-scan", id, id, "refund", "key", "extra"],
            vec!["--enroll-reversal-scan", "bad", id, "refund", "key"],
            vec!["--enroll-reversal-scan", id, "bad", "refund", "key"],
            vec!["--enroll-reversal-scan", id, id, "Refund", "key"],
            vec!["--enroll-reversal-scan", id, id, "refund", "secret_value"],
            vec!["--inspect-reversal-scan", "bad"],
        ] {
            assert!(parse_mode(&args(&command)).is_err());
        }
    }
}
