use super::*;
#[sqlx::test(migrator = "MIGRATOR")]
async fn database_history_is_immutable_independent_and_contiguous_with_positive_import_control(
    pool: PgPool,
) {
    ready(&pool).await;
    for sql in [
        "DELETE FROM promotion_policy_anchors",
        "UPDATE promotion_policy_anchors SET id='OTHER'",
        "UPDATE promotion_policy_drafts SET policy=policy",
        "DELETE FROM promotion_policy_drafts",
        "UPDATE promotion_policy_reviews SET decision=decision",
        "DELETE FROM promotion_policy_reviews",
        "INSERT INTO promotion_policy_drafts SELECT id,3,jsonb_set(policy,'{promotion,revision}','3'),'gap',actor,reason,recorded_at FROM promotion_policy_drafts",
        "INSERT INTO promotion_policy_drafts SELECT id,2,policy,'identity',actor,reason,recorded_at FROM promotion_policy_drafts",
        "INSERT INTO promotion_policy_drafts SELECT id,2,jsonb_set(jsonb_set(policy,'{promotion,revision}','2'),'{currency}','\"USD\"'),'currency',actor,reason,recorded_at FROM promotion_policy_drafts",
        "INSERT INTO promotion_policy_reviews SELECT id,draft_revision,3,decision,evidence_ref,qualification_ref,'review-gap',actor,reason,recorded_at FROM promotion_policy_reviews",
        "INSERT INTO promotion_policy_reviews SELECT id,draft_revision,2,decision,evidence_ref,qualification_ref,'self','author',reason,recorded_at FROM promotion_policy_reviews",
        "INSERT INTO promotion_policy_reviews SELECT id,99,1,decision,evidence_ref,qualification_ref,'missing',actor,reason,recorded_at FROM promotion_policy_reviews",
    ] {
        let e = pool.execute(sql).await.expect_err("invalid import");
        assert!(
            e.as_database_error()
                .and_then(|e| e.code())
                .is_some_and(|code| code.starts_with("23")),
            "{e}"
        );
    }
    pool.execute("INSERT INTO promotion_policy_drafts SELECT id,2,jsonb_set(policy,'{promotion,revision}','2'),'exact-import',actor,reason,recorded_at FROM promotion_policy_drafts").await.expect("valid import control");
    assert!(matches!(approved(&pool).await, Err(PolicyError::NotFound)));
    let e=pool.execute("INSERT INTO promotion_policy_reviews SELECT id,draft_revision,2,decision,evidence_ref,qualification_ref,'stale',actor,reason,recorded_at FROM promotion_policy_reviews").await.expect_err("old approval denied");
    assert_eq!(
        e.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("23514")
    );
    pool.execute("INSERT INTO promotion_policy_reviews SELECT id,2,1,decision,evidence_ref,qualification_ref,'exact-review',actor,reason,recorded_at FROM promotion_policy_reviews").await.expect("independent current import control");
    assert_eq!(
        approved(&pool)
            .await
            .expect("current")
            .policy
            .promotion
            .revision,
        2
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn corrupt_restored_policy_shapes_limits_and_source_headers_fail_normal_readers(
    pool: PgPool,
) {
    ready(&pool).await;
    pool.execute("ALTER TABLE promotion_policy_drafts DISABLE TRIGGER guard_promotion_draft; ALTER TABLE promotion_policy_drafts DROP CONSTRAINT promotion_policy_drafts_policy_check").await.expect("isolated damaged restore fixture");
    let original = serde_json::to_value(policy()).expect("policy");
    for n in 0..17 {
        let mut v = original.clone();
        match n {
            0 => v = serde_json::json!([]),
            1 => v["promotion"] = serde_json::json!([]),
            2 => v["limits"] = serde_json::json!([]),
            3 => v["promotion"]["discount"] = serde_json::json!([]),
            4 => v["promotion"]["products"] = serde_json::json!({}),
            5 => v["promotion"]["products"] = serde_json::json!([["SET-FILLED", 1]]),
            6 => v["role"] = "admin".into(),
            7 => v["promotion"]["id"] = "OTHER".into(),
            8 => v["promotion"]["revision"] = 2.into(),
            9 => v["limits"]["total_discount_minor"] = (-1i64).into(),
            10 => v["promotion"]["countries"] = serde_json::json!(["NL", "DE"]),
            11 => v["promotion"]["evidence_ref"] = "\nprivate invalid ref".into(),
            12 => v["large"] = "x".repeat(33000).into(),
            13 => {
                v["promotion"]["products"] =
                    serde_json::json!(vec![v["promotion"]["products"][0].clone(); 21])
            }
            14 => v["limits"]["per_customer_redemptions"] = 0.into(),
            15 => v["currency"] = "USD".into(),
            _ => {
                v.as_object_mut().expect("object").remove("currency");
            }
        }
        sqlx::query("UPDATE promotion_policy_drafts SET policy=$1")
            .bind(v)
            .execute(&pool)
            .await
            .expect("damaged restore");
        assert!(
            matches!(inspect(&pool, 1).await, Err(PolicyError::InvalidStoredData)),
            "case {n}"
        );
        assert!(
            matches!(approved(&pool).await, Err(PolicyError::InvalidStoredData)),
            "case {n}"
        );
    }
    sqlx::query("UPDATE promotion_policy_drafts SET policy=$1")
        .bind(original)
        .execute(&pool)
        .await
        .expect("restored");
    pool.execute("UPDATE promotion_policy_drafts SET reason=E'bad\\nreason'")
        .await
        .expect("damaged reason");
    assert!(matches!(
        inspect(&pool, 1).await,
        Err(PolicyError::InvalidStoredData)
    ));
    pool.execute("UPDATE promotion_policy_drafts SET reason='restored'")
        .await
        .expect("restore reason");
    pool.execute("ALTER TABLE promotion_policy_drafts DROP CONSTRAINT promotion_policy_drafts_actor_fkey; UPDATE promotion_policy_drafts SET actor=E'bad\\nactor'").await.expect("damaged restore identity");
    assert!(matches!(
        inspect(&pool, 1).await,
        Err(PolicyError::InvalidStoredData)
    ));
    assert!(matches!(
        approved(&pool).await,
        Err(PolicyError::InvalidStoredData)
    ));
    pool.execute("UPDATE promotion_policy_drafts SET actor='author'")
        .await
        .expect("restore identity");
    assert!(approved(&pool).await.is_ok());
    pool.execute("ALTER TABLE promotion_policy_reviews DISABLE TRIGGER guard_promotion_review; ALTER TABLE promotion_policy_reviews DROP CONSTRAINT promotion_policy_reviews_revision_check, DROP CONSTRAINT promotion_policy_reviews_decision_check").await.expect("review corruption fixture");
    for sql in [
        "UPDATE promotion_policy_reviews SET revision=0",
        "UPDATE promotion_policy_reviews SET decision='unknown'",
        "UPDATE promotion_policy_reviews SET actor='author'",
        "UPDATE promotion_policy_reviews SET evidence_ref=E'bad\\nref'",
        "UPDATE promotion_policy_reviews SET qualification_ref=E'bad\\nref'",
        "UPDATE promotion_policy_reviews SET reason=E'bad\\nreason'",
    ] {
        pool.execute(sql).await.expect("damage");
        assert!(matches!(
            inspect(&pool, 1).await,
            Err(PolicyError::InvalidStoredData)
        ));
        pool.execute("UPDATE promotion_policy_reviews SET revision=1,decision='approved',actor='reviewer',evidence_ref='restored',qualification_ref='restored',reason='restored'").await.expect("restore");
    }
    assert!(approved(&pool).await.is_ok());
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn stale_repeatable_read_promotion_import_cannot_advance_a_prior_snapshot(pool: PgPool) {
    ready(&pool).await;
    let mut stale = pool.begin().await.expect("snapshot");
    stale
        .execute(
            "SET TRANSACTION ISOLATION LEVEL REPEATABLE READ;SELECT * FROM promotion_policy_drafts",
        )
        .await
        .expect("pin");
    let mut p = policy();
    p.promotion.revision = 2;
    draft(&pool, &p, "concurrent")
        .await
        .expect("concurrent edit");
    let error=sqlx::query("INSERT INTO promotion_policy_drafts(id,revision,policy,effect_key,actor,reason) VALUES('COURTYARD',2,$1,'stale','author','synthetic')").bind(serde_json::to_value(p).expect("policy")).execute(&mut *stale).await.expect_err("stale snapshot rejected");
    assert_eq!(
        error.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("40001")
    );
}
#[sqlx::test(migrations = false)]
async fn upgrade_preserves_prior_authority_without_issuing_promotions_or_new_grants(pool: PgPool) {
    let prior = MIGRATOR
        .iter()
        .filter(|m| m.version < 21)
        .map(|m| m.version)
        .max()
        .expect("prior schema");
    MIGRATOR.run_to(prior, &pool).await.expect("prior schema");
    pool.execute("INSERT INTO staff_accounts VALUES('legacy',true); INSERT INTO staff_capabilities VALUES('legacy','market.manage'),('legacy','market.review'),('legacy','warehouse.manage')").await.expect("prior authority");
    sqlx::query("INSERT INTO staff_sessions(token_hash,staff_id,created_at,expires_at,mfa_verified_at) VALUES($1,'legacy',1000,4600,1100)").bind(token(9).digest().as_slice()).execute(&pool).await.expect("prior session");
    MIGRATOR.run(&pool).await.expect("expand");
    MIGRATOR.run(&pool).await.expect("idempotent runner");
    let counts:(i64,i64,i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM staff_capabilities),(SELECT count(*) FROM staff_sessions),(SELECT count(*) FROM promotion_policy_anchors),(SELECT count(*) FROM promotion_policy_drafts),(SELECT count(*) FROM promotion_policy_reviews)").fetch_one(&pool).await.expect("preserved empty upgrade");
    assert_eq!(counts, (3, 1, 0, 0, 0));
    pool.execute("INSERT INTO staff_capabilities VALUES('legacy','promotion.manage')")
        .await
        .expect("explicit isolated grant positive control");
}
