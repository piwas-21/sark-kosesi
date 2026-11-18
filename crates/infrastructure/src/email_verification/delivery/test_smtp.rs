//! Explicitly invoked local SMTP rehearsal for one reserved synthetic identity.
//! This is not a production mail sender or a provider integration.
use std::fmt::Write;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use lettre::address::Envelope;
use lettre::transport::smtp::extension::ClientId;
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};

use super::{opening, outcome, *};

const RECIPIENT: &str = "verify@example.test";
const SENDER: &str = "sender@example.test";
const TIMEOUT: Duration = Duration::from_secs(2);

/// The constructor accepts only numeric localhost, never names or arbitrary addresses.
pub struct LocalTestEndpoint(SocketAddr);

impl LocalTestEndpoint {
    pub fn new(explicitly_enabled: bool, endpoint: SocketAddr) -> Result<Self, VerificationError> {
        if !explicitly_enabled
            || endpoint.port() == 0
            || !matches!(
                endpoint.ip(),
                IpAddr::V4(Ipv4Addr::LOCALHOST) | IpAddr::V6(Ipv6Addr::LOCALHOST)
            )
        {
            return Err(VerificationError::Unavailable);
        }
        Ok(Self(endpoint))
    }
}

/// One start, one local SMTP transaction, and one bounded durable outcome.
/// Cancellation and failures to record leave an unknown start; no task is detached.
pub async fn deliver(
    pool: &PgPool,
    verifier: &[u8; 32],
    clock: &dyn Clock,
    key: &VerificationDeliveryKey,
    endpoint: &LocalTestEndpoint,
) -> Result<opening::OpenOutcome, VerificationError> {
    // Avoid starting a challenge for any other mailbox. The authenticated open
    // repeats this check after obtaining the one-use permit.
    let synthetic: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM email_verification_challenges WHERE verifier=$1 AND email_identity=$2)")
        .bind(verifier.as_slice()).bind(RECIPIENT).fetch_one(pool).await
        .map_err(|_| VerificationError::Unavailable)?;
    if !synthetic {
        return Err(VerificationError::Denied);
    }
    let observed = ObservedClock::new(clock, 0);
    let Some(opened) = opening::open(pool, verifier, &observed, key).await? else {
        return Ok(opening::OpenOutcome::AlreadyStarted);
    };
    if opened.identity.as_str() != RECIPIENT {
        return Err(VerificationError::Unavailable);
    }
    let result = send(endpoint, &opened.bearer).await?;
    let opening::FreshOpening {
        identity,
        bearer,
        permit,
    } = opened;
    drop((identity, bearer));
    outcome::record(pool, &permit, key, &observed, result).await?;
    Ok(opening::OpenOutcome::Opened)
}

async fn send(
    endpoint: &LocalTestEndpoint,
    bearer: &[u8; 32],
) -> Result<outcome::TransportOutcome, VerificationError> {
    let envelope = Envelope::new(
        Some(SENDER.parse().map_err(|_| VerificationError::Unavailable)?),
        vec![
            RECIPIENT
                .parse()
                .map_err(|_| VerificationError::Unavailable)?,
        ],
    )
    .map_err(|_| VerificationError::Unavailable)?;
    // Fixed ASCII headers and body: neither input can inject a header. The proof
    // is a test-only hex value, not a public redemption URL.
    let mut message = Zeroizing::new(String::from(
        "From: sender@example.test\r\nTo: verify@example.test\r\nSubject: Synthetic verification\r\nContent-Type: text/plain; charset=us-ascii\r\n\r\nproof=",
    ));
    for byte in bearer {
        write!(&mut *message, "{byte:02x}").map_err(|_| VerificationError::Unavailable)?;
    }
    message.push_str("\r\n");
    let transport =
        AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(endpoint.0.ip().to_string())
            .port(endpoint.0.port())
            .hello_name(ClientId::Domain("example.test".into()))
            .timeout(Some(TIMEOUT))
            .build();
    let result =
        tokio::time::timeout(TIMEOUT, transport.send_raw(&envelope, message.as_bytes())).await;
    Ok(match result {
        Ok(Ok(_)) => outcome::TransportOutcome::Accepted,
        Ok(Err(error)) if error.is_transient() || error.is_permanent() => {
            outcome::TransportOutcome::ExplicitRejection
        }
        _ => outcome::TransportOutcome::Uncertain,
    })
}
