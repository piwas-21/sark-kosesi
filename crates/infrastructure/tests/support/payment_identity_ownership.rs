//! Provider metadata edits must not associate one same-profile charge to two orders.
use super::observation_cases::{ids, prepared};
use super::*;
use crate::payment_attempts::{identities::resolve, observations::record_verified};
use crate::{MIGRATOR, guest_baskets::GuestSessionStore};
use sqlx::{Executor, PgPool};
use std::sync::{Arc, atomic::AtomicU64};
async fn additional(
    pool: &PgPool,
    time: Arc<crate::payment_test_fixture::quote::Time>,
    profile_id: &str,
) -> FrozenAttempt {
    use crate::payment_test_fixture::{Ids, intent, order, quote};
    pool.execute("UPDATE stock_balances SET on_hand=12")
        .await
        .expect("two physically backed orders");
    let i = Arc::new(Ids(AtomicU64::new(100000)));
    let store = GuestSessionStore::new(
        pool.clone(),
        time.clone(),
        i.clone(),
        commerce_application::guest_access::GuestLimits::default(),
    )
    .expect("second owner");
    let (session, csrf) = quote::guest(&store).await;
    let owner = GuestCredential::from_session(&session).expect("owner");
    let q = crate::checkout_quotes::issue_quote(
        &store,
        &owner,
        &csrf,
        commerce_application::Locale::Nl,
        &quote::request("quote"),
    )
    .await
    .expect("second quote")
    .snapshot;
    let o = crate::checkout_orders::create_order(
        &store,
        &owner,
        &csrf,
        "place",
        &order::intent(&q.binding.quote_id),
        &order::seller(),
    )
    .await
    .expect("second order")
    .snapshot;
    let key = TestCredential::capture(&raw_key(), profile_id).expect("explicit synthetic profile");
    let c = config(&key);
    let f = prepare_attempt(&store, &owner, &csrf, "prepare", &intent(&o), &c)
        .await
        .expect("second attempt")
        .frozen;
    start_dispatch(
        pool,
        time.as_ref(),
        i.as_ref(),
        &f.snapshot().binding.id,
        &c,
    )
    .await
    .expect("durable second dispatch");
    f
}
async fn fetched_identity(a: &AttemptSnapshot) -> VerifiedPayment {
    let mut p = payment(a);
    p["id"] = "tr_SHARED".into();
    p["profileId"] = a.binding.profile.id.clone().into();
    let server = Server::start(vec![
        Reply::Json(200, json!({"resource":"profile","id":a.binding.profile.id})),
        Reply::Json(200, p),
    ]);
    let key = TestCredential::capture(&raw_key(), &a.binding.profile.id).expect("test profile");
    let c = config(&key);
    TestClient::local(key, c, server.base.clone(), Duration::from_secs(2))
        .expect("loopback")
        .verify_profile()
        .await
        .expect("authenticated profile")
        .fetch(a, "tr_SHARED")
        .await
        .expect("separate verified fetch")
}
async fn contend(pool: PgPool, different_profile: bool) {
    let (time, first) = prepared(&pool, true).await;
    let second = additional(
        &pool,
        time.clone(),
        if different_profile {
            "pfl_OTHER_SYNTHETIC"
        } else {
            &first.snapshot().binding.profile.id
        },
    )
    .await;
    let i = Arc::new(ids());
    for f in [&first, &second] {
        record_verified(
            &pool,
            time.as_ref(),
            i.as_ref(),
            &f.snapshot().binding.id,
            "fetched",
            &fetched_identity(f.snapshot()).await,
        )
        .await
        .expect("preserve matching fetched evidence");
    }
    let mut tasks = vec![];
    for f in [&first, &second] {
        let p = pool.clone();
        let t = time.clone();
        let i = i.clone();
        let a = f.snapshot().binding.id.clone();
        tasks.push(tokio::spawn(async move {
            resolve(&p, t.as_ref(), i.as_ref(), &a, "resolve").await
        }));
    }
    let mut successes = 0;
    for task in tasks {
        if task.await.expect("task").is_ok() {
            successes += 1;
        }
    }
    assert_eq!(
        successes,
        if different_profile { 2 } else { 1 },
        "one profile/provider identity cannot fund two original orders"
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM payment_observations")
        .fetch_one(&pool)
        .await
        .expect("observations retained");
    assert_eq!(count, 2);
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn same_profile_identity_conflict_has_one_concurrent_owner_and_retains_both_observations(
    pool: PgPool,
) {
    contend(pool, false).await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn different_profiles_have_independent_provider_identity_namespaces(pool: PgPool) {
    contend(pool, true).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn native_repeatable_read_claims_cannot_commit_two_owners_from_stale_snapshots(pool: PgPool) {
    let (time, first) = prepared(&pool, true).await;
    let second = additional(&pool, time.clone(), &first.snapshot().binding.profile.id).await;
    let i = ids();
    for f in [&first, &second] {
        record_verified(
            &pool,
            time.as_ref(),
            &i,
            &f.snapshot().binding.id,
            "fetched",
            &fetched_identity(f.snapshot()).await,
        )
        .await
        .expect("authenticated evidence");
    }
    let mut a = pool.begin().await.expect("first RR");
    let mut b = pool.begin().await.expect("second RR");
    for tx in [&mut a, &mut b] {
        tx.execute("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
            .await
            .expect("RR importer");
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM payment_identity_resolutions")
            .fetch_one(&mut **tx)
            .await
            .expect("snapshot");
        assert_eq!(n, 0);
    }
    let insert = "INSERT INTO payment_identity_resolutions(id,attempt_id,effect_key,revision,through_observation,recorded_at,summary) VALUES($1,$2,'native',1,1,1500,'{\"provider_ids\":[\"tr_SHARED\"],\"paid_provider_ids\":[\"tr_SHARED\"]}')";
    sqlx::query(insert)
        .bind("e".repeat(32))
        .bind(&first.snapshot().binding.id)
        .execute(&mut *a)
        .await
        .expect("first native claim");
    let second_attempt = second.snapshot().binding.id.clone();
    let task = tokio::spawn(async move {
        let result = sqlx::query(insert)
            .bind("f".repeat(32))
            .bind(second_attempt)
            .execute(&mut *b)
            .await;
        assert!(
            result.is_err(),
            "stale RR writer must abort instead of duplicating ownership"
        );
        b.rollback().await.expect("failed claim rollback");
    });
    tokio::time::timeout(Duration::from_secs(2),async {loop {
  let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'INSERT INTO payment_identity_resolutions%')").fetch_one(&pool).await.expect("native unique wait");
  if waiting {break;}tokio::time::sleep(Duration::from_millis(5)).await;
 }}).await.expect("second writer actually waited");
    let order = &first.snapshot().binding.order_id;
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES($1,$2,'payment.identity_resolved.v1',$3,1500)").bind("d".repeat(32)).bind(order).bind(json!({"schema_version":1,"order_id":order,"attempt_id":first.snapshot().binding.id,"resolution_id":"e".repeat(32)})).execute(&mut *a).await.expect("first atomic event");
    a.commit().await.expect("first owner commit");
    task.await.expect("second conflict");
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM payment_provider_identity_claims")
        .fetch_one(&pool)
        .await
        .expect("one owner");
    assert_eq!(n, 1);
    // A sequential rebind is also rejected, while the original owner can resolve again.
    assert!(
        resolve(
            &pool,
            time.as_ref(),
            &i,
            &second.snapshot().binding.id,
            "rebind"
        )
        .await
        .is_err()
    );
    resolve(
        &pool,
        time.as_ref(),
        &i,
        &first.snapshot().binding.id,
        "original-again",
    )
    .await
    .expect("same owner retry");
    for sql in [
        "UPDATE payment_provider_identity_claims SET provider_id='tr_CHANGED'",
        "DELETE FROM payment_provider_identity_claims",
    ] {
        assert!(pool.execute(sql).await.is_err());
    }
    let mut orphan = pool.begin().await.expect("orphan claim");
    sqlx::query("INSERT INTO payment_provider_identity_claims(profile_id,provider_id,attempt_id,initial_resolution_id) VALUES('pfl_WRONG','tr_ORPHAN',$1,$2)").bind(&first.snapshot().binding.id).bind("e".repeat(32)).execute(&mut *orphan).await.expect("deferred candidate");
    assert!(
        orphan.commit().await.is_err(),
        "wrong profile/identity membership cannot commit"
    );
}
