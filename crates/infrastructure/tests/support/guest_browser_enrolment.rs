//! Guarded enrolment for one reserved identity in the disposable browser harness.
use crate::guarded_pool;
use commerce_application::request_context::Clock;
use commerce_infrastructure::{
    customer_credential_admission::{AdmissionKey, AdmissionLimits},
    customer_passwords::{self, BlocklistManifest, PasswordBlocklist, PasswordCandidate},
    runtime::SystemClock,
};
use sqlx::PgPool;
use zeroize::Zeroizing;

const EMAIL: &str = "verify@example.test";
const OTHER_EMAIL: &str = "unrelated-enrolment@example.test";
const PASSWORD: &str = "synthetic verification fixture passphrase";

pub(super) fn verification_admission() -> (AdmissionKey, AdmissionLimits) {
    let hex = Zeroizing::new(
        std::env::var("TEST_VERIFICATION_ADMISSION_KEY").expect("isolated verification key"),
    );
    assert!(
        hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "invalid isolated verification key representation"
    );
    let mut secret = Zeroizing::new([0; 32]);
    for (byte, pair) in secret.iter_mut().zip(hex.as_bytes().as_chunks::<2>().0) {
        let value = std::str::from_utf8(pair).expect("lowercase hexadecimal");
        *byte = u8::from_str_radix(value, 16).expect("lowercase hexadecimal");
    }
    (
        AdmissionKey::new(1, *secret).expect("verification admission key"),
        AdmissionLimits::new(120, 5).expect("same verification admission limits as the API"),
    )
}

pub(super) fn password_policy(now: u64) -> (AdmissionKey, AdmissionLimits, PasswordBlocklist) {
    let at = i64::try_from(now).expect("fixture clock");
    let manifest = BlocklistManifest {
        version: "synthetic-browser-v1".into(),
        provenance: "local synthetic fixture".into(),
        qualification_ref: "not an operational corpus".into(),
        valid_from: at - 1,
        valid_until: at + 3600,
        digests: vec![[0; 32]],
    };
    let integrity = manifest.integrity_digest();
    (
        AdmissionKey::new(1, [7; 32]).expect("isolated password admission key"),
        AdmissionLimits::new(1000, 100).expect("isolated password admission limits"),
        PasswordBlocklist::new(manifest, integrity).expect("synthetic browser blocklist"),
    )
}

#[cfg(feature = "test-verification-smtp")]
#[tokio::test]
#[ignore = "browser harness delivers one reserved synthetic proof to its loopback mailbox"]
async fn deliver_browser_enrolment_proof() {
    use commerce_infrastructure::email_verification::delivery::{
        VerificationDeliveryKey, issue_for_delivery,
        opening::OpenOutcome,
        outcome::{self, DeliveryState, TransportOutcome},
        register_key,
        test_smtp::{LocalTestEndpoint, deliver},
    };
    use std::net::SocketAddr;

    let pool = guarded_pool().await;
    let port = std::env::var("GUEST_BROWSER_VERIFICATION_SMTP_PORT")
        .expect("guarded verification sink port")
        .parse::<u16>()
        .expect("numeric loopback sink port");
    let endpoint = LocalTestEndpoint::new(true, SocketAddr::from(([127, 0, 0, 1], port)))
        .expect("explicit loopback sink");
    let (admission, limits) = verification_admission();
    let key = VerificationDeliveryKey::capture("synthetic-browser-enrolment", [44; 32])
        .expect("isolated delivery key");
    register_key(&pool, &SystemClock, &key)
        .await
        .expect("register owned test delivery key");
    let receipt = issue_for_delivery(&pool, EMAIL, &SystemClock, &admission, limits, &key)
        .await
        .expect("commit reserved challenge and envelope");
    assert_eq!(
        deliver(&pool, &receipt.verifier, &SystemClock, &key, &endpoint).await,
        Ok(OpenOutcome::Opened)
    );
    assert_eq!(
        outcome::inspect(&pool, &receipt.verifier).await,
        Ok(DeliveryState::Recorded(TransportOutcome::Accepted))
    );
    assert_eq!(
        deliver(&pool, &receipt.verifier, &SystemClock, &key, &endpoint).await,
        Ok(OpenOutcome::AlreadyStarted)
    );
    pool.close().await;
}

