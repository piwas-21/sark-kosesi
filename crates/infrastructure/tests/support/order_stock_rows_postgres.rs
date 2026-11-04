//! Independent PostgreSQL boundaries below the public settlement orchestration.
use super::super::*;
use crate::guest_baskets::{GuestCredential, GuestSessionStore};
use crate::payment_attempts::{
    identities::resolve, prepare_attempt, recognition::recognize, start_dispatch,
};
use crate::{MIGRATOR, payment_test_fixture as fixture};
use commerce_application::{Locale, payment_requests::FrozenAttempt, request_context::Identifiers};
use commerce_domain::{
    Currency,
    checkout_orders::OrderSnapshot,
    payment_attempts::{
        PaymentMode, PaymentStatus, ProviderPayment,
        observations::{ObservationIntent, ObservationSnapshot},
    },
};
use sqlx::{Executor, PgPool};
use std::sync::atomic::{AtomicU64, Ordering};

fn fresh(ids: &fixture::Ids) -> String {
    format!(
        "{:032x}",
        u128::from_be_bytes(ids.next().expect("isolated fixture identity"))
    )
}
async fn paid_attempt(
    pool: &PgPool,
    store: &GuestSessionStore,
    time: &fixture::quote::Time,
    owner_scope: (&tower_sessions::Session, &str),
    order: &OrderSnapshot,
    ids: &fixture::Ids,
    provenance: (&str, &str),
) -> (FrozenAttempt, recognition::Recognition) {
    let (session, csrf) = owner_scope;
    let (provider, effect) = provenance;
    let owner = GuestCredential::from_session(session).expect("same original owner");
    let config = fixture::config();
    let frozen = prepare_attempt(
        store,
        &owner,
        csrf,
        effect,
        &fixture::intent(order),
        &config,
    )
    .await
    .expect("genuine original prepared attempt")
    .frozen;
    assert_eq!(frozen.snapshot().binding.amount_minor, 38245);
    start_dispatch(pool, time, ids, &frozen.snapshot().binding.id, &config)
        .await
        .expect("original durable dispatch");
    let id = fresh(ids);
    let event = fresh(ids);
    let attempt = &frozen.snapshot().binding.id;
    let observation = ObservationSnapshot {
        schema_version: 1,
        id: id.clone(),
        attempt_id: attempt.clone(),
        effect_key: effect.into(),
        revision: 1,
        recorded_at: 1500,
        intent: ObservationIntent {
            payment: ProviderPayment {
                resource: "payment".into(),
                id: provider.into(),
                profile_id: "pfl_SYNTHETIC_NotARealProfile".into(),
                mode: PaymentMode::Test,
                currency: Currency::EUR,
                amount_minor: 38245,
                metadata_schema_version: 1,
                order_id: order.identity.id.clone(),
                attempt_id: attempt.clone(),
                status: PaymentStatus::Paid,
            },
            credential_fingerprint: "a".repeat(64),
        },
    };
    let mut tx = pool.begin().await.expect("native verified-import fixture");
    sqlx::query("INSERT INTO payment_observations(id,attempt_id,effect_key,revision,recorded_at,provider_id,credential_fingerprint,status,snapshot) VALUES($1,$2,$3,1,1500,$4,$5,'paid',$6)").bind(&id).bind(attempt).bind(effect).bind(provider).bind("a".repeat(64)).bind(serde_json::to_value(observation).expect("closed original native observation tuple")).execute(&mut *tx).await.expect("complete matching native observation");
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES($1,$2,'payment.observation_recorded.v1',$3,1500)").bind(event).bind(&order.identity.id).bind(serde_json::json!({"schema_version":1,"order_id":order.identity.id,"attempt_id":attempt,"observation_id":id})).execute(&mut *tx).await.expect("exact native observation event");
    tx.commit()
        .await
        .expect("atomic imported observation custody");
    resolve(pool, time, ids, attempt, effect)
        .await
        .expect("complete authenticated prefix resolution");
    let received = recognize(pool, time, ids, attempt)
        .await
        .expect("canonical received recognition")
        .recognition;
    assert_eq!(received.state, recognition::State::Received);
    (frozen, received)
}
async fn reseal(tx: &mut Transaction<'_, Postgres>) {
    sqlx::query("UPDATE commerce_outbox e SET payload=jsonb_build_object('schema_version',1,'order_id',s.order_id,'settlement_id',s.id,'settlement_sha256',order_stock_digest(s)),created_at=s.settled_at FROM order_stock_settlements s WHERE e.kind='order.stock_settled.v1' AND e.aggregate_id=s.order_id").execute(&mut **tx).await.expect("matching coherent event envelope");
}
async fn begin_damage(pool: &PgPool) -> Transaction<'_, Postgres> {
    let mut tx = pool
        .begin()
        .await
        .expect("owned coherent corruption transaction");
    tx.execute("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
        .await
        .expect("same supported isolation");
    tx.execute("ALTER TABLE order_stock_settlements DISABLE TRIGGER USER; ALTER TABLE order_stock_allocations DISABLE TRIGGER USER; ALTER TABLE commerce_outbox DISABLE TRIGGER USER; ALTER TABLE stock_holds DISABLE TRIGGER USER; ALTER TABLE stock_movements DISABLE TRIGGER USER").await.expect("isolated fixture guards only");
    tx
}
async fn repaired(pool: &PgPool, original: &Settlement) {
    assert!(
        crate::order_stock::read_settlement(pool, &original.order_id)
            .await
            .expect("exact healthy stored receipt after rollback")
            == Some(original.clone())
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn stored_settlement_headers_active_sources_and_nonconsumed_sources_fail_independently_of_digest(
    pool: PgPool,
) {
    let (store, time, session, csrf, order) = fixture::accepted(&pool).await;
    let ids = fixture::Ids(AtomicU64::new(0x100000));
    paid_attempt(
        &pool,
        &store,
        time.as_ref(),
        (&session, &csrf),
        &order,
        &ids,
        ("tr_ROWS_HEADERS", "rows-headers"),
    )
    .await;
    let original = crate::order_stock::settle(&pool, time.as_ref(), &ids, &order.identity.id)
        .await
        .expect("genuine complete original settlement")
        .settlement;
    for sql in [
        "ALTER TABLE order_stock_settlements DROP CONSTRAINT order_stock_settlements_attempt_id_fkey; ALTER TABLE order_stock_allocations DROP CONSTRAINT order_stock_allocations_attempt_id_fkey; UPDATE order_stock_settlements SET attempt_id='broken'; UPDATE order_stock_allocations SET attempt_id='broken'",
        "ALTER TABLE order_stock_settlements DROP CONSTRAINT order_stock_settlements_recognition_id_fkey; UPDATE order_stock_settlements SET recognition_id='broken'",
        "ALTER TABLE order_stock_settlements DROP CONSTRAINT order_stock_settlements_group_id_fkey; UPDATE order_stock_settlements SET group_id='ffffffffffffffffffffffffffffffff'",
        "ALTER TABLE order_stock_settlements DROP CONSTRAINT order_stock_settlements_fence_key_check; ALTER TABLE order_stock_settlements ALTER COLUMN fence_key SET GENERATED BY DEFAULT; UPDATE order_stock_settlements SET fence_key=0",
        "UPDATE stock_holds SET state='active'; DELETE FROM stock_movements WHERE action='consume-hold'",
        "UPDATE stock_holds SET state='released'; UPDATE stock_movements SET action='release-hold',effect_key=replace(effect_key,':consume-hold:',':release-hold:'),on_hand_delta=0,on_hand_after=6 WHERE action='consume-hold'; UPDATE stock_balances SET on_hand=6,reserved=0",
    ] {
        let mut tx = begin_damage(&pool).await;
        sqlx::raw_sql(sqlx::AssertSqlSafe(sql.to_owned()))
            .execute(&mut *tx)
            .await
            .expect("controlled single-boundary corruption");
        reseal(&mut tx).await;
        let owning_claim_matches:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM order_stock_allocations a JOIN order_stock_settlements s ON s.order_id=a.order_id WHERE s.order_id=$1 AND a.attempt_id=s.attempt_id AND a.kind='original' AND a.receipt_id=s.id)").bind(&order.identity.id).fetch_one(&mut *tx).await.expect("coherent shared ownership reaches actual stored boundary");
        assert!(owning_claim_matches);
        assert_eq!(
            load(&mut tx, &order.identity.id).await.err(),
            Some(SettlementError::InvalidStoredData),
            "independent stored receipt boundary: {sql}"
        );
        tx.rollback()
            .await
            .expect("exact schema/data/constraint restoration");
        repaired(&pool, &original).await;
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn stored_settlement_rejects_another_valid_paid_attempt_in_the_same_owner_basket(
    pool: PgPool,
) {
    let (store, time, session, csrf, first) = fixture::accepted(&pool).await;
    let ids = fixture::Ids(AtomicU64::new(0x200000));
    // A second genuine order shares the guest owner but has its own captured
    // quote, reservation, attempt, paid evidence and canonical recognition.
    for component in ["SEAT-COVER", "SEAT-FOAM"] {
        crate::stock::warehouse_movement(
            &pool,
            crate::stock::WarehouseRequest {
                effect_key: component,
                component_code: component,
                change: commerce_domain::inventory::WarehouseChange::Receive(6),
                actor: "fixture-warehouse",
                reason: "second physically backed order",
            },
        )
        .await
        .expect("real second-order stock receipt");
    }
    time.0.store(1100, Ordering::SeqCst);
    let owner = GuestCredential::from_session(&session).expect("same guest owner");
    let quote = crate::checkout_quotes::issue_quote(
        &store,
        &owner,
        &csrf,
        Locale::Nl,
        &fixture::quote::request("second-quote"),
    )
    .await
    .expect("second genuine captured quote")
    .snapshot;
    let second = crate::checkout_orders::create_order(
        &store,
        &owner,
        &csrf,
        "second-order",
        &fixture::order::intent(&quote.binding.quote_id),
        &fixture::order::seller(),
    )
    .await
    .expect("second genuine same-owner order")
    .snapshot;
    assert_eq!(first.identity.basket_id, second.identity.basket_id);
    assert_ne!(first.identity.id, second.identity.id);
    time.0.store(1500, Ordering::SeqCst);
    paid_attempt(
        &pool,
        &store,
        time.as_ref(),
        (&session, &csrf),
        &first,
        &ids,
        ("tr_ROWS_FIRST", "rows-first"),
    )
    .await;
    let (second_attempt, second_received) = paid_attempt(
        &pool,
        &store,
        time.as_ref(),
        (&session, &csrf),
        &second,
        &ids,
        ("tr_ROWS_SECOND", "rows-second"),
    )
    .await;
    let original = crate::order_stock::settle(&pool, time.as_ref(), &ids, &first.identity.id)
        .await
        .expect("first genuine stock settlement")
        .settlement;
    let mut tx = begin_damage(&pool).await;
    sqlx::query(
        "UPDATE order_stock_settlements SET attempt_id=$1,recognition_id=$2 WHERE order_id=$3",
    )
    .bind(&second_attempt.snapshot().binding.id)
    .bind(&second_received.id)
    .bind(&first.identity.id)
    .execute(&mut *tx)
    .await
    .expect("both foreign keys remain genuine and valid");
    sqlx::query("UPDATE order_stock_allocations SET attempt_id=$1 WHERE order_id=$2")
        .bind(&second_attempt.snapshot().binding.id)
        .bind(&first.identity.id)
        .execute(&mut *tx)
        .await
        .expect("matching shared claim avoids masking the exact original order proof");
    reseal(&mut tx).await;
    let source_valid: bool = sqlx::query_scalar(
        "SELECT payment_recognition_source(r,true) FROM payment_recognitions r WHERE id=$1",
    )
    .bind(&second_received.id)
    .fetch_one(&mut *tx)
    .await
    .expect("second order exact financial custody positive control");
    assert!(source_valid);
    assert_eq!(
        load(&mut tx, &first.identity.id).await.err(),
        Some(SettlementError::InvalidStoredData),
        "same basket ownership cannot replace the exact original order/payment binding"
    );
    tx.rollback().await.expect("restore exact first receipt");
    repaired(&pool, &original).await;
    assert!(
        crate::order_stock::read_settlement(&pool, &second.identity.id)
            .await
            .expect("no invented second consumption")
            .is_none()
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn native_receipt_insert_rejects_oversized_serialized_footprint_before_sql_and_accepts_exact_original_source(
    pool: PgPool,
) {
    let (store, time, session, csrf, order) = fixture::accepted(&pool).await;
    let ids = fixture::Ids(AtomicU64::new(0x300000));
    let (frozen, received) = paid_attempt(
        &pool,
        &store,
        time.as_ref(),
        (&session, &csrf),
        &order,
        &ids,
        ("tr_ROWS_INSERT", "rows-insert"),
    )
    .await;
    let hold: String = sqlx::query_scalar("SELECT id FROM stock_holds WHERE basket_group_id=$1")
        .bind(&order.identity.stock_group_id)
        .fetch_one(&pool)
        .await
        .expect("original exact one member");
    let original = Settlement {
        id: "e".repeat(32),
        order_id: order.identity.id.clone(),
        attempt_id: frozen.snapshot().binding.id.clone(),
        recognition_id: received.id,
        group_id: order.identity.stock_group_id.clone(),
        actor: "payment-settlement".into(),
        settled_at: 1500,
        footprint: Footprint {
            schema_version: 1,
            members: vec![Member {
                hold_id: hold,
                sku: "SET-FILLED".into(),
                revision: 1,
                units: 3,
                components: vec![
                    Quantity {
                        component_code: "SEAT-COVER".into(),
                        quantity: 6,
                    },
                    Quantity {
                        component_code: "SEAT-FOAM".into(),
                        quantity: 6,
                    },
                ],
            }],
            components: vec![
                Component {
                    component_code: "SEAT-COVER".into(),
                    quantity: 6,
                    before: Balance {
                        on_hand: 6,
                        quarantine: 0,
                        reserved: 6,
                    },
                    after: Balance {
                        on_hand: 0,
                        quarantine: 0,
                        reserved: 0,
                    },
                },
                Component {
                    component_code: "SEAT-FOAM".into(),
                    quantity: 6,
                    before: Balance {
                        on_hand: 6,
                        quarantine: 0,
                        reserved: 6,
                    },
                    after: Balance {
                        on_hand: 0,
                        quarantine: 0,
                        reserved: 0,
                    },
                },
            ],
        },
    };
    assert_eq!(original.validate(), Ok(()));
    let mut oversized = original.clone();
    oversized.footprint.members[0].sku = "X".repeat(1_048_577);
    let mut tx = pool.begin().await.expect("size-bound negative control");
    assert_eq!(
        insert(&mut tx, &oversized).await.err(),
        Some(SettlementError::InvalidStoredData)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM order_stock_settlements")
            .fetch_one(&mut *tx)
            .await
            .expect("no SQL receipt effect"),
        0
    );
    tx.rollback().await.expect("rollback oversized probe");
    let mut tx = pool
        .begin()
        .await
        .expect("paired exact native prepared positive");
    insert(&mut tx, &original)
        .await
        .expect("valid original current-received native receipt prepared before effects");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM order_stock_settlements")
            .fetch_one(&mut *tx)
            .await
            .expect("actual prepared row"),
        1
    );
    tx.rollback()
        .await
        .expect("healthy preparation probe grants no partial durable effects");
    assert!(
        crate::order_stock::read_settlement(&pool, &order.identity.id)
            .await
            .expect("no durable receipt after positive rollback")
            .is_none()
    );
}
