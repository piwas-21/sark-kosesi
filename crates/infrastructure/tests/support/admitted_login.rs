//! Common admission configuration for existing session and linked-order fixtures.
use commerce_application::request_context::Clock;
use commerce_infrastructure::{
    customer_credential_admission::{AdmissionKey, AdmissionLimits},
    customer_sessions::{self, IssuedSession, SessionError},
};
use sqlx::PgPool;

pub async fn login(
    pool: &PgPool,
    email: &str,
    password: &str,
    clock: &dyn Clock,
) -> Result<IssuedSession, SessionError> {
    let key = AdmissionKey::new(1, [7; 32]).expect("synthetic admission key");
    let limits = AdmissionLimits::new(1000, 100).expect("fixture admission limits");
    customer_sessions::login(pool, email, password, clock, &key, limits).await
}
