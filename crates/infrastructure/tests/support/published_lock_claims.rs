//! Disposable roles and synthetic approved catalogue only; no operational grants.
use super::*;
use sqlx::postgres::PgPoolOptions;
use std::time::Duration;

fn denied(error: sqlx::Error, code: &str) {
    assert_eq!(
        error.as_database_error().and_then(|e| e.code()).as_deref(),
        Some(code)
    );
}

// Every interpolated identifier below is a fixed prefix plus process ID; the
// deliberately hyphenated default role is wrapped in double quotes. This is
// isolated fixture DDL only and never includes caller or external input.
async fn fixture_ddl(pool: &PgPool, sql: String) {
    sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
        .execute(pool)
        .await
        .expect("isolated fixture DDL");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn published_claims_support_restricted_login_and_hold_both_locks(pool: PgPool) {
    let _case = media_seed(&pool).await;
    activate(&pool).await;
    pool.execute("INSERT INTO catalogue_translations(sku,locale,title,description,care) VALUES ('DEMO-COVERS','nl','Synthetic cover','Synthetic description','Synthetic care')").await.expect("synthetic legacy translation");
    candidate(&pool, "SET-SECOND").await;
    pool.execute("INSERT INTO catalogue_media_assets(asset_id,effect_key,payload,actor,reason) VALUES ('photo-second','photo-second', '{\"sha256\":\"synthetic-second\"}'::jsonb,'author','synthetic')").await.expect("second synthetic asset");
    let expected = public(&pool)
        .await
        .expect("owner projects synthetic publication");

    // Names are isolated to this disposable cluster and contain only a PID.
    let suffix = std::process::id();
    let runtime = format!("preview_test_runtime_{suffix}");
    let helper = format!("preview_test_helper_{suffix}");
    let image_helper = format!("preview_test_image_helper_{suffix}");
    let other = format!("preview_test_other_{suffix}");
    let legacy = format!("preview_test_legacy_{suffix}");
    fixture_ddl(&pool, format!("CREATE ROLE {runtime} LOGIN PASSWORD 'synthetic-only'; CREATE ROLE {other} LOGIN PASSWORD 'synthetic-only'; CREATE ROLE {helper} NOLOGIN; CREATE ROLE {image_helper} NOLOGIN; CREATE ROLE {legacy} LOGIN PASSWORD 'synthetic-only'; REVOKE CREATE ON SCHEMA public FROM PUBLIC; GRANT USAGE ON SCHEMA public TO {runtime},{helper},{image_helper},{other},{legacy}; GRANT SELECT ON public.product_families,public.physical_components,public.catalogue_items,public.catalogue_variants,public.catalogue_boms,public.catalogue_bom_lines,public.catalogue_definition_events,public.catalogue_factual_reviews,public.catalogue_family_routes,public.catalogue_image_renditions,public.catalogue_image_reviews,public.catalogue_media_assets,public.catalogue_media_rights,public.catalogue_media_serialization,public.catalogue_price_revisions,public.catalogue_publication_revisions,public.catalogue_specification_revisions,public.catalogue_translation_reviews,public.catalogue_translation_revisions,public.stock_balances TO {runtime},{legacy}; GRANT SELECT ON public.catalogue_image_originals TO {legacy}; GRANT SELECT(asset_id,recipe,sha256,actor,display_width_px,display_height_px) ON public.catalogue_image_originals TO {runtime}; GRANT SELECT(asset_id,recipe,sha256,bytes) ON public.catalogue_image_originals TO {image_helper}; ALTER FUNCTION public.preview_original_intact(text) OWNER TO {image_helper}; GRANT SELECT(version,success,checksum) ON public._sqlx_migrations TO {runtime}; GRANT SELECT(sku,locale,title,description,care) ON public.catalogue_translations TO {runtime}; GRANT UPDATE(sku) ON public.catalogue_variants TO {legacy}; GRANT UPDATE(asset_id) ON public.catalogue_media_serialization TO {legacy}; GRANT SELECT(sku),UPDATE(sku) ON public.catalogue_variants TO {helper}; GRANT SELECT(asset_id),UPDATE(asset_id) ON public.catalogue_media_serialization TO {helper}; ALTER FUNCTION public.claim_published_variants(text[]) OWNER TO {helper}; ALTER FUNCTION public.claim_published_assets(text[]) OWNER TO {helper}")).await;
    let options = (*pool.connect_options()).clone();
    let restricted = PgPoolOptions::new()
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("SET search_path=public,pg_catalog,pg_temp")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .max_connections(3)
        .connect_with(
            options
                .clone()
                .username(&runtime)
                .password("synthetic-only"),
        )
        .await
        .expect("restricted login");
    let legacy_pool = PgPoolOptions::new()
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("SET search_path=public,pg_catalog,pg_temp")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .max_connections(1)
        .connect_with(options.clone().username(&legacy).password("synthetic-only"))
        .await
        .expect("legacy login");
    let ungranted = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options.username(&other).password("synthetic-only"))
        .await
        .expect("ungranted login");
    let privilege: (bool, bool, bool, bool, bool) = sqlx::query_as("SELECT rolcanlogin,has_table_privilege($1,'public.catalogue_variants','UPDATE'),has_column_privilege($1,'public.catalogue_variants','sku','UPDATE'),has_table_privilege($1,'public.catalogue_media_serialization','UPDATE'),has_column_privilege($1,'public.catalogue_media_serialization','asset_id','UPDATE') FROM pg_roles WHERE rolname=$1").bind(&helper).fetch_one(&pool).await.expect("helper least privileges");
    assert_eq!(privilege, (false, false, true, false, true));
    let boundary: (bool, bool, bool, bool) = sqlx::query_as("SELECT pg_catalog.pg_has_role(current_user,$1,'MEMBER'),pg_catalog.has_schema_privilege(current_user,'public','CREATE'),pg_catalog.has_table_privilege(current_user,'public.catalogue_image_originals','SELECT'),current_user=pg_catalog.pg_get_userbyid((SELECT relowner FROM pg_catalog.pg_class WHERE oid='public.catalogue_items'::pg_catalog.regclass))")
        .bind(&helper).fetch_one(&restricted).await.expect("effective observer privileges");
    assert_eq!(boundary, (false, false, false, false));
    let original_access: (bool, bool, bool, bool) = sqlx::query_as("SELECT pg_catalog.has_column_privilege(current_user,'public.catalogue_image_originals','bytes','SELECT'), pg_catalog.has_column_privilege(current_user,'public.catalogue_image_originals','sha256','SELECT'), pg_catalog.pg_has_role(current_user,$1,'MEMBER'), pg_catalog.has_function_privilege(current_user,'public.preview_original_intact(text)','EXECUTE')")
        .bind(&image_helper).fetch_one(&restricted).await.expect("original privilege boundary");
    assert_eq!(original_access, (false, true, false, false));
    let image_owner: (bool, bool, bool, bool, bool) = sqlx::query_as("SELECT rolcanlogin,rolsuper,rolbypassrls,pg_catalog.has_table_privilege($1,'public.catalogue_image_originals','SELECT'),pg_catalog.has_column_privilege($1,'public.catalogue_image_originals','bytes','SELECT') FROM pg_catalog.pg_roles WHERE rolname=$1")
        .bind(&image_helper).fetch_one(&pool).await.expect("integrity owner boundary");
    assert_eq!(image_owner, (false, false, false, false, true));
    for candidate in [&restricted, &ungranted] {
        denied(
            sqlx::query("SELECT public.preview_original_intact($1)")
                .bind("photo")
                .execute(candidate)
                .await
                .expect_err("integrity helper initially revoked"),
            "42501",
        );
    }
    for candidate in [&restricted, &ungranted] {
        denied(
            sqlx::query("SELECT * FROM public.claim_published_variants($1)")
                .bind(vec!["SET-FILLED"])
                .execute(candidate)
                .await
                .expect_err("execute initially revoked"),
            "42501",
        );
    }
    assert!(matches!(
        published_product(&restricted, &FixedClock(1100), Locale::Nl, "SET-FILLED").await,
        Err(PublishedError::Database(_))
    ));
    assert_eq!(
        published_product(&legacy_pool, &FixedClock(1100), Locale::Nl, "SET-FILLED")
            .await
            .expect("legacy direct claims"),
        Some(expected.clone())
    );
    fixture_ddl(
        &pool,
        format!("GRANT EXECUTE ON FUNCTION public.claim_published_variants(text[]) TO {runtime}"),
    )
    .await;
    assert!(matches!(
        published_product(&restricted, &FixedClock(1100), Locale::Nl, "SET-FILLED").await,
        Err(PublishedError::Database(_))
    ));
    fixture_ddl(
        &pool,
        format!("GRANT EXECUTE ON FUNCTION public.claim_published_assets(text[]) TO {runtime}"),
    )
    .await;
    assert!(matches!(
        published_product(&restricted, &FixedClock(1100), Locale::Nl, "SET-FILLED").await,
        Err(PublishedError::Publication(_)) | Err(PublishedError::Database(_))
    ));
    fixture_ddl(
        &pool,
        format!("GRANT EXECUTE ON FUNCTION public.preview_original_intact(text) TO {runtime}"),
    )
    .await;
    assert_eq!(
        published_product(&restricted, &FixedClock(1100), Locale::Nl, "SET-FILLED")
            .await
            .expect("restricted projection"),
        Some(expected.clone())
    );
    let original_hash: String = sqlx::query_scalar("SELECT pg_catalog.encode(sha256,'hex') FROM public.catalogue_image_originals WHERE asset_id='photo' AND recipe='image-v1'")
        .fetch_one(&pool).await.expect("original digest");
    install_shadow_sha(&pool, &restricted, &original_hash).await;
    rehearse_observer(&pool, &restricted, &expected).await;
    corrupt_original_and_repair(&pool, &restricted, &legacy_pool, &expected, Shadow::Hash).await;
    let rendition_hash: String = sqlx::query_scalar("SELECT pg_catalog.encode(sha256,'hex') FROM public.catalogue_image_renditions WHERE asset_id='photo' AND recipe='image-v1' AND edge=320")
        .fetch_one(&pool).await.expect("rendition digest");
    install_shadow_sha(&pool, &restricted, &rendition_hash).await;
    corrupt_rendition_and_repair(&pool, &restricted, &legacy_pool, &expected, Shadow::Hash).await;
    fixture_ddl(&pool, "DROP FUNCTION public.sha256(bytea)".into()).await;
    let removed: bool =
        sqlx::query_scalar("SELECT pg_catalog.to_regprocedure('public.sha256(bytea)') IS NULL")
            .fetch_one(&pool)
            .await
            .expect("shadow cleanup");
    assert!(removed);
    install_shadow_operators(&pool, &restricted).await;
    for candidate in [&restricted, &legacy_pool] {
        assert_eq!(
            published_product(candidate, &FixedClock(1100), Locale::Nl, "SET-FILLED")
                .await
                .expect("intact publication under hostile operators"),
            Some(expected.clone())
        );
    }
    corrupt_original_and_repair(
        &pool,
        &restricted,
        &legacy_pool,
        &expected,
        Shadow::Operator,
    )
    .await;
    corrupt_rendition_and_repair(
        &pool,
        &restricted,
        &legacy_pool,
        &expected,
        Shadow::Operator,
    )
    .await;
    fixture_ddl(&pool, "DROP OPERATOR public.= (bytea,bytea); DROP OPERATOR public.<> (bytea,bytea); DROP FUNCTION public.fixture_bytea_equal(bytea,bytea); DROP FUNCTION public.fixture_bytea_unequal(bytea,bytea)".into()).await;
    let operators_removed: bool = sqlx::query_scalar("SELECT pg_catalog.to_regoperator('public.=(bytea,bytea)') IS NULL AND pg_catalog.to_regoperator('public.<>(bytea,bytea)') IS NULL")
        .fetch_one(&pool).await.expect("operator shadow cleanup");
    assert!(operators_removed);
    assert_eq!(
        published_page(&restricted, &FixedClock(1100), Locale::Nl, None)
            .await
            .expect("restricted page")
            .items
            .len(),
        2
    );
    denied(
        sqlx::query("SELECT * FROM public.claim_published_assets($1)")
            .bind(vec!["photo"])
            .execute(&ungranted)
            .await
            .expect_err("other role denied"),
        "42501",
    );
    for statement in [
        "SELECT sku FROM public.catalogue_variants FOR SHARE",
        "SELECT asset_id FROM public.catalogue_media_serialization FOR SHARE",
        "UPDATE public.catalogue_variants SET size_code=size_code",
        "UPDATE public.catalogue_media_serialization SET anchor=anchor",
        "CREATE TABLE public.preview_forbidden(id int)",
        "SELECT bytes FROM public.catalogue_image_originals",
        "SELECT * FROM public.catalogue_image_originals",
        "SELECT substring(bytes FROM 1 FOR 1) FROM public.catalogue_image_originals",
        "SELECT o FROM public.catalogue_image_originals o",
        "SELECT * FROM public.customer_sessions",
        "SELECT * FROM public.payment_attempts",
        "SELECT description FROM public._sqlx_migrations",
        "UPDATE public._sqlx_migrations SET success=success",
        "UPDATE public.stock_balances SET on_hand=on_hand",
        "DELETE FROM public.commerce_outbox",
    ] {
        denied(
            sqlx::query(statement)
                .execute(&restricted)
                .await
                .expect_err("direct privilege denied"),
            "42501",
        );
    }
    for (statement, invalid) in [
        (
            "SELECT * FROM public.claim_published_variants($1)",
            vec!["X".to_owned(); 26],
        ),
        (
            "SELECT * FROM public.claim_published_variants($1)",
            vec!["X".repeat(66)],
        ),
        (
            "SELECT * FROM public.claim_published_assets($1)",
            vec!["X".to_owned(); 26],
        ),
        (
            "SELECT * FROM public.claim_published_assets($1)",
            vec!["X".repeat(129)],
        ),
    ] {
        denied(
            sqlx::query(statement)
                .bind(invalid)
                .execute(&restricted)
                .await
                .expect_err("bounded arguments"),
            "22023",
        );
    }
    // Both helpers can accept a full supported page without truncation.
    for (statement, item) in [
        (
            "SELECT * FROM public.claim_published_variants($1)",
            "SET-FILLED",
        ),
        ("SELECT * FROM public.claim_published_assets($1)", "photo"),
    ] {
        let claimed: Vec<String> = sqlx::query_scalar(statement)
            .bind(vec![item; 25])
            .fetch_all(&restricted)
            .await
            .expect("maximum supported claim");
        assert_eq!(claimed, [item]);
        let mut read_only = restricted.begin().await.expect("read only tx");
        sqlx::query("SET TRANSACTION READ ONLY")
            .execute(&mut *read_only)
            .await
            .expect("read only");
        denied(
            sqlx::query(statement)
                .bind(vec![item])
                .execute(&mut *read_only)
                .await
                .expect_err("read-only protocol cannot claim"),
            "25006",
        );
        read_only.rollback().await.expect("read only rollback");
    }

    // Both calls use one held transaction, so competing writes wait until it ends.
    let mut holder = restricted.begin().await.expect("claim transaction");
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *holder)
        .await
        .expect("holder pid");
    let variants: Vec<String> =
        sqlx::query_scalar("SELECT * FROM public.claim_published_variants($1)")
            .bind(vec!["SET-SECOND", "MISSING", "SET-FILLED"])
            .fetch_all(&mut *holder)
            .await
            .expect("variant claim");
    let assets: Vec<String> = sqlx::query_scalar("SELECT * FROM public.claim_published_assets($1)")
        .bind(vec!["photo-second", "missing", "photo"])
        .fetch_all(&mut *holder)
        .await
        .expect("asset claim");
    assert_eq!(variants, ["SET-FILLED", "SET-SECOND"]);
    assert_eq!(assets, ["photo", "photo-second"]);
    let a = pool.clone();
    let variant = tokio::spawn(async move {
        sqlx::query("UPDATE catalogue_variants SET size_code=size_code WHERE sku='SET-FILLED'")
            .execute(&a)
            .await
    });
    let b = pool.clone();
    let asset = tokio::spawn(async move {
        sqlx::query("UPDATE catalogue_media_serialization SET anchor=anchor WHERE asset_id='photo'")
            .execute(&b)
            .await
    });
    let c = pool.clone();
    let variant_second = tokio::spawn(async move {
        sqlx::query("UPDATE catalogue_variants SET size_code=size_code WHERE sku='SET-SECOND'")
            .execute(&c)
            .await
    });
    let d = pool.clone();
    let asset_second = tokio::spawn(async move {
        sqlx::query(
            "UPDATE catalogue_media_serialization SET anchor=anchor WHERE asset_id='photo-second'",
        )
        .execute(&d)
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let blocked: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid))").bind(pid).fetch_one(&pool).await.expect("blocking state");
            if blocked == 4 { break; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("all four writers blocked");
    holder.rollback().await.expect("release claims");
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), variant)
            .await
            .expect("variant released")
            .expect("task")
            .expect("update")
            .rows_affected(),
        1
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), asset)
            .await
            .expect("asset released")
            .expect("task")
            .expect("update")
            .rows_affected(),
        1
    );
    for writer in [variant_second, asset_second] {
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), writer)
                .await
                .expect("writer released")
                .expect("task")
                .expect("update")
                .rows_affected(),
            1
        );
    }
    // Owner-only fixture publication change; the observer must immediately
    // stop serving the prior synthetic product and its stored derivative.
    let mut withdrawal = request("withdraw-rehearsal", &PublicationChoice::Withdraw {});
    withdrawal.expected_revision = 1;
    record_publication(&pool, &author(), &FixedClock(1100), withdrawal)
        .await
        .expect("synthetic withdrawal");
    assert!(
        published_product(&restricted, &FixedClock(1100), Locale::Nl, "SET-FILLED")
            .await
            .expect("withdrawn product read")
            .is_none()
    );
    assert!(
        published_image(
            &restricted,
            &FixedClock(1100),
            Locale::Nl,
            "SET-FILLED",
            1,
            320
        )
        .await
        .expect("withdrawn image read")
        .is_none()
    );
    restricted.close().await;
    legacy_pool.close().await;
    ungranted.close().await;
    fixture_ddl(&pool, format!("ALTER FUNCTION public.claim_published_variants(text[]) OWNER TO CURRENT_USER; ALTER FUNCTION public.claim_published_assets(text[]) OWNER TO CURRENT_USER; ALTER FUNCTION public.preview_original_intact(text) OWNER TO CURRENT_USER; DROP OWNED BY {runtime},{helper},{image_helper},{other},{legacy}; DROP ROLE {runtime},{helper},{image_helper},{other},{legacy}")).await;
}

