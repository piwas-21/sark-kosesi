//! Synthetic data for the owned, native PostgreSQL restore rehearsal only.
#[path = "restore_connection.rs"]
mod restore_connection;
use crate::{
    MIGRATOR,
    customer_credential_admission::{AdmissionKey, AdmissionLimits},
    email_verification::delivery::{
        VerificationDeliveryKey, issue_for_delivery,
        opening::{OpenOutcome, claim_and_open},
        register_key,
        start::{ClaimOutcome, claim},
    },
};
use commerce_application::request_context::{Clock, ContextError};
use sha2::{Digest, Sha256};

struct FixtureClock(u64);
impl Clock for FixtureClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(self.0)
    }
}

async fn fixture_pool() -> sqlx::PgPool {
    let url = std::env::var("RESTORE_FIXTURE_DATABASE_URL").expect("owned rehearsal URL");
    let database = std::env::var("RESTORE_FIXTURE_DATABASE_NAME").expect("owned database name");
    assert!(restore_connection::is_owned_url(&url, &database));
    let pool = crate::connect(&url)
        .await
        .expect("owned database connection");
    let actual: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .expect("owned database identity");
    assert_eq!(actual, database);
    pool
}

async fn synthetic_challenge(pool: &sqlx::PgPool, address: &str) -> ([u8; 32], i64) {
    let (raw, created): (Vec<u8>, i64) = sqlx::query_as(
        "SELECT verifier,created_at FROM email_verification_challenges WHERE email_identity=$1",
    )
    .bind(address)
    .fetch_one(pool)
    .await
    .expect("synthetic challenge");
    (raw.try_into().expect("bounded verifier"), created)
}

#[tokio::test]
#[ignore = "run only through scripts/rehearse_restore.py against its owned database"]
async fn seed_synthetic() {
    let pool = fixture_pool().await;
    MIGRATOR
        .run(&pool)
        .await
        .expect("embedded project migrations");
    // This path uses actual catalogue, stock, basket, quote and order APIs. The
    // false paid flag avoids the fixture's mock provider server entirely.
    let fixture = super::order_stock_cases::fixture::accepted(&pool, false, false).await;
    let key = VerificationDeliveryKey::capture("restore-synthetic", [41; 32]).expect("test key");
    let admission = AdmissionKey::new(1, [27; 32]).expect("test admission key");
    let limits = AdmissionLimits::new(1000, 100).expect("test limits");
    register_key(&pool, fixture.time.as_ref(), &key)
        .await
        .expect("register test key");
    issue_for_delivery(
        &pool,
        "restore@example.test",
        fixture.time.as_ref(),
        &admission,
        limits,
        &key,
    )
    .await
    .expect("synthetic encrypted envelope");
    let started = issue_for_delivery(
        &pool,
        "started-restore@example.test",
        fixture.time.as_ref(),
        &admission,
        limits,
        &key,
    )
    .await
    .expect("synthetic prestarted envelope");
    assert_eq!(
        claim(&pool, &started.verifier, fixture.time.as_ref(), &key).await,
        Ok(ClaimOutcome::Started)
    );
    pool.close().await;
}

#[tokio::test]
#[ignore = "run only through scripts/rehearse_restore.py against its restored owned database"]
async fn verify_restored_keys() {
    let pool = fixture_pool().await;
    let old =
        VerificationDeliveryKey::capture("restore-synthetic", [41; 32]).expect("old test key");
    let wrong =
        VerificationDeliveryKey::capture("restore-synthetic", [42; 32]).expect("wrong test key");
    let fresh = VerificationDeliveryKey::capture("restore-fresh-synthetic", [43; 32])
        .expect("fresh test key");
    let (verifier, created) = synthetic_challenge(&pool, "restore@example.test").await;
    let (started, started_at) = synthetic_challenge(&pool, "started-restore@example.test").await;
    assert_eq!(created, started_at);
    let clock = FixtureClock(u64::try_from(created).expect("synthetic clock"));
    let mut called = false;
    assert!(
        claim_and_open(&pool, &verifier, &clock, &wrong, |_, _| called = true)
            .await
            .is_err()
    );
    assert!(!called);
    assert_eq!(
        claim_and_open(&pool, &started, &clock, &old, |_, _| called = true).await,
        Ok(OpenOutcome::AlreadyStarted)
    );
    assert!(!called);
    assert_eq!(
        claim_and_open(&pool, &verifier, &clock, &old, |identity, bearer| {
            let digest: [u8; 32] = Sha256::new()
                .chain_update(b"piwas:email-verification:verifier:v1\0")
                .chain_update(bearer)
                .finalize()
                .into();
            called = identity == "restore@example.test" && digest == verifier;
        })
        .await,
        Ok(OpenOutcome::Opened)
    );
    assert!(called);
    register_key(&pool, &clock, &fresh)
        .await
        .expect("register fresh synthetic key");
    let admission = AdmissionKey::new(1, [27; 32]).expect("test admission key");
    let limits = AdmissionLimits::new(1000, 100).expect("test limits");
    let issued = issue_for_delivery(
        &pool,
        "fresh-restore@example.test",
        &clock,
        &admission,
        limits,
        &fresh,
    )
    .await
    .expect("fresh-key synthetic envelope");
    let mut fresh_called = false;
    assert!(
        claim_and_open(&pool, &issued.verifier, &clock, &old, |_, _| fresh_called =
            true)
        .await
        .is_err()
    );
    assert!(!fresh_called);
    assert_eq!(
        claim_and_open(
            &pool,
            &issued.verifier,
            &clock,
            &fresh,
            |identity, bearer| {
                let digest: [u8; 32] = Sha256::new()
                    .chain_update(b"piwas:email-verification:verifier:v1\0")
                    .chain_update(bearer)
                    .finalize()
                    .into();
                fresh_called =
                    identity == "fresh-restore@example.test" && digest == issued.verifier;
            }
        )
        .await,
        Ok(OpenOutcome::Opened)
    );
    assert!(fresh_called);
    pool.close().await;
}

