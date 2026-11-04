//! A review is a source-bound immutable stop, never physical allocation authority.
use super::*;
async fn review_candidate(pool: &PgPool, f: &Fixture) -> ReviewCase {
    let s = candidate(pool, f).await;
    let mut original = s.footprint.original;
    assert_eq!(original[0].member.sku, "SET-COVERS");
    transition_hold(
        pool,
        &original[0].member.hold_id,
        HoldCommand::Release {
            owner_id: &f.order.identity.id,
            actor_id: "fixture-release",
        },
        1500,
    )
    .await
    .expect("genuine release makes admission stop meaningful");
    original[0].state = OriginalState::Released;
    let released_hold = original[0].member.hold_id.clone();
    for pair in &mut original[0].pairs {
        let (sequence,digest):(i64,String)=sqlx::query_as("SELECT sequence,late_stock_movement_digest(m) FROM stock_movements m WHERE hold_id=$1 AND component_code=$2 AND action='release-hold'").bind(&released_hold).bind(&pair.component_code).fetch_one(pool).await.expect("exact genuine terminal source");
        pair.terminal_sequence = Some(sequence);
        pair.terminal_digest = Some(digest);
    }
    ReviewCase {
        id: s.id,
        order_id: s.order_id,
        attempt_id: s.attempt_id,
        recognition_id: s.recognition_id,
        group_id: s.original_group_id,
        actor: "late-stock-review".into(),
        reason: ReviewReason::ReleaseIntentUnresolved,
        assessed_at: 2000,
        footprint: ReviewFootprint {
            schema_version: 1,
            original,
            components: vec![
                AssessedComponent {
                    component_code: "SEAT-COVER".into(),
                    demand: 4,
                    active: 2,
                    before: Balance {
                        on_hand: 10,
                        quarantine: 1,
                        reserved: 2,
                    },
                },
                AssessedComponent {
                    component_code: "SEAT-FOAM".into(),
                    demand: 2,
                    active: 2,
                    before: Balance {
                        on_hand: 4,
                        quarantine: 1,
                        reserved: 2,
                    },
                },
            ],
        },
    }
}
async fn insert_review(
    tx: &mut Transaction<'_, Postgres>,
    r: &ReviewCase,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO late_stock_review_cases(id,order_id,attempt_id,recognition_id,group_id,actor,reason,assessed_at,footprint) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)").bind(&r.id).bind(&r.order_id).bind(&r.attempt_id).bind(&r.recognition_id).bind(&r.group_id).bind(&r.actor).bind(r.reason.code()).bind(r.assessed_at).bind(serde_json::to_value(&r.footprint).expect("literal review source")).execute(&mut **tx).await.map(|_|())
}
async fn review_event(tx: &mut Transaction<'_, Postgres>, r: &ReviewCase) {
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) SELECT $1,r.order_id,'order.stock_recovery_review.v1',jsonb_build_object('schema_version',1,'order_id',r.order_id,'case_id',r.id,'case_sha256',late_stock_review_digest(r)),r.assessed_at FROM late_stock_review_cases r WHERE r.id=$2").bind("b".repeat(32)).bind(&r.id).execute(&mut **tx).await.expect("exact digest event");
}
async fn healthy_review(pool: &PgPool, r: &ReviewCase) {
    let mut tx = pool.begin().await.expect("same raw healthy review");
    insert_review(&mut tx, r)
        .await
        .expect("valid source-bound stop");
    review_event(&mut tx, r).await;
    tx.execute("SET CONSTRAINTS ALL IMMEDIATE")
        .await
        .expect("whole native source/event proof");
    tx.rollback()
        .await
        .expect("leave source fresh for next case");
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_native_review_requires_rc_exact_reason_source_and_event_and_never_creates_allocation_claim(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    let r = review_candidate(&pool, &f).await;
    let before = all_effects(&pool).await;
    for isolation in [
        "SET TRANSACTION ISOLATION LEVEL REPEATABLE READ",
        "SET TRANSACTION ISOLATION LEVEL SERIALIZABLE",
    ] {
        let mut tx = pool.begin().await.expect("review importer");
        tx.execute(isolation).await.expect("hostile isolation");
        constraint(
            &insert_review(&mut tx, &r)
                .await
                .expect_err("native RC required"),
        );
        tx.rollback().await.expect("rollback");
        healthy_review(&pool, &r).await;
    }
    for reason in [
        ReviewReason::ComponentScarcity,
        ReviewReason::PriorConsumptionWithoutAllocation,
    ] {
        let mut wrong = r.clone();
        wrong.reason = reason;
        let mut tx = pool.begin().await.expect("false review reason");
        constraint(
            &insert_review(&mut tx, &wrong)
                .await
                .expect_err("actual released source requires exact stop reason"),
        );
        tx.rollback().await.expect("rollback");
        healthy_review(&pool, &r).await;
    }
    let mut tx = pool.begin().await.expect("missing review event");
    insert_review(&mut tx, &r).await.expect("source admission");
    constraint(
        &tx.execute("SET CONSTRAINTS ALL IMMEDIATE")
            .await
            .expect_err("no event means incomplete durable review"),
    );
    tx.rollback().await.expect("rollback");
    healthy_review(&pool, &r).await;
    assert_eq!(all_effects(&pool).await, before);
    let mut tx = pool.begin().await.expect("healthy actual review commit");
    insert_review(&mut tx, &r)
        .await
        .expect("same source admission");
    review_event(&mut tx, &r).await;
    tx.commit().await.expect("complete review commits");
    assert_eq!(allocation_counts(&pool).await, (0, 0, 1, 0, 1, 2, 0, 0, 3));
    replay_without_ports(&pool, &f, Outcome::NeedsReview(r)).await;
}
