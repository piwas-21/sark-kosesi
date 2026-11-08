//! Native source-boundary faults after an otherwise valid Rust checkout.
use super::*;
use serde_json::{Value, json};

fn set(value: &mut Value, path: &str, replacement: Value) {
    *value.pointer_mut(path).expect("synthetic quote field") = replacement;
}
fn amounts(gross: i64, net: i64, vat: i64) -> Value {
    json!({"gross_minor": gross, "net_minor": net, "vat_minor": vat})
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn coherent_forged_original_price_vat_and_shipping_cannot_authorize_debit(pool: PgPool) {
    let mut policy = promotion::policy();
    policy.promotion.discount =
        commerce_domain::promotions::DiscountKind::Percentage { basis_points: 1000 };
    policy.promotion.maximum_discount_minor = 5000;
    seed(&pool, &policy).await;
    let (store, _) = fixture::store(&pool, 1100);
    let (session, csrf) = fixture::guest(&store).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    let quote = issue_promoted_quote(
        &store,
        &key,
        &csrf,
        Locale::Nl,
        &fixture::request("quote"),
        "COURTYARD",
    )
    .await
    .expect("approved quote")
    .snapshot;
    assert_eq!(
        (
            quote.original.lines[0].unit_gross_minor,
            quote.original.lines[0].quantity,
            quote.discount_minor
        ),
        (12345, 3, 3704)
    );
    let original = serde_json::to_value(&quote).expect("quote JSON");
    let mut scenarios = Vec::new();
    let mut price = original.clone();
    set(
        &mut price,
        "/original/lines/0/unit_gross_minor",
        json!(12355),
    );
    set(
        &mut price,
        "/original/lines/0/amounts",
        amounts(37065, 30632, 6433),
    );
    set(&mut price, "/original/total", amounts(38275, 31632, 6643));
    set(&mut price, "/discount_minor", json!(3707));
    set(&mut price, "/lines/0/discount_minor", json!(3707));
    set(&mut price, "/lines/0/amounts", amounts(33358, 27569, 5789));
    set(&mut price, "/total", amounts(34568, 28569, 5999));
    scenarios.push(("catalogue price", price));
    let mut vat = original.clone();
    set(&mut vat, "/original/lines/0/vat_basis_points", json!(900));
    set(
        &mut vat,
        "/original/lines/0/amounts",
        amounts(37035, 33977, 3058),
    );
    set(&mut vat, "/original/total", amounts(38245, 34977, 3268));
    set(&mut vat, "/lines/0/amounts", amounts(33331, 30579, 2752));
    set(&mut vat, "/total", amounts(34541, 31579, 2962));
    scenarios.push(("product VAT", vat));
    let mut shipping = original.clone();
    set(&mut shipping, "/original/shipping", amounts(1209, 999, 210));
    set(&mut shipping, "/shipping", amounts(1209, 999, 210));
    set(
        &mut shipping,
        "/original/total",
        amounts(38244, 31606, 6638),
    );
    set(&mut shipping, "/total", amounts(34540, 28545, 5995));
    scenarios.push(("shipping gross", shipping));
    let mut shipping_vat = original.clone();
    set(
        &mut shipping_vat,
        "/original/shipping_vat_basis_points",
        json!(900),
    );
    set(
        &mut shipping_vat,
        "/original/shipping",
        amounts(1210, 1110, 100),
    );
    set(&mut shipping_vat, "/shipping", amounts(1210, 1110, 100));
    set(
        &mut shipping_vat,
        "/original/total",
        amounts(38245, 31717, 6528),
    );
    set(&mut shipping_vat, "/total", amounts(34541, 28656, 5885));
    scenarios.push(("shipping VAT", shipping_vat));
    let mut total = original.clone();
    set(&mut total, "/original/total", amounts(38245, 31608, 6637));
    scenarios.push(("original total", total));
    // A nested native insert is caught only if the precise source guard fires.
    // The sequence is deliberately non-transactional, making the assertion
    // survive rollback without accepting any order or ledger effects.
    pool.execute("ALTER TABLE guest_quote_snapshots DISABLE TRIGGER guard_guest_quote; ALTER TABLE checkout_orders DISABLE TRIGGER guard_checkout_order; CREATE TABLE source_fault(snapshot jsonb NOT NULL); CREATE SEQUENCE source_guard_observed; CREATE FUNCTION forge_order_source() RETURNS trigger LANGUAGE plpgsql AS $$ DECLARE v jsonb; BEGIN IF pg_trigger_depth()>1 THEN RETURN NEW; END IF; SELECT snapshot INTO v FROM source_fault; UPDATE guest_quote_snapshots SET snapshot=v WHERE id=NEW.quote_id; UPDATE checkout_orders SET snapshot=jsonb_set(snapshot,'{quote}',v) WHERE id=NEW.order_id; NEW.discount_minor=(v->>'discount_minor')::bigint; BEGIN INSERT INTO promotion_redemptions(order_id,quote_id,promotion_id,promotion_revision,promotion_review_revision,discount_minor,created_at) VALUES(NEW.order_id,NEW.quote_id,NEW.promotion_id,NEW.promotion_revision,NEW.promotion_review_revision,NEW.discount_minor,NEW.created_at); RAISE EXCEPTION 'forged source was accepted'; EXCEPTION WHEN check_violation THEN IF SQLERRM <> 'Redemption requires exact original quote source' THEN RAISE; END IF; PERFORM nextval('source_guard_observed'); END; RETURN NULL; END; $$; CREATE TRIGGER aaa_forge_order_source BEFORE INSERT ON promotion_redemptions FOR EACH ROW EXECUTE FUNCTION forge_order_source()").await.expect("isolated bypass and precise native guard probe");
    let intent = order::intent(&quote.original.binding.quote_id);
    for (index, (name, forged)) in scenarios.into_iter().enumerate() {
        pool.execute("DELETE FROM source_fault")
            .await
            .expect("reset fault");
        sqlx::query("INSERT INTO source_fault VALUES($1)")
            .bind(forged)
            .execute(&pool)
            .await
            .expect("configure fault");
        assert!(
            create_order(&store, &key, &csrf, "place", &intent, &order::seller())
                .await
                .is_err(),
            "forged {name} must fail"
        );
        let (observed, called): (i64, bool) =
            sqlx::query_as("SELECT last_value,is_called FROM source_guard_observed")
                .fetch_one(&pool)
                .await
                .expect("native rejection probe");
        assert!(
            called && observed == i64::try_from(index + 1).expect("small index"),
            "native original-source guard specifically rejected {name}"
        );
        assert_eq!(effects(&pool).await, (0, 0, 0, 0, 0, 0, 0));
    }
    pool.execute("DROP TRIGGER aaa_forge_order_source ON promotion_redemptions; ALTER TABLE guest_quote_snapshots ENABLE TRIGGER guard_guest_quote; ALTER TABLE checkout_orders ENABLE TRIGGER guard_checkout_order").await.expect("restore native history guards");
    let accepted = create_order(&store, &key, &csrf, "place", &intent, &order::seller())
        .await
        .expect("exact approved source succeeds");
    assert!(accepted.snapshot.promoted_quote.as_ref() == Some(&quote));
    assert_eq!(effects(&pool).await, (1, 1, 1, 1, 2, 1, 12));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn original_source_rejects_malformed_and_out_of_range_revisions_without_cast_errors(
    pool: PgPool,
) {
    seed(&pool, &promotion::policy()).await;
    let (store, _) = fixture::store(&pool, 1100);
    let (session, csrf) = fixture::guest(&store).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    let quote = issue_promoted_quote(
        &store,
        &key,
        &csrf,
        Locale::Nl,
        &fixture::request("revision-boundary"),
        "COURTYARD",
    )
    .await
    .expect("approved quote")
    .snapshot;
    let id = &quote.original.binding.quote_id;
    let valid: bool = sqlx::query_scalar(
        "SELECT promoted_redemption_original_source(q) FROM guest_quote_snapshots q WHERE id=$1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .expect("valid original source");
    assert!(valid);
    for (value, expected) in [
        (json!(null), false),
        (json!("1"), false),
        (json!(0), false),
        (json!(-1), false),
        (json!(1.5), false),
        (json!(2147483648_i64), false),
        (json!(2147483647_i64), false), // in range, but not a retained revision
        (json!(1.0), true),             // jsonb numeric equality preserves the existing semantics
    ] {
        for source in ["publication", "price"] {
            let mut tx = pool.begin().await.expect("isolated fault transaction");
            if source == "publication" {
                sqlx::query("ALTER TABLE guest_quote_snapshots DISABLE TRIGGER guard_guest_quote")
                    .execute(&mut *tx)
                    .await
                    .expect("isolate quote fault");
                sqlx::query("UPDATE guest_quote_snapshots SET snapshot=jsonb_set(snapshot,'{original,lines,0,publication_revision}',$1) WHERE id=$2")
                    .bind(&value).bind(id).execute(&mut *tx).await.expect("quote revision fault");
            } else {
                sqlx::query("ALTER TABLE catalogue_publication_revisions DISABLE TRIGGER guard_publication_history")
                    .execute(&mut *tx).await.expect("isolate publication fault");
                sqlx::query("UPDATE catalogue_publication_revisions SET choice=jsonb_set(choice,'{selection,facts,price_revision}',$1) WHERE sku='SET-FILLED' AND locale='nl' AND revision=1")
                    .bind(&value).execute(&mut *tx).await.expect("price revision fault");
            }
            let result: bool = sqlx::query_scalar("SELECT promoted_redemption_original_source(q) FROM guest_quote_snapshots q WHERE id=$1")
                .bind(id).fetch_one(&mut *tx).await.expect("invalid value fails closed without cast error");
            assert_eq!(result, expected, "{source} revision {value}");
            tx.rollback().await.expect("restore exact source");
        }
    }
    assert_eq!(effects(&pool).await, (0, 0, 0, 0, 0, 0, 0));
    create_order(
        &store,
        &key,
        &csrf,
        "place",
        &order::intent(id),
        &order::seller(),
    )
    .await
    .expect("unchanged source remains redeemable");
}
