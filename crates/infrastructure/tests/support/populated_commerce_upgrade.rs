//! One disposable populated upgrade; all catalogue, payments and people are synthetic.
use super::*;
use crate::{
    MIGRATOR, admitted_proofs,
    checkout_orders::create_order,
    checkout_quotes::issue_quote,
    guest_baskets::GuestCredential,
    guest_order_links::{self, delivery},
    order_stock,
    packing_custody::{self, Action, Component, Request, Units},
    payment_attempts::{
        callbacks::lookup::continuation, dispatch_and_observe_test_payment, observations,
        prepare_attempt, reversals,
    },
    published_catalogue::published_product,
    staff_access::SessionToken,
};
use commerce_application::Locale;
use commerce_domain::inventory::WarehouseChange;
use sha2::{Digest, Sha256};
use sqlx::{Executor, PgPool};
use std::sync::atomic::{AtomicU64, Ordering};

#[path = "admitted_login.rs"]
mod admitted_login;
#[sqlx::test(migrations = false)]
async fn populated_commerce_history_survives_0067_through_0078(pool: PgPool) {
    use order_stock_cases::fixture;
    MIGRATOR
        .run_to(67, &pool)
        .await
        .expect("old populated schema");
    fixture::seed(&pool).await;
    // Reserve enough distinct physical components for the later guest orders.
    for component in ["SEAT-COVER", "SEAT-FOAM"] {
        fixture::receive(
            &pool,
            &format!("upgrade-extra-{component}"),
            component,
            WarehouseChange::Receive(20),
        )
        .await;
    }
    let (store, clock) = crate::payment_test_fixture::quote::store(&pool, 1100);
    let f = fixture::accepted_with_store(&pool, store.clone(), clock.clone(), false, true).await;
    let order = &f.order.identity.id;
    let attempt = &f.frozen.snapshot().binding.id;
    let (old_session, old_csrf) = crate::payment_test_fixture::quote::guest(&store).await;
    let guest = GuestCredential::from_session(&old_session).expect("old guest");
    let old_quote = issue_quote(
        &store,
        &guest,
        &old_csrf,
        Locale::Nl,
        &crate::payment_test_fixture::quote::request("upgrade-old-quote"),
    )
    .await
    .expect("old guest quote")
    .snapshot;
    let old_order = create_order(
        &store,
        &guest,
        &old_csrf,
        "upgrade-old-order",
        &crate::payment_test_fixture::order::intent(&old_quote.binding.quote_id),
        &crate::payment_test_fixture::order::seller(),
    )
    .await
    .expect("old guest order")
    .snapshot
    .identity
    .id;
    let old_proof = guest_order_links::issue(&store, &guest, &old_csrf, &old_order)
        .await
        .expect("direct proof before envelope migration");
    let proof_verifier: [u8; 32] = Sha256::new()
        .chain_update(b"piwas:guest-order-link:verifier:v1\0")
        .chain_update(old_proof.expose_bytes())
        .finalize()
        .into();
    order_stock::settle(&pool, clock.as_ref(), f.ids.as_ref(), order)
        .await
        .expect("old paid settlement");
    pool.execute("INSERT INTO staff_accounts VALUES('upgrade-packer',true); INSERT INTO staff_capabilities VALUES('upgrade-packer','warehouse.manage')").await.expect("synthetic staff");
    let staff = SessionToken::from_bytes([71; 32]);
    sqlx::query("INSERT INTO staff_sessions(token_hash,staff_id,created_at,expires_at,mfa_verified_at) VALUES($1,'upgrade-packer',1000,8000,1450)")
        .bind(staff.digest().as_slice()).execute(&pool).await.expect("staff session");
    let units = [Units {
        sku: "SET-FILLED".into(),
        units: 1,
    }];
    let packages = [vec![
        Component {
            code: "SEAT-COVER".into(),
            quantity: 2,
        },
        Component {
            code: "SEAT-FOAM".into(),
            quantity: 2,
        },
    ]];
    let packing = packing_custody::record(
        &pool,
        &staff,
        clock.as_ref(),
        f.ids.as_ref(),
        Request {
            order_id: order,
            effect_key: "upgrade-pack",
            confirmed: true,
            action: Action::Packed {
                units: &units,
                packages: &packages,
            },
        },
    )
    .await
    .expect("old packing");

    let mut parent = payment(f.frozen.snapshot());
    parent["id"] = "tr_SETTLEMENT".into();
    let amount = f.frozen.snapshot().binding.amount_minor;
    parent["amount"]["value"] = format!("{}.{:02}", amount / 100, amount % 100).into();
    let refund = json!({"resource":"refund","id":"re_UPGRADE","paymentId":"tr_SETTLEMENT","mode":"test","amount":{"currency":"EUR","value":"12.34"},"status":"pending","createdAt":"2024-03-20T09:13:37.123456789Z"});
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, parent),
        Reply::Json(200, refund),
    ]);
    let verified = local(&server)
        .verify_profile()
        .await
        .expect("loopback profile")
        .into_reconciliation()
        .fetch_refund(f.frozen.snapshot(), "tr_SETTLEMENT", "re_UPGRADE")
        .await
        .expect("loopback reversal");
    let reversal = reversals::record(
        &pool,
        clock.as_ref(),
        f.ids.as_ref(),
        "upgrade-refund",
        &verified,
    )
    .await
    .expect("old reversal");
    assert_eq!((&reversal.order_id, &reversal.attempt_id), (order, attempt));
    let before_product = published_product(&pool, clock.as_ref(), Locale::Nl, "SET-FILLED")
        .await
        .expect("old projection")
        .expect("published");
    let proof_before: Value =
        sqlx::query_scalar("SELECT to_jsonb(p) FROM guest_order_link_proofs p WHERE verifier=$1")
            .bind(proof_verifier.as_slice())
            .fetch_one(&pool)
            .await
            .expect("old proof");
    let observations_before: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(o)-'source_transaction_id' FROM payment_observations o WHERE attempt_id=$1 ORDER BY revision").bind(attempt).fetch_all(&pool).await.expect("old observations");
    let events_before: Vec<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(e) FROM commerce_outbox e WHERE aggregate_id=$1 ORDER BY id",
    )
    .bind(order)
    .fetch_all(&pool)
    .await
    .expect("old events");
    assert!(!observations_before.is_empty());

    for version in 68..=75 {
        MIGRATOR
            .run_to(version, &pool)
            .await
            .expect("forward migration");
    }
    let proof_after: (Value, String, bool) = sqlx::query_as("SELECT to_jsonb(p)-'delivery_mode'-'delivery_transaction_id',delivery_mode,delivery_transaction_id IS NULL FROM guest_order_link_proofs p WHERE verifier=$1").bind(proof_verifier.as_slice()).fetch_one(&pool).await.expect("preserved proof");
    assert_eq!(proof_after, (proof_before, "direct".into(), true));
    let observations_after: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(o)-'source_transaction_id' FROM payment_observations o WHERE attempt_id=$1 ORDER BY revision").bind(attempt).fetch_all(&pool).await.expect("preserved observations");
    assert_eq!(observations_after, observations_before);
    let events_after: Vec<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(e) FROM commerce_outbox e WHERE aggregate_id=$1 ORDER BY id",
    )
    .bind(order)
    .fetch_all(&pool)
    .await
    .expect("preserved events");
    assert_eq!(events_after, events_before);
    let legacy: (bool, i64) = sqlx::query_as("SELECT bool_and(source_transaction_id IS NULL),(SELECT count(*) FROM payment_initial_dispatch_provenance WHERE attempt_id=$1) FROM payment_observations WHERE attempt_id=$1").bind(attempt).fetch_one(&pool).await.expect("no inferred provenance");
    assert_eq!(legacy, (true, 0));
    let old_start: bool = sqlx::query_scalar(
        "SELECT source_transaction_id IS NULL FROM payment_dispatch_starts WHERE attempt_id=$1",
    )
    .bind(attempt)
    .fetch_one(&pool)
    .await
    .expect("no inferred start transaction");
    assert!(old_start);
    assert!(continuation::read(&pool, attempt).await.is_err());
    assert_eq!(
        reversals::inspect_set(&pool, attempt)
            .await
            .expect("reversal set")
            .observations,
        1
    );
    let inspected = packing_custody::inspect(&pool, &staff, clock.as_ref(), order)
        .await
        .expect("packing inspection");
    assert_eq!(inspected.history.len(), 1);
    assert_eq!(inspected.history[0].id, packing.record.id);
    assert_eq!(
        published_product(&pool, clock.as_ref(), Locale::Nl, "SET-FILLED")
            .await
            .expect("new projection")
            .as_ref(),
        Some(&before_product)
    );

    let key = delivery::DeliveryKey::capture("upgrade-key", [7; 32]).expect("synthetic key");
    delivery::register_key(&pool, clock.as_ref(), &key)
        .await
        .expect("key");
    let encrypted = delivery::issue_for_delivery(&store, &guest, &old_csrf, &old_order, &key)
        .await
        .expect("new encrypted proof");
    assert_eq!(
        delivery::claim_open_and_record(
            &pool,
            &encrypted.verifier,
            clock.as_ref(),
            &key,
            |email, _| {
                assert_eq!(email, "buyer@example.test");
                delivery::outcome::TransportOutcome::Accepted
            }
        )
        .await,
        Ok(delivery::OpenOutcome::Opened)
    );
    assert_eq!(
        delivery::outcome::inspect(&pool, &encrypted.verifier).await,
        Ok(delivery::outcome::DeliveryState::Recorded(
            delivery::outcome::TransportOutcome::Accepted
        ))
    );
    let email = "buyer@example.test";
    let proof = admitted_proofs::email_verification::issue(&pool, email, clock.as_ref())
        .await
        .expect("account proof");
    admitted_proofs::verified_customer_accounts::establish(&pool, email, &proof, clock.as_ref())
        .await
        .expect("account");
    let proof = admitted_proofs::customer_passwords::issue(&pool, email, clock.as_ref())
        .await
        .expect("password proof");
    admitted_proofs::customer_passwords::replace(
        &pool,
        email,
        &proof,
        "correct horse battery staple",
        clock.as_ref(),
    )
    .await
    .expect("password");
    let customer =
        admitted_login::login(&pool, email, "correct horse battery staple", clock.as_ref())
            .await
            .expect("customer session");
    guest_order_links::link(
        &pool,
        &customer.token,
        &old_order,
        &old_proof,
        clock.as_ref(),
    )
    .await
    .expect("old direct proof still redeems");

    let (new_session, csrf) = crate::payment_test_fixture::quote::guest(&store).await;
    let new_guest = GuestCredential::from_session(&new_session).expect("new guest");
    let quoted = issue_quote(
        &store,
        &new_guest,
        &csrf,
        Locale::Nl,
        &crate::payment_test_fixture::quote::request("upgrade-new-quote"),
    )
    .await
    .expect("new quote")
    .snapshot;
    let new_order = create_order(
        &store,
        &new_guest,
        &csrf,
        "upgrade-new-order",
        &crate::payment_test_fixture::order::intent(&quoted.binding.quote_id),
        &crate::payment_test_fixture::order::seller(),
    )
    .await
    .expect("new order")
    .snapshot;
    let configuration = config(&credential());
    let frozen = prepare_attempt(
        &store,
        &new_guest,
        &csrf,
        "upgrade-new-attempt",
        &crate::payment_test_fixture::intent(&new_order),
        &configuration,
    )
    .await
    .expect("new attempt")
    .frozen;
    let mut payment = payment(frozen.snapshot());
    let amount = frozen.snapshot().binding.amount_minor;
    payment["amount"]["value"] = format!("{}.{:02}", amount / 100, amount % 100).into();
    let server = Server::start(vec![
        profile(),
        Reply::Json(201, payment.clone()),
        Reply::Json(200, payment),
    ]);
    let client = local(&server).verify_profile().await.expect("test profile");
    let ids = crate::payment_test_fixture::Ids(AtomicU64::new(900_000));
    let new_attempt = &frozen.snapshot().binding.id;
    dispatch_and_observe_test_payment(&pool, clock.as_ref(), &ids, &client, new_attempt)
        .await
        .expect("fresh loopback dispatch and GET");
    let source =
        observations::read_recorded(&pool, new_attempt, &format!("initial-fetch-{new_attempt}"))
            .await
            .expect("initial receipt")
            .expect("source");
    let receipt = continuation::acquire(&pool, clock.as_ref(), &ids, new_attempt)
        .await
        .expect("fresh continuation");
    assert!(!receipt.replayed);
    let replay = continuation::read(&pool, new_attempt)
        .await
        .expect("read")
        .expect("existing");
    assert_eq!(replay.operation_id, receipt.operation_id);
    let operation =
        crate::payment_attempts::callbacks::lookup::read_operation(&pool, &receipt.operation_id)
            .await
            .expect("exact lookup");
    assert_eq!(operation.snapshot().provider_id, source.intent.payment.id);
    assert_eq!(
        operation.snapshot().profile_id,
        frozen.snapshot().binding.profile.id
    );
    // The operation was committed under 0074. Upgrade with it already present;
    // the generic GET capability must remain fenced while the sealed one starts once.
    MIGRATOR
        .run_to(76, &pool)
        .await
        .expect("upgrade populated continuation");
    let preserved = continuation::read(&pool, new_attempt)
        .await
        .expect("preserved continuation")
        .expect("existing binding");
    assert_eq!(preserved.operation_id, receipt.operation_id);
    assert!(matches!(
        crate::payment_attempts::callbacks::lookup::start_lookup(&pool, clock.as_ref(), &operation)
            .await,
        Err(crate::payment_attempts::PaymentStoreError::Unavailable)
    ));
    let starts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM callback_lookup_fetch_starts WHERE operation_id=$1",
    )
    .bind(&receipt.operation_id)
    .fetch_one(&pool)
    .await
    .expect("no generic start");
    assert_eq!(starts, 0);
    let sealed = continuation::start(&pool, clock.as_ref(), new_attempt)
        .await
        .expect("one sealed start");
    assert_eq!(sealed.operation_id(), receipt.operation_id);
    assert!(matches!(
        continuation::start(&pool, clock.as_ref(), new_attempt).await,
        Err(crate::payment_attempts::PaymentStoreError::Unavailable)
    ));
    assert_eq!(
        continuation::read(&pool, new_attempt)
            .await
            .expect("historical read")
            .expect("binding")
            .operation_id,
        receipt.operation_id
    );
    // The same marked start existed before 0077; migration must preserve its
    // unresolved state and cannot create or recover a transport capability.
    let calls_before_receipt_upgrade = server.count();
    assert_eq!(calls_before_receipt_upgrade, 3);
    MIGRATOR
        .run_to(77, &pool)
        .await
        .expect("upgrade populated marked start");
    assert_eq!(
        continuation::inspect(&pool, new_attempt)
            .await
            .expect("offline receipt inspection"),
        Some(continuation::ContinuationOutcome::Unknown)
    );
    assert!(matches!(
        crate::payment_attempts::callbacks::lookup::start_lookup(&pool, clock.as_ref(), &operation)
            .await,
        Err(crate::payment_attempts::PaymentStoreError::Unavailable)
    ));
    assert!(matches!(
        continuation::start(&pool, clock.as_ref(), new_attempt).await,
        Err(crate::payment_attempts::PaymentStoreError::Unavailable)
    ));
    let retained: (i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM callback_lookup_fetch_starts WHERE operation_id=$1),(SELECT count(*) FROM callback_lookup_receipts WHERE operation_id=$1)")
        .bind(&receipt.operation_id).fetch_one(&pool).await.expect("retained unknown custody");
    assert_eq!(retained, (1, 0));
    assert_eq!(server.count(), calls_before_receipt_upgrade);
    server
        .inspect(|requests| assert_eq!(requests.iter().filter(|r| r.method == "POST").count(), 1));
    MIGRATOR
        .run_to(78, &pool)
        .await
        .expect("upgrade populated catalogue integrity");
    assert_eq!(
        published_product(&pool, clock.as_ref(), Locale::Nl, "SET-FILLED")
            .await
            .expect("upgraded published product"),
        Some(before_product)
    );
    assert_eq!(
        continuation::inspect(&pool, new_attempt)
            .await
            .expect("retained unresolved start after catalogue migration"),
        Some(continuation::ContinuationOutcome::Unknown)
    );
    assert_eq!(server.count(), calls_before_receipt_upgrade);
    assert_eq!(
        reversals::inspect_set(&pool, attempt)
            .await
            .expect("old reversal remains")
            .observations,
        1
    );
    assert_eq!(
        packing_custody::inspect(&pool, &staff, clock.as_ref(), order)
            .await
            .expect("old packing remains")
            .history[0]
            .id,
        packing.record.id
    );
    let final_observations: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(o)-'source_transaction_id' FROM payment_observations o WHERE attempt_id=$1 ORDER BY revision")
        .bind(attempt).fetch_all(&pool).await.expect("old observations remain exact");
    assert_eq!(final_observations, observations_before);
    let final_events: Vec<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(e) FROM commerce_outbox e WHERE aggregate_id=$1 ORDER BY id",
    )
    .bind(order)
    .fetch_all(&pool)
    .await
    .expect("old events remain exact");
    assert_eq!(final_events, events_before);
    assert_eq!(
        delivery::outcome::inspect(&pool, &encrypted.verifier).await,
        Ok(delivery::outcome::DeliveryState::Recorded(
            delivery::outcome::TransportOutcome::Accepted
        ))
    );
    assert_eq!(clock.0.load(Ordering::SeqCst), 1500);
}
