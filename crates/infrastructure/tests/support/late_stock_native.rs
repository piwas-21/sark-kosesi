//! Native independent candidates: old shape guards alone do not prove balances.
use super::*;
use commerce_domain::order_stock::{Balance, Member, Quantity};
use sqlx::{Postgres, Transaction};
use std::collections::BTreeMap;

pub(super) async fn candidate(pool: &PgPool, f: &Fixture) -> LateAllocation {
    let holds: Vec<(String, String, String)> =
        sqlx::query_as("SELECT id,sku,state FROM stock_holds WHERE basket_group_id=$1 ORDER BY id")
            .bind(&f.order.identity.stock_group_id)
            .fetch_all(pool)
            .await
            .expect("literal original identities");
    let mut original = vec![];
    for (hold_id, sku, state) in holds {
        assert_eq!(state, "active");
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
        let mut pairs = vec![];
        for line in &components {
            let (sequence,digest):(i64,String)=sqlx::query_as("SELECT sequence,late_stock_movement_digest(m) FROM stock_movements m WHERE hold_id=$1 AND component_code=$2 AND action='reserve'").bind(&hold_id).bind(&line.component_code).fetch_one(pool).await.expect("actual original reserve reference");
            pairs.push(PairReference {
                component_code: line.component_code.clone(),
                reserve_sequence: sequence,
                reserve_digest: digest,
                terminal_sequence: None,
                terminal_digest: None,
            });
        }
        original.push(OriginalMember {
            member: Member {
                hold_id,
                sku,
                revision: 1,
                units: 1,
                components,
            },
            state: OriginalState::Active,
            pairs,
        });
    }
    let mut fresh: Vec<_> = original.iter().map(|m| m.member.clone()).collect();
    fresh.sort_by(|a, b| a.sku.cmp(&b.sku));
    for (index, m) in fresh.iter_mut().enumerate() {
        m.hold_id = format!("{:032x}", 0xc0000000 + index);
    }
    let recognition_id = sqlx::query_scalar(
        "SELECT recognition_id FROM payment_recognition_current WHERE order_id=$1",
    )
    .bind(&f.order.identity.id)
    .fetch_one(pool)
    .await
    .expect("captured genuine recognition");
    LateAllocation {
        id: "a".repeat(32),
        order_id: f.order.identity.id.clone(),
        attempt_id: f.frozen.snapshot().binding.id.clone(),
        recognition_id,
        original_group_id: f.order.identity.stock_group_id.clone(),
        fresh_group_id: "b".repeat(32),
        actor: "late-payment-allocation".into(),
        allocated_at: 2000,
        expires_at: 2060,
        footprint: LateFootprint {
            schema_version: 1,
            original,
            fresh,
            components: vec![
                StageComponent {
                    component_code: "SEAT-COVER".into(),
                    demand: 4,
                    active: 4,
                    before: Balance {
                        on_hand: 10,
                        quarantine: 1,
                        reserved: 4,
                    },
                    expired: Balance {
                        on_hand: 10,
                        quarantine: 1,
                        reserved: 0,
                    },
                    reserved: Balance {
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
                StageComponent {
                    component_code: "SEAT-FOAM".into(),
                    demand: 2,
                    active: 2,
                    before: Balance {
                        on_hand: 4,
                        quarantine: 1,
                        reserved: 2,
                    },
                    expired: Balance {
                        on_hand: 4,
                        quarantine: 1,
                        reserved: 0,
                    },
                    reserved: Balance {
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
async fn prepare(
    tx: &mut Transaction<'_, Postgres>,
    s: &LateAllocation,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO basket_hold_groups(id,owner_id,created_at,expires_at,members_count) VALUES($1,$2,2000,2060,$3)").bind(&s.fresh_group_id).bind(&s.order_id).bind(i32::try_from(s.footprint.fresh.len()).expect("bounded test group")).execute(&mut **tx).await?;
    for member in &s.footprint.fresh {
        sqlx::query("INSERT INTO stock_holds(id,owner_id,sku,revision,units,created_at,expires_at,basket_group_id) VALUES($1,$2,$3,1,1,2000,2060,$4)").bind(&member.hold_id).bind(&s.order_id).bind(&member.sku).bind(&s.fresh_group_id).execute(&mut **tx).await?;
    }
    Ok(())
}
async fn insert(tx: &mut Transaction<'_, Postgres>, s: &LateAllocation) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO late_stock_allocations(id,order_id,attempt_id,recognition_id,original_group_id,fresh_group_id,actor,allocated_at,expires_at,footprint) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)").bind(&s.id).bind(&s.order_id).bind(&s.attempt_id).bind(&s.recognition_id).bind(&s.original_group_id).bind(&s.fresh_group_id).bind(&s.actor).bind(s.allocated_at).bind(s.expires_at).bind(serde_json::to_value(&s.footprint).expect("literal candidate envelope")).execute(&mut **tx).await.map(|_|())
}
async fn event(tx: &mut Transaction<'_, Postgres>, s: &LateAllocation) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) SELECT $1,s.order_id,'order.stock_late_allocated.v1',jsonb_build_object('schema_version',1,'order_id',s.order_id,'allocation_id',s.id,'allocation_sha256',late_stock_digest(s)),s.allocated_at FROM late_stock_allocations s WHERE s.id=$2").bind("d".repeat(32)).bind(&s.id).execute(&mut **tx).await.map(|_|())
}
#[derive(Clone, Copy)]
enum Fault {
    Healthy,
    NoPhysicalChange,
    WrongPrefix,
    WrongActor,
    Partial,
}
async fn movement(
    tx: &mut Transaction<'_, Postgres>,
    member: &Member,
    line: &Quantity,
    action: &str,
    actor: &str,
    running: &mut BTreeMap<String, Balance>,
    injection: (Fault, usize),
) {
    let (fault, index) = injection;
    let b = running
        .get_mut(&line.component_code)
        .expect("literal union");
    let (on_hand_delta, reserved_delta) = match action {
        "reserve" => (0, line.quantity),
        "expire-hold" => (0, -line.quantity),
        "consume-hold" => (-line.quantity, -line.quantity),
        _ => panic!("closed fixture movement"),
    };
    b.on_hand += on_hand_delta;
    b.reserved += reserved_delta;
    if !matches!(fault, Fault::NoPhysicalChange) {
        sqlx::query("UPDATE stock_balances SET on_hand=$2,quarantine=$3,reserved=$4 WHERE component_code=$1").bind(&line.component_code).bind(b.on_hand).bind(b.quarantine).bind(b.reserved).execute(&mut **tx).await.expect("actual corresponding physical write");
    }
    if matches!(fault, Fault::Partial) && action == "consume-hold" && index == 3 {
        return;
    }
    sqlx::query("INSERT INTO stock_movements(effect_key,component_code,action,quantity,actor,reason,on_hand_delta,quarantine_delta,reserved_delta,on_hand_after,quarantine_after,reserved_after,hold_id) VALUES($1,$2,$3,$4,$5,'sealed BOM hold transition',$6,0,$7,$8,$9,$10,$11)").bind(format!("hold:{}:{action}:{}",member.hold_id,line.component_code)).bind(&line.component_code).bind(action).bind(line.quantity).bind(if matches!(fault,Fault::WrongActor)&&action=="consume-hold"&&index==1{"forged-consumer"}else{actor}).bind(on_hand_delta).bind(reserved_delta).bind(b.on_hand+if matches!(fault,Fault::WrongPrefix)&&action=="consume-hold"&&index==1{1}else{0}).bind(b.quarantine).bind(b.reserved).bind(&member.hold_id).execute(&mut **tx).await.expect("shaped native movement");
}
async fn effects(tx: &mut Transaction<'_, Postgres>, s: &LateAllocation, fault: Fault) {
    let mut running = s
        .footprint
        .components
        .iter()
        .map(|c| (c.component_code.clone(), c.before.clone()))
        .collect::<BTreeMap<_, _>>();
    for original in &s.footprint.original {
        for line in &original.member.components {
            movement(
                tx,
                &original.member,
                line,
                "expire-hold",
                "late-payment-expiry",
                &mut running,
                (fault, 0),
            )
            .await;
        }
        sqlx::query("UPDATE stock_holds SET state='expired' WHERE id=$1")
            .bind(&original.member.hold_id)
            .execute(&mut **tx)
            .await
            .expect("complete original expiry");
    }
    for m in &s.footprint.fresh {
        for line in &m.components {
            movement(
                tx,
                m,
                line,
                "reserve",
                &s.order_id,
                &mut running,
                (fault, 0),
            )
            .await;
            sqlx::query(
                "INSERT INTO stock_hold_lines(hold_id,component_code,quantity) VALUES($1,$2,$3)",
            )
            .bind(&m.hold_id)
            .bind(&line.component_code)
            .bind(line.quantity)
            .execute(&mut **tx)
            .await
            .expect("frozen fresh source line");
        }
        sqlx::query("UPDATE stock_holds SET state='active' WHERE id=$1")
            .bind(&m.hold_id)
            .execute(&mut **tx)
            .await
            .expect("exact sealed BOM active");
        sqlx::query("INSERT INTO basket_hold_members(group_id,hold_id,sku,revision,units) VALUES($1,$2,$3,1,1)").bind(&s.fresh_group_id).bind(&m.hold_id).bind(&m.sku).execute(&mut **tx).await.expect("complete fresh membership");
    }
    sqlx::query("UPDATE basket_hold_groups SET sealed=true WHERE id=$1")
        .bind(&s.fresh_group_id)
        .execute(&mut **tx)
        .await
        .expect("seal exact fresh group");
    let mut index = 0;
    for m in &s.footprint.fresh {
        for line in &m.components {
            index += 1;
            movement(
                tx,
                m,
                line,
                "consume-hold",
                "payment-settlement",
                &mut running,
                (fault, index),
            )
            .await;
        }
        sqlx::query("UPDATE stock_holds SET state='consumed' WHERE id=$1")
            .bind(&m.hold_id)
            .execute(&mut **tx)
            .await
            .expect("fresh complete consumed");
    }
}
fn constraint(error: &sqlx::Error) {
    assert_eq!(
        error.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("23514")
    );
}
pub(super) async fn healthy_probe(pool: &PgPool, s: &LateAllocation) {
    let mut tx = pool.begin().await.expect("identical healthy native probe");
    prepare(&mut tx, s).await.expect("fresh building plan");
    insert(&mut tx, s)
        .await
        .expect("prepared receipt and automatic canonical claim");
    effects(&mut tx, s, Fault::Healthy).await;
    event(&mut tx, s).await.expect("exact event");
    tx.execute("SET CONSTRAINTS ALL IMMEDIATE")
        .await
        .expect("same candidate complete physical proof passes");
    tx.rollback()
        .await
        .expect("leave canonical slot empty for next fault");
}

#[sqlx::test(migrations = false)]
async fn late_stock_schema33_upgrade_preserves_native_allocation_clock_bounds(pool: PgPool) {
    MIGRATOR.run_to(33, &pool).await.expect("schema 33");
    let f = accepted(&pool, false, true).await;
    let s = candidate(&pool, &f).await;
    let before = all_effects(&pool).await;
    MIGRATOR
        .run(&pool)
        .await
        .expect("upgrade through schema 37");
    assert_eq!(all_effects(&pool).await, before);

    for (allocated_at, expires_at) in [
        (1999, 2060),
        (2060, 2060),
        (i64::MIN, 2060),
        (2000, i64::MIN),
        (9_007_199_254_740_992, 2060),
        (2000, 9_007_199_254_740_992),
    ] {
        let mut invalid = s.clone();
        invalid.allocated_at = allocated_at;
        invalid.expires_at = expires_at;
        let mut tx = pool.begin().await.expect("invalid native clock");
        prepare(&mut tx, &invalid).await.expect("same fresh source");
        constraint(
            &insert(&mut tx, &invalid)
                .await
                .expect_err("invalid clock rejected"),
        );
        tx.rollback().await.expect("rollback rejected candidate");
        assert_eq!(all_effects(&pool).await, before);
        healthy_probe(&pool, &s).await;
    }

    let mut last = s.clone();
    last.allocated_at = 2059;
    healthy_probe(&pool, &last).await;
    let mut tx = pool
        .begin()
        .await
        .expect("valid upgraded native allocation");
    prepare(&mut tx, &s).await.expect("fresh source");
    insert(&mut tx, &s).await.expect("valid receipt");
    effects(&mut tx, &s, Fault::Healthy).await;
    event(&mut tx, &s).await.expect("exact event");
    tx.commit().await.expect("complete allocation commits");

    // Evaluate a copy of the committed receipt so no insert or table constraint
    // can mask a regression in the historical predicate's window check.
    for (allocated_at, expires_at, expected) in [
        (2000, 2060, true),
        (2059, 2060, true),
        (1999, 2060, false),
        (2060, 2060, false),
        (2000, i64::MIN, false),
        (2000, 9_007_199_254_740_992, false),
    ] {
        let valid: bool = sqlx::query_scalar("SELECT late_stock_allocation_exact(jsonb_populate_record(s, jsonb_build_object('allocated_at',$2::bigint,'expires_at',$3::bigint)),false,false) FROM late_stock_allocations s WHERE s.id=$1")
            .bind(&s.id).bind(allocated_at).bind(expires_at)
            .fetch_one(&pool).await.expect("historical clock predicate");
        assert_eq!(valid, expected, "clock pair ({allocated_at}, {expires_at})");
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_native_rr_serializable_and_orphan_claims_fail_with_same_rc_candidate_healthy(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    let s = candidate(&pool, &f).await;
    let before = all_effects(&pool).await;
    for isolation in [
        "SET TRANSACTION ISOLATION LEVEL REPEATABLE READ",
        "SET TRANSACTION ISOLATION LEVEL SERIALIZABLE",
    ] {
        let mut tx = pool.begin().await.expect("native importer");
        tx.execute(isolation).await.expect("hostile isolation");
        prepare(&mut tx, &s).await.expect("fresh candidate only");
        constraint(&insert(&mut tx, &s).await.expect_err("native RC required"));
        tx.rollback().await.expect("rollback");
        healthy_probe(&pool, &s).await;
    }
    let mut tx = pool.begin().await.expect("orphan claim importer");
    let result=sqlx::query("INSERT INTO order_stock_allocations(order_id,attempt_id,kind,receipt_id) VALUES($1,$2,'late',$3)").bind(&s.order_id).bind(&s.attempt_id).bind(&s.id).execute(&mut *tx).await;
    let error = match result {
        Ok(_) => tx
            .execute("SET CONSTRAINTS ALL IMMEDIATE")
            .await
            .expect_err("unowned claim cannot commit"),
        Err(error) => error,
    };
    constraint(&error);
    tx.rollback().await.expect("rollback orphan");
    healthy_probe(&pool, &s).await;
    assert_eq!(all_effects(&pool).await, before);
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_native_shaped_stage_ledger_without_actual_balances_and_forged_partial_prefixes_fail_deferred(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    let s = candidate(&pool, &f).await;
    let before = all_effects(&pool).await;
    for fault in [
        Fault::NoPhysicalChange,
        Fault::WrongActor,
        Fault::WrongPrefix,
        Fault::Partial,
    ] {
        for deferred in [false, true] {
            let mut tx = pool.begin().await.expect("shaped counterfeit");
            prepare(&mut tx, &s).await.expect("building children");
            insert(&mut tx, &s)
                .await
                .expect("valid admission prevents earlier masking");
            effects(&mut tx, &s, fault).await;
            let error = if deferred {
                tx.execute("ALTER TABLE commerce_outbox DISABLE TRIGGER guard_commerce_outbox")
                    .await
                    .expect("isolate parent deferred proof from earlier event proof");
                event(&mut tx, &s)
                    .await
                    .expect("exact matching digest event with only earlier outer guard bypassed");
                tx.execute("ALTER TABLE commerce_outbox ENABLE TRIGGER guard_commerce_outbox")
                    .await
                    .expect("restore original event guard before proof");
                tx.execute("SET CONSTRAINTS ALL IMMEDIATE")
                    .await
                    .expect_err("independent native deferred physical proof rejects counterfeit")
            } else if matches!(fault, Fault::NoPhysicalChange) {
                event(&mut tx, &s).await.expect(
                    "event proves ledger shape while final actual balances remain deferred",
                );
                tx.execute("SET CONSTRAINTS ALL IMMEDIATE").await.expect_err("native deferred proof rejects shaped ledger with no actual physical change")
            } else {
                event(&mut tx, &s)
                    .await
                    .expect_err("outer event guard independently rejects malformed stages")
            };
            constraint(&error);
            assert_eq!(
                error
                    .as_database_error()
                    .expect("specific physical proof")
                    .message(),
                if deferred || matches!(fault, Fault::NoPhysicalChange) {
                    "Late allocation requires complete exact physical stages claim and event"
                } else {
                    "Late allocation event requires exact complete receipt and effects"
                }
            );
            tx.rollback().await.expect("rollback counterfeit");
            assert_eq!(all_effects(&pool).await, before);
            let mut healthy = pool
                .begin()
                .await
                .expect("paired identical valid write with actual physical changes");
            prepare(&mut healthy, &s)
                .await
                .expect("same fresh candidate");
            insert(&mut healthy, &s)
                .await
                .expect("same receipt admission");
            effects(&mut healthy, &s, Fault::Healthy).await;
            if deferred {
                healthy
                    .execute("ALTER TABLE commerce_outbox DISABLE TRIGGER guard_commerce_outbox")
                    .await
                    .expect("same event guard isolation");
            }
            event(&mut healthy, &s).await.expect("healthy exact event");
            if deferred {
                healthy
                    .execute("ALTER TABLE commerce_outbox ENABLE TRIGGER guard_commerce_outbox")
                    .await
                    .expect("same guard restoration");
            }
            healthy
                .execute("SET CONSTRAINTS ALL IMMEDIATE")
                .await
                .expect("same parent proof passes with real physical changes");
            healthy
                .rollback()
                .await
                .expect("leave canonical slot empty");
        }
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_native_complete_effects_require_prepared_receipt_and_exact_atomic_event(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    let s = candidate(&pool, &f).await;
    let before = all_effects(&pool).await;
    let mut tx = pool.begin().await.expect("effects preceding parent");
    prepare(&mut tx, &s).await.expect("fresh children");
    effects(&mut tx, &s, Fault::Healthy).await;
    constraint(
        &insert(&mut tx, &s)
            .await
            .expect_err("completed unowned effects cannot be adopted"),
    );
    tx.rollback().await.expect("rollback");
    healthy_probe(&pool, &s).await;
    let mut tx = pool.begin().await.expect("missing event");
    prepare(&mut tx, &s).await.expect("fresh children");
    insert(&mut tx, &s).await.expect("admission");
    effects(&mut tx, &s, Fault::Healthy).await;
    constraint(
        &tx.execute("SET CONSTRAINTS ALL IMMEDIATE")
            .await
            .expect_err("whole effects without exact order event cannot commit"),
    );
    tx.rollback().await.expect("rollback");
    healthy_probe(&pool, &s).await;
    assert_eq!(all_effects(&pool).await, before);
}
#[path = "late_stock_seal.rs"]
mod seal;

#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_native_genuine_unconfirmed_receipt_cannot_authorize_stock_but_same_order_received_can(
    pool: PgPool,
) {
    let f = accepted(&pool, false, false).await;
    observe(
        &pool,
        &f,
        "tr_SETTLEMENT",
        "open",
        "initial-authenticated-open",
    )
    .await;
    recognize(
        &pool,
        f.time.as_ref(),
        f.ids.as_ref(),
        &f.frozen.snapshot().binding.id,
    )
    .await
    .expect("genuine canonical unconfirmed recognition");
    let s = candidate(&pool, &f).await;
    let before = all_effects(&pool).await;
    let mut tx = pool
        .begin()
        .await
        .expect("native unconfirmed authority attempt");
    prepare(&mut tx, &s)
        .await
        .expect("same original frozen plan");
    constraint(
        &insert(&mut tx, &s)
            .await
            .expect_err("native current received authority required without Rust entry guard"),
    );
    tx.rollback().await.expect("rollback native rejection");
    assert_eq!(all_effects(&pool).await, before);
    observe(
        &pool,
        &f,
        "tr_SETTLEMENT",
        "paid",
        "later-authenticated-paid",
    )
    .await;
    recognize(
        &pool,
        f.time.as_ref(),
        f.ids.as_ref(),
        &f.frozen.snapshot().binding.id,
    )
    .await
    .expect("same actual order now received");
    let received = candidate(&pool, &f).await;
    healthy_probe(&pool, &received).await;
    assert_eq!(allocation_counts(&pool).await, (0, 0, 0, 0, 1, 2, 0, 0, 3));
}
#[path = "late_stock_native_review.rs"]
mod review;