async fn verification_custody(pool: &PgPool) -> (i64, i64, i64, i64, i64, i64) {
    sqlx::query_as(
        "WITH challenges AS (
           SELECT * FROM email_verification_challenges WHERE email_identity=$1
         ), account AS (
           SELECT a.* FROM verified_customer_accounts a WHERE a.email_identity=$1
         )
         SELECT
           (SELECT count(*) FROM challenges),
           (SELECT count(*) FROM challenges c
             JOIN email_verification_delivery_envelopes e USING(verifier)
             JOIN email_verification_delivery_starts s USING(verifier)
             JOIN email_verification_delivery_outcomes o USING(verifier)
             WHERE c.delivery_mode='envelope' AND c.delivery_transaction_id=e.transaction_id
               AND s.key_id=e.key_id AND s.nonce_counter=e.nonce_counter
               AND o.start_transaction_id=s.transaction_id AND o.outcome='accepted'
               AND o.evidence_version=1 AND o.ciphertext_sha256=pg_catalog.sha256(e.ciphertext)),
           (SELECT count(*) FROM challenges WHERE consumed_at IS NULL AND consumed_xid IS NULL),
           (SELECT count(*) FROM account),
           (SELECT count(*) FROM account a JOIN challenges c ON c.verifier=a.verification_verifier
             WHERE a.verified_at=c.consumed_at AND c.consumed_xid IS NOT NULL),
           (SELECT count(*) FROM verified_customer_accounts WHERE email_identity=$2)",
    )
    .bind(EMAIL)
    .bind(OTHER_EMAIL)
    .fetch_one(pool)
    .await
    .expect("exact reserved verification custody")
}

async fn credential_custody(pool: &PgPool) -> (i64, i64, i64, i64, i64) {
    sqlx::query_as(
        "WITH account AS (
           SELECT id FROM verified_customer_accounts WHERE email_identity=$1
         ), proofs AS (
           SELECT p.* FROM customer_password_challenges p JOIN account a ON p.account_id=a.id
         ), credentials AS (
           SELECT c.* FROM customer_password_credentials c JOIN account a ON c.account_id=a.id
         ), sessions AS (
           SELECT s.* FROM customer_sessions s JOIN account a ON s.account_id=a.id
         )
         SELECT
           (SELECT count(*) FROM proofs),
           (SELECT count(*) FROM credentials),
           (SELECT count(*) FROM proofs p JOIN credentials c ON c.challenge_verifier=p.verifier
             WHERE p.expected_revision=0 AND c.revision=1
               AND p.consumed_at=c.changed_at AND p.consumed_xid=c.changed_xid),
           (SELECT count(*) FROM sessions),
           (SELECT count(*) FROM sessions s JOIN credentials c USING(account_id)
             JOIN customer_session_csrf b ON b.session_verifier=s.verifier
             WHERE s.credential_revision=c.revision AND s.created_at>=c.changed_at
               AND s.revoked_at IS NULL AND b.created_xid=s.created_xid)",
    )
    .bind(EMAIL)
    .fetch_one(pool)
    .await
    .expect("exact initial password and bound session custody")
}

#[tokio::test]
#[ignore = "browser harness verifies denied redemption left its proof unconsumed"]
async fn verify_browser_enrolment_pristine() {
    let pool = guarded_pool().await;
    assert_eq!(verification_custody(&pool).await, (1, 1, 1, 0, 0, 0));
    assert_eq!(credential_custody(&pool).await, (0, 0, 0, 0, 0));
    pool.close().await;
}

#[tokio::test]
#[ignore = "browser harness completes the separately authorized initial password operation"]
async fn complete_browser_enrolment_password() {
    let pool = guarded_pool().await;
    assert_eq!(verification_custody(&pool).await, (1, 1, 0, 1, 1, 0));
    assert_eq!(credential_custody(&pool).await, (0, 0, 0, 0, 0));
    let now = SystemClock.unix_seconds().expect("fixture clock");
    let (admission, limits, blocklist) = password_policy(now);
    let proof = customer_passwords::issue(&pool, EMAIL, &SystemClock, &admission, limits)
        .await
        .expect("trusted revision-zero password proof");
    let pending: Option<(i32, bool)> = sqlx::query_as(
        "SELECT p.expected_revision, p.consumed_at IS NULL AND p.consumed_xid IS NULL
         FROM customer_password_challenges p
         JOIN verified_customer_accounts a ON a.id=p.account_id WHERE a.email_identity=$1",
    )
    .bind(EMAIL)
    .fetch_optional(&pool)
    .await
    .expect("exact pending password challenge");
    assert_eq!(pending, Some((0, true)));
    assert_eq!(
        customer_passwords::replace(
            &pool,
            EMAIL,
            &proof,
            PasswordCandidate {
                password: PASSWORD,
                blocklist: Some(&blocklist),
            },
            &SystemClock,
            &admission,
            limits,
        )
        .await,
        Ok(1)
    );
    assert_eq!(credential_custody(&pool).await, (1, 1, 1, 0, 0));
    pool.close().await;
}

#[tokio::test]
#[ignore = "browser harness verifies exact initial credential and login custody"]
async fn verify_browser_enrolment_custody() {
    let pool = guarded_pool().await;
    assert_eq!(verification_custody(&pool).await, (1, 1, 0, 1, 1, 0));
    assert_eq!(credential_custody(&pool).await, (1, 1, 1, 1, 1));
    pool.close().await;
}