// An ordinary schema really can shadow functions when explicitly placed first.
// This synthetic function returns a chosen stored digest for every input.
async fn install_shadow_sha(owner: &PgPool, observer: &PgPool, digest: &str) {
    assert_eq!(digest.len(), 64);
    assert!(digest.bytes().all(|b| b.is_ascii_hexdigit()));
    fixture_ddl(owner, format!("CREATE OR REPLACE FUNCTION public.sha256(bytea) RETURNS bytea LANGUAGE sql IMMUTABLE AS 'SELECT pg_catalog.decode(''{digest}'',''hex'')'" )).await;
    let (fake, builtin): (Vec<u8>, Vec<u8>) = sqlx::query_as("SELECT sha256(pg_catalog.decode('01','hex')),pg_catalog.sha256(pg_catalog.decode('01','hex'))")
        .fetch_one(observer).await.expect("hostile function resolves first");
    let expected: Vec<u8> = sqlx::query_scalar("SELECT pg_catalog.decode($1,'hex')")
        .bind(digest)
        .fetch_one(owner)
        .await
        .expect("fixture digest");
    assert_eq!(fake, expected);
    assert_ne!(fake, builtin);
}

#[derive(Clone, Copy)]
enum Shadow {
    Hash,
    Operator,
}

// Both operator names resolve in an ordinary caller-writable schema before pg_catalog.
async fn install_shadow_operators(owner: &PgPool, observer: &PgPool) {
    fixture_ddl(owner, "CREATE FUNCTION public.fixture_bytea_equal(bytea,bytea) RETURNS boolean LANGUAGE sql IMMUTABLE AS 'SELECT true'; CREATE FUNCTION public.fixture_bytea_unequal(bytea,bytea) RETURNS boolean LANGUAGE sql IMMUTABLE AS 'SELECT false'; CREATE OPERATOR public.= (LEFTARG=bytea,RIGHTARG=bytea,FUNCTION=public.fixture_bytea_equal); CREATE OPERATOR public.<> (LEFTARG=bytea,RIGHTARG=bytea,FUNCTION=public.fixture_bytea_unequal)".into()).await;
    let witness: (bool, bool, bool, bool) = sqlx::query_as("SELECT pg_catalog.decode('01','hex') = pg_catalog.decode('02','hex'), pg_catalog.decode('01','hex') <> pg_catalog.decode('02','hex'), pg_catalog.decode('01','hex') OPERATOR(pg_catalog.=) pg_catalog.decode('02','hex'), pg_catalog.decode('01','hex') OPERATOR(pg_catalog.<>) pg_catalog.decode('02','hex')")
        .fetch_one(observer).await.expect("hostile operators resolve first");
    assert_eq!(witness, (true, false, false, true));
}

