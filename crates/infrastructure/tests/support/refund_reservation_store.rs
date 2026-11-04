//! PostgreSQL store reads against authentic and deliberately damaged custody rows.
use super::*;
use crate::{
    MIGRATOR,
    mollie::tests::order_stock_cases::fixture::accepted,
    refund_reservations::{RefundLine, RefundRequest, reserve},
    staff_access::SessionToken,
};
use sqlx::{Executor, PgPool};

#[sqlx::test(migrator = "MIGRATOR")]
async fn stored_receipts_fail_closed_on_damage_and_incomplete_lists(pool: PgPool) {
    let fixture = accepted(&pool, false, true).await;
    let order = &fixture.order.identity.id;
    let recognition: String = sqlx::query_scalar(
        "SELECT recognition_id FROM payment_recognition_current WHERE order_id=$1",
    )
    .bind(order)
    .fetch_one(&pool)
    .await
    .expect("received proof");
    let token = SessionToken::from_bytes([94; 32]);
    pool.execute("INSERT INTO staff_accounts(id,enabled) VALUES('store-finance',true)")
        .await
        .expect("staff");
    pool.execute("INSERT INTO staff_capabilities VALUES('store-finance','refund.prepare')")
        .await
        .expect("grant");
    sqlx::query("INSERT INTO staff_sessions VALUES($1,'store-finance',1000,4000,1400,NULL)")
        .bind(token.digest().as_slice())
        .execute(&pool)
        .await
        .expect("session");
    let lines = [
        RefundLine {
            sku: "SET-COVERS",
            gross_minor: 1000,
        },
        RefundLine {
            sku: "SET-FILLED",
            gross_minor: 0,
        },
    ];
    let original = reserve(
        &pool,
        &token,
        fixture.time.as_ref(),
        fixture.ids.as_ref(),
        RefundRequest {
            order_id: order,
            expected_recognition_id: &recognition,
            effect_key: "store-original",
            lines: &lines,
            shipping_gross_minor: 0,
            reason: "Synthetic store test",
            confirmed: true,
        },
    )
    .await
    .expect("authentic receipt");

    let mut tx = pool.begin().await.expect("isolated transaction");
    assert_eq!(require_exact_history(&mut tx, order).await, Ok(()));
    assert_eq!(
        require_exact_history(&mut tx, "00000000000000000000000000000000").await,
        Err(RefundError::InvalidStoredData)
    );
    assert_eq!(lookup(&mut tx, "absent").await, Ok(None));
    assert_eq!(
        lookup(&mut tx, "store-original").await,
        Ok(Some((order.clone(), original.attempt_id.clone())))
    );
    let actual = history(&mut tx, order).await.expect("stored receipt");
    assert_eq!(actual.len(), 1);
    assert_eq!(actual[0], original);
    assert_eq!(
        take_receipt(actual, "missing"),
        Err(RefundError::InvalidStoredData)
    );

    // Shadow only this transaction's table. The original receipt was written and
    // native-proved above; the copy permits simulating damaged imports without
    // disabling any production constraint or trigger.
    tx.execute("CREATE TEMP TABLE refund_reservations ON COMMIT DROP AS SELECT * FROM public.refund_reservations")
        .await.expect("isolated receipt copy");
    tx.execute("SET LOCAL search_path TO pg_temp, public")
        .await
        .expect("bind test reads to the isolated copy");
    let mutations = [
        "UPDATE refund_reservations SET id='bad'",
        "UPDATE refund_reservations SET order_id='bad'",
        "UPDATE refund_reservations SET attempt_id='bad'",
        "UPDATE refund_reservations SET recognition_id='bad'",
        "UPDATE refund_reservations SET actor_id=''",
        "UPDATE refund_reservations SET effect_key=''",
        "UPDATE refund_reservations SET generation=0",
        "UPDATE refund_reservations SET generation=17",
        "UPDATE refund_reservations SET reason=''",
        "UPDATE refund_reservations SET reason=repeat('x',241)",
        "UPDATE refund_reservations SET reason=' leading'",
        "UPDATE refund_reservations SET reason=concat('line',chr(10),'control')",
        "UPDATE refund_reservations SET allocation=NULL",
        "UPDATE refund_reservations SET allocation='{}'::jsonb",
        "UPDATE refund_reservations SET allocation=jsonb_set(allocation,'{schema_version}','2')",
        "UPDATE refund_reservations SET allocation=jsonb_set(allocation,'{lines}','[]')",
        "UPDATE refund_reservations SET allocation=jsonb_set(allocation,'{lines}',(SELECT jsonb_agg(x.value) FROM jsonb_array_elements(allocation->'lines') AS x(value) CROSS JOIN generate_series(1,21)))",
        "UPDATE refund_reservations SET allocation=allocation||jsonb_build_object('padding',repeat('x',17000))",
    ];
    for mutation in mutations {
        tx.execute("DELETE FROM refund_reservations")
            .await
            .expect("clear copy");
        tx.execute("INSERT INTO refund_reservations SELECT * FROM public.refund_reservations")
            .await
            .expect("restore copy");
        sqlx::query(mutation)
            .execute(&mut *tx)
            .await
            .expect("damage copy");
        // A changed order id no longer matches the original filter.
        let selected_order = if mutation == "UPDATE refund_reservations SET order_id='bad'" {
            "bad"
        } else {
            order
        };
        assert_eq!(
            history(&mut tx, selected_order).await,
            Err(RefundError::InvalidStoredData),
            "{mutation}"
        );
    }
    tx.execute("DELETE FROM refund_reservations")
        .await
        .expect("clear copy");
    tx.execute("INSERT INTO refund_reservations SELECT * FROM public.refund_reservations")
        .await
        .expect("restore copy");
    tx.execute("UPDATE refund_reservations SET attempt_id='bad'")
        .await
        .expect("damage identity");
    assert_eq!(
        lookup(&mut tx, "store-original").await,
        Err(RefundError::InvalidStoredData)
    );
    tx.execute("UPDATE refund_reservations SET order_id='bad', attempt_id=repeat('a',32)")
        .await
        .expect("damage order identity");
    assert_eq!(
        lookup(&mut tx, "store-original").await,
        Err(RefundError::InvalidStoredData)
    );
    tx.execute("DELETE FROM refund_reservations")
        .await
        .expect("clear copy");
    tx.execute("INSERT INTO refund_reservations SELECT p.* FROM public.refund_reservations p CROSS JOIN generate_series(1,17)").await.expect("overfull list");
    assert_eq!(
        history(&mut tx, order).await,
        Err(RefundError::InvalidStoredData)
    );
    tx.rollback().await.expect("discard damaged copy");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn suppressed_reservation_or_event_fails_without_partial_commit_and_same_key_recovers(
    pool: PgPool,
) {
    let fixture = accepted(&pool, false, true).await;
    let order = &fixture.order.identity.id;
    let recognition: String = sqlx::query_scalar(
        "SELECT recognition_id FROM payment_recognition_current WHERE order_id=$1",
    )
    .bind(order)
    .fetch_one(&pool)
    .await
    .expect("received proof");
    let token = SessionToken::from_bytes([94; 32]);
    pool.execute("INSERT INTO staff_accounts(id,enabled) VALUES('store-finance',true)")
        .await
        .expect("staff");
    pool.execute("INSERT INTO staff_capabilities VALUES('store-finance','refund.prepare')")
        .await
        .expect("grant");
    sqlx::query("INSERT INTO staff_sessions VALUES($1,'store-finance',1000,4000,1400,NULL)")
        .bind(token.digest().as_slice())
        .execute(&pool)
        .await
        .expect("session");
    let lines = [
        RefundLine {
            sku: "SET-COVERS",
            gross_minor: 1000,
        },
        RefundLine {
            sku: "SET-FILLED",
            gross_minor: 0,
        },
    ];
    let request = || RefundRequest {
        order_id: order,
        expected_recognition_id: &recognition,
        effect_key: "store-suppressed",
        lines: &lines,
        shipping_gross_minor: 0,
        reason: "Synthetic store test",
        confirmed: true,
    };
    pool.execute("CREATE FUNCTION test_suppress_refund_insert() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NULL; END; $$")
        .await.expect("test-only suppressor in isolated database");
    pool.execute("CREATE TRIGGER aaa_test_suppress_refund BEFORE INSERT ON refund_reservations FOR EACH ROW EXECUTE FUNCTION test_suppress_refund_insert()")
        .await.expect("suppress reservation insert");
    assert_eq!(
        reserve(
            &pool,
            &token,
            fixture.time.as_ref(),
            fixture.ids.as_ref(),
            request()
        )
        .await,
        Err(RefundError::InvalidStoredData)
    );
    let counts = || {
        sqlx::query_as::<_, (i64, i64)>(
            "SELECT (SELECT count(*) FROM refund_reservations WHERE effect_key='store-suppressed'), (SELECT count(*) FROM commerce_outbox WHERE kind='order.refund_reserved.v1')",
        )
    };
    assert_eq!(
        counts()
            .fetch_one(&pool)
            .await
            .expect("no partial receipt or event"),
        (0, 0)
    );
    pool.execute("DROP TRIGGER aaa_test_suppress_refund ON refund_reservations")
        .await
        .expect("restore reservation writes");
    pool.execute("CREATE TRIGGER aaa_test_suppress_refund BEFORE INSERT ON commerce_outbox FOR EACH ROW EXECUTE FUNCTION test_suppress_refund_insert()")
        .await.expect("suppress reciprocal event");
    assert_eq!(
        reserve(
            &pool,
            &token,
            fixture.time.as_ref(),
            fixture.ids.as_ref(),
            request()
        )
        .await,
        Err(RefundError::InvalidStoredData)
    );
    assert_eq!(
        counts()
            .fetch_one(&pool)
            .await
            .expect("reservation rolled back with event"),
        (0, 0)
    );
    pool.execute("DROP TRIGGER aaa_test_suppress_refund ON commerce_outbox")
        .await
        .expect("restore event writes");
    let receipt = reserve(
        &pool,
        &token,
        fixture.time.as_ref(),
        fixture.ids.as_ref(),
        request(),
    )
    .await
    .expect("same key succeeds after both failures");
    assert_eq!(receipt.effect_key, "store-suppressed");
    assert!(!receipt.replayed);
    assert_eq!(
        counts()
            .fetch_one(&pool)
            .await
            .expect("one receipt and event"),
        (1, 1)
    );
}