/// A real original settlement, using an isolated pair whose locale order can differ.
#[tokio::test]
#[ignore = "run only through the owned paired-image stock custody rehearsal"]
async fn seed_stock_custody() {
    use super::order_stock_cases::fixture::{additional_variant, bounds};
    use commerce_domain::ComponentKind;

    let pool = fixture_pool().await;
    for code in ["A-C", "AB"] {
        bounds::component(&pool, code, ComponentKind::Cover, 10).await;
    }
    let sku = "COLLATION-COVERS".to_owned();
    additional_variant(&pool, &sku, false, &[("A-C".into(), 1), ("AB".into(), 1)]).await;
    bounds::market(&pool, std::slice::from_ref(&sku)).await;
    let fixture = bounds::accepted_lines_with_ids(&pool, &[sku], 2_000_000, 3_000_000).await;
    let receipt = crate::order_stock::settle(
        &pool,
        fixture.time.as_ref(),
        fixture.ids.as_ref(),
        &fixture.order.identity.id,
    )
    .await
    .expect("synthetic original settlement");
    assert!(!receipt.replayed);
    assert_eq!(receipt.settlement.footprint.members.len(), 1);
    assert_eq!(receipt.settlement.footprint.members[0].components.len(), 2);
    assert_eq!(stock_custody(&pool).await, "compatible");
    pool.close().await;
}