// Only this isolated fault temporarily removes the native hash CHECK and guard.
async fn corrupt_rendition_and_repair(
    owner: &PgPool,
    observer: &PgPool,
    legacy: &PgPool,
    expected: &PublishedProduct,
    shadow: Shadow,
) {
    let bytes: Vec<u8> = sqlx::query_scalar("SELECT bytes FROM public.catalogue_image_renditions WHERE asset_id='photo' AND recipe='image-v1' AND edge=320")
        .fetch_one(owner).await.expect("synthetic rendition");
    let checks: Vec<(String,String)> = sqlx::query_as("SELECT pg_catalog.quote_ident(conname),pg_catalog.pg_get_constraintdef(oid) FROM pg_catalog.pg_constraint WHERE conrelid='public.catalogue_image_renditions'::pg_catalog.regclass AND contype='c' AND pg_catalog.pg_get_constraintdef(oid) LIKE '%sha256(bytes)%'")
        .fetch_all(owner).await.expect("rendition hash constraint");
    assert_eq!(checks.len(), 1);
    let (name, definition) = &checks[0];
    fixture_ddl(owner, format!("ALTER TABLE public.catalogue_image_renditions DROP CONSTRAINT {name}; ALTER TABLE public.catalogue_image_renditions DISABLE TRIGGER guard_image_renditions; UPDATE public.catalogue_image_renditions SET bytes=set_byte(bytes,0,get_byte(bytes,0) # 1) WHERE asset_id='photo' AND recipe='image-v1' AND edge=320; ALTER TABLE public.catalogue_image_renditions ENABLE TRIGGER guard_image_renditions")).await;
    let query = match shadow {
        Shadow::Hash => {
            "SELECT sha256 OPERATOR(pg_catalog.<>) sha256(bytes),sha256 OPERATOR(pg_catalog.<>) pg_catalog.sha256(bytes) FROM public.catalogue_image_renditions WHERE asset_id='photo' AND recipe='image-v1' AND edge=320"
        }
        Shadow::Operator => {
            "SELECT sha256 <> pg_catalog.sha256(bytes),sha256 OPERATOR(pg_catalog.<>) pg_catalog.sha256(bytes) FROM public.catalogue_image_renditions WHERE asset_id='photo' AND recipe='image-v1' AND edge=320"
        }
    };
    let (fake_mismatch, real_mismatch): (bool, bool) = sqlx::query_as(query)
        .fetch_one(observer)
        .await
        .expect("shadow masks damaged rendition");
    assert!(!fake_mismatch);
    assert!(real_mismatch);
    for pool in [observer, legacy] {
        assert!(
            published_product(pool, &FixedClock(1100), Locale::Nl, "SET-FILLED")
                .await
                .is_err()
        );
    }
    fixture_ddl(
        owner,
        "ALTER TABLE public.catalogue_image_renditions DISABLE TRIGGER guard_image_renditions"
            .into(),
    )
    .await;
    assert_eq!(sqlx::query("UPDATE public.catalogue_image_renditions SET bytes=$1 WHERE asset_id='photo' AND recipe='image-v1' AND edge=320").bind(&bytes).execute(owner).await.expect("restore exact rendition").rows_affected(), 1);
    fixture_ddl(owner, format!("ALTER TABLE public.catalogue_image_renditions ENABLE TRIGGER guard_image_renditions; ALTER TABLE public.catalogue_image_renditions ADD CONSTRAINT {name} {definition}")).await;
    for pool in [observer, legacy] {
        assert_eq!(
            published_product(pool, &FixedClock(1100), Locale::Nl, "SET-FILLED")
                .await
                .expect("repaired rendition publication"),
            Some(expected.clone())
        );
    }
}

