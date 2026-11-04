//! Shared synthetic quota configuration for integration tests of proof operations.
//! Every call still commits the real PostgreSQL admission debit before proof work.
use commerce_application::request_context::Clock;
use commerce_infrastructure::{
    customer_credential_admission::{AdmissionKey, AdmissionLimits},
    email_verification::{VerificationError, VerificationToken},
    verified_customer_accounts::VerifiedCustomerAccountId,
};
use sqlx::PgPool;

fn key() -> AdmissionKey {
    AdmissionKey::new(1, [7; 32]).expect("synthetic admission key")
}
fn limits() -> AdmissionLimits {
    AdmissionLimits::new(1000, 100).expect("fixture admission limits")
}

pub mod email_verification {
    use super::*;
    pub async fn issue(
        pool: &PgPool,
        email: &str,
        clock: &dyn Clock,
    ) -> Result<VerificationToken, VerificationError> {
        commerce_infrastructure::email_verification::issue(pool, email, clock, &key(), limits())
            .await
    }
    pub async fn consume(
        pool: &PgPool,
        email: &str,
        token: &VerificationToken,
        clock: &dyn Clock,
    ) -> Result<(), VerificationError> {
        commerce_infrastructure::email_verification::consume(
            pool,
            email,
            token,
            clock,
            &key(),
            limits(),
        )
        .await
    }
}
pub mod verified_customer_accounts {
    use super::*;
    pub async fn establish(
        pool: &PgPool,
        email: &str,
        token: &VerificationToken,
        clock: &dyn Clock,
    ) -> Result<VerifiedCustomerAccountId, VerificationError> {
        commerce_infrastructure::verified_customer_accounts::establish(
            pool,
            email,
            token,
            clock,
            &key(),
            limits(),
        )
        .await
    }
}
pub mod customer_passwords {
    use super::*;
    use commerce_infrastructure::customer_passwords::{PasswordChangeToken, PasswordError};
    pub async fn issue(
        pool: &PgPool,
        email: &str,
        clock: &dyn Clock,
    ) -> Result<PasswordChangeToken, PasswordError> {
        commerce_infrastructure::customer_passwords::issue(pool, email, clock, &key(), limits())
            .await
    }
    pub async fn replace(
        pool: &PgPool,
        email: &str,
        token: &PasswordChangeToken,
        password: &str,
        clock: &dyn Clock,
    ) -> Result<i32, PasswordError> {
        commerce_infrastructure::customer_passwords::replace(
            pool,
            email,
            token,
            password,
            clock,
            &key(),
            limits(),
        )
        .await
    }
}
