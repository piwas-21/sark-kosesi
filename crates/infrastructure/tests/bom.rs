use commerce_domain::{BomComponent, ComponentKind};
use commerce_infrastructure::{MIGRATOR, bom::bill_of_materials};
use sqlx::{Executor, PgPool};

const FIXTURE: &str = include_str!("../../../tests/fixtures/bom.sql");

#[sqlx::test(migrator = "MIGRATOR")]
async fn configurations_share_real_components_and_keep_old_revisions(pool: PgPool) {
    pool.execute(FIXTURE).await.expect("fixture");
    let covers = bill_of_materials(&pool, "DEMO-COVERS", 1)
        .await
        .expect("read")
        .expect("sealed");
    assert_eq!(covers.sku, "DEMO-COVERS");
    assert_eq!(covers.revision, 1);
    assert_eq!(
        covers.components,
        vec![BomComponent {
            code: "DEMO-COVER".into(),
            kind: ComponentKind::Cover,
            quantity: 2,
        }]
    );
    let filled = bill_of_materials(&pool, "DEMO-FILLED", 1)
        .await
        .expect("read")
        .expect("sealed");
    assert_eq!(
        filled.components,
        vec![
            covers.components[0].clone(),
            BomComponent {
                code: "DEMO-FOAM".into(),
                kind: ComponentKind::Foam,
                quantity: 2
            },
        ]
    );
    pool.execute(
        "INSERT INTO catalogue_boms (sku,revision) VALUES ('DEMO-COVERS',2);
        INSERT INTO catalogue_bom_lines VALUES ('DEMO-COVERS',2,'DEMO-COVER',3)",
    )
    .await
    .expect("new draft");
    for (sku, revision) in [("DEMO-COVERS", 2), ("DEMO-COVERS", 0), ("' OR true --", 1)] {
        assert_eq!(
            bill_of_materials(&pool, sku, revision)
                .await
                .expect("bounded query"),
            None
        );
    }
    pool.execute("UPDATE catalogue_boms SET sealed=true WHERE sku='DEMO-COVERS' AND revision=2")
        .await
        .expect("seal new version");
    assert_eq!(
        bill_of_materials(&pool, "DEMO-COVERS", 2)
            .await
            .expect("read")
            .expect("sealed")
            .components[0]
            .quantity,
        3
    );
    assert_eq!(
        bill_of_materials(&pool, "DEMO-COVERS", 1)
            .await
            .expect("read"),
        Some(covers)
    );
    pool.close().await;
    assert!(bill_of_materials(&pool, "DEMO-COVERS", 1).await.is_err());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn invalid_relations_fill_and_mutating_sealed_revisions_fail(pool: PgPool) {
    pool.execute(FIXTURE).await.expect("fixture");
    for invalid in [
        "UPDATE catalogue_bom_lines SET quantity=3 WHERE sku='DEMO-COVERS'",
        "DELETE FROM catalogue_bom_lines WHERE sku='DEMO-COVERS'",
        "INSERT INTO catalogue_bom_lines VALUES ('DEMO-COVERS',1,'DEMO-FOAM',1)",
        "UPDATE catalogue_boms SET sealed=false WHERE sku='DEMO-COVERS'",
        "DELETE FROM catalogue_boms WHERE sku='DEMO-COVERS'",
        "UPDATE catalogue_items SET fill='with-foam' WHERE sku='DEMO-COVERS'",
        "UPDATE physical_components SET kind='foam' WHERE code='DEMO-COVER'",
        "UPDATE physical_components SET code='DEMO-OTHER' WHERE code='DEMO-COVER'",
        "DELETE FROM physical_components WHERE code='DEMO-COVER'",
        "INSERT INTO catalogue_variants VALUES ('DEMO-MISSING','DEMO-FAMILY','size','textile','separable')",
        "UPDATE catalogue_variants SET stock_policy='prepacked'",
        "INSERT INTO product_families VALUES ('REAL-FAMILY',false)",
        "INSERT INTO physical_components VALUES ('DEMO-BAD','kit',true)",
        "INSERT INTO catalogue_boms VALUES ('DEMO-COVERS',0,false)",
        "INSERT INTO catalogue_boms VALUES ('DEMO-COVERS',2,true)",
    ] {
        assert!(
            pool.execute(invalid).await.is_err(),
            "accepted invalid SQL: {invalid}"
        );
    }
    pool.execute(
        "INSERT INTO catalogue_boms (sku,revision) VALUES ('DEMO-COVERS',2),('DEMO-FILLED',2)",
    )
    .await
    .expect("drafts");
    for invalid in [
        "INSERT INTO catalogue_bom_lines VALUES ('DEMO-COVERS',2,'DEMO-COVER',0)",
        "INSERT INTO catalogue_bom_lines VALUES ('DEMO-COVERS',2,'DEMO-COVER',-1)",
        "INSERT INTO catalogue_bom_lines VALUES ('DEMO-COVERS',2,'DEMO-COVER',10001)",
        "INSERT INTO catalogue_bom_lines VALUES ('DEMO-COVERS',2,'DEMO-MISSING',1)",
        "UPDATE catalogue_boms SET revision=3 WHERE revision=2",
        "UPDATE catalogue_boms SET sealed=true WHERE revision=2",
    ] {
        assert!(
            pool.execute(invalid).await.is_err(),
            "accepted invalid SQL: {invalid}"
        );
    }
    pool.execute("INSERT INTO catalogue_bom_lines VALUES ('DEMO-COVERS',2,'DEMO-COVER',1),('DEMO-COVERS',2,'DEMO-FOAM',1),('DEMO-FILLED',2,'DEMO-COVER',1)")
        .await.expect("mismatched drafts");
    for sku in ["DEMO-COVERS", "DEMO-FILLED"] {
        assert!(
            sqlx::query("UPDATE catalogue_boms SET sealed=true WHERE sku=$1 AND revision=2")
                .bind(sku)
                .execute(&pool)
                .await
                .is_err()
        );
    }
    assert!(
        pool.execute("UPDATE catalogue_bom_lines SET revision=1 WHERE revision=2")
            .await
            .is_err()
    );
    pool.execute(
        "DELETE FROM catalogue_bom_lines WHERE component_code='DEMO-FOAM' AND revision=2;
        UPDATE catalogue_bom_lines SET quantity=10000 WHERE sku='DEMO-COVERS' AND revision=2;
        DELETE FROM catalogue_bom_lines WHERE sku='DEMO-FILLED' AND revision=2;
        DELETE FROM catalogue_boms WHERE sku='DEMO-FILLED' AND revision=2;
        UPDATE physical_components SET kind=kind;
        UPDATE catalogue_items SET fill=fill;
        UPDATE catalogue_boms SET sealed=true WHERE sku='DEMO-COVERS' AND revision=2",
    )
    .await
    .expect("valid draft edits and seal");
    assert_eq!(
        bill_of_materials(&pool, "DEMO-COVERS", 2)
            .await
            .expect("read")
            .expect("sealed")
            .components[0]
            .quantity,
        10000
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn sealing_serializes_with_competing_line_edit(pool: PgPool) {
    pool.execute(FIXTURE).await.expect("fixture");
    pool.execute(
        "INSERT INTO catalogue_boms (sku,revision) VALUES ('DEMO-COVERS',2);
        INSERT INTO catalogue_bom_lines VALUES ('DEMO-COVERS',2,'DEMO-COVER',1)",
    )
    .await
    .expect("draft");
    let mut seal = pool.begin().await.expect("transaction");
    sqlx::query("UPDATE catalogue_boms SET sealed=true WHERE sku='DEMO-COVERS' AND revision=2")
        .execute(&mut *seal)
        .await
        .expect("seal lock");
    let other_pool = pool.clone();
    let edit = tokio::spawn(async move {
        sqlx::query(
            "UPDATE catalogue_bom_lines SET quantity=9 WHERE sku='DEMO-COVERS' AND revision=2",
        )
        .execute(&other_pool)
        .await
    });
    // Prove the contender reached a database lock, not an assumed scheduling delay.
    tokio::time::timeout(std::time::Duration::from_secs(5),async {
        loop {
            let (waiting,): (bool,) = sqlx::query_as("SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'UPDATE catalogue_bom_lines%')")
                .fetch_one(&pool).await.expect("lock probe");
            if waiting {break;}
            tokio::task::yield_now().await;
        }
    }).await.expect("contender blocked");
    seal.commit().await.expect("commit");
    assert!(
        edit.await.expect("task").is_err(),
        "post-seal line mutation must fail"
    );
    assert_eq!(
        bill_of_materials(&pool, "DEMO-COVERS", 2)
            .await
            .expect("read")
            .expect("sealed")
            .components[0]
            .quantity,
        1
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn corrupt_component_data_is_rejected_by_reader(pool: PgPool) {
    pool.execute(FIXTURE).await.expect("fixture");
    pool.execute(
        "ALTER TABLE physical_components DISABLE TRIGGER guard_component_kind;
        ALTER TABLE physical_components DROP CONSTRAINT physical_components_kind_check;
        UPDATE physical_components SET kind='mystery' WHERE code='DEMO-COVER'",
    )
    .await
    .expect("hostile corrupt kind");
    assert!(bill_of_materials(&pool, "DEMO-COVERS", 1).await.is_err());
    pool.execute(
        "UPDATE physical_components SET kind='cover';
        ALTER TABLE catalogue_bom_lines DISABLE TRIGGER guard_bom_line;
        ALTER TABLE catalogue_bom_lines DROP CONSTRAINT catalogue_bom_lines_quantity_check",
    )
    .await
    .expect("hostile quantities");
    for quantity in [-1, 0, 10001] {
        sqlx::query("UPDATE catalogue_bom_lines SET quantity=$1")
            .bind(quantity)
            .execute(&pool)
            .await
            .expect("corrupt quantity");
        assert!(bill_of_materials(&pool, "DEMO-COVERS", 1).await.is_err());
    }
}

#[sqlx::test(migrations = false)]
async fn p1_upgrade_preserves_preview_without_inventing_bom(pool: PgPool) {
    MIGRATOR.run_to(2, &pool).await.expect("P1");
    pool.execute(include_str!("../../../tests/fixtures/catalogue.sql"))
        .await
        .expect("P1 seed");
    MIGRATOR.run(&pool).await.expect("expand");
    assert_eq!(
        commerce_infrastructure::catalogue(&pool, commerce_application::Locale::Nl)
            .await
            .expect("old projection")
            .len(),
        1
    );
    assert_eq!(
        bill_of_materials(&pool, "DEMO-COURTYARD-COVERS", 1)
            .await
            .expect("no invented BOM"),
        None
    );
    pool.execute(FIXTURE).await.expect("new schema usable");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn sealing_rechecks_lines_after_waiting_for_an_editor(pool: PgPool) {
    pool.execute(FIXTURE).await.expect("fixture");
    pool.execute(
        "INSERT INTO catalogue_boms (sku,revision) VALUES ('DEMO-COVERS',2);
        INSERT INTO catalogue_bom_lines VALUES ('DEMO-COVERS',2,'DEMO-COVER',1)",
    )
    .await
    .expect("draft");
    let mut editor = pool.begin().await.expect("transaction");
    sqlx::query("INSERT INTO catalogue_bom_lines VALUES ('DEMO-COVERS',2,'DEMO-FOAM',1)")
        .execute(&mut *editor)
        .await
        .expect("hold parent lock with invalid draft edit");
    let other = pool.clone();
    let seal = tokio::spawn(async move {
        sqlx::query("UPDATE catalogue_boms SET sealed=true WHERE sku='DEMO-COVERS' AND revision=2")
            .execute(&other)
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5),async {
        loop {
            let (waiting,): (bool,) = sqlx::query_as("SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'UPDATE catalogue_boms%')")
                .fetch_one(&pool).await.expect("lock probe");
            if waiting {break;}
            tokio::task::yield_now().await;
        }
    }).await.expect("sealing blocked");
    editor.commit().await.expect("commit draft edit");
    assert!(
        seal.await.expect("task").is_err(),
        "cannot seal covers-only with newly committed foam"
    );
    assert_eq!(
        bill_of_materials(&pool, "DEMO-COVERS", 2)
            .await
            .expect("read"),
        None
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn sealing_limits_component_fanout(pool: PgPool) {
    pool.execute(FIXTURE).await.expect("fixture");
    pool.execute("INSERT INTO catalogue_boms (sku,revision) VALUES ('DEMO-COVERS',2);
        INSERT INTO physical_components (code,kind) SELECT 'DEMO-PART-' || n,'cover' FROM generate_series(1,65) n;
        INSERT INTO catalogue_bom_lines SELECT 'DEMO-COVERS',2,'DEMO-PART-' || n,1 FROM generate_series(1,65) n")
        .await.expect("oversized draft");
    assert!(
        pool.execute("UPDATE catalogue_boms SET sealed=true WHERE revision=2")
            .await
            .is_err()
    );
    pool.execute(
        "DELETE FROM catalogue_bom_lines WHERE component_code='DEMO-PART-65';
        UPDATE catalogue_boms SET sealed=true WHERE revision=2",
    )
    .await
    .expect("maximum valid fanout");
    assert_eq!(
        bill_of_materials(&pool, "DEMO-COVERS", 2)
            .await
            .expect("read")
            .expect("sealed")
            .components
            .len(),
        64
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn stale_repeatable_read_cannot_seal_or_change_fill(pool: PgPool) {
    pool.execute(FIXTURE).await.expect("fixture");
    pool.execute(
        "INSERT INTO catalogue_boms (sku,revision) VALUES ('DEMO-COVERS',2);
        INSERT INTO catalogue_bom_lines VALUES ('DEMO-COVERS',2,'DEMO-COVER',1)",
    )
    .await
    .expect("draft");
    let mut stale_sealer = pool.begin().await.expect("sealer");
    stale_sealer
        .execute("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .await
        .expect("isolation");
    stale_sealer
        .execute("SELECT * FROM catalogue_boms")
        .await
        .expect("snapshot");
    pool.execute("INSERT INTO catalogue_bom_lines VALUES ('DEMO-COVERS',2,'DEMO-FOAM',1)")
        .await
        .expect("later editor commits");
    let error =
        sqlx::query("UPDATE catalogue_boms SET sealed=true WHERE sku='DEMO-COVERS' AND revision=2")
            .execute(&mut *stale_sealer)
            .await
            .expect_err("stale snapshot must fail");
    assert_eq!(
        error.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("40001")
    );
    stale_sealer.rollback().await.expect("rollback");
    pool.execute("DELETE FROM catalogue_bom_lines WHERE revision=2 AND component_code='DEMO-FOAM'")
        .await
        .expect("correct draft");
    let mut stale_fill = pool.begin().await.expect("fill writer");
    stale_fill
        .execute("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .await
        .expect("isolation");
    stale_fill
        .execute("SELECT * FROM catalogue_items")
        .await
        .expect("snapshot");
    pool.execute("UPDATE catalogue_boms SET sealed=true WHERE sku='DEMO-COVERS' AND revision=2")
        .await
        .expect("later sealer commits");
    let error = sqlx::query("UPDATE catalogue_items SET fill='with-foam' WHERE sku='DEMO-COVERS'")
        .execute(&mut *stale_fill)
        .await
        .expect_err("cannot miss concurrent seal");
    assert_eq!(
        error.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("40001")
    );
    stale_fill.rollback().await.expect("rollback");
}