// The synthetic fault removes the original-byte CHECK only in this isolated
// database. The observer must reject actual changed bytes and recover after the
// original bytes, constraint and immutable trigger are restored.
async fn corrupt_original_and_repair(
    owner: &PgPool,
    observer: &PgPool,
    legacy: &PgPool,
    expected: &PublishedProduct,
    shadow: Shadow,
) {
    let original: Vec<u8> = sqlx::query_scalar("SELECT bytes FROM public.catalogue_image_originals WHERE asset_id='photo' AND recipe='image-v1'")
        .fetch_one(owner).await.expect("synthetic original");
    let checks: Vec<(String,String)> = sqlx::query_as("SELECT pg_catalog.quote_ident(conname),pg_catalog.pg_get_constraintdef(oid) FROM pg_catalog.pg_constraint WHERE conrelid='public.catalogue_image_originals'::pg_catalog.regclass AND contype='c' AND pg_catalog.pg_get_constraintdef(oid) LIKE '%sha256(bytes)%'")
        .fetch_all(owner).await.expect("original hash constraint");
    assert_eq!(checks.len(), 1);
    let (name, definition) = &checks[0];
    fixture_ddl(owner, format!("ALTER TABLE public.catalogue_image_originals DROP CONSTRAINT {name}; ALTER TABLE public.catalogue_image_originals DISABLE TRIGGER guard_image_originals; UPDATE public.catalogue_image_originals SET bytes=set_byte(bytes,0,get_byte(bytes,0) # 1) WHERE asset_id='photo' AND recipe='image-v1'; ALTER TABLE public.catalogue_image_originals ENABLE TRIGGER guard_image_originals")).await;
    let mut owner_shadow = owner.begin().await.expect("original shadow transaction");
    sqlx::query("SET LOCAL search_path=public,pg_catalog,pg_temp")
        .execute(&mut *owner_shadow)
        .await
        .expect("hostile owner path");
    let query = match shadow {
        Shadow::Hash => {
            "SELECT sha256 OPERATOR(pg_catalog.=) sha256(bytes),sha256 OPERATOR(pg_catalog.=) pg_catalog.sha256(bytes) FROM public.catalogue_image_originals WHERE asset_id='photo' AND recipe='image-v1'"
        }
        Shadow::Operator => {
            "SELECT sha256 = pg_catalog.sha256(bytes),sha256 OPERATOR(pg_catalog.=) pg_catalog.sha256(bytes) FROM public.catalogue_image_originals WHERE asset_id='photo' AND recipe='image-v1'"
        }
    };
    let (fake_matches, real_matches): (bool, bool) = sqlx::query_as(query)
        .fetch_one(&mut *owner_shadow)
        .await
        .expect("shadow masks damaged original");
    assert!(fake_matches);
    assert!(!real_matches);
    owner_shadow
        .rollback()
        .await
        .expect("finish original shadow check");
    let intact: bool = sqlx::query_scalar("SELECT public.preview_original_intact('photo')")
        .fetch_one(observer)
        .await
        .expect("corrupt original integrity");
    assert!(!intact);
    assert!(
        published_product(legacy, &FixedClock(1100), Locale::Nl, "SET-FILLED")
            .await
            .is_err()
    );
    assert!(
        published_product(observer, &FixedClock(1100), Locale::Nl, "SET-FILLED")
            .await
            .is_err()
    );
    fixture_ddl(
        owner,
        "ALTER TABLE public.catalogue_image_originals DISABLE TRIGGER guard_image_originals".into(),
    )
    .await;
    assert_eq!(sqlx::query("UPDATE public.catalogue_image_originals SET bytes=$1 WHERE asset_id='photo' AND recipe='image-v1'").bind(&original).execute(owner).await.expect("restore exact original").rows_affected(),1);
    fixture_ddl(owner, format!("ALTER TABLE public.catalogue_image_originals ENABLE TRIGGER guard_image_originals; ALTER TABLE public.catalogue_image_originals ADD CONSTRAINT {name} {definition}")).await;
    let intact: bool = sqlx::query_scalar("SELECT public.preview_original_intact('photo')")
        .fetch_one(observer)
        .await
        .expect("repaired integrity");
    assert!(intact);
    assert_eq!(
        published_product(legacy, &FixedClock(1100), Locale::Nl, "SET-FILLED")
            .await
            .expect("repaired legacy publication"),
        Some(expected.clone())
    );
    assert_eq!(
        published_product(observer, &FixedClock(1100), Locale::Nl, "SET-FILLED")
            .await
            .expect("repaired publication"),
        Some(expected.clone())
    );
}

