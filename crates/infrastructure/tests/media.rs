use commerce_application::request_context::{Clock, ContextError};
use commerce_infrastructure::{
    MIGRATOR,
    media::{
        MediaError, MediaRightsRequest, RegisterMediaRequest, inspect_media, record_media_rights,
        register_media,
    },
    staff_access::{SessionToken, StaffError},
};
use sqlx::{Executor, PgPool};
include!("../../../tests/fixtures/media.rs");
struct FixedClock(u64);
impl Clock for FixedClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(self.0)
    }
}
fn registration<'a>(draft: &'a MediaAssetDraft, key: &'a str) -> RegisterMediaRequest<'a> {
    RegisterMediaRequest {
        asset_id: "photo",
        effect_key: key,
        draft,
        reason: "isolated media test",
        confirmed: true,
    }
}
fn rights<'a>(change: &'a MediaRightsChange, key: &'a str) -> MediaRightsRequest<'a> {
    MediaRightsRequest {
        asset_id: "photo",
        expected_revision: 0,
        effect_key: key,
        change,
        reason: "isolated media test",
        confirmed: true,
    }
}
async fn seed(pool: &PgPool) {
    pool.execute("INSERT INTO staff_accounts VALUES ('fixture-catalogue',true),('fixture-warehouse',true),('fixture-other',true); INSERT INTO staff_capabilities VALUES ('fixture-catalogue','catalogue.manage'),('fixture-warehouse','warehouse.manage'),('fixture-other','catalogue.manage')").await.expect("fixture grants");
    for (number, actor) in [
        (1, "fixture-catalogue"),
        (2, "fixture-warehouse"),
        (3, "fixture-other"),
    ] {
        sqlx::query("INSERT INTO staff_sessions (token_hash,staff_id,created_at,expires_at,mfa_verified_at) VALUES ($1,$2,1000,4600,1100)")
            .bind(SessionToken::from_bytes([number;32]).digest().as_slice()).bind(actor).execute(pool).await.expect("test-only attestation");
    }
}
async fn counts(pool: &PgPool) -> (i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM catalogue_media_assets),(SELECT count(*) FROM catalogue_media_rights),(SELECT count(*) FROM catalogue_media_serialization)").fetch_one(pool).await.expect("counts")
}
async fn registered(pool: &PgPool) {
    seed(pool).await;
    register_media(
        pool,
        &SessionToken::from_bytes([1; 32]),
        &FixedClock(1100),
        registration(&photo(), "register"),
    )
    .await
    .expect("registered draft metadata");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn metadata_rights_expiry_revoke_and_exact_replay_preserve_audit_history(pool: PgPool) {
    registered(&pool).await;
    let token = SessionToken::from_bytes([1; 32]);
    let empty = inspect_media(&pool, &token, &FixedClock(1100), "photo")
        .await
        .expect("read")
        .expect("draft");
    assert_eq!(empty.asset, photo());
    assert_eq!(empty.rights_revision, None);
    assert!(!empty.product_rights_active);
    assert!(!empty.requires_generated_label);
    let r = record_media_rights(&pool, &token, &FixedClock(1100), rights(&grant(), "grant"))
        .await
        .expect("grant assertion");
    assert_eq!((r.revision, r.replayed), (1, false));
    for (now, active) in [(1100, true), (1299, true), (1300, false)] {
        let decision = inspect_media(&pool, &token, &FixedClock(now), "photo")
            .await
            .expect("rights")
            .expect("asset");
        assert_eq!(decision.product_rights_active, active);
        assert_eq!(decision.evaluated_at, i64::try_from(now).expect("oracle"));
    }
    let mut revoke = rights(&MediaRightsChange::Revoke, "revoke");
    revoke.expected_revision = 1;
    assert_eq!(
        record_media_rights(&pool, &token, &FixedClock(1200), revoke)
            .await
            .expect("revoke")
            .revision,
        2
    );
    let old = record_media_rights(&pool, &token, &FixedClock(1200), rights(&grant(), "grant"))
        .await
        .expect("old receipt");
    assert_eq!((old.revision, old.replayed), (1, true));
    let decision = inspect_media(&pool, &token, &FixedClock(1200), "photo")
        .await
        .expect("read")
        .expect("asset");
    assert_eq!(decision.rights_revision, Some(2));
    assert!(!decision.product_rights_active);
    let perpetual = MediaRightsChange::Grant {
        use_scope: MediaUse::ProductPhotography,
        valid_from: 1100,
        term: RightsTerm::Perpetual,
        permission_ref: "fixture perpetual assertion".into(),
    };
    let mut regrant = rights(&perpetual, "regrant");
    regrant.expected_revision = 2;
    assert_eq!(
        record_media_rights(&pool, &token, &FixedClock(1200), regrant)
            .await
            .expect("explicit regrant")
            .revision,
        3
    );
    let history:Vec<(i32,String,String,String,bool)>=sqlx::query_as("SELECT revision,change->>'action',actor,reason,recorded_at IS NOT NULL FROM catalogue_media_rights ORDER BY revision").fetch_all(&pool).await.expect("audit oracle");
    assert_eq!(
        history,
        vec![
            (
                1,
                "grant".into(),
                "fixture-catalogue".into(),
                "isolated media test".into(),
                true
            ),
            (
                2,
                "revoke".into(),
                "fixture-catalogue".into(),
                "isolated media test".into(),
                true
            ),
            (
                3,
                "grant".into(),
                "fixture-catalogue".into(),
                "isolated media test".into(),
                true
            )
        ]
    );
    assert_eq!(counts(&pool).await, (1, 3, 1));
    assert_eq!(
        inspect_media(&pool, &token, &FixedClock(1100), "missing")
            .await
            .expect("missing"),
        None
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn role_confirmation_bounds_and_generated_or_fixture_product_rights_fail_closed(
    pool: PgPool,
) {
    seed(&pool).await;
    let draft = photo();
    let change = grant();
    let token = SessionToken::from_bytes([1; 32]);
    for number in [2, 99] {
        let wrong = SessionToken::from_bytes([number; 32]);
        assert!(matches!(
            register_media(
                &pool,
                &wrong,
                &FixedClock(1100),
                registration(&draft, "denied")
            )
            .await,
            Err(MediaError::Staff(StaffError::Denied))
        ));
        assert!(matches!(
            record_media_rights(&pool, &wrong, &FixedClock(1100), rights(&change, "denied")).await,
            Err(MediaError::Staff(StaffError::Denied))
        ));
        assert!(matches!(
            inspect_media(&pool, &wrong, &FixedClock(1100), "photo").await,
            Err(MediaError::Staff(StaffError::Denied))
        ));
    }
    for case in 0..7 {
        let mut r = registration(&draft, "invalid");
        let long = "x".repeat(129);
        let reason = "x".repeat(501);
        match case {
            0 => r.confirmed = false,
            1 => r.asset_id = "../",
            2 => r.asset_id = &long,
            3 => r.effect_key = "",
            4 => r.effect_key = "bad_key",
            5 => r.reason = " ",
            6 => r.reason = &reason,
            _ => unreachable!(),
        }
        assert!(matches!(
            register_media(&pool, &token, &FixedClock(1100), r).await,
            Err(MediaError::InvalidRequest)
        ));
    }
    assert!(matches!(
        inspect_media(&pool, &token, &FixedClock(1100), "bad/path").await,
        Err(MediaError::InvalidRequest)
    ));
    let mut bad = draft.clone();
    bad.width_px = 0;
    assert!(matches!(
        register_media(
            &pool,
            &token,
            &FixedClock(1100),
            registration(&bad, "bad-metadata")
        )
        .await,
        Err(MediaError::InvalidRequest)
    ));
    for revision in [-1, i32::MAX] {
        let mut r = rights(&change, "invalid");
        r.expected_revision = revision;
        assert!(matches!(
            record_media_rights(&pool, &token, &FixedClock(1100), r).await,
            Err(MediaError::InvalidRequest)
        ));
    }
    let mut r = rights(&change, "unconfirmed");
    r.confirmed = false;
    assert!(matches!(
        record_media_rights(&pool, &token, &FixedClock(1100), r).await,
        Err(MediaError::InvalidRequest)
    ));
    assert!(matches!(
        record_media_rights(&pool, &token, &FixedClock(1100), rights(&change, "missing")).await,
        Err(MediaError::NotFound)
    ));
    for (id, kind, fixture) in [
        ("generated", MediaKind::GeneratedInspiration, false),
        ("fixture", MediaKind::OriginalPhoto, true),
    ] {
        let mut draft = draft.clone();
        draft.kind = kind;
        draft.fixture = fixture;
        draft.sha256 = [if fixture { 0x66 } else { 0x55 }; 32];
        let mut r = registration(&draft, id);
        r.asset_id = id;
        register_media(&pool, &token, &FixedClock(1100), r)
            .await
            .expect("non-product metadata");
        let mut r = rights(&change, id);
        r.asset_id = id;
        assert!(matches!(
            record_media_rights(&pool, &token, &FixedClock(1100), r).await,
            Err(MediaError::InvalidRequest)
        ));
        let inspiration = MediaRightsChange::Grant {
            use_scope: MediaUse::Inspiration,
            valid_from: 1100,
            term: RightsTerm::Perpetual,
            permission_ref: "fixture inspiration assertion".into(),
        };
        let mut r = rights(&inspiration, id);
        r.asset_id = id;
        record_media_rights(&pool, &token, &FixedClock(1100), r)
            .await
            .expect("inspiration scope");
        let status = inspect_media(&pool, &token, &FixedClock(1100), id)
            .await
            .expect("read")
            .expect("asset");
        assert!(!status.product_rights_active);
        assert_eq!(
            status.requires_generated_label,
            kind == MediaKind::GeneratedInspiration
        );
        let mut relabelled = photo();
        relabelled.sha256 = draft.sha256;
        let alias = format!("{id}-relabelled");
        let mut alias_request = registration(&relabelled, &alias);
        alias_request.asset_id = &alias;
        assert!(
            matches!(
                register_media(&pool, &token, &FixedClock(1100), alias_request).await,
                Err(MediaError::Conflict)
            ),
            "a known content digest cannot be reclassified under a new ID"
        );
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn registration_and_rights_effect_keys_bind_identity_payload_actor_and_reason(pool: PgPool) {
    registered(&pool).await;
    let token = SessionToken::from_bytes([1; 32]);
    let draft = photo();
    assert!(
        register_media(
            &pool,
            &token,
            &FixedClock(1100),
            registration(&draft, "register")
        )
        .await
        .expect("exact retry")
        .replayed
    );
    let mut changed = draft.clone();
    changed.width_px = 1201;
    for case in 0..5 {
        let mut r = registration(&draft, "register");
        let other = SessionToken::from_bytes([3; 32]);
        let actor = if case == 3 { &other } else { &token };
        match case {
            0 => r.asset_id = "other",
            1 => r.draft = &changed,
            2 => r.reason = "changed",
            3 => {}
            4 => r.effect_key = "fresh-key",
            _ => unreachable!(),
        }
        assert!(
            matches!(
                register_media(&pool, actor, &FixedClock(1100), r).await,
                Err(MediaError::Conflict)
            ),
            "registration case {case}"
        );
    }
    let mut second_draft = draft.clone();
    second_draft.sha256 = [0xcd; 32];
    let mut other = registration(&second_draft, "register-other");
    other.asset_id = "other";
    register_media(&pool, &token, &FixedClock(1100), other)
        .await
        .expect("other asset");
    let original = grant();
    record_media_rights(&pool, &token, &FixedClock(1100), rights(&original, "grant"))
        .await
        .expect("rights");
    let changed = MediaRightsChange::Revoke;
    for case in 0..6 {
        let mut r = rights(&original, "grant");
        let other = SessionToken::from_bytes([3; 32]);
        let actor = if case == 4 { &other } else { &token };
        match case {
            0 => r.asset_id = "other",
            1 => r.expected_revision = 1,
            2 => r.change = &changed,
            3 => r.reason = "changed",
            4 => {}
            5 => r.effect_key = "stale-edit",
            _ => unreachable!(),
        }
        assert!(
            matches!(
                record_media_rights(&pool, actor, &FixedClock(1100), r).await,
                Err(MediaError::Conflict)
            ),
            "rights case {case}"
        );
    }
    sqlx::query("UPDATE staff_sessions SET revoked_at=1200 WHERE token_hash=$1")
        .bind(token.digest().as_slice())
        .execute(&pool)
        .await
        .expect("revoke authority");
    assert!(matches!(
        register_media(
            &pool,
            &token,
            &FixedClock(1200),
            registration(&draft, "register")
        )
        .await,
        Err(MediaError::Staff(StaffError::Denied))
    ));
    assert!(matches!(
        record_media_rights(&pool, &token, &FixedClock(1200), rights(&original, "grant")).await,
        Err(MediaError::Staff(StaffError::Denied))
    ));
    assert_eq!(counts(&pool).await, (2, 1, 2));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn competing_creates_edits_and_exact_retries_have_one_effect(pool: PgPool) {
    seed(&pool).await;
    let token = SessionToken::from_bytes([1; 32]);
    let clock = FixedClock(1100);
    let draft = photo();
    let change = grant();
    let (a, b) = tokio::join!(
        register_media(&pool, &token, &clock, registration(&draft, "a")),
        register_media(&pool, &token, &clock, registration(&draft, "b"))
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(matches!(a, Err(MediaError::Conflict)) || matches!(b, Err(MediaError::Conflict)));
    let mut second_draft = draft.clone();
    second_draft.sha256 = [0xcd; 32];
    let mut left = registration(&second_draft, "same-asset");
    left.asset_id = "second";
    let mut right = registration(&second_draft, "same-asset");
    right.asset_id = "second";
    let (a, b) = tokio::join!(
        register_media(&pool, &token, &clock, left),
        register_media(&pool, &token, &clock, right)
    );
    assert_ne!(
        a.expect("create or retry").replayed,
        b.expect("create or retry").replayed
    );
    let (a, b) = tokio::join!(
        record_media_rights(&pool, &token, &clock, rights(&change, "left")),
        record_media_rights(&pool, &token, &clock, rights(&change, "right"))
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(matches!(a, Err(MediaError::Conflict)) || matches!(b, Err(MediaError::Conflict)));
    let mut left = rights(&change, "same");
    left.expected_revision = 1;
    let mut right = rights(&change, "same");
    right.expected_revision = 1;
    let (a, b) = tokio::join!(
        record_media_rights(&pool, &token, &clock, left),
        record_media_rights(&pool, &token, &clock, right)
    );
    let a = a.expect("write/retry");
    let b = b.expect("write/retry");
    assert_eq!((a.revision, b.revision), (2, 2));
    assert_ne!(a.replayed, b.replayed);
    assert_eq!(counts(&pool).await, (2, 2, 2));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn immutable_history_database_product_rule_and_corrupt_imports_fail_closed(pool: PgPool) {
    registered(&pool).await;
    let token = SessionToken::from_bytes([1; 32]);
    let change = grant();
    record_media_rights(&pool, &token, &FixedClock(1100), rights(&change, "grant"))
        .await
        .expect("rights");
    for invalid in [
        "UPDATE catalogue_media_assets SET reason='rewrite'",
        "DELETE FROM catalogue_media_assets",
        "DELETE FROM catalogue_media_serialization",
        "UPDATE catalogue_media_rights SET reason='rewrite'",
        "DELETE FROM catalogue_media_rights",
        "INSERT INTO catalogue_media_rights SELECT asset_id,3,'skip',change,actor,reason,recorded_at FROM catalogue_media_rights WHERE revision=1",
    ] {
        assert!(pool.execute(invalid).await.is_err(), "accepted {invalid}");
    }
    let mut generated = photo();
    generated.kind = MediaKind::GeneratedInspiration;
    generated.sha256 = [0x55; 32];
    let mut r = registration(&generated, "generated");
    r.asset_id = "generated";
    register_media(&pool, &token, &FixedClock(1100), r)
        .await
        .expect("generated metadata");
    let error=sqlx::query("INSERT INTO catalogue_media_rights(asset_id,revision,effect_key,change,actor,reason) VALUES('generated',1,'raw-product',$1::jsonb,'fixture-catalogue','raw fixture')")
        .bind(serde_json::to_string(&change).expect("JSON")).execute(&pool).await.expect_err("database product rule");
    assert_eq!(
        error.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("23514")
    );
    // Malformed or logically invalid direct imports cannot produce a rights decision.
    for (id, payload) in [
        ("bad-shape", serde_json::json!({"publish":true})),
        ("bad-value", {
            let mut value = photo();
            value.width_px = 0;
            value.sha256 = [0x77; 32];
            serde_json::to_value(value).expect("JSON")
        }),
    ] {
        sqlx::query("INSERT INTO catalogue_media_assets(asset_id,effect_key,payload,actor,reason) VALUES($1,$1,$2::jsonb,'fixture-catalogue','hostile fixture')").bind(id).bind(payload.to_string()).execute(&pool).await.expect("raw draft");
        assert!(matches!(
            inspect_media(&pool, &token, &FixedClock(1100), id).await,
            Err(MediaError::InvalidStoredData)
        ));
    }
    for (revision, payload) in [
        (2, serde_json::json!({"action":"unknown"})),
        (
            3,
            serde_json::json!({"action":"grant","use_scope":"product-photography","valid_from":1100,"term":{"kind":"until","expires_at":1000},"permission_ref":"fixture"}),
        ),
    ] {
        sqlx::query("INSERT INTO catalogue_media_rights(asset_id,revision,effect_key,change,actor,reason) VALUES('photo',$1,$2,$3::jsonb,'fixture-catalogue','hostile fixture')")
            .bind(revision).bind(format!("corrupt-{revision}")).bind(payload.to_string()).execute(&pool).await.expect("raw corrupt rights");
        assert!(matches!(
            inspect_media(&pool, &token, &FixedClock(1100), "photo").await,
            Err(MediaError::InvalidStoredData)
        ));
    }
    pool.execute("ALTER TABLE catalogue_media_assets DROP CONSTRAINT media_serialization_required; DELETE FROM catalogue_media_serialization WHERE asset_id='generated'").await.expect("hostile missing control");
    assert!(matches!(
        inspect_media(&pool, &token, &FixedClock(1100), "generated").await,
        Err(MediaError::InvalidStoredData)
    ));
    let mut r = rights(&MediaRightsChange::Revoke, "missing-control");
    r.asset_id = "generated";
    assert!(matches!(
        record_media_rights(&pool, &token, &FixedClock(1100), r).await,
        Err(MediaError::InvalidStoredData)
    ));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn authority_expiry_and_clock_failure_roll_back_writes_and_hide_private_receipts(
    pool: PgPool,
) {
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
    let draft = photo();
    let change = grant();
    for last in [Ok(2001), Err(ContextError::ClockUnavailable)] {
        let clock = StepClock {
            calls: AtomicUsize::new(0),
            last,
        };
        assert!(matches!(
            register_media(&pool, &token, &clock, registration(&draft, "rollback")).await,
            Err(MediaError::Staff(
                StaffError::Denied | StaffError::ClockUnavailable
            ))
        ));
        assert_eq!(counts(&pool).await, (0, 0, 0));
    }
    register_media(
        &pool,
        &token,
        &FixedClock(1100),
        registration(&draft, "registered"),
    )
    .await
    .expect("metadata");
    let clock = StepClock {
        calls: AtomicUsize::new(0),
        last: Ok(2001),
    };
    assert!(matches!(
        register_media(&pool, &token, &clock, registration(&draft, "registered")).await,
        Err(MediaError::Staff(StaffError::Denied))
    ));
    for last in [Ok(2001), Err(ContextError::ClockUnavailable)] {
        let clock = StepClock {
            calls: AtomicUsize::new(0),
            last,
        };
        assert!(matches!(
            record_media_rights(&pool, &token, &clock, rights(&change, "rollback")).await,
            Err(MediaError::Staff(
                StaffError::Denied | StaffError::ClockUnavailable
            ))
        ));
        assert_eq!(counts(&pool).await, (1, 0, 1));
    }
    record_media_rights(&pool, &token, &FixedClock(1100), rights(&change, "grant"))
        .await
        .expect("grant");
    let clock = StepClock {
        calls: AtomicUsize::new(0),
        last: Ok(2001),
    };
    assert!(matches!(
        record_media_rights(&pool, &token, &clock, rights(&change, "grant")).await,
        Err(MediaError::Staff(StaffError::Denied))
    ));
    for id in ["photo", "missing"] {
        let clock = StepClock {
            calls: AtomicUsize::new(0),
            last: Err(ContextError::ClockUnavailable),
        };
        assert!(matches!(
            inspect_media(&pool, &token, &clock, id).await,
            Err(MediaError::Staff(StaffError::ClockUnavailable))
        ));
    }
    assert_eq!(counts(&pool).await, (1, 1, 1));
}
async fn wait_for_lock(pool: &PgPool, pattern: &str) {
    tokio::time::timeout(std::time::Duration::from_millis(800),async {loop {
        let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE $1)")
            .bind(pattern).fetch_one(pool).await.expect("lock probe");if waiting {break;}tokio::task::yield_now().await;
    }}).await.expect("operation reached database lock");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn waiting_inspection_uses_fresh_licence_time_and_revocation_is_observed(pool: PgPool) {
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
    registered(&pool).await;
    let token = SessionToken::from_bytes([1; 32]);
    record_media_rights(&pool, &token, &FixedClock(1100), rights(&grant(), "grant"))
        .await
        .expect("grant");
    for expired in [false, true] {
        let mut blocker = pool.begin().await.expect("rights blocker");
        blocker.execute("SELECT asset_id FROM catalogue_media_serialization WHERE asset_id='photo' FOR UPDATE").await.expect("hold rights claim");
        if !expired {
            blocker.execute("INSERT INTO catalogue_media_rights(asset_id,revision,effect_key,change,actor,reason) VALUES('photo',2,'revoke','{\"action\":\"revoke\"}','fixture-catalogue','isolated pending revoke')").await.expect("uncommitted revoke");
        }
        let clock = Arc::new(MovingClock(AtomicU64::new(1200)));
        let read_clock = clock.clone();
        let read_pool = pool.clone();
        let read = tokio::spawn(async move {
            inspect_media(
                &read_pool,
                &SessionToken::from_bytes([1; 32]),
                read_clock.as_ref(),
                "photo",
            )
            .await
        });
        wait_for_lock(
            &pool,
            "SELECT asset_id FROM catalogue_media_serialization%FOR SHARE",
        )
        .await;
        if expired {
            clock.0.store(1300, Ordering::SeqCst);
            blocker
                .rollback()
                .await
                .expect("licence expired during wait");
        } else {
            blocker.commit().await.expect("revoke commits before read");
        }
        let decision = read
            .await
            .expect("reader")
            .expect("authorized")
            .expect("asset");
        assert!(!decision.product_rights_active);
        if expired {
            assert_eq!(decision.evaluated_at, 1300);
        } else {
            assert_eq!(decision.rights_revision, Some(2));
            let restored_grant = grant();
            let mut restore = rights(&restored_grant, "restore");
            restore.expected_revision = 2;
            record_media_rights(&pool, &token, &FixedClock(1200), restore)
                .await
                .expect("restore for expiry case");
        }
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn authority_revocation_waits_for_pending_media_effect_and_blocks_later_replay(pool: PgPool) {
    registered(&pool).await;
    let mut blocker = pool.begin().await.expect("claim blocker");
    blocker
        .execute(
            "SELECT asset_id FROM catalogue_media_serialization WHERE asset_id='photo' FOR UPDATE",
        )
        .await
        .expect("pause rights operation");
    let writer_pool = pool.clone();
    let writer = tokio::spawn(async move {
        record_media_rights(
            &writer_pool,
            &SessionToken::from_bytes([1; 32]),
            &FixedClock(1100),
            rights(&grant(), "before-revoke"),
        )
        .await
    });
    wait_for_lock(
        &pool,
        "SELECT asset_id FROM catalogue_media_serialization%FOR UPDATE",
    )
    .await;
    let revoke_pool = pool.clone();
    let revoke = tokio::spawn(async move {
        sqlx::query("DELETE FROM staff_capabilities WHERE staff_id='fixture-catalogue'")
            .execute(&revoke_pool)
            .await
    });
    wait_for_lock(&pool, "DELETE FROM staff_capabilities%").await;
    blocker.rollback().await.expect("allow rights commit");
    assert_eq!(
        writer
            .await
            .expect("write task")
            .expect("authorized before revoke")
            .revision,
        1
    );
    revoke
        .await
        .expect("revoke task")
        .expect("grant removed after commit");
    assert!(matches!(
        record_media_rights(
            &pool,
            &SessionToken::from_bytes([1; 32]),
            &FixedClock(1100),
            rights(&grant(), "before-revoke")
        )
        .await,
        Err(MediaError::Staff(StaffError::Denied))
    ));
    assert_eq!(counts(&pool).await, (1, 1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn bounded_dependency_failures_and_stale_repeatable_read_imports_have_no_partial_effect(
    pool: PgPool,
) {
    registered(&pool).await;
    let token = SessionToken::from_bytes([1; 32]);
    let mut blocker = pool.begin().await.expect("blocker");
    blocker
        .execute("SELECT pg_advisory_xact_lock(hashtextextended('media-register:locked',0))")
        .await
        .expect("lock operation");
    let result = register_media(
        &pool,
        &token,
        &FixedClock(1100),
        registration(&photo(), "locked"),
    )
    .await;
    match result {
        Err(MediaError::Database(e)) => assert_eq!(
            e.as_database_error().and_then(|e| e.code()).as_deref(),
            Some("55P03")
        ),
        other => panic!("unexpected {other:?}"),
    }
    blocker.rollback().await.expect("unlock");
    assert_eq!(counts(&pool).await, (1, 0, 1));
    let mut stale = pool.begin().await.expect("stale importer");
    stale
        .execute("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .await
        .expect("RR");
    let old: i64 = sqlx::query_scalar("SELECT count(*) FROM catalogue_media_rights")
        .fetch_one(&mut *stale)
        .await
        .expect("old snapshot");
    assert_eq!(old, 0);
    record_media_rights(
        &pool,
        &token,
        &FixedClock(1100),
        rights(&grant(), "intervening"),
    )
    .await
    .expect("new rights");
    let error=sqlx::query("INSERT INTO catalogue_media_rights(asset_id,revision,effect_key,change,actor,reason) VALUES('photo',2,'stale',$1::jsonb,'fixture-catalogue','stale isolated fixture')")
        .bind(serde_json::to_string(&grant()).expect("JSON")).execute(&mut *stale).await.expect_err("stale import retries");
    assert_eq!(
        error.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("40001")
    );
    stale.rollback().await.expect("rollback");
    pool.execute("ALTER TABLE catalogue_media_rights RENAME TO unavailable_media_rights")
        .await
        .expect("missing dependency");
    assert!(matches!(
        inspect_media(&pool, &token, &FixedClock(1100), "photo").await,
        Err(MediaError::Database(_))
    ));
    assert!(matches!(
        record_media_rights(
            &pool,
            &token,
            &FixedClock(1100),
            rights(&grant(), "unavailable")
        )
        .await,
        Err(MediaError::Database(_))
    ));
}

#[sqlx::test(migrations = false)]
async fn schema_upgrade_preserves_specification_and_price_state_without_seeding_media(
    pool: PgPool,
) {
    MIGRATOR.run_to(9, &pool).await.expect("prior schema");
    seed(&pool).await;
    pool.execute(include_str!("../../../tests/fixtures/bom.sql"))
        .await
        .expect("old catalogue");
    pool.execute("INSERT INTO catalogue_price_revisions(sku,revision,effect_key,minor_units,evidence_ref,actor,reason) VALUES('DEMO-FILLED',1,'prior',14500,'fixture','fixture-catalogue','prior isolated fixture')").await.expect("old price");
    let old_specification = prior_specification::snapshot();
    commerce_infrastructure::specifications::record_specification(
        &pool,
        &SessionToken::from_bytes([1; 32]),
        &FixedClock(1100),
        commerce_infrastructure::specifications::SpecificationRequest {
            sku: "DEMO-FILLED",
            bom_revision: 1,
            expected_revision: 0,
            effect_key: "prior-specification",
            draft: &old_specification,
            reason: "prior isolated specification",
            confirmed: true,
        },
    )
    .await
    .expect("old specification");
    MIGRATOR.run(&pool).await.expect("expand media schema");
    let preserved = commerce_infrastructure::specifications::specification(
        &pool,
        &SessionToken::from_bytes([1; 32]),
        &FixedClock(1100),
        "DEMO-FILLED",
        1,
    )
    .await
    .expect("old reader")
    .expect("old snapshot");
    assert_eq!(preserved.draft, old_specification);
    assert_eq!(counts(&pool).await, (0, 0, 0));
    let price: i64 = sqlx::query_scalar(
        "SELECT minor_units FROM catalogue_price_revisions WHERE effect_key='prior'",
    )
    .fetch_one(&pool)
    .await
    .expect("price oracle");
    assert_eq!(price, 14500);
    register_media(
        &pool,
        &SessionToken::from_bytes([1; 32]),
        &FixedClock(1100),
        registration(&photo(), "upgraded"),
    )
    .await
    .expect("new operation");
}

mod prior_specification {
    include!("../../../tests/fixtures/specification.rs");
    pub(super) fn snapshot() -> SpecificationDraft {
        draft(true)
    }
}
