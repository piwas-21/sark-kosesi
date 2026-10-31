//! Migration/relational guards, independently exercised through the actual writer.
use commerce_application::Locale;
use commerce_infrastructure::{MIGRATOR, checkout_orders::*, checkout_quotes::*, guest_baskets::*};
use sqlx::{Executor, PgPool};
#[path = "support/checkout_order_fixture.rs"]
mod fixture;
#[allow(dead_code)]
#[path = "support/checkout_quote_fixture.rs"]
mod quotes;
#[sqlx::test(migrations = false)]
async fn upgrade_keeps_approved_quotes_stock_and_existing_authority_without_seeding_orders(
    pool: PgPool,
) {
    MIGRATOR.run_to(21, &pool).await.expect("previous schema");
    quotes::seed(&pool).await;
    let (store, _) = quotes::store(&pool, 1100);
    let (s, c) = quotes::guest(&store).await;
    let k = GuestCredential::from_session(&s).expect("owner");
    let q = issue_quote(&store, &k, &c, Locale::Nl, &quotes::request("q"))
        .await
        .expect("prior reader/writer")
        .snapshot;
    let grants: Vec<(String, String)> = sqlx::query_as(
        "SELECT staff_id,capability FROM staff_capabilities ORDER BY staff_id,capability",
    )
    .fetch_all(&pool)
    .await
    .expect("grants");
    MIGRATOR.run(&pool).await.expect("upgrade");
    MIGRATOR.run(&pool).await.expect("idempotent runner");
    assert!(
        owned_quote(&store, &k, &q.binding.quote_id)
            .await
            .expect("preserved quote")
            == q
    );
    let after: Vec<(String, String)> = sqlx::query_as(
        "SELECT staff_id,capability FROM staff_capabilities ORDER BY staff_id,capability",
    )
    .fetch_all(&pool)
    .await
    .expect("grants");
    assert_eq!(after, grants);
    let counts:(i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM checkout_orders),(SELECT count(*) FROM commerce_outbox),(SELECT sum(reserved)::bigint FROM stock_balances)").fetch_one(&pool).await.expect("empty");
    assert_eq!(counts, (0, 0, 0));
    assert!(
        create_order(
            &store,
            &k,
            &c,
            "place",
            &fixture::intent(&q.binding.quote_id),
            &fixture::seller()
        )
        .await
        .is_ok()
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn sql_guards_reject_missing_or_forged_owned_quote_intent_and_manifest_bindings(
    pool: PgPool,
) {
    quotes::seed(&pool).await;
    let (store, _) = quotes::store(&pool, 1100);
    let (s, c) = quotes::guest(&store).await;
    let k = GuestCredential::from_session(&s).expect("owner");
    let q = issue_quote(&store, &k, &c, Locale::Nl, &quotes::request("q"))
        .await
        .expect("quote")
        .snapshot;
    let i = fixture::intent(&q.binding.quote_id);
    pool.execute("CREATE TABLE order_fault_fixture(mode text);INSERT INTO order_fault_fixture VALUES('schema');CREATE FUNCTION fault_new_order() RETURNS trigger LANGUAGE plpgsql AS $$ DECLARE mode text;BEGIN SELECT f.mode INTO mode FROM order_fault_fixture f;CASE mode WHEN 'schema' THEN NEW.snapshot=jsonb_set(NEW.snapshot,'{schema_version}','2');WHEN 'identity' THEN NEW.snapshot=jsonb_set(NEW.snapshot,'{identity,id}','\"99999999999999999999999999999999\"');WHEN 'quote' THEN NEW.snapshot=jsonb_set(NEW.snapshot,'{quote,currency}','\"USD\"');WHEN 'market' THEN NEW.snapshot=jsonb_set(NEW.snapshot,'{market,revision}','2');WHEN 'intentquote' THEN NEW.intent=jsonb_set(NEW.intent,'{quote_id}','\"99999999999999999999999999999999\"');WHEN 'intentversion' THEN NEW.intent=jsonb_set(NEW.intent,'{expected_basket_revision}','2');WHEN 'confirm' THEN NEW.intent=jsonb_set(NEW.intent,'{confirmed}','false');WHEN 'buyer' THEN NEW.intent=jsonb_set(NEW.intent,'{buyer,name}','\"Different\"');WHEN 'shipping' THEN NEW.intent=jsonb_set(NEW.intent,'{shipping_address,recipient}','\"Different\"');WHEN 'billing' THEN NEW.intent=jsonb_set(NEW.intent,'{billing_address,recipient}','\"Different\"');WHEN 'terms' THEN NEW.intent=jsonb_set(NEW.intent,'{accepted_terms_version}','\"different\"');WHEN 'products' THEN NEW.snapshot=NEW.snapshot-'products';WHEN 'productsource' THEN NEW.snapshot=jsonb_set(NEW.snapshot,'{products,0,bom_revision}','2');WHEN 'group' THEN NEW.stock_group_id='99999999999999999999999999999999';NEW.snapshot=jsonb_set(NEW.snapshot,'{identity,stock_group_id}',to_jsonb(NEW.stock_group_id));END CASE;RETURN NEW;END;$$;CREATE TRIGGER aaa_order_fault BEFORE INSERT ON checkout_orders FOR EACH ROW EXECUTE FUNCTION fault_new_order()").await.expect("isolated relational fault instrument");
    for mode in [
        "schema",
        "identity",
        "quote",
        "market",
        "intentquote",
        "intentversion",
        "confirm",
        "buyer",
        "shipping",
        "billing",
        "terms",
        "products",
        "productsource",
        "group",
    ] {
        sqlx::query("UPDATE order_fault_fixture SET mode=$1")
            .bind(mode)
            .execute(&pool)
            .await
            .expect("fault selection");
        assert!(
            matches!(
                create_order(&store, &k, &c, "place", &i, &fixture::seller()).await,
                Err(OrderStoreError::Database)
            ),
            "{mode}"
        );
        let effects:(i64,i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM checkout_orders),(SELECT count(*) FROM commerce_outbox),(SELECT count(*) FROM stock_holds),(SELECT count(*) FROM stock_movements)").fetch_one(&pool).await.expect("atomic rejection");
        assert_eq!(effects, (0, 0, 0, 0), "{mode}");
    }
    pool.execute("DROP TRIGGER aaa_order_fault ON checkout_orders;CREATE FUNCTION omit_order_event() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NULL;END;$$;CREATE TRIGGER omit_order_event BEFORE INSERT ON commerce_outbox FOR EACH ROW EXECUTE FUNCTION omit_order_event()").await.expect("missing event instrument");
    assert!(matches!(
        create_order(&store, &k, &c, "place", &i, &fixture::seller()).await,
        Err(OrderStoreError::Database)
    ));
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM checkout_orders")
        .fetch_one(&pool)
        .await
        .expect("deferred complete-order guard");
    assert_eq!(n, 0);
    pool.execute("DROP TRIGGER omit_order_event ON commerce_outbox")
        .await
        .expect("restore");
    assert!(
        create_order(&store, &k, &c, "place", &i, &fixture::seller())
            .await
            .is_ok()
    );
}
