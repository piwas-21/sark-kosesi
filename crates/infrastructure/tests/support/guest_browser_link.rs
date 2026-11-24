//! Synthetic account and guest proof for the isolated browser fixture only.
use crate::{fixture, guarded_pool};
use commerce_infrastructure::{
    customer_credential_admission::{AdmissionKey, AdmissionLimits},
    customer_passwords::{self, BlocklistManifest, PasswordBlocklist, PasswordCandidate},
};
use std::time::{SystemTime, UNIX_EPOCH};

#[allow(dead_code)]
#[path = "admitted_proofs.rs"]
mod admitted_proofs;

const PASSWORD: &str = "synthetic horse battery staple";

fn time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_secs()
}

pub async fn seed_accounts(pool: &sqlx::PgPool) {
    let now = time();
    let clock = fixture::Time(std::sync::atomic::AtomicU64::new(now));
    let manifest = BlocklistManifest {
        version: "synthetic-browser-v1".into(),
        provenance: "local synthetic fixture".into(),
        qualification_ref: "not an operational corpus".into(),
        valid_from: i64::try_from(now).expect("time") - 1,
        valid_until: i64::try_from(now).expect("time") + 3600,
        digests: vec![[0; 32]],
    };
    let integrity = manifest.integrity_digest();
    let blocklist = PasswordBlocklist::new(manifest, integrity).expect("synthetic blocklist");
    let key = AdmissionKey::new(1, [7; 32]).expect("synthetic admission key");
    let limits = AdmissionLimits::new(1000, 100).expect("fixture limits");
    for email in ["buyer@example.test", "other@example.test"] {
        let proof = admitted_proofs::email_verification::issue(pool, email, &clock)
            .await
            .expect("fixture verification proof");
        admitted_proofs::verified_customer_accounts::establish(pool, email, &proof, &clock)
            .await
            .expect("fixture verified account");
        let proof = admitted_proofs::customer_passwords::issue(pool, email, &clock)
            .await
            .expect("fixture password proof");
        customer_passwords::replace(
            pool,
            email,
            &proof,
            PasswordCandidate {
                password: PASSWORD,
                blocklist: Some(&blocklist),
            },
            &clock,
            &key,
            limits,
        )
        .await
        .expect("fixture password");
    }
}

#[cfg(feature = "test-verification-smtp")]
#[tokio::test]
#[ignore = "manual browser harness delivers one encrypted proof to its owned SMTP sink"]
async fn deliver_browser_guest_link_proof() {
    use commerce_infrastructure::{
        guest_baskets::GuestCredential,
        guest_order_links::delivery::{self, DeliveryKey, OpenOutcome, outcome, test_smtp},
    };
    use std::{net::SocketAddr, sync::Arc};
    use tower_sessions::{Session, session::Id};

    let pool = guarded_pool().await;
    let id: Id = std::env::var("GUEST_BROWSER_GUEST_COOKIE")
        .expect("guest cookie")
        .parse()
        .expect("guest session ID");
    let csrf = std::env::var("GUEST_BROWSER_GUEST_CSRF").expect("guest synchronizer");
    let order = std::env::var("GUEST_BROWSER_ORDER_ID").expect("owned order");
    assert!(
        order.len() == 32
            && order
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
    let port: u16 = std::env::var("GUEST_BROWSER_LINK_SMTP_PORT")
        .expect("owned SMTP sink port")
        .parse()
        .expect("numeric SMTP port");
    let endpoint =
        test_smtp::LocalTestEndpoint::new(true, SocketAddr::from(([127, 0, 0, 1], port)))
            .expect("numeric local SMTP endpoint");
    let (store, clock) = fixture::store(&pool, time());
    let session = Session::new(Some(id), Arc::new(store.clone()), None);
    let guest = GuestCredential::from_session(&session).expect("guest credential");
    let key = DeliveryKey::capture("synthetic-browser-guest-link", [43; 32]).expect("test key");
    delivery::register_key(&pool, clock.as_ref(), &key)
        .await
        .expect("register seed key in the disposable database");
    let receipt = delivery::issue_for_delivery(&store, &guest, &csrf, &order, &key)
        .await
        .expect("durable owned envelope");
    assert_eq!(
        test_smtp::deliver(&pool, &receipt.verifier, clock.as_ref(), &key, &endpoint).await,
        Ok(OpenOutcome::Opened),
    );
    assert_eq!(
        outcome::inspect(&pool, &receipt.verifier).await,
        Ok(outcome::DeliveryState::Recorded(
            outcome::TransportOutcome::Accepted
        )),
    );
    assert_eq!(
        test_smtp::deliver(&pool, &receipt.verifier, clock.as_ref(), &key, &endpoint).await,
        Ok(OpenOutcome::AlreadyStarted),
    );
    pool.close().await;
}

#[tokio::test]
#[ignore = "manual browser harness checks failed proof attempts before redemption"]
async fn verify_browser_link_unconsumed() {
    let pool = guarded_pool().await;
    let custody: (i64, i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM guest_order_link_proofs),(SELECT count(*) FROM guest_order_link_proofs WHERE consumed_at IS NULL AND consumed_account_id IS NULL AND consumed_xid IS NULL),(SELECT count(*) FROM customer_order_links),(SELECT count(*) FROM guest_link_delivery_outcomes WHERE outcome='accepted')")
        .fetch_one(&pool).await.expect("unconsumed proof and accepted synthetic sink custody");
    assert_eq!(custody, (1, 1, 0, 1));
    pool.close().await;
}
