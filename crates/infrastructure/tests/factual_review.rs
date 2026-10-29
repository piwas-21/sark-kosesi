// Synthetic merchant candidates in isolated PostgreSQL only; never supplier facts.
use commerce_application::request_context::{Clock, ContextError};
use commerce_domain::{
    ComponentKind,
    catalogue_authoring::{BomLineDraft, CatalogueCommand, Fill},
    factual_review::{FactDecision, FactTarget},
};
use commerce_infrastructure::{
    MIGRATOR,
    catalogue_authoring::{DefinitionRequest, record_definition},
    catalogue_prices::{PriceRevisionRequest, record_price_revision},
    factual_review::*,
    specifications::{SpecificationRequest, record_specification},
    staff_access::{SessionToken, StaffError},
};
use sqlx::{Executor, PgPool};
use std::sync::atomic::{AtomicUsize, Ordering};
include!("../../../tests/fixtures/specification.rs");
struct FixedClock(u64);
impl Clock for FixedClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(self.0)
    }
}
struct StepClock {
    calls: AtomicUsize,
    final_value: Result<u64, ContextError>,
}
impl Clock for StepClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            Ok(1100)
        } else {
            self.final_value
        }
    }
}
fn target() -> FactTarget {
    FactTarget {
        bom_revision: 1,
        specification_revision: 1,
        price_revision: 1,
    }
}
fn request(key: &str) -> FactReviewRequest<'_> {
    FactReviewRequest {
        sku: "SET-FILLED",
        target: target(),
        expected_review_revision: 0,
        effect_key: key,
        decision: FactDecision::Approved,
        evidence_ref: "isolated factual comparison",
        qualification_ref: "isolated reviewer qualification",
        reason: "isolated review",
        confirmed: true,
    }
}
fn author() -> SessionToken {
    SessionToken::from_bytes([1; 32])
}
fn reviewer() -> SessionToken {
    SessionToken::from_bytes([2; 32])
}
async fn definition(pool: &PgPool, command: &CatalogueCommand, key: &str) {
    record_definition(
        pool,
        &author(),
        &FixedClock(1100),
        DefinitionRequest {
            command,
            effect_key: key,
            source_ref: "isolated synthetic source",
            reason: "isolated graph",
            confirmed: true,
        },
    )
    .await
    .expect("definition");
}
async fn seed(pool: &PgPool) {
    pool.execute(include_str!("../../../tests/fixtures/bom.sql"))
        .await
        .expect("legacy fixtures");
    pool.execute("INSERT INTO staff_accounts VALUES('author',true),('reviewer',true),('other',true),('warehouse',true); INSERT INTO staff_capabilities VALUES('author','catalogue.manage'),('reviewer','catalogue.review'),('other','catalogue.review'),('warehouse','warehouse.manage')").await.expect("isolated grants");
    for (number, id) in [
        (1, "author"),
        (2, "reviewer"),
        (3, "other"),
        (4, "warehouse"),
    ] {
        sqlx::query("INSERT INTO staff_sessions(token_hash,staff_id,created_at,expires_at,mfa_verified_at) VALUES($1,$2,1000,4600,1100)").bind(SessionToken::from_bytes([number;32]).digest().as_slice()).bind(id).execute(pool).await.expect("isolated sessions");
    }
    definition(
        pool,
        &CatalogueCommand::Family {
            code: "ROOM".into(),
        },
        "family",
    )
    .await;
    for (code, kind) in [
        ("SEAT-COVER", ComponentKind::Cover),
        ("SEAT-FOAM", ComponentKind::Foam),
    ] {
        definition(
            pool,
            &CatalogueCommand::Component {
                code: code.into(),
                component_kind: kind,
            },
            code,
        )
        .await;
    }
    definition(
        pool,
        &CatalogueCommand::Variant {
            sku: "SET-FILLED".into(),
            family_code: "ROOM".into(),
            size_code: "isolated size".into(),
            textile_code: "isolated textile".into(),
            fill: Fill::WithFoam,
            lines: lines(2),
        },
        "variant",
    )
    .await;
    specification(pool, 0, 1, "spec-1").await;
    price(pool, 0, "price-1").await;
}
fn lines(quantity: u32) -> Vec<BomLineDraft> {
    vec![
        BomLineDraft {
            component_code: "SEAT-COVER".into(),
            quantity,
        },
        BomLineDraft {
            component_code: "SEAT-FOAM".into(),
            quantity,
        },
    ]
}
async fn specification(pool: &PgPool, expected: i32, bom_revision: i32, key: &str) {
    let mut value = draft(true);
    value.components[0].component_code = "SEAT-COVER".into();
    value.components[1].component_code = "SEAT-FOAM".into();
    record_specification(
        pool,
        &author(),
        &FixedClock(1100),
        SpecificationRequest {
            sku: "SET-FILLED",
            bom_revision,
            expected_revision: expected,
            effect_key: key,
            draft: &value,
            reason: "isolated specification",
            confirmed: true,
        },
    )
    .await
    .expect("specification");
}
async fn price(pool: &PgPool, expected: i32, key: &str) {
    record_price_revision(
        pool,
        &author(),
        &FixedClock(1100),
        PriceRevisionRequest {
            sku: "SET-FILLED",
            expected_revision: expected,
            effect_key: key,
            price: commerce_domain::Money::eur(12345).expect("fixture price"),
            evidence_ref: "isolated price evidence",
            reason: "isolated price",
            confirmed: true,
        },
    )
    .await
    .expect("price");
}
async fn count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM catalogue_factual_reviews")
        .fetch_one(pool)
        .await
        .expect("review count")
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn exact_facts_are_inspectable_and_review_never_activates_them(pool: PgPool) {
    seed(&pool).await;
    let record = facts_for_review(
        &pool,
        &reviewer(),
        &FixedClock(1100),
        "SET-FILLED",
        target(),
    )
    .await
    .expect("inspect")
    .expect("bundle");
    assert_eq!(record.price.minor_units(), 12345);
    assert_eq!(record.identity.actor, "author");
    assert_eq!(record.bom.actor, "author");
    assert_eq!(record.specification.draft.footprint.width_mm, 1000);
    assert_eq!(record.specification.bom_revision, 1);
    assert_eq!(record.specification_actor, "author");
    assert_eq!(record.price_evidence_ref, "isolated price evidence");
    assert!(record.review.is_none());
    let receipt = record_factual_review(&pool, &reviewer(), &FixedClock(1100), request("approve"))
        .await
        .expect("approve");
    assert_eq!((receipt.revision, receipt.replayed), (1, false));
    assert!(
        record_factual_review(&pool, &reviewer(), &FixedClock(1100), request("approve"))
            .await
            .expect("replay")
            .replayed
    );
    let record = facts_for_review(
        &pool,
        &reviewer(),
        &FixedClock(1100),
        "SET-FILLED",
        target(),
    )
    .await
    .expect("inspect")
    .expect("bundle");
    let review = record.review.expect("review");
    assert_eq!(review.decision, FactDecision::Approved);
    assert_eq!(review.actor, "reviewer");
    assert_eq!(review.revision, 1);
    let inactive:(Option<i64>,Option<String>,i64,i64)=sqlx::query_as("SELECT minor_units,slug,(SELECT count(*) FROM stock_movements),(SELECT count(*) FROM catalogue_translations WHERE sku='SET-FILLED') FROM catalogue_items WHERE sku='SET-FILLED'").fetch_one(&pool).await.expect("no activation");
    assert_eq!(inactive, (None, None, 0, 0));
    assert!(
        facts_for_review(&pool, &reviewer(), &FixedClock(1100), "MISSING", target())
            .await
            .expect("missing read")
            .is_none()
    );
    let mut missing = request("missing");
    missing.sku = "MISSING";
    assert!(matches!(
        record_factual_review(&pool, &reviewer(), &FixedClock(1100), missing).await,
        Err(FactError::NotFound)
    ));
    let mut absent = request("absent-spec");
    absent.target.specification_revision = 2;
    assert!(matches!(
        record_factual_review(&pool, &reviewer(), &FixedClock(1100), absent).await,
        Err(FactError::NotFound)
    ));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn stale_each_source_refuses_new_approval_but_historical_rejection_withdraws(pool: PgPool) {
    seed(&pool).await;
    record_factual_review(&pool, &reviewer(), &FixedClock(1100), request("approve"))
        .await
        .expect("first approval");
    price(&pool, 1, "price-2").await;
    assert!(matches!(
        record_factual_review(
            &pool,
            &reviewer(),
            &FixedClock(1100),
            request("stale-price")
        )
        .await,
        Err(FactError::Conflict)
    ));
    let mut latest = request("latest-price");
    latest.target.price_revision = 2;
    record_factual_review(&pool, &reviewer(), &FixedClock(1100), latest)
        .await
        .expect("current price");
    specification(&pool, 1, 1, "spec-2").await;
    let mut stale = request("stale-spec");
    stale.target.price_revision = 2;
    assert!(matches!(
        record_factual_review(&pool, &reviewer(), &FixedClock(1100), stale).await,
        Err(FactError::Conflict)
    ));
    let mut latest = request("latest-spec");
    latest.target.price_revision = 2;
    latest.target.specification_revision = 2;
    record_factual_review(&pool, &reviewer(), &FixedClock(1100), latest)
        .await
        .expect("current spec");
    definition(
        &pool,
        &CatalogueCommand::Bom {
            sku: "SET-FILLED".into(),
            expected_revision: 1,
            lines: lines(3),
        },
        "bom2",
    )
    .await;
    let mut stale = request("stale-bom");
    stale.target.price_revision = 2;
    stale.target.specification_revision = 2;
    stale.expected_review_revision = 1;
    assert!(matches!(
        record_factual_review(&pool, &reviewer(), &FixedClock(1100), stale).await,
        Err(FactError::Conflict)
    ));
    let mut reject = request("withdraw");
    reject.decision = FactDecision::Rejected;
    reject.expected_review_revision = 1;
    assert_eq!(
        record_factual_review(&pool, &reviewer(), &FixedClock(1100), reject)
            .await
            .expect("historical withdrawal")
            .revision,
        2
    );
    assert!(
        record_factual_review(&pool, &reviewer(), &FixedClock(1100), request("approve"))
            .await
            .expect("historical receipt")
            .replayed
    );
    let review = facts_for_review(
        &pool,
        &reviewer(),
        &FixedClock(1100),
        "SET-FILLED",
        target(),
    )
    .await
    .expect("read")
    .expect("bundle")
    .review
    .expect("latest decision");
    assert_eq!(
        (review.revision, review.decision),
        (2, FactDecision::Rejected)
    );
    specification(&pool, 2, 2, "spec-3").await;
    let mut latest = request("current-all");
    latest.target = FactTarget {
        bom_revision: 2,
        specification_revision: 3,
        price_revision: 2,
    };
    assert_eq!(
        record_factual_review(&pool, &reviewer(), &FixedClock(1100), latest)
            .await
            .expect("new exact target")
            .revision,
        1
    );
    assert_eq!(count(&pool).await, 5);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn every_retry_intent_field_and_actor_is_bound(pool: PgPool) {
    seed(&pool).await;
    record_factual_review(&pool, &reviewer(), &FixedClock(1100), request("same"))
        .await
        .expect("first");
    for field in 0..10 {
        let mut change = request("same");
        match field {
            0 => change.sku = "OTHER",
            1 => change.target.bom_revision = 2,
            2 => change.target.specification_revision = 2,
            3 => change.target.price_revision = 2,
            4 => change.expected_review_revision = 1,
            5 => change.decision = FactDecision::Rejected,
            6 => change.evidence_ref = "other evidence",
            7 => change.qualification_ref = "other qualification",
            8 => change.reason = "other reason",
            9 => {}
            _ => unreachable!(),
        }
        let token = if field == 9 {
            SessionToken::from_bytes([3; 32])
        } else {
            reviewer()
        };
        assert!(
            matches!(
                record_factual_review(&pool, &token, &FixedClock(1100), change).await,
                Err(FactError::Conflict)
            ),
            "field {field}"
        );
    }
    assert!(matches!(
        record_factual_review(
            &pool,
            &reviewer(),
            &FixedClock(1100),
            request("stale-review")
        )
        .await,
        Err(FactError::Conflict)
    ));
    assert_eq!(count(&pool).await, 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn authority_confirmation_and_bounded_envelopes_fail_closed(pool: PgPool) {
    seed(&pool).await;
    for number in [1, 4, 9] {
        let token = SessionToken::from_bytes([number; 32]);
        assert!(matches!(
            record_factual_review(&pool, &token, &FixedClock(1100), request("denied")).await,
            Err(FactError::Staff(StaffError::Denied))
        ));
        assert!(matches!(
            facts_for_review(&pool, &token, &FixedClock(1100), "SET-FILLED", target()).await,
            Err(FactError::Staff(StaffError::Denied))
        ));
    }
    for field in 0..13 {
        let mut invalid = request("invalid");
        let huge = "x".repeat(501);
        match field {
            0 => invalid.confirmed = false,
            1 => invalid.sku = "DEMO-FILLED",
            2 => invalid.target.bom_revision = 0,
            3 => invalid.target.specification_revision = 0,
            4 => invalid.target.price_revision = 0,
            5 => invalid.expected_review_revision = -1,
            6 => invalid.expected_review_revision = i32::MAX,
            7 => invalid.effect_key = "invalid/key",
            8 => invalid.evidence_ref = " ",
            9 => invalid.qualification_ref = " ",
            10 => invalid.reason = " ",
            11 => invalid.reason = "hidden\0text",
            12 => invalid.evidence_ref = &huge,
            _ => unreachable!(),
        }
        assert!(
            matches!(
                record_factual_review(&pool, &reviewer(), &FixedClock(1100), invalid).await,
                Err(FactError::InvalidRequest)
            ),
            "field {field}"
        );
    }
    for (sku, t) in [
        ("DEMO-FILLED", target()),
        (
            "SET-FILLED",
            FactTarget {
                price_revision: 0,
                ..target()
            },
        ),
    ] {
        assert!(matches!(
            facts_for_review(&pool, &reviewer(), &FixedClock(1100), sku, t).await,
            Err(FactError::InvalidRequest)
        ));
    }
    pool.execute("INSERT INTO staff_capabilities VALUES('author','catalogue.review')")
        .await
        .expect("dual grant");
    assert!(matches!(
        record_factual_review(&pool, &author(), &FixedClock(1100), request("self")).await,
        Err(FactError::SelfReview)
    ));
    assert_eq!(count(&pool).await, 0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn current_clock_checked_before_write_read_and_historical_receipt_return(pool: PgPool) {
    seed(&pool).await;
    for final_value in [Ok(2001), Err(ContextError::ClockUnavailable), Ok(u64::MAX)] {
        let clock = StepClock {
            calls: AtomicUsize::new(0),
            final_value,
        };
        assert!(matches!(
            record_factual_review(&pool, &reviewer(), &clock, request("clock")).await,
            Err(FactError::Staff(_))
        ));
        assert_eq!(count(&pool).await, 0);
        let clock = StepClock {
            calls: AtomicUsize::new(0),
            final_value,
        };
        assert!(matches!(
            facts_for_review(&pool, &reviewer(), &clock, "SET-FILLED", target()).await,
            Err(FactError::Staff(_))
        ));
    }
    record_factual_review(&pool, &reviewer(), &FixedClock(1100), request("clock"))
        .await
        .expect("valid clock");
    let clock = StepClock {
        calls: AtomicUsize::new(0),
        final_value: Ok(2001),
    };
    assert!(matches!(
        record_factual_review(&pool, &reviewer(), &clock, request("clock")).await,
        Err(FactError::Staff(StaffError::Denied))
    ));
    assert_eq!(count(&pool).await, 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn competing_reviewers_and_exact_retries_serialize_one_effect(pool: PgPool) {
    seed(&pool).await;
    let token = reviewer();
    let (a, b) = tokio::join!(
        record_factual_review(&pool, &token, &FixedClock(1100), request("exact")),
        record_factual_review(&pool, &token, &FixedClock(1100), request("exact"))
    );
    let (a, b) = (a.expect("first"), b.expect("retry"));
    assert_ne!(a.replayed, b.replayed);
    assert_eq!((a.revision, b.revision), (1, 1));
    let mut first = request("race-a");
    first.expected_review_revision = 1;
    let mut second = request("race-b");
    second.expected_review_revision = 1;
    second.decision = FactDecision::Rejected;
    let token = reviewer();
    let other = SessionToken::from_bytes([3; 32]);
    let (a, b) = tokio::join!(
        record_factual_review(&pool, &token, &FixedClock(1100), first),
        record_factual_review(&pool, &other, &FixedClock(1100), second)
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(matches!(a, Err(FactError::Conflict)) || matches!(b, Err(FactError::Conflict)));
    assert_eq!(count(&pool).await, 2);
}

#[test]
fn closed_targets_and_decisions_cannot_claim_actor_or_publication() {
    assert_eq!(
        serde_json::from_str::<FactTarget>(
            r#"{"bom_revision":1,"specification_revision":2,"price_revision":3}"#
        )
        .expect("target")
        .price_revision,
        3
    );
    for extra in ["actor", "published", "approved", "fixture"] {
        let value = serde_json::json!({"bom_revision":1,"specification_revision":1,"price_revision":1,extra:true});
        assert!(serde_json::from_value::<FactTarget>(value).is_err());
    }
    assert!(serde_json::from_str::<FactDecision>("\"active\"").is_err());
}

async fn waiting_on(pool: &PgPool, pattern: &str) {
    tokio::time::timeout(std::time::Duration::from_millis(800),async {loop {
        let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE $1)").bind(pattern).fetch_one(pool).await.expect("wait oracle");
        if waiting {break;}tokio::task::yield_now().await;
    }}).await.expect("actual database lock wait");
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn real_wait_expiry_and_cancellation_cannot_commit_reviews(pool: PgPool) {
    use std::sync::{Arc, atomic::AtomicU64};
    struct MovingClock(AtomicU64);
    impl Clock for MovingClock {
        fn unix_seconds(&self) -> Result<u64, ContextError> {
            Ok(self.0.load(Ordering::SeqCst))
        }
    }
    seed(&pool).await;
    for cancel in [false, true] {
        let mut blocker = pool.begin().await.expect("blocker");
        blocker
            .execute("LOCK TABLE catalogue_factual_reviews IN SHARE MODE")
            .await
            .expect("block review insert");
        let clock = Arc::new(MovingClock(AtomicU64::new(1100)));
        let writer_clock = clock.clone();
        let writer_pool = pool.clone();
        let writer = tokio::spawn(async move {
            record_factual_review(
                &writer_pool,
                &reviewer(),
                writer_clock.as_ref(),
                request("waiting"),
            )
            .await
        });
        waiting_on(&pool, "INSERT INTO catalogue_factual_reviews%").await;
        if cancel {
            writer.abort();
            assert!(writer.await.expect_err("cancelled").is_cancelled());
            blocker.rollback().await.expect("release");
        } else {
            clock.0.store(2001, Ordering::SeqCst);
            blocker.rollback().await.expect("release");
            assert!(matches!(
                writer.await.expect("task"),
                Err(FactError::Staff(StaffError::Denied))
            ));
        }
        assert_eq!(count(&pool).await, 0);
    }
    record_factual_review(&pool, &reviewer(), &FixedClock(1100), request("waiting"))
        .await
        .expect("retry after rollback");
    assert_eq!(count(&pool).await, 1);
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn revocation_waits_through_commit_then_denies_private_read_and_replay(pool: PgPool) {
    seed(&pool).await;
    let mut blocker = pool.begin().await.expect("blocker");
    blocker
        .execute("LOCK TABLE catalogue_factual_reviews IN SHARE MODE")
        .await
        .expect("block insert");
    let writer_pool = pool.clone();
    let writer = tokio::spawn(async move {
        record_factual_review(
            &writer_pool,
            &reviewer(),
            &FixedClock(1100),
            request("authorized"),
        )
        .await
    });
    waiting_on(&pool, "INSERT INTO catalogue_factual_reviews%").await;
    let revoke_pool = pool.clone();
    let revoke = tokio::spawn(async move {
        revoke_pool.execute("DELETE FROM staff_capabilities WHERE staff_id='reviewer' AND capability='catalogue.review'").await
    });
    waiting_on(&pool, "DELETE FROM staff_capabilities%").await;
    assert!(!revoke.is_finished());
    blocker.rollback().await.expect("resume");
    writer.await.expect("writer").expect("authorized commit");
    assert_eq!(
        revoke
            .await
            .expect("revoker")
            .expect("revoke")
            .rows_affected(),
        1
    );
    assert!(matches!(
        record_factual_review(&pool, &reviewer(), &FixedClock(1100), request("authorized")).await,
        Err(FactError::Staff(StaffError::Denied))
    ));
    assert!(matches!(
        facts_for_review(
            &pool,
            &reviewer(),
            &FixedClock(1100),
            "SET-FILLED",
            target()
        )
        .await,
        Err(FactError::Staff(StaffError::Denied))
    ));
    assert_eq!(count(&pool).await, 1);
}
const RAW_REVIEW: &str = "INSERT INTO catalogue_factual_reviews(sku,bom_revision,specification_revision,price_revision,revision,effect_key,decision,evidence_ref,qualification_ref,actor,reason) VALUES('SET-FILLED',1,1,1,$1,$2,$3,'isolated evidence','isolated qualification',$4,'isolated review')";
#[sqlx::test(migrator = "MIGRATOR")]
async fn schema_preserves_history_and_requires_independent_current_contiguous_targets(
    pool: PgPool,
) {
    seed(&pool).await;
    for (revision, key, actor) in [(1, "self", "author"), (2, "skip", "reviewer")] {
        let error = sqlx::query(RAW_REVIEW)
            .bind(revision)
            .bind(key)
            .bind("approved")
            .bind(actor)
            .execute(&pool)
            .await
            .expect_err("schema rejects");
        assert_eq!(
            error.as_database_error().and_then(|e| e.code()).as_deref(),
            Some("23514")
        );
    }
    record_factual_review(&pool, &reviewer(), &FixedClock(1100), request("approved"))
        .await
        .expect("approved");
    for sql in [
        "UPDATE catalogue_factual_reviews SET reason=reason",
        "DELETE FROM catalogue_factual_reviews",
    ] {
        let error = pool.execute(sql).await.expect_err("immutable");
        assert_eq!(
            error.as_database_error().and_then(|e| e.code()).as_deref(),
            Some("23514")
        );
    }
    price(&pool, 1, "new-price").await;
    let error = sqlx::query(RAW_REVIEW)
        .bind(2)
        .bind("stale")
        .bind("approved")
        .bind("reviewer")
        .execute(&pool)
        .await
        .expect_err("stale raw source");
    assert_eq!(
        error.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("23514")
    );
    sqlx::query(RAW_REVIEW)
        .bind(2)
        .bind("withdraw")
        .bind("rejected")
        .bind("reviewer")
        .execute(&pool)
        .await
        .expect("historical rejection");
    assert_eq!(count(&pool).await, 2);
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn stale_repeatable_read_import_has_no_lost_source_change(pool: PgPool) {
    seed(&pool).await;
    let mut stale = pool.begin().await.expect("stale");
    stale
        .execute("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .await
        .expect("snapshot isolation");
    let old: i32 = sqlx::query_scalar(
        "SELECT max(revision) FROM catalogue_price_revisions WHERE sku='SET-FILLED'",
    )
    .fetch_one(&mut *stale)
    .await
    .expect("old snapshot");
    assert_eq!(old, 1);
    price(&pool, 1, "latest-price").await;
    let error = sqlx::query(RAW_REVIEW)
        .bind(1)
        .bind("stale-import")
        .bind("approved")
        .bind("reviewer")
        .execute(&mut *stale)
        .await
        .expect_err("must retry fresh");
    assert_eq!(
        error.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("40001")
    );
    stale.rollback().await.expect("rollback");
    assert_eq!(count(&pool).await, 0);
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn stored_specification_and_definition_corruption_blocks_read_write_and_receipts(
    pool: PgPool,
) {
    seed(&pool).await;
    record_factual_review(&pool, &reviewer(), &FixedClock(1100), request("approve"))
        .await
        .expect("prior approval");
    for corruption in [
        "UPDATE catalogue_specification_revisions SET payload='{}'",
        "UPDATE catalogue_specification_revisions SET payload=jsonb_build_object('oversize',repeat('x',131073))",
        "UPDATE catalogue_definition_events SET payload=payload || '{\"unknown\":true}'::jsonb WHERE effect_key='variant'",
    ] {
        let mut tx = pool.begin().await.expect("corruption diagnostic");
        tx.execute("ALTER TABLE catalogue_specification_revisions DISABLE TRIGGER USER; ALTER TABLE catalogue_specification_revisions DROP CONSTRAINT catalogue_specification_revisions_payload_check; ALTER TABLE catalogue_definition_events DISABLE TRIGGER USER").await.expect("isolated corruption controls");
        tx.execute(corruption).await.expect("inject bad row");
        // Release the relation locks so public consumers can inspect the corrupted fixture.
        tx.commit().await.expect("corrupt test-only database");
        assert!(matches!(
            facts_for_review(
                &pool,
                &reviewer(),
                &FixedClock(1100),
                "SET-FILLED",
                target()
            )
            .await,
            Err(FactError::Specification(_) | FactError::Definition(_))
        ));
        assert!(
            record_factual_review(&pool, &reviewer(), &FixedClock(1100), request("approve"))
                .await
                .is_err()
        );
        let mut restore = draft(true);
        restore.components[0].component_code = "SEAT-COVER".into();
        restore.components[1].component_code = "SEAT-FOAM".into();
        sqlx::query("UPDATE catalogue_specification_revisions SET payload=$1::jsonb")
            .bind(serde_json::to_string(&restore).expect("fixture payload"))
            .execute(&pool)
            .await
            .expect("restore specification");
        pool.execute("UPDATE catalogue_definition_events SET payload=payload-'unknown' WHERE effect_key='variant'; ALTER TABLE catalogue_specification_revisions ENABLE TRIGGER USER; ALTER TABLE catalogue_specification_revisions ADD CONSTRAINT catalogue_specification_revisions_payload_check CHECK(jsonb_typeof(payload)='object' AND octet_length(payload::text)<=131072); ALTER TABLE catalogue_definition_events ENABLE TRIGGER USER").await.expect("restore all controls");
    }
    assert!(
        record_factual_review(&pool, &reviewer(), &FixedClock(1100), request("approve"))
            .await
            .expect("restored")
            .replayed
    );
    assert_eq!(count(&pool).await, 1);
}
#[sqlx::test(migrations = false)]
async fn upgrade_retains_candidates_and_sources_without_seeding_approval(pool: PgPool) {
    MIGRATOR
        .run_to(13, &pool)
        .await
        .expect("prior authoring schema");
    seed(&pool).await;
    let before: Vec<(String, Option<i64>, Option<String>, bool)> =
        sqlx::query_as("SELECT sku,minor_units,slug,fixture FROM catalogue_items ORDER BY sku")
            .fetch_all(&pool)
            .await
            .expect("prior data");
    MIGRATOR.run(&pool).await.expect("upgrade");
    let after: Vec<(String, Option<i64>, Option<String>, bool)> =
        sqlx::query_as("SELECT sku,minor_units,slug,fixture FROM catalogue_items ORDER BY sku")
            .fetch_all(&pool)
            .await
            .expect("retained data");
    assert_eq!(before, after);
    assert_eq!(count(&pool).await, 0);
    record_factual_review(
        &pool,
        &reviewer(),
        &FixedClock(1100),
        request("after-upgrade"),
    )
    .await
    .expect("explicit review");
    assert_eq!(count(&pool).await, 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn bounded_effect_lock_timeout_rolls_back_and_exact_retry_can_commit(pool: PgPool) {
    seed(&pool).await;
    let mut blocker = pool.begin().await.expect("effect blocker");
    blocker
        .execute("SELECT pg_advisory_xact_lock(hashtextextended('catalogue-facts:bounded',0))")
        .await
        .expect("lock effect");
    let error = record_factual_review(&pool, &reviewer(), &FixedClock(1100), request("bounded"))
        .await
        .expect_err("bounded wait");
    assert!(
        matches!(error,FactError::Database(ref e) if e.as_database_error().and_then(|e|e.code()).as_deref()==Some("55P03"))
    );
    assert_eq!(count(&pool).await, 0);
    blocker.rollback().await.expect("release");
    assert!(
        !record_factual_review(&pool, &reviewer(), &FixedClock(1100), request("bounded"))
            .await
            .expect("retry")
            .replayed
    );
    assert_eq!(count(&pool).await, 1);
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn raw_fixture_approval_and_mismatched_bom_specification_are_rejected(pool: PgPool) {
    seed(&pool).await;
    let value = draft(true);
    record_specification(
        &pool,
        &author(),
        &FixedClock(1100),
        SpecificationRequest {
            sku: "DEMO-FILLED",
            bom_revision: 1,
            expected_revision: 0,
            effect_key: "fixture-spec",
            draft: &value,
            reason: "isolated fixture",
            confirmed: true,
        },
    )
    .await
    .expect("private fixture specification");
    record_price_revision(
        &pool,
        &author(),
        &FixedClock(1100),
        PriceRevisionRequest {
            sku: "DEMO-FILLED",
            expected_revision: 0,
            effect_key: "fixture-price",
            price: commerce_domain::Money::eur(12345).expect("fixture money"),
            evidence_ref: "isolated fixture",
            reason: "isolated fixture",
            confirmed: true,
        },
    )
    .await
    .expect("private fixture price");
    let error = sqlx::query("INSERT INTO catalogue_factual_reviews(sku,bom_revision,specification_revision,price_revision,revision,effect_key,decision,evidence_ref,qualification_ref,actor,reason) VALUES('DEMO-FILLED',1,1,1,$1,$2,$3,'isolated evidence','isolated qualification',$4,'isolated review')")
        .bind(1)
        .bind("fixture-approval")
        .bind("approved")
        .bind("reviewer")
        .execute(&pool)
        .await
        .expect_err("known fixture must not approve");
    assert_eq!(
        error.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("23514")
    );
    definition(
        &pool,
        &CatalogueCommand::Bom {
            sku: "SET-FILLED".into(),
            expected_revision: 1,
            lines: lines(3),
        },
        "bom2",
    )
    .await;
    specification(&pool, 1, 2, "new-spec").await;
    let mismatch = FactTarget {
        bom_revision: 1,
        specification_revision: 2,
        price_revision: 1,
    };
    assert!(
        facts_for_review(
            &pool,
            &reviewer(),
            &FixedClock(1100),
            "SET-FILLED",
            mismatch
        )
        .await
        .expect("mismatch read")
        .is_none()
    );
    let mut invalid = request("mismatch");
    invalid.target = mismatch;
    assert!(matches!(
        record_factual_review(&pool, &reviewer(), &FixedClock(1100), invalid).await,
        Err(FactError::NotFound)
    ));
    let error = sqlx::query("INSERT INTO catalogue_factual_reviews(sku,bom_revision,specification_revision,price_revision,revision,effect_key,decision,evidence_ref,qualification_ref,actor,reason) VALUES('SET-FILLED',1,2,1,$1,$2,$3,'isolated evidence','isolated qualification',$4,'isolated review')")
    .bind(1)
    .bind("raw-mismatch")
    .bind("rejected")
    .bind("reviewer")
    .execute(&pool)
    .await
    .expect_err("even rejection must bind correct BOM");
    assert_eq!(
        error.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("23514")
    );
    assert_eq!(count(&pool).await, 0);
}
