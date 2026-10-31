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
