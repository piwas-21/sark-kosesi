//! Private verified identity establishment. This grants no session or order authority.
use commerce_application::request_context::Clock;
use sqlx::PgPool;

use super::{self as email_verification, VerificationError, VerificationToken};

/// Identifies a committed private account. It grants no authentication authority.
pub struct VerifiedCustomerAccountId(i64);
impl VerifiedCustomerAccountId {
    pub fn value(&self) -> i64 {
        self.0
    }
}

/// Consumes exact proof and creates its account as one indivisible effect.
pub async fn establish(
    pool: &PgPool,
    email: &str,
    token: &VerificationToken,
    clock: &dyn Clock,
) -> Result<VerifiedCustomerAccountId, VerificationError> {
    let mut tx = email_verification::begin(pool).await?;
    let proof = email_verification::consume_in_transaction(&mut tx, email, token, clock).await?;
    let result: Result<Option<(i64, String, Vec<u8>, i64)>, sqlx::Error> = sqlx::query_as(
        "INSERT INTO verified_customer_accounts(email_identity,verification_verifier,verified_at) VALUES ($1,$2,$3) RETURNING id,email_identity,verification_verifier,verified_at"
    ).bind(&proof.identity).bind(proof.verifier.as_slice()).bind(proof.captured)
        .fetch_optional(&mut *tx).await;
    let row = match result {
        Ok(row) => row,
        Err(sqlx::Error::Database(error)) if error.code().as_deref() == Some("23505") => {
            return Err(VerificationError::Denied);
        }
        Err(_) => return Err(VerificationError::Unavailable),
    };
    let Some((id, identity, verifier, verified_at)) = row else {
        return Err(VerificationError::Unavailable);
    };
    if identity != proof.identity || verifier != proof.verifier || verified_at != proof.captured {
        return Err(VerificationError::Unavailable);
    }
    proof.check(clock)?;
    tx.commit()
        .await
        .map_err(|_| VerificationError::Unavailable)?;
    Ok(VerifiedCustomerAccountId(id))
}
