use commerce_application::request_context::{Clock, ContextError};
use commerce_infrastructure::{
    MIGRATOR,
    specifications::{
        SpecificationRequest, SpecificationWriteError, record_specification, specification,
    },
    staff_access::{SessionToken, StaffError},
};
use sqlx::{Executor, PgPool};
include!("../../../tests/fixtures/specification.rs");
struct FixedClock(u64);
impl Clock for FixedClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(self.0)
    }
}
fn request<'a>(draft: &'a SpecificationDraft, effect: &'a str) -> SpecificationRequest<'a> {
    SpecificationRequest {
        sku: "DEMO-FILLED",
        bom_revision: 1,
        expected_revision: 0,
        effect_key: effect,
        draft,
        reason: "isolated specification test",
        confirmed: true,
    }
}
async fn seed(pool: &PgPool) {
    pool.execute(include_str!("../../../tests/fixtures/bom.sql"))
        .await
        .expect("fixture");
    pool.execute("INSERT INTO staff_accounts VALUES ('fixture-catalogue',true),('fixture-warehouse',true),('fixture-other',true);
        INSERT INTO staff_capabilities VALUES ('fixture-catalogue','catalogue.manage'),('fixture-warehouse','warehouse.manage'),('fixture-other','catalogue.manage')").await.expect("fixture grants");
    for (number, actor) in [
        (1, "fixture-catalogue"),
        (2, "fixture-warehouse"),
        (3, "fixture-other"),
    ] {
        sqlx::query("INSERT INTO staff_sessions (token_hash,staff_id,created_at,expires_at,mfa_verified_at) VALUES ($1,$2,1000,4600,1100)")
            .bind(SessionToken::from_bytes([number;32]).digest().as_slice()).bind(actor).execute(pool).await.expect("test-only attestation");
    }
}
async fn count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM catalogue_specification_revisions")
        .fetch_one(pool)
        .await
        .expect("history count")
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn immutable_draft_history_preserves_bom_measurements_materials_and_preview(pool: PgPool) {
    seed(&pool).await;
    let token = SessionToken::from_bytes([1; 32]);
    let first = draft(true);
    let preview: Vec<(String, i64, bool)> =
        sqlx::query_as("SELECT sku,minor_units,fixture FROM catalogue_items ORDER BY sku")
            .fetch_all(&pool)
            .await
            .expect("preview oracle");
    let receipt = record_specification(&pool, &token, &FixedClock(1100), request(&first, "first"))
        .await
        .expect("version1");
    assert_eq!((receipt.revision, receipt.replayed), (1, false));
    let mut second = first.clone();
    second.footprint.width_mm = 1100;
    let mut next = request(&second, "second");
    next.expected_revision = 1;
    assert_eq!(
        record_specification(&pool, &token, &FixedClock(1100), next)
            .await
            .expect("version2")
            .revision,
        2
    );
    let replay = record_specification(&pool, &token, &FixedClock(1100), request(&first, "first"))
        .await
        .expect("exact replay");
    assert_eq!((replay.revision, replay.replayed), (1, true));
    for (revision, expected) in [(1, first.clone()), (2, second)] {
        let record = specification(&pool, &token, &FixedClock(1100), "DEMO-FILLED", revision)
            .await
            .expect("guarded read")
            .expect("snapshot");
        assert_eq!(record.bom_revision, 1);
        assert_eq!(record.draft, expected);
    }
    let covers = draft(false);
    let mut covers_request = request(&covers, "covers");
    covers_request.sku = "DEMO-COVERS";
    assert_eq!(
        record_specification(&pool, &token, &FixedClock(1100), covers_request)
            .await
            .expect("covers only")
            .revision,
        1
    );
    let audit:(String,String,bool)=sqlx::query_as("SELECT actor,reason,recorded_at IS NOT NULL FROM catalogue_specification_revisions WHERE effect_key='first'").fetch_one(&pool).await.expect("audit oracle");
    assert_eq!(
        audit,
        (
            "fixture-catalogue".into(),
            "isolated specification test".into(),
            true
        )
    );
    let after: Vec<(String, i64, bool)> =
        sqlx::query_as("SELECT sku,minor_units,fixture FROM catalogue_items ORDER BY sku")
            .fetch_all(&pool)
            .await
            .expect("preview unchanged");
    assert_eq!(after, preview);
    assert_eq!(
        specification(&pool, &token, &FixedClock(1100), "DEMO-FILLED", 99)
            .await
            .expect("missing revision"),
        None
    );
    assert_eq!(count(&pool).await, 3);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn authority_confirmation_bounds_and_unsealed_boms_cannot_create_drafts(pool: PgPool) {
    seed(&pool).await;
    let candidate = draft(true);
    let token = SessionToken::from_bytes([1; 32]);
    for number in [2, 99] {
        let other = SessionToken::from_bytes([number; 32]);
        assert!(matches!(
            record_specification(
                &pool,
                &other,
                &FixedClock(1100),
                request(&candidate, "deny")
            )
            .await,
            Err(SpecificationWriteError::Staff(StaffError::Denied))
        ));
        assert!(matches!(
            specification(&pool, &other, &FixedClock(1100), "DEMO-FILLED", 1).await,
            Err(SpecificationWriteError::Staff(StaffError::Denied))
        ));
    }
    for case in 0..12 {
        let mut r = request(&candidate, "invalid");
        let sku = "x".repeat(66);
        let key = "x".repeat(129);
        let reason = "x".repeat(501);
        match case {
            0 => r.confirmed = false,
            1 => r.sku = "",
            2 => r.sku = &sku,
            3 => r.bom_revision = 0,
            4 => r.expected_revision = -1,
            5 => r.expected_revision = i32::MAX,
            6 => r.effect_key = "",
            7 => r.effect_key = &key,
            8 => r.effect_key = "bad_key",
            9 => r.effect_key = "é",
            10 => r.reason = " ",
            11 => r.reason = &reason,
            _ => unreachable!(),
        }
        assert!(
            matches!(
                record_specification(&pool, &token, &FixedClock(1100), r).await,
                Err(SpecificationWriteError::InvalidRequest)
            ),
            "invalid case {case}"
        );
    }
    for (sku, revision) in [
        ("", 1),
        ("DEMO-FILLED", 0),
        (
            "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
            1,
        ),
    ] {
        assert!(matches!(
            specification(&pool, &token, &FixedClock(1100), sku, revision).await,
            Err(SpecificationWriteError::InvalidRequest)
        ));
    }
    let mut missing = request(&candidate, "missing");
    missing.sku = "DEMO-ABSENT";
    assert!(matches!(
        record_specification(&pool, &token, &FixedClock(1100), missing).await,
        Err(SpecificationWriteError::NotFound)
    ));
    pool.execute("INSERT INTO catalogue_boms (sku,revision) VALUES ('DEMO-FILLED',2)")
        .await
        .expect("draft BOM");
    let mut unsealed = request(&candidate, "unsealed");
    unsealed.bom_revision = 2;
    assert!(matches!(
        record_specification(&pool, &token, &FixedClock(1100), unsealed).await,
        Err(SpecificationWriteError::NotFound)
    ));
    let mut invalid = candidate.clone();
    invalid.components[0].traceability_ref.clear();
    assert!(matches!(
        record_specification(
            &pool,
            &token,
            &FixedClock(1100),
            request(&invalid, "invalid-data")
        )
        .await,
        Err(SpecificationWriteError::InvalidRequest)
    ));
    assert_eq!(count(&pool).await, 0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn conflicting_request_identity_and_stale_versions_leave_history_unchanged(pool: PgPool) {
    seed(&pool).await;
    let token = SessionToken::from_bytes([1; 32]);
    let original = draft(true);
    let covers = draft(false);
    record_specification(&pool, &token, &FixedClock(1100), request(&original, "key"))
        .await
        .expect("original");
    pool.execute("INSERT INTO catalogue_boms(sku,revision) VALUES ('DEMO-FILLED',2); INSERT INTO catalogue_bom_lines SELECT sku,2,component_code,quantity FROM catalogue_bom_lines WHERE sku='DEMO-FILLED' AND revision=1; UPDATE catalogue_boms SET sealed=true WHERE sku='DEMO-FILLED' AND revision=2").await.expect("another sealed revision");
    let mut changed = original.clone();
    changed.footprint.width_mm = 1100;
    for case in 0..7 {
        let mut r = request(&original, "key");
        let other = SessionToken::from_bytes([3; 32]);
        let identity = if case == 5 { &other } else { &token };
        match case {
            0 => {
                r.sku = "DEMO-COVERS";
                r.draft = &covers;
            }
            1 => r.expected_revision = 1,
            2 => r.bom_revision = 2,
            3 => r.draft = &changed,
            4 => r.reason = "different reason",
            5 => {}
            6 => r.effect_key = "fresh-stale",
            _ => unreachable!(),
        }
        assert!(
            matches!(
                record_specification(&pool, identity, &FixedClock(1100), r).await,
                Err(SpecificationWriteError::Conflict)
            ),
            "conflict {case}"
        );
    }
    sqlx::query("UPDATE staff_sessions SET revoked_at=1200 WHERE token_hash=$1")
        .bind(token.digest().as_slice())
        .execute(&pool)
        .await
        .expect("revoke");
    assert!(matches!(
        record_specification(&pool, &token, &FixedClock(1200), request(&original, "key")).await,
        Err(SpecificationWriteError::Staff(StaffError::Denied))
    ));
    assert_eq!(count(&pool).await, 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn concurrent_edits_and_exact_retries_have_one_effect(pool: PgPool) {
    seed(&pool).await;
    let candidate = draft(true);
    let token = SessionToken::from_bytes([1; 32]);
    let clock = FixedClock(1100);
    let (a, b) = tokio::join!(
        record_specification(&pool, &token, &clock, request(&candidate, "a")),
        record_specification(&pool, &token, &clock, request(&candidate, "b"))
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(
        matches!(a, Err(SpecificationWriteError::Conflict))
            || matches!(b, Err(SpecificationWriteError::Conflict))
    );
    let mut left = request(&candidate, "same");
    left.expected_revision = 1;
    let mut right = request(&candidate, "same");
    right.expected_revision = 1;
    let (a, b) = tokio::join!(
        record_specification(&pool, &token, &clock, left),
        record_specification(&pool, &token, &clock, right)
    );
    let a = a.expect("winner/replay");
    let b = b.expect("winner/replay");
    assert_eq!((a.revision, b.revision), (2, 2));
    assert_ne!(a.replayed, b.replayed);
    assert_eq!(count(&pool).await, 2);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn raw_schema_rewrites_gaps_unsealed_boms_and_malformed_payloads_fail_closed(pool: PgPool) {
    seed(&pool).await;
    let candidate = draft(true);
    let token = SessionToken::from_bytes([1; 32]);
    record_specification(
        &pool,
        &token,
        &FixedClock(1100),
        request(&candidate, "original"),
    )
    .await
    .expect("snapshot");
    pool.execute("INSERT INTO catalogue_boms(sku,revision) VALUES ('DEMO-FILLED',2)")
        .await
        .expect("unsealed BOM");
    for invalid in [
        "UPDATE catalogue_specification_revisions SET reason='rewrite'",
        "DELETE FROM catalogue_specification_revisions",
        "INSERT INTO catalogue_specification_revisions SELECT sku,3,bom_revision,'skip',payload,actor,reason,recorded_at FROM catalogue_specification_revisions WHERE revision=1",
        "INSERT INTO catalogue_specification_revisions SELECT sku,2,2,'unsealed',payload,actor,reason,recorded_at FROM catalogue_specification_revisions WHERE revision=1",
        "INSERT INTO catalogue_specification_revisions SELECT sku,2,1,'array','[]'::jsonb,actor,reason,recorded_at FROM catalogue_specification_revisions WHERE revision=1",
        "INSERT INTO catalogue_specification_revisions SELECT sku,2,1,'huge',jsonb_build_object('x',repeat('x',131073)),actor,reason,recorded_at FROM catalogue_specification_revisions WHERE revision=1",
    ] {
        assert!(
            pool.execute(invalid).await.is_err(),
            "accepted invalid SQL {invalid}"
        );
    }
    // A raw draft import is still untrusted. Guarded readers decode and revalidate.
    for (index, mut payload) in [
        serde_json::json!({"publish":true}),
        serde_json::to_value(&candidate).expect("fixture JSON"),
        serde_json::to_value(&candidate).expect("fixture JSON"),
    ]
    .into_iter()
    .enumerate()
    {
        if index == 1 {
            payload["footprint"]["width_mm"] = serde_json::json!(0);
        }
        if index == 2 {
            payload["components"][0]["component_code"] = serde_json::json!("DEMO-FOREIGN");
        }
        let revision = i32::try_from(index + 2).expect("bounded fixture");
        sqlx::query("INSERT INTO catalogue_specification_revisions (sku,revision,bom_revision,effect_key,payload,actor,reason) VALUES ('DEMO-FILLED',$1,1,$2,$3::jsonb,'fixture-catalogue','hostile isolated fixture')")
            .bind(revision).bind(format!("corrupt-{index}")).bind(payload.to_string()).execute(&pool).await.expect("raw corrupt draft");
        assert!(matches!(
            specification(&pool, &token, &FixedClock(1100), "DEMO-FILLED", revision).await,
            Err(SpecificationWriteError::InvalidStoredData)
        ));
    }
}

#[sqlx::test(migrations = false)]
async fn upgrading_price_schema_keeps_existing_facts_stock_and_empty_specification_history(
    pool: PgPool,
) {
    MIGRATOR.run_to(8, &pool).await.expect("prior schema");
    seed(&pool).await;
    pool.execute("INSERT INTO stock_balances VALUES ('DEMO-COVER',12,2,3)")
        .await
        .expect("old stock");
    commerce_infrastructure::catalogue_prices::record_price_revision(
        &pool,
        &SessionToken::from_bytes([1; 32]),
        &FixedClock(1100),
        commerce_infrastructure::catalogue_prices::PriceRevisionRequest {
            sku: "DEMO-FILLED",
            expected_revision: 0,
            effect_key: "prior-price",
            price: commerce_domain::Money::eur(14500).expect("oracle"),
            evidence_ref: "isolated pricing fixture",
            reason: "prior schema test",
            confirmed: true,
        },
    )
    .await
    .expect("prior price history");
    MIGRATOR.run(&pool).await.expect("specification expansion");
    let price: (i32, i64) = sqlx::query_as(
        "SELECT revision,minor_units FROM catalogue_price_revisions WHERE effect_key='prior-price'",
    )
    .fetch_one(&pool)
    .await
    .expect("old price oracle");
    assert_eq!(price, (1, 14500));
    assert_eq!(count(&pool).await, 0);
    let old: (i64, i64, i64) = sqlx::query_as(
        "SELECT on_hand,quarantine,reserved FROM stock_balances WHERE component_code='DEMO-COVER'",
    )
    .fetch_one(&pool)
    .await
    .expect("stock oracle");
    assert_eq!(old, (12, 2, 3));
    let candidate = draft(true);
    assert_eq!(
        record_specification(
            &pool,
            &SessionToken::from_bytes([1; 32]),
            &FixedClock(1100),
            request(&candidate, "upgraded")
        )
        .await
        .expect("new guard")
        .revision,
        1
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn session_expiry_and_clock_failure_roll_back_mutations_and_hide_private_reads(pool: PgPool) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct StepClock {
        calls: AtomicUsize,
        last: Result<u64, ContextError>,
    }
    impl Clock for StepClock {
        fn unix_seconds(&self) -> Result<u64, ContextError> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(1100)
            } else {
                self.last
            }
        }
    }
    seed(&pool).await;
    let token = SessionToken::from_bytes([1; 32]);
    let candidate = draft(true);
    for last in [Ok(2001), Err(ContextError::ClockUnavailable)] {
        let clock = StepClock {
            calls: AtomicUsize::new(0),
            last,
        };
        assert!(matches!(
            record_specification(&pool, &token, &clock, request(&candidate, "rollback")).await,
            Err(SpecificationWriteError::Staff(
                StaffError::Denied | StaffError::ClockUnavailable
            ))
        ));
        assert_eq!(count(&pool).await, 0);
    }
    record_specification(
        &pool,
        &token,
        &FixedClock(1100),
        request(&candidate, "first"),
    )
    .await
    .expect("seed");
    let clock = StepClock {
        calls: AtomicUsize::new(0),
        last: Ok(2001),
    };
    assert!(matches!(
        record_specification(&pool, &token, &clock, request(&candidate, "first")).await,
        Err(SpecificationWriteError::Staff(StaffError::Denied))
    ));
    for revision in [1, 99] {
        let clock = StepClock {
            calls: AtomicUsize::new(0),
            last: Err(ContextError::ClockUnavailable),
        };
        assert!(matches!(
            specification(&pool, &token, &clock, "DEMO-FILLED", revision).await,
            Err(SpecificationWriteError::Staff(StaffError::ClockUnavailable))
        ));
    }
    assert_eq!(count(&pool).await, 1);
}
async fn wait_for_lock(pool: &PgPool, pattern: &str) {
    tokio::time::timeout(std::time::Duration::from_millis(800),async {
        loop {let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE $1)")
            .bind(pattern).fetch_one(pool).await.expect("lock probe");if waiting {break;} tokio::task::yield_now().await;}
    }).await.expect("operation reached lock");
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn authority_locks_and_final_clock_checks_span_waiting_specification_writes(pool: PgPool) {
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };
    struct MovingClock(AtomicU64);
    impl Clock for MovingClock {
        fn unix_seconds(&self) -> Result<u64, ContextError> {
            Ok(self.0.load(Ordering::SeqCst))
        }
    }
    seed(&pool).await;
    for (block_sql, pattern) in [
        (
            "SELECT sku FROM catalogue_variants WHERE sku='DEMO-FILLED' FOR UPDATE",
            "SELECT sku FROM catalogue_variants%",
        ),
        (
            "SELECT pg_advisory_xact_lock(hashtextextended('catalogue-specification:wait',0))",
            "SELECT pg_advisory_xact_lock%",
        ),
    ] {
        let mut blocker = pool.begin().await.expect("blocker");
        blocker.execute(block_sql).await.expect("pause writer");
        let clock = Arc::new(MovingClock(AtomicU64::new(1100)));
        let writer_clock = clock.clone();
        let writer_pool = pool.clone();
        let writer = tokio::spawn(async move {
            let candidate = draft(true);
            record_specification(
                &writer_pool,
                &SessionToken::from_bytes([1; 32]),
                writer_clock.as_ref(),
                request(&candidate, "wait"),
            )
            .await
        });
        wait_for_lock(&pool, pattern).await;
        clock.0.store(2001, Ordering::SeqCst);
        blocker.rollback().await.expect("resume after expiry");
        assert!(matches!(
            writer.await.expect("task"),
            Err(SpecificationWriteError::Staff(StaffError::Denied))
        ));
        assert_eq!(count(&pool).await, 0);
    }
    let mut blocker = pool.begin().await.expect("SKU blocker");
    blocker
        .execute("SELECT sku FROM catalogue_variants WHERE sku='DEMO-FILLED' FOR UPDATE")
        .await
        .expect("pause");
    let writer_pool = pool.clone();
    let writer = tokio::spawn(async move {
        let candidate = draft(true);
        record_specification(
            &writer_pool,
            &SessionToken::from_bytes([1; 32]),
            &FixedClock(1100),
            request(&candidate, "before-revoke"),
        )
        .await
    });
    wait_for_lock(&pool, "SELECT sku FROM catalogue_variants%").await;
    let revoke_pool = pool.clone();
    let revoke = tokio::spawn(async move {
        sqlx::query("UPDATE staff_accounts SET enabled=false WHERE id='fixture-catalogue'")
            .execute(&revoke_pool)
            .await
    });
    wait_for_lock(&pool, "UPDATE staff_accounts%").await;
    blocker.rollback().await.expect("allow commit");
    assert_eq!(
        writer
            .await
            .expect("task")
            .expect("authorized before disabling")
            .revision,
        1
    );
    revoke
        .await
        .expect("disable task")
        .expect("disabled after commit");
    assert!(matches!(
        specification(
            &pool,
            &SessionToken::from_bytes([1; 32]),
            &FixedClock(1100),
            "DEMO-FILLED",
            1
        )
        .await,
        Err(SpecificationWriteError::Staff(StaffError::Denied))
    ));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn database_faults_and_stale_imports_leave_no_partial_specification_effects(pool: PgPool) {
    seed(&pool).await;
    let token = SessionToken::from_bytes([1; 32]);
    let candidate = draft(true);
    let mut blocker = pool.begin().await.expect("blocker");
    blocker
        .execute(
            "SELECT pg_advisory_xact_lock(hashtextextended('catalogue-specification:locked',0))",
        )
        .await
        .expect("effect lock");
    let result = record_specification(
        &pool,
        &token,
        &FixedClock(1100),
        request(&candidate, "locked"),
    )
    .await;
    match result {
        Err(SpecificationWriteError::Database(e)) => assert_eq!(
            e.as_database_error().and_then(|e| e.code()).as_deref(),
            Some("55P03")
        ),
        other => panic!("unexpected {other:?}"),
    }
    blocker.rollback().await.expect("release");
    assert_eq!(count(&pool).await, 0);
    let mut stale = pool.begin().await.expect("stale import");
    stale
        .execute("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .await
        .expect("RR");
    let old: i64 = sqlx::query_scalar("SELECT count(*) FROM catalogue_specification_revisions")
        .fetch_one(&mut *stale)
        .await
        .expect("old snapshot");
    assert_eq!(old, 0);
    record_specification(
        &pool,
        &token,
        &FixedClock(1100),
        request(&candidate, "intervening"),
    )
    .await
    .expect("writer commits");
    let error=sqlx::query("INSERT INTO catalogue_specification_revisions (sku,revision,bom_revision,effect_key,payload,actor,reason) VALUES ('DEMO-FILLED',2,1,'stale',$1::jsonb,'fixture-catalogue','stale import fixture')")
        .bind(serde_json::to_string(&candidate).expect("JSON")).execute(&mut *stale).await.expect_err("stale transaction must retry");
    assert_eq!(
        error.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("40001")
    );
    stale.rollback().await.expect("rollback");
    assert_eq!(count(&pool).await, 1);
    pool.execute(
        "ALTER TABLE catalogue_specification_revisions RENAME TO unavailable_specifications",
    )
    .await
    .expect("missing storage");
    assert!(matches!(
        record_specification(
            &pool,
            &token,
            &FixedClock(1100),
            request(&candidate, "missing-storage")
        )
        .await,
        Err(SpecificationWriteError::Database(_))
    ));
    assert!(matches!(
        specification(&pool, &token, &FixedClock(1100), "DEMO-FILLED", 1).await,
        Err(SpecificationWriteError::Database(_))
    ));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn logically_valid_large_component_payload_is_rejected_before_persistence(pool: PgPool) {
    seed(&pool).await;
    pool.execute("INSERT INTO physical_components(code,kind) SELECT 'DEMO-SPEC-'||lpad(n::text,2,'0'),'cover' FROM generate_series(0,63)n;
        INSERT INTO catalogue_boms(sku,revision) VALUES ('DEMO-COVERS',2);
        INSERT INTO catalogue_bom_lines SELECT 'DEMO-COVERS',2,code,1 FROM physical_components WHERE code LIKE 'DEMO-SPEC-%';
        UPDATE catalogue_boms SET sealed=true WHERE sku='DEMO-COVERS' AND revision=2").await.expect("bounded fanout fixture");
    let mut candidate = draft(false);
    let template = candidate.components[0].clone();
    candidate.components.clear();
    for i in 0..64 {
        let mut component = template.clone();
        component.component_code = format!("DEMO-SPEC-{i:02}");
        component.source_ref = "s".repeat(512);
        component.traceability_ref = "t".repeat(512);
        component.material = MaterialSpecification::Cover {
            fibres: (0..16)
                .map(|n| FibrePart {
                    name: format!("fibre-{n:02}-{}", "n".repeat(70)),
                    basis_points: 625,
                })
                .collect(),
            care_evidence_ref: "c".repeat(512),
        };
        candidate.components.push(component);
    }
    let bom = commerce_infrastructure::bom::bill_of_materials(&pool, "DEMO-COVERS", 2)
        .await
        .expect("read")
        .expect("sealed");
    assert_eq!(candidate.validate(&bom), Ok(()));
    assert!(serde_json::to_string(&candidate).expect("JSON").len() > 65536);
    let mut r = request(&candidate, "too-large");
    r.sku = "DEMO-COVERS";
    r.bom_revision = 2;
    assert!(matches!(
        record_specification(
            &pool,
            &SessionToken::from_bytes([1; 32]),
            &FixedClock(1100),
            r
        )
        .await,
        Err(SpecificationWriteError::InvalidRequest)
    ));
    assert_eq!(count(&pool).await, 0);
}
