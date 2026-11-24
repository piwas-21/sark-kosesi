//! One local SMTP transaction for the reserved synthetic guest-order buyer.
use super::{opening, outcome, *};
pub use crate::email_verification::delivery::test_smtp::LocalTestEndpoint;
use crate::email_verification::delivery::{
    outcome::TransportOutcome as SmtpOutcome,
    test_smtp::{GUEST_LINK_RECIPIENT, Message, send_proof},
};

/// A fresh authorized opening sends one proof to the numeric loopback sink.
/// A cancelled or unrecorded result leaves the committed start closed to replay.
pub async fn deliver(
    pool: &PgPool,
    verifier: &[u8; 32],
    clock: &dyn Clock,
    key: &DeliveryKey,
    endpoint: &LocalTestEndpoint,
) -> Result<OpenOutcome, LinkError> {
    let observed = ObservedClock::new(clock, 0);
    let Some(opened) =
        opening::open(pool, verifier, &observed, key, Some(GUEST_LINK_RECIPIENT)).await?
    else {
        return Ok(OpenOutcome::AlreadyStarted);
    };
    // The opening revalidates the exact identity and releases all database claims.
    let result = send_proof(
        endpoint,
        Message::GuestOrderLink,
        opened.token.expose_bytes(),
    )
    .await
    .map_err(|_| LinkError::Unavailable)?;
    let outcome = match result {
        SmtpOutcome::Accepted => outcome::TransportOutcome::Accepted,
        SmtpOutcome::ExplicitRejection => outcome::TransportOutcome::ExplicitRejection,
        SmtpOutcome::Uncertain => outcome::TransportOutcome::Uncertain,
    };
    let opening::FreshOpening {
        identity,
        token,
        permit,
    } = opened;
    drop((identity, token));
    outcome::record(pool, &permit, &observed, outcome).await?;
    Ok(OpenOutcome::Opened)
}
