use commerce_application::Locale;
use commerce_infrastructure::{MIGRATOR, catalogue, readiness};
use sqlx::{Executor, PgPool};

#[sqlx::test(migrator = "MIGRATOR")]
async fn empty_migration_and_seeded_catalogue(pool: PgPool) {
    readiness(&pool).await.expect("current schema ready");
    assert!(
        catalogue(&pool, Locale::Nl)
            .await
            .expect("empty")
            .is_empty()
    );
    pool.execute(include_str!("../../../tests/fixtures/catalogue.sql"))
        .await
        .expect("isolated fixture");
    for locale in [
        Locale::Nl,
        Locale::De,
        Locale::Fr,
        Locale::En,
        Locale::Ar,
        Locale::Tr,
        Locale::Kmr,
    ] {
        let items = catalogue(&pool, locale).await.expect("query");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].sku, "DEMO-COURTYARD-COVERS");
        assert_eq!(items[0].price.minor_units(), 12345);
        assert_eq!(items[0].fill, "covers-only");
        assert!(items[0].fixture);
        assert!(!items[0].care.is_empty());
    }
}

#[sqlx::test(migrations = false)]
async fn prior_schema_upgrade_preserves_old_reader_and_data(pool: PgPool) {
    MIGRATOR.run_to(1, &pool).await.expect("previous version");
    pool.execute("INSERT INTO catalogue_items VALUES ('DEMO-OLD','old-demo',77,'EUR','with-foam',true); INSERT INTO catalogue_translations VALUES ('DEMO-OLD','nl','Old fixture','Preserve me');").await.expect("prior data");
    assert!(readiness(&pool).await.is_err());
    MIGRATOR.run(&pool).await.expect("expand migration");
    let old: (String,) =
        sqlx::query_as("SELECT description FROM catalogue_translations WHERE sku='DEMO-OLD'")
            .fetch_one(&pool)
            .await
            .expect("old reader");
    assert_eq!(old.0, "Preserve me");
    let items = catalogue(&pool, Locale::Nl).await.expect("new reader");
    assert_eq!(items[0].price.minor_units(), 77);
    assert_eq!(items[0].care, "");
    MIGRATOR.run(&pool).await.expect("idempotent runner");
    assert_eq!(
        catalogue(&pool, Locale::Nl).await.expect("preserved").len(),
        1
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn invalid_money_fill_and_nonfixture_publication_are_rejected(pool: PgPool) {
    for sql in [
        "INSERT INTO catalogue_items VALUES ('DEMO-NEGATIVE','negative',-1,'EUR','covers-only',true)",
        "INSERT INTO catalogue_items VALUES ('DEMO-CURRENCY','currency',100,'USD','covers-only',true)",
        "INSERT INTO catalogue_items VALUES ('DEMO-FILL','fill',100,'EUR','mystery',true)",
        "INSERT INTO catalogue_items VALUES ('DEMO-PUBLISH','publish',100,'EUR','covers-only',false)",
    ] {
        assert!(pool.execute(sql).await.is_err(), "invalid row must fail");
    }
    pool.execute(include_str!("../../../tests/fixtures/catalogue.sql"))
        .await
        .expect("positive control");
    assert!(
        pool.execute(include_str!("../../../tests/fixtures/catalogue.sql"))
            .await
            .is_err(),
        "duplicate SKU"
    );
    pool.execute("DELETE FROM catalogue_translations WHERE locale='ar'")
        .await
        .expect("draft missing");
    assert!(
        catalogue(&pool, Locale::Ar)
            .await
            .expect("no fallback")
            .is_empty()
    );
    assert_eq!(
        catalogue(&pool, Locale::Nl)
            .await
            .expect("other unchanged")
            .len(),
        1
    );
    pool.close().await;
    assert!(readiness(&pool).await.is_err());
    assert!(catalogue(&pool, Locale::Nl).await.is_err());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn corrupt_storage_and_lock_timeout_are_not_published(pool: PgPool) {
    pool.execute(include_str!("../../../tests/fixtures/catalogue.sql"))
        .await
        .expect("seed");
    let mut transaction = pool.begin().await.expect("transaction");
    sqlx::query("LOCK TABLE catalogue_items IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *transaction)
        .await
        .expect("hold lock");
    assert!(
        readiness(&pool).await.is_err(),
        "bounded dependency timeout"
    );
    transaction.rollback().await.expect("release lock");
    readiness(&pool).await.expect("recovered");
    pool.execute("ALTER TABLE catalogue_items DROP CONSTRAINT catalogue_items_minor_units_check; UPDATE catalogue_items SET minor_units=-1").await.expect("hostile storage fixture");
    assert!(
        catalogue(&pool, Locale::Nl).await.is_err(),
        "domain money rejects corrupt storage"
    );
    assert!(
        commerce_infrastructure::connect("postgresql://p1_test@127.0.0.1:1/unavailable")
            .await
            .is_err()
    );
}
