//! Native imports cannot counterfeit physical consumption or adopt terminal groups.
use super::*;
use sqlx::{Postgres, Transaction};
use std::collections::BTreeMap;
async fn candidate(pool: &PgPool, f: &Fixture) -> Settlement {
    let holds: Vec<(String, String)> = sqlx::query_as("SELECT id,sku FROM stock_holds ORDER BY id")
        .fetch_all(pool)
        .await
        .expect("original hold IDs");
    let members = holds
        .into_iter()
        .map(|(hold_id, sku)| {
            let mut components = vec![Quantity {
                component_code: "SEAT-COVER".into(),
                quantity: 2,
            }];
            if sku == "SET-FILLED" {
                components.push(Quantity {
                    component_code: "SEAT-FOAM".into(),
                    quantity: 2,
                });
            }
            Member {
                hold_id,
                sku,
                revision: 1,
                units: 1,
                components,
            }
        })
        .collect();
    let recognition_id: String = sqlx::query_scalar(
        "SELECT recognition_id FROM payment_recognition_current WHERE order_id=$1",
    )
    .bind(&f.order.identity.id)
    .fetch_one(pool)
    .await
    .expect("original received receipt");
    Settlement {
        id: "a".repeat(32),
        order_id: f.order.identity.id.clone(),
        attempt_id: f.frozen.snapshot().binding.id.clone(),
        recognition_id,
        group_id: f.order.identity.stock_group_id.clone(),
        actor: "payment-settlement".into(),
        settled_at: 1500,
        footprint: Footprint {
            schema_version: 1,
            members,
            components: vec![
                Component {
                    component_code: "SEAT-COVER".into(),
                    quantity: 4,
                    before: Balance {
                        on_hand: 10,
                        quarantine: 1,
                        reserved: 4,
                    },
                    after: Balance {
                        on_hand: 6,
                        quarantine: 1,
                        reserved: 0,
                    },
                },
                Component {
                    component_code: "SEAT-FOAM".into(),
                    quantity: 2,
                    before: Balance {
                        on_hand: 4,
                        quarantine: 1,
                        reserved: 2,
                    },
                    after: Balance {
                        on_hand: 2,
                        quarantine: 1,
                        reserved: 0,
                    },
                },
            ],
        },
    }
}
async fn insert(tx: &mut Transaction<'_, Postgres>, s: &Settlement) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO order_stock_settlements(id,order_id,attempt_id,recognition_id,group_id,actor,settled_at,footprint) VALUES($1,$2,$3,$4,$5,$6,$7,$8)").bind(&s.id).bind(&s.order_id).bind(&s.attempt_id).bind(&s.recognition_id).bind(&s.group_id).bind(&s.actor).bind(s.settled_at).bind(serde_json::to_value(&s.footprint).expect("footprint fixture")).execute(&mut **tx).await.map(|_|())
}
async fn event(tx: &mut Transaction<'_, Postgres>, s: &Settlement) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) SELECT $1,s.order_id,'order.stock_settled.v1',jsonb_build_object('schema_version',1,'order_id',s.order_id,'settlement_id',s.id,'settlement_sha256',order_stock_digest(s)),s.settled_at FROM order_stock_settlements s WHERE s.id=$2").bind("b".repeat(32)).bind(&s.id).execute(&mut **tx).await.map(|_|())
}
#[derive(Clone, Copy)]
enum Fault {
    Healthy,
    NoPhysicalChange,
    WrongActor,
    WrongPrefix,
    Partial,
}
async fn effects(tx: &mut Transaction<'_, Postgres>, s: &Settlement, fault: Fault) {
    let mut running: BTreeMap<String, Balance> = s
        .footprint
        .components
        .iter()
        .map(|c| (c.component_code.clone(), c.before.clone()))
        .collect();
    let mut index = 0;
    for member in &s.footprint.members {
        for line in &member.components {
            index += 1;
            if matches!(fault, Fault::Partial) && index == 3 {
                continue;
            }
            let balance = running
                .get_mut(&line.component_code)
                .expect("fixture union");
            balance.on_hand -= line.quantity;
            balance.reserved -= line.quantity;
            if !matches!(fault, Fault::NoPhysicalChange) {
                sqlx::query("UPDATE stock_balances SET on_hand=$2,quarantine=$3,reserved=$4 WHERE component_code=$1").bind(&line.component_code).bind(balance.on_hand).bind(balance.quarantine).bind(balance.reserved).execute(&mut **tx).await.expect("actual physical consumption");
            }
            sqlx::query("INSERT INTO stock_movements(effect_key,component_code,action,quantity,actor,reason,on_hand_delta,quarantine_delta,reserved_delta,on_hand_after,quarantine_after,reserved_after,hold_id) VALUES($1,$2,'consume-hold',$3,$4,'sealed BOM hold transition',$5,0,$6,$7,$8,$9,$10)").bind(format!("hold:{}:consume-hold:{}",member.hold_id,line.component_code)).bind(&line.component_code).bind(line.quantity).bind(if matches!(fault,Fault::WrongActor)&&index==1{"forged-actor"}else{"payment-settlement"}).bind(-line.quantity).bind(-line.quantity).bind(balance.on_hand+if matches!(fault,Fault::WrongPrefix)&&index==1{1}else{0}).bind(balance.quarantine).bind(balance.reserved).bind(&member.hold_id).execute(&mut **tx).await.expect("native canonical movement candidate");
        }
        sqlx::query("UPDATE stock_holds SET state='consumed' WHERE id=$1")
            .bind(&member.hold_id)
            .execute(&mut **tx)
            .await
            .expect("native terminal state candidate");
    }
}
fn constraint(error: &sqlx::Error) {
    assert_eq!(
        error.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("23514")
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn native_settlement_prepared_before_effects_is_required_and_repeatable_read_or_serializable_cannot_import(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    let s = candidate(&pool, &f).await;
    let before = footprint(&pool).await;
    for isolation in [
        "SET TRANSACTION ISOLATION LEVEL REPEATABLE READ",
        "SET TRANSACTION ISOLATION LEVEL SERIALIZABLE",
    ] {
        let mut tx = pool.begin().await.expect("native importer");
        tx.execute(isolation).await.expect("isolation control");
        constraint(
            &insert(&mut tx, &s)
                .await
                .expect_err("native settlement requires RC"),
        );
        tx.rollback().await.expect("rollback");
    }
    // Legacy hold guards allow complete direct consumption; settlement must not
    // turn that previously consumed group into a new canonical receipt.
    let mut tx = pool.begin().await.expect("preconsumed group");
    effects(&mut tx, &s, Fault::Healthy).await;
    constraint(
        &insert(&mut tx, &s)
            .await
            .expect_err("cannot adopt preconsumed group"),
    );
    tx.rollback().await.expect("rollback");
    assert_eq!(footprint(&pool).await, before);
    let mut tx = pool.begin().await.expect("prepared healthy native control");
    insert(&mut tx, &s)
        .await
        .expect("receipt prepared while source active");
    effects(&mut tx, &s, Fault::Healthy).await;
    event(&mut tx, &s).await.expect("complete canonical event");
    tx.execute("SET CONSTRAINTS ALL IMMEDIATE")
        .await
        .expect("complete physical parity");
    tx.rollback()
        .await
        .expect("leave fresh canonical slot for next control");
    assert_eq!(footprint(&pool).await, before);
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn native_shaped_consumes_without_actual_balance_change_fail_deferred_and_identical_real_changes_pass(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    let s = candidate(&pool, &f).await;
    let before = footprint(&pool).await;
    let mut forged = pool.begin().await.expect("native fake physical effects");
    insert(&mut forged, &s)
        .await
        .expect("fresh prepared receipt");
    effects(&mut forged, &s, Fault::NoPhysicalChange).await;
    event(&mut forged, &s)
        .await
        .expect("shaped ledger and state pass immutable event custody");
    let visible: (i64, i64) = sqlx::query_as(
        "SELECT on_hand,reserved FROM stock_balances WHERE component_code='SEAT-COVER'",
    )
    .fetch_one(&mut *forged)
    .await
    .expect("actual unchanged physical control");
    assert_eq!(visible, (10, 4));
    let error = forged
        .commit()
        .await
        .expect_err("deferred guard must reject ledger shapes without physical balance change");
    constraint(&error);
    assert_eq!(footprint(&pool).await, before);
    let mut healthy = pool
        .begin()
        .await
        .expect("same literal import with real physical changes");
    insert(&mut healthy, &s)
        .await
        .expect("identical prepared receipt");
    effects(&mut healthy, &s, Fault::Healthy).await;
    event(&mut healthy, &s).await.expect("identical event");
    healthy
        .commit()
        .await
        .expect("actual balances make identical custody valid");
    assert_eq!(counts(&pool).await, (1, 1, 3, 1, 2));
    assert_eq!(
        balances(&pool).await,
        vec![
            ("SEAT-COVER".into(), 6, 1, 0),
            ("SEAT-FOAM".into(), 2, 1, 0)
        ]
    );
    assert!(
        read_settlement(&pool, &f.order.identity.id)
            .await
            .expect("native imported history")
            == Some(s)
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn native_settlement_rejects_partial_missing_and_forged_member_movements_and_requires_atomic_event(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    let s = candidate(&pool, &f).await;
    let before = footprint(&pool).await;
    for fault in [Fault::WrongActor, Fault::WrongPrefix, Fault::Partial] {
        let mut tx = pool.begin().await.expect("hostile member movement import");
        insert(&mut tx, &s).await.expect("valid prepared receipt");
        effects(&mut tx, &s, fault).await;
        constraint(
            &event(&mut tx, &s)
                .await
                .expect_err("bad exact member effects cannot bind an event"),
        );
        tx.rollback().await.expect("rollback");
        assert_eq!(footprint(&pool).await, before);
    }
    for complete_effects in [false, true] {
        let mut tx = pool.begin().await.expect("missing atomic effects/event");
        insert(&mut tx, &s).await.expect("prepared receipt");
        if complete_effects {
            effects(&mut tx, &s, Fault::Healthy).await;
        }
        constraint(
            &tx.commit()
                .await
                .expect_err("receipt without complete atomic effects and event cannot commit"),
        );
        assert_eq!(footprint(&pool).await, before);
    }
    let mut tx = pool.begin().await.expect("healthy exact effects control");
    insert(&mut tx, &s).await.expect("prepared");
    effects(&mut tx, &s, Fault::Healthy).await;
    event(&mut tx, &s).await.expect("exact event");
    tx.execute("SET CONSTRAINTS ALL IMMEDIATE")
        .await
        .expect("all guards genuinely satisfied");
    tx.rollback()
        .await
        .expect("no canonical uniqueness masking");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn native_settlement_deferred_financial_proof_rejects_observation_append_after_prepared_consumption(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    let s = candidate(&pool, &f).await;
    let before = footprint(&pool).await;
    let verified = fixture::verified(&f.frozen, "tr_SETTLEMENT", "open").await;
    let observation = commerce_domain::payment_attempts::observations::ObservationSnapshot {
        schema_version: 1,
        id: "c".repeat(32),
        attempt_id: s.attempt_id.clone(),
        effect_key: "native-after-consumption".into(),
        revision: 2,
        recorded_at: 1500,
        intent: commerce_domain::payment_attempts::observations::ObservationIntent {
            payment: verified.payment().clone(),
            credential_fingerprint: verified.credential_fingerprint().into(),
        },
    };
    let mut tx = pool.begin().await.expect("caller-owned RC importer");
    insert(&mut tx, &s)
        .await
        .expect("current received while preparing");
    effects(&mut tx, &s, Fault::Healthy).await;
    event(&mut tx, &s)
        .await
        .expect("matching complete physical custody");
    sqlx::query("INSERT INTO payment_observations(id,attempt_id,effect_key,revision,recorded_at,provider_id,credential_fingerprint,status,snapshot) VALUES($1,$2,$3,2,1500,'tr_SETTLEMENT',$4,'open',$5)").bind(&observation.id).bind(&s.attempt_id).bind(&observation.effect_key).bind(&observation.intent.credential_fingerprint).bind(serde_json::to_value(&observation).expect("genuine authenticated late fact")).execute(&mut *tx).await.expect("new observation in same claimed transaction");
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES($1,$2,'payment.observation_recorded.v1',$3,1500)").bind("d".repeat(32)).bind(&s.order_id).bind(json!({"schema_version":1,"order_id":s.order_id,"attempt_id":s.attempt_id,"observation_id":observation.id})).execute(&mut *tx).await.expect("complete new observation custody");
    constraint(
        &tx.commit()
            .await
            .expect_err("same-transaction append invalidates current proof before stock commit"),
    );
    assert_eq!(footprint(&pool).await, before);
    let mut healthy = pool
        .begin()
        .await
        .expect("identical stock proof without later append");
    insert(&mut healthy, &s).await.expect("prepare");
    effects(&mut healthy, &s, Fault::Healthy).await;
    event(&mut healthy, &s).await.expect("event");
    healthy
        .execute("SET CONSTRAINTS ALL IMMEDIATE")
        .await
        .expect("complete current financial and physical proof");
    healthy
        .rollback()
        .await
        .expect("restore empty canonical slot");
}

#[path = "order_stock_seal.rs"]
mod seal;

#[sqlx::test(migrator = "MIGRATOR")]
async fn native_current_unconfirmed_recognition_cannot_authorize_stock_but_same_order_paid_recognition_can(
    pool: PgPool,
) {
    let f = accepted(&pool, false, false).await;
    let attempt = &f.frozen.snapshot().binding.id;
    observe(&pool, &f, "tr_SETTLEMENT", "open", "open").await;
    let unconfirmed = recognize(&pool, f.time.as_ref(), f.ids.as_ref(), attempt)
        .await
        .expect("genuine canonical unconfirmed recognition")
        .recognition;
    assert_eq!(
        (
            unconfirmed.state,
            unconfirmed.through_observation,
            unconfirmed.amount_minor
        ),
        (
            crate::payment_attempts::recognition::State::Unconfirmed,
            1,
            25900
        )
    );
    let source_valid: bool = sqlx::query_scalar(
        "SELECT payment_recognition_source(r,true) FROM payment_recognitions r WHERE id=$1",
    )
    .bind(&unconfirmed.id)
    .fetch_one(&pool)
    .await
    .expect("native original source custody positive control");
    assert!(
        source_valid,
        "unconfirmed canonical evidence is valid stored source, not a damaged fixture"
    );
    let original = candidate(&pool, &f).await;
    let before = footprint(&pool).await;
    let mut denied = pool.begin().await.expect("raw unconfirmed importer");
    let error = insert(&mut denied, &original)
        .await
        .expect_err("native current received proof must reject genuinely unconfirmed payment");
    constraint(&error);
    assert_eq!(
        error
            .as_database_error()
            .expect("current received native guard")
            .message(),
        "Settlement requires current exact received recognition"
    );
    denied
        .rollback()
        .await
        .expect("no stock effects or receipt");
    assert_eq!(footprint(&pool).await, before);
    observe(&pool, &f, "tr_SETTLEMENT", "paid", "paid").await;
    let received = recognize(&pool, f.time.as_ref(), f.ids.as_ref(), attempt)
        .await
        .expect("genuine later paid recognition")
        .recognition;
    assert_eq!(
        received.state,
        crate::payment_attempts::recognition::State::Received
    );
    let paid = Settlement {
        recognition_id: received.id,
        ..original
    };
    let mut healthy = pool
        .begin()
        .await
        .expect("same original order and stock candidate after genuine payment");
    insert(&mut healthy, &paid)
        .await
        .expect("current received native proof succeeds");
    effects(&mut healthy, &paid, Fault::Healthy).await;
    event(&mut healthy, &paid)
        .await
        .expect("complete exact stock event");
    healthy
        .execute("SET CONSTRAINTS ALL IMMEDIATE")
        .await
        .expect("complete genuine current received proof and physical effects");
    healthy
        .rollback()
        .await
        .expect("healthy native probe leaves original reservation intact");
    assert_eq!(footprint(&pool).await, before);
    assert_eq!(counts(&pool).await, (0, 0, 0, 1, 2));
}