/// Returns a fixed outcome only after validating the raw immutable receipt and effects.
async fn stock_custody(pool: &sqlx::PgPool) -> &'static str {
    use super::observation_cases::BadPorts;
    use crate::order_stock::{self, SettlementError};
    use serde_json::Value;

    // The synthetic effect key and selected SKU together uniquely identify the order;
    // require exactly one matching settlement before reading its bounded envelope.
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM order_stock_settlements s JOIN checkout_orders o ON o.id=s.order_id JOIN stock_holds h ON h.basket_group_id=s.group_id WHERE o.effect_key='bounded-order' AND h.sku='COLLATION-COVERS'")
        .fetch_one(pool).await.expect("synthetic settlement count");
    assert_eq!(count, 1);
    let (order_id, hold_id, footprint, digest, event_valid, claim_valid, native_exact): (String, String, Value, String, bool, bool, bool) = sqlx::query_as(
        "SELECT s.order_id,h.id,s.footprint,order_stock_digest(s), \
         (SELECT count(*)=1 FROM commerce_outbox e WHERE e.aggregate_id=s.order_id AND e.kind='order.stock_settled.v1' AND e.created_at=s.settled_at AND e.payload=jsonb_build_object('schema_version',1,'order_id',s.order_id,'settlement_id',s.id,'settlement_sha256',order_stock_digest(s))) AND \
         (SELECT count(*)=1 FROM commerce_outbox e WHERE e.aggregate_id=s.order_id AND e.kind='order.stock_settled.v1'), \
         (SELECT count(*)=1 FROM order_stock_allocations a WHERE a.order_id=s.order_id AND a.attempt_id=s.attempt_id AND a.kind='original' AND a.receipt_id=s.id), \
         order_stock_effects_exact(s,false) FROM order_stock_settlements s JOIN checkout_orders o ON o.id=s.order_id JOIN stock_holds h ON h.basket_group_id=s.group_id WHERE o.effect_key='bounded-order' AND h.sku='COLLATION-COVERS' AND octet_length(s.footprint::text)<=1048576")
        .fetch_one(pool).await.expect("bounded synthetic settlement");
    assert!(event_valid && claim_valid);
    assert_eq!(digest.len(), 64);
    assert!(
        digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
    let members = footprint["members"].as_array().expect("receipt members");
    assert_eq!(members.len(), 1);
    assert_eq!(members[0]["hold_id"], hold_id);
    assert_eq!(members[0]["sku"], "COLLATION-COVERS");
    let lines = members[0]["components"]
        .as_array()
        .expect("receipt components");
    assert_eq!(lines.len(), 2);
    assert!(lines.iter().all(|line| line["quantity"] == 1));
    let recorded: Vec<String> = lines
        .iter()
        .map(|line| line["component_code"].as_str().expect("code").to_owned())
        .collect();
    let current: Vec<String> = sqlx::query_scalar(
        "SELECT component_code FROM stock_hold_lines WHERE hold_id=$1 ORDER BY component_code",
    )
    .bind(&hold_id)
    .fetch_all(pool)
    .await
    .expect("current component order");
    let bytewise: Vec<String> = sqlx::query_scalar("SELECT component_code FROM stock_hold_lines WHERE hold_id=$1 ORDER BY component_code COLLATE \"C\"")
        .bind(&hold_id).fetch_all(pool).await.expect("bytewise component order");
    assert_eq!(bytewise, ["A-C", "AB"]);
    assert!(recorded == ["A-C", "AB"] || recorded == ["AB", "A-C"]);
    assert!(current == ["A-C", "AB"] || current == ["AB", "A-C"]);
    let balances: Vec<(String, i64, i64, i64)> = sqlx::query_as("SELECT component_code,on_hand,quarantine,reserved FROM stock_balances WHERE component_code IN ('A-C','AB') ORDER BY component_code COLLATE \"C\"")
        .fetch_all(pool).await.expect("literal component balances");
    assert_eq!(balances, [("A-C".into(), 9, 0, 0), ("AB".into(), 9, 0, 0)]);
    let effects: (i64, i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM stock_holds WHERE id=$1 AND state='consumed'),(SELECT count(*) FROM stock_hold_lines WHERE hold_id=$1 AND quantity=1),(SELECT count(*) FROM stock_movements WHERE hold_id=$1 AND action='reserve' AND quantity=1),(SELECT count(*) FROM stock_movements WHERE hold_id=$1 AND action='consume-hold' AND quantity=1)")
        .bind(&hold_id).fetch_one(pool).await.expect("literal physical effects");
    assert_eq!(effects, (1, 2, 2, 2));
    let state_sql = "SELECT jsonb_build_object('receipt',(SELECT to_jsonb(s) FROM order_stock_settlements s WHERE s.order_id=$1),'event',(SELECT to_jsonb(e) FROM commerce_outbox e WHERE e.aggregate_id=$1 AND e.kind='order.stock_settled.v1'),'claim',(SELECT to_jsonb(a) FROM order_stock_allocations a WHERE a.order_id=$1),'hold',(SELECT to_jsonb(h) FROM stock_holds h WHERE h.id=$2),'lines',(SELECT jsonb_agg(to_jsonb(l) ORDER BY l.component_code COLLATE \"C\") FROM stock_hold_lines l WHERE l.hold_id=$2),'movements',(SELECT jsonb_agg(to_jsonb(m) ORDER BY m.sequence) FROM stock_movements m WHERE m.hold_id=$2),'balances',(SELECT jsonb_agg(to_jsonb(b) ORDER BY b.component_code COLLATE \"C\") FROM stock_balances b WHERE b.component_code IN ('A-C','AB')))";
    let before: Value = sqlx::query_scalar(state_sql)
        .bind(&order_id)
        .bind(&hold_id)
        .fetch_one(pool)
        .await
        .expect("original bounded state");
    let read = order_stock::read_settlement(pool, &order_id).await;
    let replay = order_stock::settle(pool, &BadPorts, &BadPorts, &order_id).await;
    let outcome = match (read, replay) {
        (Ok(Some(read)), Ok(replay)) => {
            assert!(native_exact && replay.replayed && read == replay.settlement);
            assert_eq!(
                serde_json::to_value(&read.footprint).expect("receipt footprint"),
                footprint
            );
            "compatible"
        }
        (Err(SettlementError::InvalidStoredData), Err(SettlementError::InvalidStoredData)) => {
            assert_ne!(
                recorded, current,
                "incompatibility requires measured order change"
            );
            assert!(
                !native_exact,
                "native receipt must also reject the changed ordering"
            );
            "ordering_incompatible"
        }
        _ => panic!("unexpected stock custody replay result"),
    };
    let after: Value = sqlx::query_scalar(state_sql)
        .bind(&order_id)
        .bind(&hold_id)
        .fetch_one(pool)
        .await
        .expect("unchanged bounded state");
    assert_eq!(before, after);
    outcome
}

#[tokio::test]
#[ignore = "run only through the owned paired-image stock custody rehearsal"]
async fn verify_stock_custody() {
    let pool = fixture_pool().await;
    let outcome = stock_custody(&pool).await;
    pool.close().await;
    // libtest prints its test label without a newline under --nocapture.
    println!("\nstock_custody_outcome={outcome}");
}
