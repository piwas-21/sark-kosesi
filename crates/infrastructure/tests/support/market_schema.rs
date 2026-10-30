#[sqlx::test(migrator = "MIGRATOR")]
async fn database_rejects_history_mutation_independence_gaps_identity_changes_and_stale_approvals(
    pool: PgPool,
) {
    seed(&pool).await;
    draft(&pool, &policy(), "draft").await.expect("draft");
    decide(&pool, "review", 1, 0, true).await.expect("review");
    let mut p = policy();
    p.revision = 2;
    draft(&pool, &p, "new").await.expect("new draft");
    for sql in [
        "DELETE FROM market_policy_anchors",
        "UPDATE market_policy_anchors SET postcode='OTHER'",
        "UPDATE market_policy_drafts SET reason='changed'",
        "DELETE FROM market_policy_drafts",
        "UPDATE market_policy_reviews SET reason='changed'",
        "DELETE FROM market_policy_reviews",
        "INSERT INTO market_policy_drafts SELECT id,4,jsonb_set(policy,'{revision}','4'),'gap',actor,reason,recorded_at FROM market_policy_drafts WHERE revision=2",
        "INSERT INTO market_policy_drafts SELECT id,3,policy,'identity',actor,reason,recorded_at FROM market_policy_drafts WHERE revision=2",
        "INSERT INTO market_policy_reviews SELECT id,draft_revision,2,decision,evidence_ref,qualification_ref,'stale',actor,reason,recorded_at FROM market_policy_reviews",
        "INSERT INTO market_policy_reviews SELECT id,2,1,decision,evidence_ref,qualification_ref,'self','author',reason,recorded_at FROM market_policy_reviews",
        "INSERT INTO market_policy_reviews SELECT id,2,3,decision,evidence_ref,qualification_ref,'gap',actor,reason,recorded_at FROM market_policy_reviews",
        "INSERT INTO market_policy_reviews SELECT id,99,1,decision,evidence_ref,qualification_ref,'missing',actor,reason,recorded_at FROM market_policy_reviews",
        "INSERT INTO market_policy_anchors VALUES('OTHER','NL','1234 AB')",
        "INSERT INTO market_policy_anchors VALUES('OUTSIDE','US','12345')",
    ] {
        assert!(pool.execute(sql).await.is_err(), "must reject {sql}");
    }
    let mut wrong = policy();
    wrong.revision = 3;
    wrong.destination.postcode = "9999 ZZ".into();
    assert!(matches!(
        draft(&pool, &wrong, "changed-destination").await,
        Err(MarketError::Conflict)
    ));
    let mut other = policy();
    other.id = "OTHER".into();
    assert!(matches!(
        draft(&pool, &other, "ambiguous").await,
        Err(MarketError::NotFound)
    ));
    let mut r = review("missing", 1, 0, true);
    r.id = "UNKNOWN";
    assert!(matches!(
        record_review(&pool, &token(2), &FixedClock(1100), r).await,
        Err(MarketError::NotFound)
    ));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn stored_policy_and_review_corruption_are_revalidated_at_read_boundary(pool: PgPool) {
    seed(&pool).await;
    draft(&pool, &policy(), "draft").await.expect("draft");
    decide(&pool, "review", 1, 0, true).await.expect("review");
    pool.execute("ALTER TABLE market_policy_drafts DISABLE TRIGGER guard_market_draft")
        .await
        .expect("isolated corruption");
    for sql in [
        "UPDATE market_policy_drafts SET policy=policy || '{\"extra\":true}'::jsonb",
        "UPDATE market_policy_drafts SET policy=jsonb_set(policy,'{id}','\"OTHER\"')",
        "UPDATE market_policy_drafts SET policy=jsonb_set(policy,'{revision}','2')",
        "UPDATE market_policy_drafts SET policy=jsonb_set(policy,'{products}','[]')",
        "UPDATE market_policy_drafts SET policy=jsonb_set(policy,'{shipping,gross,minor_units}','-1')",
    ] {
        pool.execute(sql).await.expect("corrupt isolated row");
        assert!(
            matches!(inspect(&pool, 1).await, Err(MarketError::InvalidStoredData)),
            "{sql}"
        );
        sqlx::query("UPDATE market_policy_drafts SET policy=$1")
            .bind(serde_json::to_value(policy()).expect("fixture"))
            .execute(&pool)
            .await
            .expect("restore");
    }
    pool.execute("UPDATE market_policy_drafts SET reason=E'bad\\nreason'")
        .await
        .expect("corrupt reason");
    assert!(matches!(
        inspect(&pool, 1).await,
        Err(MarketError::InvalidStoredData)
    ));
    pool.execute("UPDATE market_policy_drafts SET reason='restored'; ALTER TABLE market_policy_reviews DISABLE TRIGGER guard_market_review").await.expect("isolated review corruption");
    for sql in [
        "UPDATE market_policy_reviews SET actor='author'",
        "UPDATE market_policy_reviews SET evidence_ref=E'bad\\nref'",
        "UPDATE market_policy_reviews SET qualification_ref=E'bad\\nref'",
        "UPDATE market_policy_reviews SET reason=E'bad\\nreason'",
    ] {
        pool.execute(sql).await.expect("corrupt review");
        assert!(
            matches!(inspect(&pool, 1).await, Err(MarketError::InvalidStoredData)),
            "{sql}"
        );
        pool.execute("UPDATE market_policy_reviews SET actor='reviewer',evidence_ref='restored',qualification_ref='restored',reason='restored'").await.expect("restore");
    }
    assert!(inspect(&pool, 1).await.is_ok());
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn repeatable_read_imports_cannot_advance_a_stale_history(pool: PgPool) {
    seed(&pool).await;
    draft(&pool, &policy(), "draft").await.expect("draft");
    let mut stale = pool.begin().await.expect("snapshot");
    stale
        .execute(
            "SET TRANSACTION ISOLATION LEVEL REPEATABLE READ; SELECT * FROM market_policy_drafts",
        )
        .await
        .expect("pin snapshot");
    let mut p = policy();
    p.revision = 2;
    draft(&pool, &p, "new").await.expect("concurrent edit");
    let result=sqlx::query("INSERT INTO market_policy_drafts(id,revision,policy,effect_key,actor,reason) VALUES('SYNTHETIC-NL',2,$1,'stale','author','stale import')").bind(serde_json::to_value(p).expect("fixture")).execute(&mut *stale).await.expect_err("stale importer rejected");
    assert_eq!(
        result.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("40001")
    );
}
#[sqlx::test(migrations = false)]
async fn previous_schema_upgrade_preserves_staff_and_has_no_implicit_readiness_or_grants(
    pool: PgPool,
) {
    MIGRATOR.run_to(16, &pool).await.expect("previous schema");
    pool.execute("INSERT INTO staff_accounts VALUES('existing',true); INSERT INTO staff_capabilities VALUES('existing','catalogue.manage'),('existing','warehouse.manage')").await.expect("old authority");
    MIGRATOR.run(&pool).await.expect("expand migration");
    let counts:(i64,i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM market_policy_anchors),(SELECT count(*) FROM market_policy_drafts),(SELECT count(*) FROM market_policy_reviews),(SELECT count(*) FROM staff_capabilities WHERE capability LIKE 'market.%')").fetch_one(&pool).await.expect("unready after upgrade");
    assert_eq!(counts, (0, 0, 0, 0));
    let grants: Vec<String> =
        sqlx::query_scalar("SELECT capability FROM staff_capabilities ORDER BY capability")
            .fetch_all(&pool)
            .await
            .expect("retained authority");
    assert_eq!(grants, vec!["catalogue.manage", "warehouse.manage"]);
    MIGRATOR.run(&pool).await.expect("idempotent");
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn policy_storage_fault_is_never_a_readiness_default(pool: PgPool) {
    seed(&pool).await;
    pool.execute("ALTER TABLE market_policy_anchors RENAME TO unavailable_policy_storage")
        .await
        .expect("isolated dependency failure");
    assert!(matches!(
        inspect(&pool, 1).await,
        Err(MarketError::Database(_))
    ));
}