// This is the SQLx observer path, not the API startup validator or HTTP server.
async fn rehearse_observer(owner: &PgPool, observer: &PgPool, expected: &PublishedProduct) {
    let mut history = observer
        .begin()
        .await
        .expect("observer history transaction");
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .execute(&mut *history)
        .await
        .expect("read-only history");
    sqlx::query("SET LOCAL search_path = pg_catalog, public, pg_temp")
        .execute(&mut *history)
        .await
        .expect("fixed history path");
    let rows: Vec<(i64, bool, Vec<u8>)> = sqlx::query_as(
        "SELECT version,success,checksum FROM public._sqlx_migrations ORDER BY version",
    )
    .fetch_all(&mut *history)
    .await
    .expect("observer migration history");
    let migrations: Vec<_> = MIGRATOR
        .iter()
        .filter(|m| !m.migration_type.is_down_migration())
        .collect();
    assert_eq!(rows.len(), migrations.len());
    for ((version, success, checksum), migration) in rows.iter().zip(migrations) {
        assert_eq!((*version, *success), (migration.version, true));
        assert_eq!(checksum.as_slice(), migration.checksum.as_ref());
    }
    history.rollback().await.expect("close history transaction");
    commerce_infrastructure::readiness(observer)
        .await
        .expect("observer readiness queries");
    let legacy = commerce_infrastructure::catalogue(observer, Locale::Nl)
        .await
        .expect("observer legacy catalogue");
    assert_eq!(legacy.len(), 1);
    assert_eq!(legacy[0].sku, "DEMO-COVERS");
    assert_eq!(legacy[0].title, "Synthetic cover");
    // The observer sees only a bounded integrity result, not original bytes.
    let intact: bool = sqlx::query_scalar("SELECT public.preview_original_intact($1)")
        .bind("photo")
        .fetch_one(observer)
        .await
        .expect("original integrity");
    assert!(intact);
    let metadata: (Vec<u8>, String, i32, i32) = sqlx::query_as("SELECT sha256,actor,display_width_px,display_height_px FROM public.catalogue_image_originals WHERE asset_id='photo' AND recipe='image-v1'")
        .fetch_one(observer).await.expect("allowed metadata");
    assert_eq!(metadata.0.len(), 32);
    assert_eq!((metadata.2, metadata.3), (32, 24));
    let mut shadow = observer
        .begin()
        .await
        .expect("temporary shadow transaction");
    sqlx::raw_sql("CREATE TEMP TABLE catalogue_image_originals(asset_id text,recipe text,sha256 bytea,bytes bytea); SET LOCAL search_path=pg_temp,public,pg_catalog")
        .execute(&mut *shadow).await.expect("temporary shadow fixtures");
    let shadowed: bool = sqlx::query_scalar("SELECT public.preview_original_intact('photo')")
        .fetch_one(&mut *shadow)
        .await
        .expect("qualified helper");
    assert!(shadowed);
    shadow.rollback().await.expect("discard shadow fixtures");
    denied(
        sqlx::query("SELECT public.preview_original_intact(NULL::text)")
            .execute(observer)
            .await
            .expect_err("null input"),
        "22023",
    );
    let missing: bool = sqlx::query_scalar("SELECT public.preview_original_intact($1)")
        .bind("missing")
        .fetch_one(observer)
        .await
        .expect("missing original");
    assert!(!missing);
    for bad in ["", "bad/path", &"x".repeat(129)] {
        denied(
            sqlx::query("SELECT public.preview_original_intact($1)")
                .bind(bad)
                .execute(observer)
                .await
                .expect_err("invalid input"),
            "22023",
        );
    }
    denied(
        sqlx::query("SELECT octet_length(bytes) FROM public.catalogue_image_originals")
            .execute(observer)
            .await
            .expect_err("original denied"),
        "42501",
    );

    let before: (serde_json::Value, serde_json::Value) = sqlx::query_as(
        "SELECT (SELECT coalesce(jsonb_agg(to_jsonb(b) ORDER BY component_code),'[]'::jsonb) FROM stock_balances b), (SELECT coalesce(jsonb_agg(to_jsonb(e) ORDER BY id),'[]'::jsonb) FROM commerce_outbox e)"
    ).fetch_one(owner).await.expect("read-only baseline");
    assert_eq!(
        published_product(observer, &FixedClock(1100), Locale::Nl, "SET-FILLED")
            .await
            .expect("observer product"),
        Some(expected.clone())
    );
    let image = published_image(
        observer,
        &FixedClock(1100),
        Locale::Nl,
        "SET-FILLED",
        1,
        320,
    )
    .await
    .expect("observer image")
    .expect("synthetic derivative");
    assert_eq!(
        (image.width, image.height, image.image_type.as_str()),
        (32, 24, "jpeg")
    );
    assert!(!image.bytes.is_empty());
    assert!(
        published_product(observer, &FixedClock(2000), Locale::Nl, "SET-FILLED")
            .await
            .expect("expired product")
            .is_none()
    );
    assert!(
        published_image(
            observer,
            &FixedClock(2000),
            Locale::Nl,
            "SET-FILLED",
            1,
            320
        )
        .await
        .expect("expired image")
        .is_none()
    );
    let after: (serde_json::Value, serde_json::Value) = sqlx::query_as(
        "SELECT (SELECT coalesce(jsonb_agg(to_jsonb(b) ORDER BY component_code),'[]'::jsonb) FROM stock_balances b), (SELECT coalesce(jsonb_agg(to_jsonb(e) ORDER BY id),'[]'::jsonb) FROM commerce_outbox e)"
    ).fetch_one(owner).await.expect("read-only outcome");
    assert_eq!(before, after);
}

