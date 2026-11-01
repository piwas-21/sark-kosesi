//! Deliberately damaged disposable fixtures exercise readers independently of guards.
use super::*;
async fn damage(pool: &PgPool, sql: &str) {
    let mut tx = pool.begin().await.expect("owned corruption fixture");
    tx.execute("ALTER TABLE payment_recognitions DISABLE TRIGGER USER; ALTER TABLE payment_recognition_current DISABLE TRIGGER USER; ALTER TABLE commerce_outbox DISABLE TRIGGER USER; ALTER TABLE payment_observations DISABLE TRIGGER USER; ALTER TABLE payment_identity_resolutions DISABLE TRIGGER USER; ALTER TABLE payment_provider_identity_claims DISABLE TRIGGER USER").await.expect("fixture guards only");
    // Only fixed fixture SQL and server-generated validated hex IDs reach this helper.
    sqlx::raw_sql(sqlx::AssertSqlSafe(sql.to_owned()))
        .execute(&mut *tx)
        .await
        .expect("controlled damage or repair");
    tx.execute("SET CONSTRAINTS ALL IMMEDIATE")
        .await
        .expect("flush genuine deferred foreign-key checks before restoring fixture guards");
    tx.execute("ALTER TABLE payment_recognitions ENABLE TRIGGER USER; ALTER TABLE payment_recognition_current ENABLE TRIGGER USER; ALTER TABLE commerce_outbox ENABLE TRIGGER USER; ALTER TABLE payment_observations ENABLE TRIGGER USER; ALTER TABLE payment_identity_resolutions ENABLE TRIGGER USER; ALTER TABLE payment_provider_identity_claims ENABLE TRIGGER USER").await.expect("restore guards");
    tx.commit()
        .await
        .expect("owned damage committed for public readers");
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn recognition_public_readers_revalidate_original_resolution_source_events_and_all_evidence(
    pool: PgPool,
) {
    let (time, f) = prepared(&pool, true).await;
    let a = &f.snapshot().binding.id;
    let i = ids();
    record_verified(
        &pool,
        time.as_ref(),
        &i,
        a,
        "paid",
        &fetched(f.snapshot(), "tr_A", "paid", false).await,
    )
    .await
    .expect("paid");
    resolve(&pool, time.as_ref(), &i, a, "original")
        .await
        .expect("original resolution");
    let original = recognize(&pool, time.as_ref(), &i, a)
        .await
        .expect("valid receipt")
        .recognition;
    let duplicate = resolve(&pool, time.as_ref(), &i, a, "duplicate-same-prefix")
        .await
        .expect("second equally valid resolution")
        .resolution;
    assert_ne!(original.resolution_id, duplicate.id);
    // Save complete original rows in this isolated DB; restore after every fault.
    pool.execute("CREATE TABLE recognition_test_events AS TABLE commerce_outbox; CREATE TABLE recognition_test_rows AS TABLE payment_recognitions; CREATE TABLE recognition_test_observations AS TABLE payment_observations; CREATE TABLE recognition_test_resolutions AS TABLE payment_identity_resolutions; CREATE TABLE recognition_test_claims AS TABLE payment_provider_identity_claims").await.expect("test-owned repair source");
    let repairs = "UPDATE payment_recognitions r SET (order_id,attempt_id,resolution_id,through_observation,recorded_at,state,currency,amount_minor)=(SELECT b.order_id,b.attempt_id,b.resolution_id,b.through_observation,b.recorded_at,b.state,b.currency,b.amount_minor FROM recognition_test_rows b WHERE b.id=r.id); DELETE FROM commerce_outbox; INSERT INTO commerce_outbox SELECT * FROM recognition_test_events; UPDATE payment_observations o SET (revision,recorded_at,snapshot)=(SELECT b.revision,b.recorded_at,b.snapshot FROM recognition_test_observations b WHERE b.id=o.id); UPDATE payment_identity_resolutions r SET (through_observation,summary,recorded_at)=(SELECT b.through_observation,b.summary,b.recorded_at FROM recognition_test_resolutions b WHERE b.id=r.id); DELETE FROM payment_provider_identity_claims; INSERT INTO payment_provider_identity_claims SELECT * FROM recognition_test_claims";
    for sql in [
        "UPDATE payment_recognitions SET amount_minor=1".to_owned(),
        "UPDATE payment_recognitions SET state='unconfirmed'".to_owned(),
        "UPDATE payment_recognitions SET recorded_at=1499".to_owned(),
        "UPDATE payment_recognitions SET through_observation=0".to_owned(),
        format!("UPDATE payment_recognitions SET resolution_id='{}'",duplicate.id),
        "UPDATE commerce_outbox SET payload=payload-'recognition_sha256' WHERE kind='payment.recognized.v1'".to_owned(),
        "UPDATE commerce_outbox SET payload=payload || '{\"extra\":true}'::jsonb WHERE kind='payment.recognized.v1'".to_owned(),
        "UPDATE payment_identity_resolutions SET summary='{}' WHERE effect_key='original'".to_owned(),
        "UPDATE payment_observations SET snapshot=jsonb_set(snapshot,'{intent,payment,amount_minor}','1')".to_owned(),
        "UPDATE payment_observations SET revision=2, snapshot=jsonb_set(snapshot,'{revision}','2')".to_owned(),
        "DELETE FROM payment_provider_identity_claims".to_owned(),
        "UPDATE payment_provider_identity_claims SET initial_resolution_id=(SELECT id FROM payment_identity_resolutions WHERE effect_key='duplicate-same-prefix')".to_owned(),
        "DELETE FROM commerce_outbox WHERE kind='checkout.order_created.v1'".to_owned(),
        "DELETE FROM commerce_outbox WHERE kind='payment.attempt_prepared.v1'".to_owned(),
        "DELETE FROM commerce_outbox WHERE kind='payment.reconciliation_required.v1'".to_owned(),
        "DELETE FROM commerce_outbox WHERE kind='payment.observation_recorded.v1'".to_owned(),
        "DELETE FROM commerce_outbox WHERE kind='payment.identity_resolved.v1'".to_owned(),
        "DELETE FROM commerce_outbox WHERE kind='payment.recognized.v1'".to_owned(),
    ]{
        damage(&pool,&sql).await;
        if sql == "UPDATE payment_recognitions SET through_observation=0" {
            // Indexed historical lookup cannot find a receipt whose key was moved.
            assert!(read_recorded(&pool,a,1).await.expect("missing original prefix").is_none());
            assert_eq!(read_recorded(&pool,a,0).await.err(),Some(PaymentStoreError::InvalidStoredData));
        } else {
            assert_eq!(read_recorded(&pool,a,1).await.err(),Some(PaymentStoreError::InvalidStoredData),"historical: {sql}");
        }
        assert_eq!(current(&pool,a).await.err(),Some(PaymentStoreError::InvalidStoredData),"current: {sql}");
        assert_eq!(recognize(&pool,&BadPorts,&BadPorts,a).await.err(),Some(PaymentStoreError::InvalidStoredData),"replay: {sql}");
        damage(&pool,repairs).await;
        assert!(read_recorded(&pool,a,1).await.expect("repaired historical")==Some(original.clone()));
        assert!(current(&pool,a).await.expect("repaired current")==Some(original.clone()));
        let replay=recognize(&pool,&BadPorts,&BadPorts,a).await.expect("repaired replay");assert!(replay.replayed&&replay.recognition==original);
    }
    damage(&pool, "DELETE FROM payment_recognition_current").await;
    assert_eq!(
        current(&pool, a).await.err(),
        Some(PaymentStoreError::InvalidStoredData),
        "missing pointer is damage, not absence"
    );
    damage(
        &pool,
        &format!(
            "INSERT INTO payment_recognition_current(order_id,recognition_id) VALUES('{}','{}')",
            original.order_id, original.id
        ),
    )
    .await;
    assert!(current(&pool, a).await.expect("repaired pointer") == Some(original));
}
