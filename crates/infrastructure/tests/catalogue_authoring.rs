use commerce_application::{
    Locale,
    request_context::{Clock, ContextError},
};
use commerce_domain::{
    ComponentKind,
    catalogue_authoring::{BomLineDraft, CatalogueCommand, Fill},
};
use commerce_infrastructure::{
    MIGRATOR,
    catalogue_authoring::*,
    staff_access::{SessionToken, StaffError},
};
use sqlx::{Executor, PgPool};

#[sqlx::test(migrator = "MIGRATOR")]
async fn stale_repeatable_read_bom_imports_cannot_skip_the_current_revision(pool: PgPool) {
    seed(&pool).await;
    graph(&pool).await;
    record(&pool, &variant("SET-FILLED", Fill::WithFoam), "filled").await;
    let mut stale = pool.begin().await.expect("stale importer");
    stale
        .execute("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .await
        .expect("repeatable read");
    let revision: i32 =
        sqlx::query_scalar("SELECT max(revision) FROM catalogue_boms WHERE sku='SET-FILLED'")
            .fetch_one(&mut *stale)
            .await
            .expect("old snapshot");
    assert_eq!(revision, 1);
    record(
        &pool,
        &CatalogueCommand::Bom {
            sku: "SET-FILLED".into(),
            expected_revision: 1,
            lines: vec![line("SEAT-COVER", 3), line("SEAT-FOAM", 3)],
        },
        "current-bom2",
    )
    .await;
    let body = CatalogueCommand::Bom {
        sku: "SET-FILLED".into(),
        expected_revision: 1,
        lines: vec![line("SEAT-COVER", 4), line("SEAT-FOAM", 4)],
    };
    sqlx::query("INSERT INTO catalogue_definition_events(effect_key,payload,bom_revision,actor,source_ref,reason) VALUES('stale-import',$1::jsonb,2,'cataloguer','fixture','fixture')").bind(serde_json::to_string(&body).expect("fixture body")).execute(&mut *stale).await.expect("pending old intent");
    let failed=stale.execute("INSERT INTO catalogue_boms(sku,revision,creation_effect_key) VALUES('SET-FILLED',2,'stale-import')").await.expect_err("stale importer must serialize");
    assert_eq!(
        failed.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("40001")
    );
    stale.rollback().await.expect("discard stale import");
    assert_eq!(count(&pool).await, 5);
    let quantities:Vec<i32>=sqlx::query_scalar("SELECT quantity FROM catalogue_bom_lines WHERE sku='SET-FILLED' AND revision=2 ORDER BY component_code").fetch_all(&pool).await.expect("current snapshot");
    assert_eq!(quantities, [3, 3]);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn schema_freezes_identity_and_rejects_orphan_or_partial_definition_events(pool: PgPool) {
    seed(&pool).await;
    graph(&pool).await;
    record(&pool, &variant("SET-FILLED", Fill::WithFoam), "filled").await;
    for statement in [
        "UPDATE catalogue_definition_events SET source_ref=source_ref",
        "DELETE FROM catalogue_definition_events",
        "UPDATE product_families SET fixture=false WHERE code='DEMO-FAMILY'",
        "UPDATE product_families SET code='RENAMED-FAMILY' WHERE code='ROOM-A'",
        "UPDATE physical_components SET kind='foam' WHERE code='SEAT-COVER'",
        "UPDATE physical_components SET fixture=false WHERE code='DEMO-COVER'",
        "UPDATE catalogue_items SET fixture=false WHERE sku='DEMO-FILLED'",
        "UPDATE catalogue_items SET minor_units=0 WHERE sku='SET-FILLED'",
        "UPDATE catalogue_items SET slug='invented-url' WHERE sku='SET-FILLED'",
        "UPDATE catalogue_variants SET textile_code='other' WHERE sku='SET-FILLED'",
        "UPDATE catalogue_variants SET family_code='DEMO-FAMILY' WHERE sku='SET-FILLED'",
        "UPDATE catalogue_variants SET stock_policy='prepacked' WHERE sku='SET-FILLED'",
        "UPDATE catalogue_boms SET creation_effect_key='family' WHERE sku='SET-FILLED' AND revision=1",
        "UPDATE catalogue_bom_lines SET quantity=3 WHERE sku='SET-FILLED' AND revision=1",
        "DELETE FROM product_families WHERE code='ROOM-A'",
        "DELETE FROM physical_components WHERE code='SEAT-COVER'",
        "DELETE FROM catalogue_items WHERE sku='SET-FILLED'",
    ] {
        let error = pool
            .execute(statement)
            .await
            .expect_err("identity/history must remain immutable");
        assert_eq!(
            error.as_database_error().and_then(|e| e.code()).as_deref(),
            Some("23514"),
            "{statement}"
        );
    }
    let orphan=pool.execute("INSERT INTO catalogue_definition_events(effect_key,payload,actor,source_ref,reason) VALUES('orphan','{\"kind\":\"family\",\"code\":\"ORPHAN\"}','cataloguer','fixture','fixture')").await.expect_err("orphan event cannot commit");
    assert_eq!(
        orphan.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("23514")
    );
    let mut partial = pool.begin().await.expect("partial transaction");
    let body = variant("PARTIAL-SET", Fill::WithFoam);
    sqlx::query("INSERT INTO catalogue_definition_events(effect_key,payload,bom_revision,actor,source_ref,reason) VALUES('partial',$1::jsonb,1,'cataloguer','fixture','fixture')").bind(serde_json::to_string(&body).expect("fixture body")).execute(&mut *partial).await.expect("pending event");
    partial.execute("INSERT INTO catalogue_items(sku,currency,fill,fixture,creation_effect_key) VALUES('PARTIAL-SET','EUR','with-foam',false,'partial')").await.expect("pending item only");
    let failed = partial
        .commit()
        .await
        .expect_err("incomplete variant cannot commit");
    assert_eq!(
        failed.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("23514")
    );
    let leaked: i64 =
        sqlx::query_scalar("SELECT count(*) FROM catalogue_items WHERE sku='PARTIAL-SET'")
            .fetch_one(&pool)
            .await
            .expect("no leaked item");
    assert_eq!(leaked, 0);
    assert_eq!(count(&pool).await, 4);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn fixture_and_merchant_bom_components_cannot_mix_in_either_direction(pool: PgPool) {
    seed(&pool).await;
    graph(&pool).await;
    record(&pool, &variant("SET-FILLED", Fill::WithFoam), "filled").await;
    let mut merchant = pool.begin().await.expect("merchant import fixture");
    let body = serde_json::json!({"kind":"bom","sku":"SET-FILLED","expected_revision":1,"lines":[{"component_code":"DEMO-COVER","quantity":2},{"component_code":"DEMO-FOAM","quantity":2}]});
    sqlx::query("INSERT INTO catalogue_definition_events(effect_key,payload,bom_revision,actor,source_ref,reason) VALUES('mixed',$1::jsonb,2,'cataloguer','fixture','fixture')").bind(body.to_string()).execute(&mut *merchant).await.expect("pending raw event");
    merchant.execute("INSERT INTO catalogue_boms(sku,revision,creation_effect_key) VALUES('SET-FILLED',2,'mixed'); INSERT INTO catalogue_bom_lines VALUES('SET-FILLED',2,'DEMO-COVER',2),('SET-FILLED',2,'DEMO-FOAM',2)").await.expect("pending raw lines");
    let error = merchant
        .execute("UPDATE catalogue_boms SET sealed=true WHERE sku='SET-FILLED' AND revision=2")
        .await
        .expect_err("merchant cannot use demo pools");
    assert_eq!(
        error.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("23514")
    );
    merchant.rollback().await.expect("discard bad import");
    let mut fixture = pool.begin().await.expect("fixture import");
    fixture.execute("INSERT INTO catalogue_boms(sku,revision) VALUES('DEMO-FILLED',2); INSERT INTO catalogue_bom_lines VALUES('DEMO-FILLED',2,'SEAT-COVER',2),('DEMO-FILLED',2,'SEAT-FOAM',2)").await.expect("pending demo lines");
    let error = fixture
        .execute("UPDATE catalogue_boms SET sealed=true WHERE sku='DEMO-FILLED' AND revision=2")
        .await
        .expect_err("demo cannot reserve merchant pools");
    assert_eq!(
        error.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("23514")
    );
    fixture.rollback().await.expect("discard bad fixture");
    assert_eq!(count(&pool).await, 4);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn stored_command_or_entity_binding_corruption_never_returns_a_trusted_receipt(pool: PgPool) {
    seed(&pool).await;
    graph(&pool).await;
    let author = SessionToken::from_bytes([1; 32]);
    let command = CatalogueCommand::Family {
        code: "ROOM-A".into(),
    };
    for payload in [
        serde_json::json!({"kind":"family","code":"OTHER-FAMILY"}),
        serde_json::json!({"kind":"family","code":"ROOM-A","published":true}),
    ] {
        pool.execute(
            "ALTER TABLE catalogue_definition_events DISABLE TRIGGER guard_definition_history",
        )
        .await
        .expect("isolated corruption setup");
        sqlx::query(
            "UPDATE catalogue_definition_events SET payload=$1::jsonb WHERE effect_key='family'",
        )
        .bind(payload.to_string())
        .execute(&pool)
        .await
        .expect("isolated corrupt record");
        pool.execute(
            "ALTER TABLE catalogue_definition_events ENABLE TRIGGER guard_definition_history",
        )
        .await
        .expect("restore guard");
        assert!(matches!(
            definition_event(&pool, &author, &FixedClock(1100), "family").await,
            Err(DefinitionError::InvalidStoredData)
        ));
        assert!(matches!(
            record_definition(
                &pool,
                &author,
                &FixedClock(1100),
                request(&command, "family")
            )
            .await,
            Err(DefinitionError::InvalidStoredData)
        ));
    }
    assert_eq!(count(&pool).await, 3);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn merchant_variants_still_share_physical_components_across_reservations_and_versions(
    pool: PgPool,
) {
    use commerce_domain::inventory::{StockError, WarehouseChange};
    use commerce_infrastructure::{
        holds::{HoldCommand, HoldRequest, HoldWriteError, reserve_bom, transition_hold},
        staff_operations::{StaffWarehouseRequest, warehouse_change},
    };
    seed(&pool).await;
    graph(&pool).await;
    record(&pool, &variant("SET-FILLED", Fill::WithFoam), "filled").await;
    record(&pool, &variant("SET-COVERS", Fill::CoversOnly), "covers").await;
    for (component, key) in [
        ("SEAT-COVER", "receive-covers"),
        ("SEAT-FOAM", "receive-foam"),
    ] {
        warehouse_change(
            &pool,
            &SessionToken::from_bytes([2; 32]),
            &FixedClock(1100),
            StaffWarehouseRequest {
                effect_key: key,
                component_code: component,
                change: WarehouseChange::Receive(2),
                reason: "isolated physical fixture receipt",
                confirmed: true,
            },
        )
        .await
        .expect("separate warehouse authority");
    }
    let covers = "00000000000000000000000000000001";
    let blocked = "00000000000000000000000000000002";
    let filled = "00000000000000000000000000000003";
    reserve_bom(
        &pool,
        HoldRequest {
            id: covers,
            owner_id: "fixture-owner",
            sku: "SET-COVERS",
            revision: 1,
            units: 1,
            created_at: 1100,
            expires_at: 1200,
        },
    )
    .await
    .expect("covers reserve only covers");
    let unavailable = reserve_bom(
        &pool,
        HoldRequest {
            id: blocked,
            owner_id: "fixture-owner",
            sku: "SET-FILLED",
            revision: 1,
            units: 1,
            created_at: 1100,
            expires_at: 1200,
        },
    )
    .await;
    assert!(matches!(
        unavailable,
        Err(HoldWriteError::Stock(StockError::InvalidBalance))
    ));
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT component_code,on_hand,reserved FROM stock_balances ORDER BY component_code",
    )
    .fetch_all(&pool)
    .await
    .expect("physical oracle");
    assert_eq!(
        rows,
        [("SEAT-COVER".into(), 2, 2), ("SEAT-FOAM".into(), 2, 0)]
    );
    record(
        &pool,
        &CatalogueCommand::Bom {
            sku: "SET-FILLED".into(),
            expected_revision: 1,
            lines: vec![line("SEAT-COVER", 3), line("SEAT-FOAM", 3)],
        },
        "next-bom",
    )
    .await;
    transition_hold(
        &pool,
        covers,
        HoldCommand::Release {
            owner_id: "fixture-owner",
            actor_id: "fixture-owner",
        },
        1101,
    )
    .await
    .expect("release original snapshot");
    reserve_bom(
        &pool,
        HoldRequest {
            id: filled,
            owner_id: "fixture-owner",
            sku: "SET-FILLED",
            revision: 1,
            units: 1,
            created_at: 1102,
            expires_at: 1200,
        },
    )
    .await
    .expect("explicit immutable earlier BOM remains usable");
    transition_hold(
        &pool,
        filled,
        HoldCommand::Consume {
            owner_id: "fixture-owner",
            actor_id: "fixture-warehouse",
        },
        1103,
    )
    .await
    .expect("consume exact held components");
    let remaining: Vec<(i64, i64, i64)> = sqlx::query_as(
        "SELECT on_hand,quarantine,reserved FROM stock_balances ORDER BY component_code",
    )
    .fetch_all(&pool)
    .await
    .expect("conservation");
    assert_eq!(remaining, [(0, 0, 0), (0, 0, 0)]);
    let holds: i64 = sqlx::query_scalar("SELECT count(*) FROM stock_holds")
        .fetch_one(&pool)
        .await
        .expect("no failed hold orphan");
    assert_eq!(holds, 2);
}

#[sqlx::test(migrations = false)]
async fn schema_expansion_preserves_legacy_readers_stock_and_fixture_classification(pool: PgPool) {
    use commerce_domain::inventory::WarehouseChange;
    use commerce_infrastructure::staff_operations::{StaffWarehouseRequest, warehouse_change};
    MIGRATOR.run_to(12, &pool).await.expect("prior copy schema");
    seed(&pool).await;
    warehouse_change(
        &pool,
        &SessionToken::from_bytes([2; 32]),
        &FixedClock(1100),
        StaffWarehouseRequest {
            effect_key: "old-receipt",
            component_code: "DEMO-COVER",
            change: WarehouseChange::Receive(10),
            reason: "isolated prior stock",
            confirmed: true,
        },
    )
    .await
    .expect("prior stock operation");
    let before = commerce_infrastructure::catalogue(&pool, Locale::Nl)
        .await
        .expect("prior reader");
    MIGRATOR
        .run(&pool)
        .await
        .expect("private authoring expansion");
    assert_eq!(count(&pool).await, 0);
    assert_eq!(
        commerce_infrastructure::catalogue(&pool, Locale::Nl)
            .await
            .expect("reader after upgrade"),
        before
    );
    graph(&pool).await;
    record(
        &pool,
        &variant("NEW-COVERS", Fill::CoversOnly),
        "new-covers",
    )
    .await;
    let old: (i64, i64, i64) = sqlx::query_as(
        "SELECT on_hand,quarantine,reserved FROM stock_balances WHERE component_code='DEMO-COVER'",
    )
    .fetch_one(&pool)
    .await
    .expect("old stock unchanged");
    assert_eq!(old, (10, 0, 0));
    let entries: i64 = sqlx::query_scalar("SELECT count(*) FROM stock_balances")
        .fetch_one(&pool)
        .await
        .expect("no transferred stock");
    assert_eq!(entries, 1);
    let classifications: Vec<bool> = sqlx::query_scalar(
        "SELECT fixture FROM physical_components WHERE code LIKE 'DEMO-%' ORDER BY code",
    )
    .fetch_all(&pool)
    .await
    .expect("known fixture flags");
    assert_eq!(classifications, [true, true]);
    assert_eq!(
        commerce_infrastructure::catalogue(&pool, Locale::Nl)
            .await
            .expect("new candidates absent from legacy reader"),
        before
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn bounded_effect_lock_failure_retains_no_event_and_retry_creates_once(pool: PgPool) {
    seed(&pool).await;
    let mut blocker = pool.begin().await.expect("effect blocker");
    blocker
        .execute("SELECT pg_advisory_xact_lock(hashtextextended('catalogue-definition:blocked',0))")
        .await
        .expect("hold effect");
    let command = CatalogueCommand::Family {
        code: "BLOCKED-FAMILY".into(),
    };
    let author = SessionToken::from_bytes([1; 32]);
    let clock = FixedClock(1100);
    let error = record_definition(&pool, &author, &clock, request(&command, "blocked"))
        .await
        .expect_err("bounded storage wait");
    assert!(
        matches!(error,DefinitionError::Database(ref e) if e.as_database_error().and_then(|e|e.code()).as_deref()==Some("55P03"))
    );
    assert_eq!(count(&pool).await, 0);
    blocker.rollback().await.expect("unlock");
    assert!(!record(&pool, &command, "blocked").await.replayed);
    assert_eq!(count(&pool).await, 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn final_clock_checks_roll_back_definitions_and_hide_event_reads_and_replays(pool: PgPool) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct StepClock {
        calls: AtomicUsize,
        mode: u8,
    }
    impl Clock for StepClock {
        fn unix_seconds(&self) -> Result<u64, ContextError> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(1100)
            } else {
                match self.mode {
                    0 => Ok(4600),
                    1 => Err(ContextError::ClockUnavailable),
                    _ => Ok(u64::MAX),
                }
            }
        }
    }
    seed(&pool).await;
    let author = SessionToken::from_bytes([1; 32]);
    let command = CatalogueCommand::Family {
        code: "FAMILY-A".into(),
    };
    for mode in 0..3 {
        let clock = StepClock {
            calls: AtomicUsize::new(0),
            mode,
        };
        assert!(matches!(
            record_definition(&pool, &author, &clock, request(&command, "expired")).await,
            Err(DefinitionError::Staff(
                StaffError::Denied | StaffError::ClockUnavailable
            ))
        ));
        assert_eq!(count(&pool).await, 0);
        let roots: i64 =
            sqlx::query_scalar("SELECT count(*) FROM product_families WHERE code='FAMILY-A'")
                .fetch_one(&pool)
                .await
                .expect("rolled back root");
        assert_eq!(roots, 0);
    }
    record(&pool, &command, "valid").await;
    for mode in 0..3 {
        let clock = StepClock {
            calls: AtomicUsize::new(0),
            mode,
        };
        assert!(matches!(
            record_definition(&pool, &author, &clock, request(&command, "valid")).await,
            Err(DefinitionError::Staff(
                StaffError::Denied | StaffError::ClockUnavailable
            ))
        ));
        for key in ["valid", "missing"] {
            let clock = StepClock {
                calls: AtomicUsize::new(0),
                mode,
            };
            assert!(matches!(
                definition_event(&pool, &author, &clock, key).await,
                Err(DefinitionError::Staff(
                    StaffError::Denied | StaffError::ClockUnavailable
                ))
            ));
        }
    }
    assert_eq!(count(&pool).await, 1);
}

async fn waiting_on(pool: &PgPool, pattern: &str) {
    tokio::time::timeout(std::time::Duration::from_millis(800),async {loop {
        let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE $1)").bind(pattern).fetch_one(pool).await.expect("wait oracle");
        if waiting {break;}tokio::task::yield_now().await;
    }}).await.expect("operation reached actual lock");
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn lock_wait_expiry_and_task_cancellation_leave_no_orphan_audit_or_definition(pool: PgPool) {
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
    for cancel in [false, true] {
        let mut blocker = pool.begin().await.expect("family blocker");
        blocker
            .execute("LOCK TABLE product_families IN SHARE MODE")
            .await
            .expect("pause creation");
        let clock = Arc::new(MovingClock(AtomicU64::new(1100)));
        let writer_clock = clock.clone();
        let writer_pool = pool.clone();
        let writer = tokio::spawn(async move {
            record_definition(
                &writer_pool,
                &SessionToken::from_bytes([1; 32]),
                writer_clock.as_ref(),
                request(
                    &CatalogueCommand::Family {
                        code: "WAIT-FAMILY".into(),
                    },
                    "waiting",
                ),
            )
            .await
        });
        waiting_on(&pool, "INSERT INTO product_families%").await;
        if cancel {
            writer.abort();
            assert!(writer.await.expect_err("cancelled").is_cancelled());
            blocker.rollback().await.expect("release cancelled command");
        } else {
            clock.0.store(4600, Ordering::SeqCst);
            blocker.rollback().await.expect("resume expired command");
            assert!(matches!(
                writer.await.expect("writer task"),
                Err(DefinitionError::Staff(StaffError::Denied))
            ));
        }
        assert_eq!(count(&pool).await, 0);
    }
    let recovered = record(
        &pool,
        &CatalogueCommand::Family {
            code: "WAIT-FAMILY".into(),
        },
        "waiting",
    )
    .await;
    assert!(!recovered.replayed);
    assert_eq!(count(&pool).await, 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn grant_revocation_waits_for_audited_commit_and_then_blocks_replay(pool: PgPool) {
    seed(&pool).await;
    let mut blocker = pool.begin().await.expect("family blocker");
    blocker
        .execute("LOCK TABLE product_families IN SHARE MODE")
        .await
        .expect("pause creation");
    let writer_pool = pool.clone();
    let writer = tokio::spawn(async move {
        record_definition(
            &writer_pool,
            &SessionToken::from_bytes([1; 32]),
            &FixedClock(1100),
            request(
                &CatalogueCommand::Family {
                    code: "AUTH-FAMILY".into(),
                },
                "authorized",
            ),
        )
        .await
    });
    waiting_on(&pool, "INSERT INTO product_families%").await;
    let revoke_pool = pool.clone();
    let revoke = tokio::spawn(async move {
        sqlx::query("DELETE FROM staff_capabilities WHERE staff_id='cataloguer' AND capability='catalogue.manage'").execute(&revoke_pool).await
    });
    waiting_on(&pool, "DELETE FROM staff_capabilities%").await;
    assert!(!revoke.is_finished());
    blocker
        .rollback()
        .await
        .expect("resume authorized creation");
    writer
        .await
        .expect("writer task")
        .expect("authorized commit");
    assert_eq!(
        revoke
            .await
            .expect("revocation task")
            .expect("revoke")
            .rows_affected(),
        1
    );
    let author = SessionToken::from_bytes([1; 32]);
    let command = CatalogueCommand::Family {
        code: "AUTH-FAMILY".into(),
    };
    assert!(matches!(
        record_definition(
            &pool,
            &author,
            &FixedClock(1100),
            request(&command, "authorized")
        )
        .await,
        Err(DefinitionError::Staff(StaffError::Denied))
    ));
    assert!(matches!(
        definition_event(&pool, &author, &FixedClock(1100), "authorized").await,
        Err(DefinitionError::Staff(StaffError::Denied))
    ));
    assert_eq!(count(&pool).await, 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn concurrent_creates_and_bom_edits_have_one_complete_effect_and_full_intent(pool: PgPool) {
    seed(&pool).await;
    let author = SessionToken::from_bytes([1; 32]);
    let other = SessionToken::from_bytes([4; 32]);
    let clock = FixedClock(1100);
    let family = CatalogueCommand::Family {
        code: "RACE-FAMILY".into(),
    };
    let (a, b) = tokio::join!(
        record_definition(&pool, &author, &clock, request(&family, "same-family")),
        record_definition(&pool, &author, &clock, request(&family, "same-family"))
    );
    assert_ne!(
        a.expect("create/replay").replayed,
        b.expect("create/replay").replayed
    );
    for case in 0..4 {
        let changed = CatalogueCommand::Component {
            code: "RACE-FAMILY".into(),
            component_kind: ComponentKind::Cover,
        };
        let mut r = request(if case == 0 { &changed } else { &family }, "same-family");
        let actor = if case == 3 { &other } else { &author };
        match case {
            0 => {}
            1 => r.source_ref = "other source",
            2 => r.reason = "other reason",
            3 => {}
            _ => unreachable!(),
        }
        assert!(matches!(
            record_definition(&pool, actor, &clock, r).await,
            Err(DefinitionError::Conflict)
        ));
    }
    graph(&pool).await;
    record(&pool, &variant("SET-FILLED", Fill::WithFoam), "filled").await;
    let unique = CatalogueCommand::Family {
        code: "FAMILY-UNIQUE".into(),
    };
    let (a, b) = tokio::join!(
        record_definition(&pool, &author, &clock, request(&unique, "unique-left")),
        record_definition(&pool, &author, &clock, request(&unique, "unique-right"))
    );
    assert!(matches!(
        (&a, &b),
        (Ok(_), Err(DefinitionError::Conflict)) | (Err(DefinitionError::Conflict), Ok(_))
    ));
    let left = CatalogueCommand::Bom {
        sku: "SET-FILLED".into(),
        expected_revision: 1,
        lines: vec![line("SEAT-COVER", 3), line("SEAT-FOAM", 3)],
    };
    let right = CatalogueCommand::Bom {
        sku: "SET-FILLED".into(),
        expected_revision: 1,
        lines: vec![line("SEAT-COVER", 4), line("SEAT-FOAM", 4)],
    };
    let (a, b) = tokio::join!(
        record_definition(&pool, &author, &clock, request(&left, "bom-left")),
        record_definition(&pool, &author, &clock, request(&right, "bom-right"))
    );
    assert!(matches!(
        (&a, &b),
        (Ok(_), Err(DefinitionError::Conflict)) | (Err(DefinitionError::Conflict), Ok(_))
    ));
    let expected = if a.is_ok() { 3 } else { 4 };
    let quantities:Vec<i32>=sqlx::query_scalar("SELECT quantity FROM catalogue_bom_lines WHERE sku='SET-FILLED' AND revision=2 ORDER BY component_code").fetch_all(&pool).await.expect("winning snapshot");
    assert_eq!(quantities, [expected, expected]);
    let exact = CatalogueCommand::Bom {
        sku: "SET-FILLED".into(),
        expected_revision: 2,
        lines: vec![line("SEAT-COVER", 5), line("SEAT-FOAM", 5)],
    };
    let (a, b) = tokio::join!(
        record_definition(&pool, &author, &clock, request(&exact, "bom-exact")),
        record_definition(&pool, &author, &clock, request(&exact, "bom-exact"))
    );
    let a = a.expect("BOM/replay");
    let b = b.expect("BOM/replay");
    assert_ne!(a.replayed, b.replayed);
    assert_eq!((a.bom_revision, b.bom_revision), (Some(3), Some(3)));
    assert_eq!(count(&pool).await, 8);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn a_variant_effect_cannot_be_reused_for_changed_configuration_or_evidence(pool: PgPool) {
    seed(&pool).await;
    graph(&pool).await;
    let original = variant("SET-FILLED", Fill::WithFoam);
    record(&pool, &original, "variant").await;
    let author = SessionToken::from_bytes([1; 32]);
    let other = SessionToken::from_bytes([4; 32]);
    let clock = FixedClock(1100);
    for case in 0..9 {
        let mut changed = original.clone();
        let CatalogueCommand::Variant {
            sku,
            family_code,
            size_code,
            textile_code,
            fill,
            lines,
        } = &mut changed
        else {
            panic!("variant");
        };
        match case {
            0 => *sku = "OTHER-SET".into(),
            1 => *family_code = "OTHER-FAMILY".into(),
            2 => *size_code = "other-size".into(),
            3 => *textile_code = "other-textile".into(),
            4 => *fill = Fill::CoversOnly,
            5 => lines[0].quantity = 3,
            _ => {}
        }
        let mut r = request(&changed, "variant");
        let actor = if case == 8 { &other } else { &author };
        if case == 6 {
            r.source_ref = "other-source";
        }
        if case == 7 {
            r.reason = "other-reason";
        }
        assert!(
            matches!(
                record_definition(&pool, actor, &clock, r).await,
                Err(DefinitionError::Conflict)
            ),
            "variant intent {case}"
        );
    }
    assert_eq!(count(&pool).await, 4);
}

#[test]
fn definition_commands_cannot_supply_authority_publication_price_stock_or_fixture_flags() {
    for field in [
        "actor",
        "fixture",
        "published",
        "minor_units",
        "slug",
        "stock",
        "stock_policy",
    ] {
        let mut body = serde_json::json!({"kind":"family","code":"FAMILY-A"});
        body[field] = serde_json::json!(true);
        assert!(
            serde_json::from_value::<CatalogueCommand>(body).is_err(),
            "{field}"
        );
    }
    for body in [
        serde_json::json!({"kind":"kit","code":"FAMILY-A"}),
        serde_json::json!({"kind":"component","code":"COVER","component_kind":"unknown"}),
        serde_json::json!({"kind":"bom","sku":"SET-A","expected_revision":1,"lines":[{"component_code":"COVER","quantity":-1}]}),
        serde_json::json!({"kind":"bom","sku":"SET-A","expected_revision":1,"lines":[{"component_code":"COVER","quantity":1,"fixture":false}]}),
    ] {
        assert!(serde_json::from_value::<CatalogueCommand>(body).is_err());
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn staff_roles_confirmation_evidence_and_shape_are_required_before_any_effect(pool: PgPool) {
    seed(&pool).await;
    let command = CatalogueCommand::Family {
        code: "FAMILY-A".into(),
    };
    let clock = FixedClock(1100);
    let author = SessionToken::from_bytes([1; 32]);
    for number in [0, 2, 3] {
        let token = SessionToken::from_bytes([number; 32]);
        assert!(matches!(
            record_definition(&pool, &token, &clock, request(&command, "denied")).await,
            Err(DefinitionError::Staff(StaffError::Denied))
        ));
        assert!(matches!(
            definition_event(&pool, &token, &clock, "missing").await,
            Err(DefinitionError::Staff(StaffError::Denied))
        ));
    }
    for case in 0..9 {
        let mut r = request(&command, "invalid");
        let long = "x".repeat(501);
        let long_key = "x".repeat(129);
        match case {
            0 => r.confirmed = false,
            1 => r.effect_key = "/",
            2 => r.effect_key = &long_key,
            3 => r.source_ref = " ",
            4 => r.source_ref = &long,
            5 => r.source_ref = "null\0source",
            6 => r.reason = " ",
            7 => r.reason = &long,
            8 => r.reason = "null\0reason",
            _ => unreachable!(),
        }
        assert!(
            matches!(
                record_definition(&pool, &author, &clock, r).await,
                Err(DefinitionError::InvalidRequest)
            ),
            "envelope {case}"
        );
    }
    assert!(matches!(
        definition_event(&pool, &author, &clock, "/").await,
        Err(DefinitionError::InvalidRequest)
    ));
    assert!(
        definition_event(&pool, &author, &clock, "missing")
            .await
            .expect("authorized missing event")
            .is_none()
    );
    let demo = CatalogueCommand::Family {
        code: "DEMO-FAMILY".into(),
    };
    assert!(matches!(
        record_definition(&pool, &author, &clock, request(&demo, "promote-demo")).await,
        Err(DefinitionError::InvalidRequest)
    ));
    assert_eq!(count(&pool).await, 0);
    record(&pool, &command, "valid").await;
    assert_eq!(count(&pool).await, 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn missing_members_incompatible_fill_and_conflicting_definitions_roll_back_their_events(
    pool: PgPool,
) {
    seed(&pool).await;
    graph(&pool).await;
    let author = SessionToken::from_bytes([1; 32]);
    let clock = FixedClock(1100);
    for case in 0..5 {
        let mut command = variant("FAILED-SET", Fill::WithFoam);
        let CatalogueCommand::Variant {
            family_code,
            fill,
            lines,
            ..
        } = &mut command
        else {
            panic!("variant");
        };
        match case {
            0 => *family_code = "MISSING-FAMILY".into(),
            1 => lines[0].component_code = "MISSING-FOAM".into(),
            2 => *lines = vec![line("SEAT-COVER", 2)],
            3 => *fill = Fill::CoversOnly,
            4 => *lines = vec![line("SEAT-FOAM", 2)],
            _ => unreachable!(),
        }
        let result =
            record_definition(&pool, &author, &clock, request(&command, "failed-attempt")).await;
        if case < 2 {
            assert!(matches!(result, Err(DefinitionError::NotFound)));
        } else {
            assert!(matches!(result, Err(DefinitionError::InvalidRequest)));
        }
        assert_eq!(count(&pool).await, 3);
        let roots: i64 =
            sqlx::query_scalar("SELECT count(*) FROM catalogue_items WHERE sku='FAILED-SET'")
                .fetch_one(&pool)
                .await
                .expect("partial root oracle");
        assert_eq!(roots, 0);
    }
    record(&pool, &variant("SET-FILLED", Fill::WithFoam), "filled").await;
    for (command, key) in [
        (
            CatalogueCommand::Family {
                code: "ROOM-A".into(),
            },
            "duplicate-family",
        ),
        (
            CatalogueCommand::Component {
                code: "SEAT-COVER".into(),
                component_kind: ComponentKind::Foam,
            },
            "duplicate-component",
        ),
        (variant("SET-FILLED", Fill::WithFoam), "duplicate-variant"),
        (
            CatalogueCommand::Bom {
                sku: "SET-FILLED".into(),
                expected_revision: 2,
                lines: vec![line("SEAT-COVER", 2), line("SEAT-FOAM", 2)],
            },
            "stale-version",
        ),
    ] {
        assert!(
            matches!(
                record_definition(&pool, &author, &clock, request(&command, key)).await,
                Err(DefinitionError::Conflict)
            ),
            "{key}"
        );
    }
    let missing = CatalogueCommand::Bom {
        sku: "MISSING-SET".into(),
        expected_revision: 1,
        lines: vec![line("SEAT-COVER", 2)],
    };
    assert!(matches!(
        record_definition(&pool, &author, &clock, request(&missing, "missing-sku")).await,
        Err(DefinitionError::NotFound)
    ));
    let wrong = CatalogueCommand::Bom {
        sku: "SET-FILLED".into(),
        expected_revision: 1,
        lines: vec![line("SEAT-COVER", 2)],
    };
    assert!(matches!(
        record_definition(&pool, &author, &clock, request(&wrong, "wrong-fill")).await,
        Err(DefinitionError::InvalidRequest)
    ));
    assert_eq!(count(&pool).await, 4);
    let kinds: Vec<String> =
        sqlx::query_scalar("SELECT kind FROM physical_components WHERE code='SEAT-COVER'")
            .fetch_all(&pool)
            .await
            .expect("immutable kind");
    assert_eq!(kinds, ["cover"]);
}

struct FixedClock(u64);
impl Clock for FixedClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(self.0)
    }
}
fn line(code: &str, quantity: u32) -> BomLineDraft {
    BomLineDraft {
        component_code: code.into(),
        quantity,
    }
}
fn variant(sku: &str, fill: Fill) -> CatalogueCommand {
    let lines = match fill {
        Fill::CoversOnly => vec![line("SEAT-COVER", 2)],
        Fill::WithFoam => vec![line("SEAT-FOAM", 2), line("SEAT-COVER", 2)],
    };
    CatalogueCommand::Variant {
        sku: sku.into(),
        family_code: "ROOM-A".into(),
        size_code: "isolated-size".into(),
        textile_code: "isolated-textile".into(),
        fill,
        lines,
    }
}
fn request<'a>(command: &'a CatalogueCommand, key: &'a str) -> DefinitionRequest<'a> {
    DefinitionRequest {
        effect_key: key,
        command,
        source_ref: "isolated invented source; never supplier evidence",
        reason: "isolated definition fixture",
        confirmed: true,
    }
}
async fn seed(pool: &PgPool) {
    pool.execute(include_str!("../../../tests/fixtures/bom.sql"))
        .await
        .expect("legacy fixture");
    pool.execute("INSERT INTO catalogue_translations(sku,locale,title,description,care) VALUES('DEMO-FILLED','nl','legacy fixture','legacy fixture description','legacy fixture care');
        INSERT INTO staff_accounts VALUES('cataloguer',true),('warehouse',true),('reviewer',true),('other-cataloguer',true);
        INSERT INTO staff_capabilities VALUES('cataloguer','catalogue.manage'),('warehouse','warehouse.manage'),('reviewer','catalogue.review'),('other-cataloguer','catalogue.manage')").await.expect("test-only authorities");
    for (number, actor) in [
        (1, "cataloguer"),
        (2, "warehouse"),
        (3, "reviewer"),
        (4, "other-cataloguer"),
    ] {
        sqlx::query("INSERT INTO staff_sessions(token_hash,staff_id,created_at,expires_at,mfa_verified_at) VALUES($1,$2,1000,4600,1100)").bind(SessionToken::from_bytes([number;32]).digest().as_slice()).bind(actor).execute(pool).await.expect("test-only session");
    }
}
async fn record(pool: &PgPool, command: &CatalogueCommand, key: &str) -> DefinitionReceipt {
    record_definition(
        pool,
        &SessionToken::from_bytes([1; 32]),
        &FixedClock(1100),
        request(command, key),
    )
    .await
    .expect("private candidate definition")
}
async fn graph(pool: &PgPool) {
    record(
        pool,
        &CatalogueCommand::Family {
            code: "ROOM-A".into(),
        },
        "family",
    )
    .await;
    record(
        pool,
        &CatalogueCommand::Component {
            code: "SEAT-COVER".into(),
            component_kind: ComponentKind::Cover,
        },
        "cover",
    )
    .await;
    record(
        pool,
        &CatalogueCommand::Component {
            code: "SEAT-FOAM".into(),
            component_kind: ComponentKind::Foam,
        },
        "foam",
    )
    .await;
}
async fn count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM catalogue_definition_events")
        .fetch_one(pool)
        .await
        .expect("event oracle")
}

// Synthetic merchant-class rows exist only in the isolated test database. They
// exercise the unpublished model and are never evidence of real products.
#[sqlx::test(migrator = "MIGRATOR")]
async fn private_candidates_have_atomic_shared_boms_without_price_url_or_stock(pool: PgPool) {
    seed(&pool).await;
    let before = commerce_infrastructure::catalogue(&pool, Locale::Nl)
        .await
        .expect("old preview");
    assert_eq!(before.len(), 1);
    graph(&pool).await;
    let filled = variant("SET-FILLED", Fill::WithFoam);
    let first = record(&pool, &filled, "filled").await;
    assert_eq!((first.bom_revision, first.replayed), (Some(1), false));
    record(&pool, &variant("SET-COVERS", Fill::CoversOnly), "covers").await;
    let roots: Vec<(String, Option<i64>, Option<String>, bool)> = sqlx::query_as(
        "SELECT sku,minor_units,slug,fixture FROM catalogue_items WHERE NOT fixture ORDER BY sku",
    )
    .fetch_all(&pool)
    .await
    .expect("no placeholders");
    assert_eq!(
        roots,
        [
            ("SET-COVERS".into(), None, None, false),
            ("SET-FILLED".into(), None, None, false)
        ]
    );
    let physical: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM stock_balances),(SELECT count(*) FROM stock_movements)",
    )
    .fetch_one(&pool)
    .await
    .expect("stock oracle");
    assert_eq!(physical, (0, 0));
    assert_eq!(
        commerce_infrastructure::catalogue(&pool, Locale::Nl)
            .await
            .expect("fixture-only preview"),
        before
    );
    let old_reader:Vec<(String,i64)>=sqlx::query_as("SELECT i.sku,i.minor_units FROM catalogue_items i JOIN catalogue_translations t USING(sku) WHERE t.locale='nl' ORDER BY i.sku").fetch_all(&pool).await.expect("prior SQL reader remains compatible");
    assert_eq!(old_reader, [("DEMO-FILLED".into(), 200)]);
    let event = definition_event(
        &pool,
        &SessionToken::from_bytes([1; 32]),
        &FixedClock(1100),
        "filled",
    )
    .await
    .expect("validated event")
    .expect("event");
    assert_eq!(event.actor, "cataloguer");
    let CatalogueCommand::Variant { lines, .. } = event.command else {
        panic!("variant event expected");
    };
    assert_eq!(
        lines
            .iter()
            .map(|line| line.component_code.as_str())
            .collect::<Vec<_>>(),
        ["SEAT-COVER", "SEAT-FOAM"]
    );
    let mut reordered = filled.clone();
    let CatalogueCommand::Variant { lines, .. } = &mut reordered else {
        panic!("variant expected");
    };
    lines.reverse();
    assert!(record(&pool, &reordered, "filled").await.replayed);
    let next = CatalogueCommand::Bom {
        sku: "SET-FILLED".into(),
        expected_revision: 1,
        lines: vec![line("SEAT-COVER", 3), line("SEAT-FOAM", 3)],
    };
    assert_eq!(record(&pool, &next, "bom2").await.bom_revision, Some(2));
    for (revision, expected) in [(1, 2), (2, 3)] {
        let bom = commerce_infrastructure::bom::bill_of_materials(&pool, "SET-FILLED", revision)
            .await
            .expect("BOM read")
            .expect("sealed snapshot");
        assert_eq!(bom.components.len(), 2);
        assert!(
            bom.components
                .iter()
                .all(|component| component.quantity == expected)
        );
    }
    assert!(record(&pool, &filled, "filled").await.replayed);
    assert_eq!(count(&pool).await, 6);
    let wrong_projection=pool.execute("INSERT INTO catalogue_translations(sku,locale,title,description,care) VALUES('SET-FILLED','nl','not allowed','not allowed','not allowed')").await.expect_err("merchant cannot enter legacy preview");
    assert_eq!(
        wrong_projection
            .as_database_error()
            .and_then(|e| e.code())
            .as_deref(),
        Some("23514")
    );
}