#[sqlx::test(migrations = false)]
async fn published_claims_remove_only_inherited_function_grants(pool: PgPool) {
    MIGRATOR
        .run_to(67, &pool)
        .await
        .expect("populated old schema");
    let _case = media_seed(&pool).await;
    activate(&pool).await;
    let no_helpers: bool = sqlx::query_scalar("SELECT pg_catalog.to_regprocedure('public.claim_published_variants(text[])') IS NULL AND pg_catalog.to_regprocedure('public.claim_published_assets(text[])') IS NULL")
        .fetch_one(&pool).await.expect("old schema has no helpers");
    assert!(no_helpers);
    let original_product = public(&pool).await.expect("current reader on old schema");
    let before: String = sqlx::query_scalar(
        "SELECT row_to_json(v)::text FROM catalogue_variants v WHERE sku='SET-FILLED'",
    )
    .fetch_one(&pool)
    .await
    .expect("old variant");
    let default_role = format!("preview-default-{}", std::process::id());
    let quoted_role = format!("\"{default_role}\"");
    fixture_ddl(&pool, format!("CREATE ROLE {quoted_role} NOLOGIN; ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT EXECUTE ON FUNCTIONS TO {quoted_role}; CREATE FUNCTION public.preview_unrelated_default() RETURNS integer LANGUAGE sql AS 'SELECT 1'")).await;
    let original_digest: Vec<u8> = sqlx::query_scalar("SELECT pg_catalog.sha256(bytes) FROM public.catalogue_image_originals WHERE asset_id='photo' AND recipe='image-v1'").fetch_one(&pool).await.expect("original hash");
    MIGRATOR
        .run_to(75, &pool)
        .await
        .expect("prior helper schema");
    let missing: bool = sqlx::query_scalar(
        "SELECT pg_catalog.to_regprocedure('public.preview_original_intact(text)') IS NULL",
    )
    .fetch_one(&pool)
    .await
    .expect("original helper absent");
    assert!(missing);
    assert_eq!(public(&pool).await, Some(original_product.clone()));
    MIGRATOR
        .run(&pool)
        .await
        .expect("upgrade with inherited defaults");
    let privileges: (bool, bool, bool, bool, bool, bool, bool) = sqlx::query_as("SELECT has_function_privilege($1,'public.claim_published_variants(text[])','EXECUTE'),has_function_privilege($1,'public.claim_published_assets(text[])','EXECUTE'),has_function_privilege($1,'public.preview_unrelated_default()','EXECUTE'),has_function_privilege(current_user,'public.claim_published_variants(text[])','EXECUTE'),has_function_privilege(current_user,'public.claim_published_assets(text[])','EXECUTE'),has_function_privilege($1,'public.preview_original_intact(text)','EXECUTE'),has_function_privilege(current_user,'public.preview_original_intact(text)','EXECUTE')").bind(&default_role).fetch_one(&pool).await.expect("function ACLs");
    assert_eq!(privileges, (false, false, true, true, true, false, true));
    let after_digest: Vec<u8> = sqlx::query_scalar("SELECT pg_catalog.sha256(bytes) FROM public.catalogue_image_originals WHERE asset_id='photo' AND recipe='image-v1'").fetch_one(&pool).await.expect("preserved hash");
    assert_eq!(original_digest, after_digest);
    let (dummy_acl, default_acl): (bool, bool) = sqlx::query_as("SELECT EXISTS(SELECT 1 FROM pg_proc p CROSS JOIN LATERAL aclexplode(p.proacl) a JOIN pg_roles r ON r.oid=a.grantee WHERE p.oid='public.preview_unrelated_default()'::regprocedure AND r.rolname=$1 AND a.privilege_type='EXECUTE'), EXISTS(SELECT 1 FROM pg_default_acl d JOIN pg_namespace n ON n.oid=d.defaclnamespace CROSS JOIN LATERAL aclexplode(d.defaclacl) a JOIN pg_roles r ON r.oid=a.grantee WHERE d.defaclrole=(SELECT oid FROM pg_roles WHERE rolname=current_user) AND n.nspname='public' AND d.defaclobjtype='f' AND r.rolname=$1 AND a.privilege_type='EXECUTE')").bind(&default_role).fetch_one(&pool).await.expect("unrelated named defaults retained");
    assert!(dummy_acl && default_acl);
    let after: String = sqlx::query_scalar(
        "SELECT row_to_json(v)::text FROM catalogue_variants v WHERE sku='SET-FILLED'",
    )
    .fetch_one(&pool)
    .await
    .expect("upgraded variant");
    assert_eq!(before, after);
    assert_eq!(public(&pool).await, Some(original_product));
    fixture_ddl(&pool, format!("DROP FUNCTION public.preview_unrelated_default(); ALTER DEFAULT PRIVILEGES IN SCHEMA public REVOKE EXECUTE ON FUNCTIONS FROM {quoted_role}; DROP OWNED BY {quoted_role}; DROP ROLE {quoted_role}")).await;
}
