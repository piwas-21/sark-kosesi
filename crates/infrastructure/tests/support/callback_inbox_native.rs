//! Native invariants and isolated damaged-import controls retain healthy successors.
use super::*;

#[sqlx::test(migrator = "MIGRATOR")]
async fn actual_same_profile_provider_owner_conflict_preserves_facts_and_does_not_block_the_legitimate_owner(
    pool: PgPool,
) {
    let (clock, owner) = prepared(&pool, true).await;
    let i = ids();
    let other =
        super::super::super::identity_ownership::additional(&pool, clock.clone(), PROFILE).await;
    let baseline = operation(&pool, clock.as_ref(), &i, "tr_NATIVE_OWNER").await;
    let initial_server = Server::start(vec![
        profile(),
        Reply::Json(200, response(owner.snapshot(), "tr_NATIVE_OWNER", "open")),
    ]);
    let initial = client(&initial_server).await;
    fetch_lookup(&pool, clock.as_ref(), &i, &baseline.snapshot().id, &initial)
        .await
        .expect("original owner receipt");
    process_lookup(&pool, clock.as_ref(), &i, &baseline.snapshot().id)
        .await
        .expect("owner observation");
    identities::resolve(
        &pool,
        clock.as_ref(),
        &i,
        &owner.snapshot().binding.id,
        "claim-before-callbacks",
    )
    .await
    .expect("immutable provider ownership claim");
    let conflicting = admit(&pool, clock.as_ref(), &i, PROFILE, "tr_NATIVE_OWNER").await;
    let healthy = admit(&pool, clock.as_ref(), &i, PROFILE, "tr_NATIVE_OWNER").await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, response(other.snapshot(), "tr_NATIVE_OWNER", "open")),
        Reply::Json(200, response(owner.snapshot(), "tr_NATIVE_OWNER", "open")),
    ]);
    let c = client(&server).await;
    let result = run_callback_batch(&pool, clock.as_ref(), &i, &c, 25)
        .await
        .expect("native identity-owner conflict becomes a bounded visible exception");
    assert_eq!(result.processed, 2);
    assert_eq!(result.needs_review, 1);
    assert!(
        read_callback_job(&pool, &conflicting)
            .await
            .expect("bad job")
            .expect("exists")
            .review_reason
            == Some(ReviewReason::CustodyInvalid)
    );
    assert!(
        read_callback_job(&pool, &healthy)
            .await
            .expect("legitimate owner job")
            .expect("exists")
            .settled
    );
    let claimed:String=sqlx::query_scalar("SELECT attempt_id FROM payment_provider_identity_claims WHERE profile_id=$1 AND provider_id='tr_NATIVE_OWNER'").bind(PROFILE).fetch_one(&pool).await.expect("immutable ownership retained");
    assert_eq!(claimed, owner.snapshot().binding.id);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM payment_observations WHERE attempt_id=$1"
        )
        .bind(&other.snapshot().binding.id)
        .fetch_one(&pool)
        .await
        .expect("genuine conflicting metadata fact retained for finance"),
        1
    );
    assert!(
        identities::latest(&pool, &other.snapshot().binding.id)
            .await
            .expect("no false ownership resolution")
            .is_none()
    );
    assert_eq!(server.count(), 3);
    get_only(&server);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn actual_resolution_insert_timeout_remains_a_storage_error_and_recovers_after_lock_release(
    pool: PgPool,
) {
    let (clock, f) = prepared(&pool, true).await;
    let i = Arc::new(ids());
    let op = operation(&pool, clock.as_ref(), i.as_ref(), "tr_RESOLUTION_TIMEOUT").await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, response(f.snapshot(), "tr_RESOLUTION_TIMEOUT", "open")),
    ]);
    let c = client(&server).await;
    fetch_lookup(&pool, clock.as_ref(), i.as_ref(), &op.snapshot().id, &c)
        .await
        .expect("lookup");
    process_lookup(&pool, clock.as_ref(), i.as_ref(), &op.snapshot().id)
        .await
        .expect("authenticated observation");
    let mut blocker = pool
        .begin()
        .await
        .expect("native resolution INSERT blocker");
    blocker
        .execute("LOCK TABLE payment_identity_resolutions IN SHARE MODE")
        .await
        .expect("actual resolution write timeout");
    let (p, t, i2, a) = (
        pool.clone(),
        clock.clone(),
        i.clone(),
        f.snapshot().binding.id.clone(),
    );
    let task = tokio::spawn(async move {
        identities::resolve(
            &p,
            t.as_ref(),
            i2.as_ref(),
            &a,
            "resolution-storage-timeout",
        )
        .await
    });
    waiting_query(&pool, "INSERT INTO payment_identity_resolutions%").await;
    assert_eq!(
        task.await.expect("resolution task").err(),
        Some(PaymentStoreError::Database),
        "transient storage is distinct from deterministic native ownership invalidity"
    );
    blocker
        .commit()
        .await
        .expect("release native insert blocker");
    identities::resolve(
        &pool,
        clock.as_ref(),
        i.as_ref(),
        &f.snapshot().binding.id,
        "resolution-storage-timeout",
    )
    .await
    .expect("healthy recovery after storage becomes available");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn consumed_immutable_generation_claim_cannot_reissue_an_old_lease_after_imported_job_rewind(
    pool: PgPool,
) {
    let clock = time();
    let i = ids();
    let id = admit(&pool, clock.as_ref(), &i, PROFILE, "tr_OLD_LEASE").await;
    let old = store::jobs::claim(&pool, clock.as_ref(), &i, PROFILE)
        .await
        .expect("original claim")
        .expect("pending");
    assert_eq!(old.job.generation, 1);
    let mut tx = pool
        .begin()
        .await
        .expect("isolated old pending checkpoint import");
    tx.execute("ALTER TABLE callback_inbox_jobs DISABLE TRIGGER USER")
        .await
        .expect("fixture bypass");
    sqlx::query("UPDATE callback_inbox_jobs SET generation=0,state='pending',lease_until=NULL,updated_at=created_at,due_at=created_at WHERE intake_id=$1").bind(&id).execute(&mut *tx).await.expect("coherent old pending checkpoint");
    tx.execute("ALTER TABLE callback_inbox_jobs ENABLE TRIGGER USER")
        .await
        .expect("guard restored");
    tx.commit().await.expect("import fixture");
    assert!(sqlx::query("UPDATE callback_inbox_jobs SET generation=1,state='leased',lease_until=1560,updated_at=1500,due_at=1500 WHERE intake_id=$1").bind(&id).execute(&pool).await.is_err(),"native consumed generation rejects old map reuse without inserting another map");
    assert_eq!(
        read_callback_job(&pool, &id).await.err(),
        Some(PaymentStoreError::InvalidStoredData),
        "reader rejects checkpoint behind immutable consumed generation"
    );
    let mut tx = pool.begin().await.expect("exact checkpoint repair");
    tx.execute("ALTER TABLE callback_inbox_jobs DISABLE TRIGGER USER")
        .await
        .expect("fixture repair");
    sqlx::query("UPDATE callback_inbox_jobs SET generation=1,state='leased',lease_until=1560,updated_at=1500,due_at=1500 WHERE intake_id=$1").bind(&id).execute(&mut *tx).await.expect("restore exact original lease");
    tx.execute("ALTER TABLE callback_inbox_jobs ENABLE TRIGGER USER")
        .await
        .expect("guard restored");
    tx.commit().await.expect("repaired fixture");
    assert_eq!(
        read_callback_job(&pool, &id)
            .await
            .expect("healthy immutable claim match")
            .expect("job")
            .generation,
        1
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn deterministic_identity_history_failure_stops_bad_operation_and_continues_an_unrelated_order(
    pool: PgPool,
) {
    let (clock, f) = prepared(&pool, true).await;
    let i = ids();
    let other =
        super::super::super::identity_ownership::additional(&pool, clock.clone(), PROFILE).await;
    let baseline = operation(&pool, clock.as_ref(), &i, "tr_IDENTITY_BASELINE").await;
    let baseline_server = Server::start(vec![
        profile(),
        Reply::Json(200, response(f.snapshot(), "tr_IDENTITY_BASELINE", "open")),
    ]);
    let baseline_client = client(&baseline_server).await;
    fetch_lookup(
        &pool,
        clock.as_ref(),
        &i,
        &baseline.snapshot().id,
        &baseline_client,
    )
    .await
    .expect("baseline authenticated receipt");
    process_lookup(&pool, clock.as_ref(), &i, &baseline.snapshot().id)
        .await
        .expect("baseline observation");
    let resolution = identities::resolve(
        &pool,
        clock.as_ref(),
        &i,
        &f.snapshot().binding.id,
        "baseline-before-import",
    )
    .await
    .expect("healthy exact baseline resolution");
    assert!(
        identities::read_recorded(&pool, &f.snapshot().binding.id, "baseline-before-import")
            .await
            .expect("healthy before damage")
            .is_some()
    );
    let mut tx = pool
        .begin()
        .await
        .expect("isolated damaged identity history");
    tx.execute("ALTER TABLE payment_identity_resolutions DISABLE TRIGGER USER")
        .await
        .expect("fixture bypass");
    sqlx::query("UPDATE payment_identity_resolutions SET summary=jsonb_set(summary,'{paid_provider_ids}','[\"tr_IDENTITY_BASELINE\"]') WHERE id=$1").bind(&resolution.resolution.id).execute(&mut *tx).await.expect("false summary history");
    tx.execute("ALTER TABLE payment_identity_resolutions ENABLE TRIGGER USER")
        .await
        .expect("guard restored");
    tx.commit().await.expect("damaged fixture");
    let bad = admit(&pool, clock.as_ref(), &i, PROFILE, "tr_BAD_IDENTITY").await;
    let good = admit(&pool, clock.as_ref(), &i, PROFILE, "tr_OTHER_ORDER").await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, response(f.snapshot(), "tr_BAD_IDENTITY", "open")),
        Reply::Json(200, response(other.snapshot(), "tr_OTHER_ORDER", "open")),
    ]);
    let c = client(&server).await;
    let result = run_callback_batch(&pool, clock.as_ref(), &i, &c, 25)
        .await
        .expect("invalid identity history is deterministic review, not queue head-of-line failure");
    assert_eq!(result.processed, 2);
    assert_eq!(result.needs_review, 1);
    assert!(
        read_callback_job(&pool, &bad)
            .await
            .expect("bad job")
            .expect("exists")
            .review_reason
            == Some(ReviewReason::CustodyInvalid)
    );
    assert!(
        read_callback_job(&pool, &good)
            .await
            .expect("unrelated good job")
            .expect("exists")
            .settled
    );
    let outcome:(String,Option<String>,Option<String>)=sqlx::query_as("SELECT p.outcome,p.observation_id,p.resolution_id FROM callback_receipt_processing p JOIN callback_inbox_generations g ON g.operation_id=p.operation_id WHERE g.intake_id=$1").bind(&bad).fetch_one(&pool).await.expect("redacted deterministic exception");
    assert_eq!(outcome, ("custody_invalid".into(), None, None));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM payment_observations")
            .fetch_one(&pool)
            .await
            .expect("genuine prior and new observations retained"),
        3
    );
    assert_eq!(server.count(), 3);
    get_only(&server);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn saved_processing_outcomes_never_bless_changed_receipt_observation_or_resolution_proofs(
    pool: PgPool,
) {
    let (clock, f) = prepared(&pool, true).await;
    let i = ids();
    admit(&pool, clock.as_ref(), &i, PROFILE, "tr_DISPOSITION_BINDING").await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(
            200,
            response(f.snapshot(), "tr_DISPOSITION_BINDING", "open"),
        ),
    ]);
    let c = client(&server).await;
    let claim = claimed_receipt(&pool, clock.as_ref(), &i, &c).await;
    store::drain::process_operation(&pool, clock.as_ref(), &i, &claim.operation_id)
        .await
        .expect("healthy linked OPEN processing");
    let receipt: Value = sqlx::query_scalar("SELECT snapshot FROM callback_lookup_receipts")
        .fetch_one(&pool)
        .await
        .expect("exact receipt");
    let observation: Value = sqlx::query_scalar("SELECT snapshot FROM payment_observations")
        .fetch_one(&pool)
        .await
        .expect("exact observation");
    let summary: Value = sqlx::query_scalar("SELECT summary FROM payment_identity_resolutions")
        .fetch_one(&pool)
        .await
        .expect("exact resolution");
    for field in ["outcome", "receipt", "observation", "resolution"] {
        let mut tx = pool.begin().await.expect("isolated valid-looking import");
        match field {
            "outcome" => {
                tx.execute("ALTER TABLE callback_receipt_processing DISABLE TRIGGER USER")
                    .await
                    .expect("fixture bypass");
                tx.execute("UPDATE callback_receipt_processing SET outcome='paid_observed'")
                    .await
                    .expect("false paid classification");
                tx.execute("ALTER TABLE callback_receipt_processing ENABLE TRIGGER USER")
                    .await
                    .expect("guard restored");
            }
            "receipt" => {
                tx.execute("ALTER TABLE callback_lookup_receipts DISABLE TRIGGER USER")
                    .await
                    .expect("fixture bypass");
                tx.execute("UPDATE callback_lookup_receipts SET snapshot=jsonb_set(snapshot,'{payment,status}','\"paid\"')").await.expect("changed otherwise-valid authenticated fact");
                tx.execute("ALTER TABLE callback_lookup_receipts ENABLE TRIGGER USER")
                    .await
                    .expect("guard restored");
            }
            "observation" => {
                tx.execute("ALTER TABLE payment_observations DISABLE TRIGGER USER")
                    .await
                    .expect("fixture bypass");
                tx.execute("UPDATE payment_observations SET status='paid',snapshot=jsonb_set(snapshot,'{intent,payment,status}','\"paid\"')").await.expect("coherent false observation");
                tx.execute("ALTER TABLE payment_observations ENABLE TRIGGER USER")
                    .await
                    .expect("guard restored");
            }
            _ => {
                tx.execute("ALTER TABLE payment_identity_resolutions DISABLE TRIGGER USER")
                    .await
                    .expect("fixture bypass");
                tx.execute("UPDATE payment_identity_resolutions SET summary=jsonb_set(summary,'{paid_provider_ids}','[\"tr_DISPOSITION_BINDING\"]')").await.expect("false sticky-paid summary");
                tx.execute("ALTER TABLE payment_identity_resolutions ENABLE TRIGGER USER")
                    .await
                    .expect("guard restored");
            }
        }
        tx.commit().await.expect("import fixture");
        clock.0.store(1560, Ordering::SeqCst);
        assert_eq!(
            store::drain::read_disposition(&pool, &claim.operation_id)
                .await
                .err(),
            Some(PaymentStoreError::InvalidStoredData),
            "{field}"
        );
        assert_eq!(server.count(), 2);
        let mut tx = pool.begin().await.expect("restore only isolated fixture");
        match field {
            "outcome" => {
                tx.execute("ALTER TABLE callback_receipt_processing DISABLE TRIGGER USER")
                    .await
                    .expect("fixture repair");
                tx.execute("UPDATE callback_receipt_processing SET outcome='observed'")
                    .await
                    .expect("restore exact outcome");
                tx.execute("ALTER TABLE callback_receipt_processing ENABLE TRIGGER USER")
                    .await
                    .expect("guard restored");
            }
            "receipt" => {
                tx.execute("ALTER TABLE callback_lookup_receipts DISABLE TRIGGER USER")
                    .await
                    .expect("fixture repair");
                sqlx::query("UPDATE callback_lookup_receipts SET snapshot=$1")
                    .bind(&receipt)
                    .execute(&mut *tx)
                    .await
                    .expect("exact receipt");
                tx.execute("ALTER TABLE callback_lookup_receipts ENABLE TRIGGER USER")
                    .await
                    .expect("guard restored");
            }
            "observation" => {
                tx.execute("ALTER TABLE payment_observations DISABLE TRIGGER USER")
                    .await
                    .expect("fixture repair");
                sqlx::query("UPDATE payment_observations SET status='open',snapshot=$1")
                    .bind(&observation)
                    .execute(&mut *tx)
                    .await
                    .expect("exact observation");
                tx.execute("ALTER TABLE payment_observations ENABLE TRIGGER USER")
                    .await
                    .expect("guard restored");
            }
            _ => {
                tx.execute("ALTER TABLE payment_identity_resolutions DISABLE TRIGGER USER")
                    .await
                    .expect("fixture repair");
                sqlx::query("UPDATE payment_identity_resolutions SET summary=$1")
                    .bind(&summary)
                    .execute(&mut *tx)
                    .await
                    .expect("exact resolution");
                tx.execute("ALTER TABLE payment_identity_resolutions ENABLE TRIGGER USER")
                    .await
                    .expect("guard restored");
            }
        }
        tx.commit().await.expect("repaired source");
        clock.0.store(1500, Ordering::SeqCst);
        assert!(matches!(
            store::drain::read_disposition(&pool, &claim.operation_id)
                .await
                .expect("paired healthy exact source"),
            Some(store::drain::Disposition::Observed)
        ));
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn invalid_custody_cannot_premark_an_inflight_operation_without_any_receipt_even_with_an_orphan_event(
    pool: PgPool,
) {
    let (clock, f) = prepared(&pool, true).await;
    let i = ids();
    admit(&pool, clock.as_ref(), &i, PROFILE, "tr_EVENT_DONOR").await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, response(f.snapshot(), "tr_EVENT_DONOR", "open")),
    ]);
    let c = client(&server).await;
    let original = claimed_receipt(&pool, clock.as_ref(), &i, &c).await;
    let target_id = admit(&pool, clock.as_ref(), &i, PROFILE, "tr_NO_RECEIPT").await;
    let target = store::jobs::claim(&pool, clock.as_ref(), &i, PROFILE)
        .await
        .expect("inflight target")
        .expect("pending");
    let (event_id, receipt_id, payload): (String, String, Value) = sqlx::query_as(
        "SELECT id,receipt_id,payload FROM callback_processing_events WHERE operation_id=$1",
    )
    .bind(&original.operation_id)
    .fetch_one(&pool)
    .await
    .expect("original legitimate event");
    let mut tx = pool.begin().await.expect("isolated orphan-event import");
    tx.execute("ALTER TABLE callback_processing_events DISABLE TRIGGER USER")
        .await
        .expect("test fixture bypass");
    sqlx::query("UPDATE callback_processing_events SET operation_id=$1 WHERE id=$2")
        .bind(&target.operation_id)
        .bind(&event_id)
        .execute(&mut *tx)
        .await
        .expect("coherent raw event with no receipt for target operation");
    tx.execute("ALTER TABLE callback_processing_events ENABLE TRIGGER USER")
        .await
        .expect("restore guard");
    tx.commit().await.expect("imported orphan");
    assert!(sqlx::query("INSERT INTO callback_receipt_processing(operation_id,event_id,processed_at,outcome) VALUES($1,$2,1500,'custody_invalid')").bind(&target.operation_id).bind(&event_id).execute(&pool).await.is_err(),"native invalid exception requires an actual receipt for this operation");
    let mut tx = pool
        .begin()
        .await
        .expect("isolated imported processing premarker");
    tx.execute("ALTER TABLE callback_receipt_processing DISABLE TRIGGER USER")
        .await
        .expect("test fixture bypass");
    sqlx::query("INSERT INTO callback_receipt_processing(operation_id,event_id,processed_at,outcome) VALUES($1,$2,1500,'custody_invalid')").bind(&target.operation_id).bind(&event_id).execute(&mut *tx).await.expect("imported false exception");
    tx.execute("ALTER TABLE callback_receipt_processing ENABLE TRIGGER USER")
        .await
        .expect("restore guard");
    tx.commit().await.expect("imported premarker");
    assert_eq!(
        store::drain::read_disposition(&pool, &target.operation_id)
            .await
            .err(),
        Some(PaymentStoreError::InvalidStoredData)
    );
    clock.0.store(1560, Ordering::SeqCst);
    assert_eq!(
        store::jobs::sweep(&pool, clock.as_ref(), PROFILE)
            .await
            .err(),
        Some(PaymentStoreError::InvalidStoredData)
    );
    assert!(
        read_callback_job(&pool, &target_id)
            .await
            .expect("target checkpoint retained")
            .expect("job")
            .state
            == JobState::Leased
    );
    let mut tx = pool.begin().await.expect("exact fixture repair");
    tx.execute("ALTER TABLE callback_receipt_processing DISABLE TRIGGER USER")
        .await
        .expect("repair");
    sqlx::query("DELETE FROM callback_receipt_processing WHERE operation_id=$1")
        .bind(&target.operation_id)
        .execute(&mut *tx)
        .await
        .expect("remove imported false exception");
    tx.execute("ALTER TABLE callback_receipt_processing ENABLE TRIGGER USER")
        .await
        .expect("guard restored");
    tx.execute("ALTER TABLE callback_processing_events DISABLE TRIGGER USER")
        .await
        .expect("repair");
    sqlx::query("UPDATE callback_processing_events SET operation_id=$1,receipt_id=$2,payload=$3 WHERE id=$4").bind(&original.operation_id).bind(receipt_id).bind(payload).bind(event_id).execute(&mut *tx).await.expect("restore exact original event");
    tx.execute("ALTER TABLE callback_processing_events ENABLE TRIGGER USER")
        .await
        .expect("guard restored");
    tx.commit().await.expect("repaired fixture");
    assert!(
        read_lookup(&pool, &original.operation_id)
            .await
            .expect("paired restored source")
            .is_some()
    );
    assert!(
        store::drain::read_disposition(&pool, &target.operation_id)
            .await
            .expect("target remains unprocessed")
            .is_none()
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn native_orphan_intakes_and_false_source_events_rollback_quota_and_custody(pool: PgPool) {
    let mut tx = pool.begin().await.expect("bare native intake");
    tx.execute("INSERT INTO callback_intakes(id,profile_id,provider_id,received_at) VALUES(repeat('f',32),'pfl_NATIVE','tr_NATIVE',1500)").await.expect("pending native source");
    assert!(
        tx.commit().await.is_err(),
        "deferred intake requires event and initial job"
    );
    assert_eq!(intake_counts(&pool).await, (0, 0, 0));
    for field in [
        "schema_version",
        "intake_id",
        "profile_id",
        "provider_id",
        "received_at",
    ] {
        let mut tx = pool.begin().await.expect("isolated native event control");
        tx.execute("INSERT INTO callback_intakes(id,profile_id,provider_id,received_at) VALUES(repeat('f',32),'pfl_NATIVE','tr_NATIVE',1500)").await.expect("valid pending source");
        let mut payload = json!({"schema_version":1,"intake_id":"f".repeat(32),"profile_id":"pfl_NATIVE","provider_id":"tr_NATIVE","received_at":1500});
        payload[field] = match field {
            "schema_version" => json!(2),
            "received_at" => json!(1501),
            "intake_id" => json!("e".repeat(32)),
            _ => json!("other"),
        };
        assert!(sqlx::query("INSERT INTO callback_intake_events(id,intake_id,created_at,kind,payload) VALUES(repeat('e',32),repeat('f',32),1500,'payment.callback_received.v1',$1)").bind(payload).execute(&mut *tx).await.is_err(),"{field}");
        tx.rollback().await.expect("native control rollback");
    }
    assert_eq!(
        sqlx::query_scalar::<_, i32>(
            "SELECT admitted FROM callback_admission_quota WHERE scope='global'"
        )
        .fetch_one(&pool)
        .await
        .expect("quota rollback"),
        0
    );
    let clock = time();
    let i = ids();
    admit(&pool, clock.as_ref(), &i, "pfl_NATIVE", "tr_NATIVE").await;
    assert_eq!(intake_counts(&pool).await, (1, 1, 1));
    for query in [
        "UPDATE callback_admission_quota SET admitted=0",
        "UPDATE callback_admission_quota SET rate_window=0,last_at=0",
        "DELETE FROM callback_admission_quota",
    ] {
        assert!(
            pool.execute(query).await.is_err(),
            "native exact quota history rejects rewind"
        );
    }
}

async fn claimed_receipt(
    pool: &PgPool,
    clock: &dyn commerce_application::request_context::Clock,
    i: &dyn commerce_application::request_context::Identifiers,
    c: &VerifiedReconciliationClient,
) -> store::jobs::Claim {
    let mut claim = store::jobs::claim(pool, clock, i, PROFILE)
        .await
        .expect("claim")
        .expect("pending");
    let fetched = c
        .fetch_lookup(claim.take_permit().expect("one-use start"))
        .await
        .expect("authenticated callback");
    record_lookup(pool, clock, i, &claim.operation_id, &fetched)
        .await
        .expect("durable receipt before processing");
    claim
}

async fn bad_receipt_does_not_block_healthy(pool: &PgPool, missing_event: bool) {
    let (clock, f) = prepared(pool, true).await;
    let i = ids();
    let bad = admit(pool, clock.as_ref(), &i, PROFILE, "tr_OLDEST_CORRUPT").await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, response(f.snapshot(), "tr_OLDEST_CORRUPT", "paid")),
        Reply::Json(200, response(f.snapshot(), "tr_HEALTHY_SUCCESSOR", "open")),
    ]);
    let c = client(&server).await;
    let claim = claimed_receipt(pool, clock.as_ref(), &i, &c).await;
    let good = admit(pool, clock.as_ref(), &i, PROFILE, "tr_HEALTHY_SUCCESSOR").await;
    let mut tx = pool.begin().await.expect("isolated imported damage");
    if missing_event {
        tx.execute("ALTER TABLE callback_processing_events DISABLE TRIGGER USER")
            .await
            .expect("fixture bypass");
        sqlx::query("DELETE FROM callback_processing_events WHERE operation_id=$1")
            .bind(&claim.operation_id)
            .execute(&mut *tx)
            .await
            .expect("missing event import");
        tx.execute("ALTER TABLE callback_processing_events ENABLE TRIGGER USER")
            .await
            .expect("restore guard");
    } else {
        tx.execute("ALTER TABLE callback_lookup_receipts DISABLE TRIGGER USER")
            .await
            .expect("fixture bypass");
        sqlx::query("UPDATE callback_lookup_receipts SET snapshot=jsonb_set(snapshot,'{payment,status}','\"open\"') WHERE operation_id=$1").bind(&claim.operation_id).execute(&mut *tx).await.expect("otherwise-valid changed fact");
        tx.execute("ALTER TABLE callback_lookup_receipts ENABLE TRIGGER USER")
            .await
            .expect("restore guard");
    }
    tx.commit().await.expect("damaged fixture");
    let result = run_callback_batch(pool, clock.as_ref(), &i, &c, 1)
        .await
        .expect("bounded bad custody stop never blocks healthy successor");
    assert_eq!(result.processed, 1);
    assert_eq!(result.needs_review, 1);
    let bad_job = read_callback_job(pool, &bad)
        .await
        .expect("visible bad source job")
        .expect("retained");
    assert!(!bad_job.settled);
    let good_job = read_callback_job(pool, &good)
        .await
        .expect("healthy successor job")
        .expect("retained");
    assert!(good_job.state == JobState::Done && good_job.settled);
    let (outcome,observation,resolution):(String,Option<String>,Option<String>)=sqlx::query_as("SELECT outcome,observation_id,resolution_id FROM callback_receipt_processing WHERE operation_id=$1").bind(&claim.operation_id).fetch_one(pool).await.expect("redacted invalid-custody disposition");
    assert_eq!(outcome, "custody_invalid");
    assert!(observation.is_none() && resolution.is_none());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM payment_observations")
            .fetch_one(pool)
            .await
            .expect("only healthy source observation"),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT provider_id FROM payment_observations")
            .fetch_one(pool)
            .await
            .expect("healthy external identity"),
        "tr_HEALTHY_SUCCESSOR"
    );
    let later = run_callback_batch(pool, clock.as_ref(), &i, &c, 1)
        .await
        .expect("unchanged exception remains visible");
    assert_eq!(later.processed, 0);
    assert_eq!(later.needs_review, 1);
    assert_eq!(server.count(), 3);
    get_only(&server);
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn oldest_corrupt_receipt_digest_stops_visibly_without_blocking_a_healthy_successor(
    pool: PgPool,
) {
    bad_receipt_does_not_block_healthy(&pool, false).await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn oldest_receipt_with_missing_event_stops_visibly_without_blocking_a_healthy_successor(
    pool: PgPool,
) {
    bad_receipt_does_not_block_healthy(&pool, true).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn independently_bound_intake_headers_reject_coherent_import_rebinding_before_processing(
    pool: PgPool,
) {
    let clock = time();
    let i = ids();
    let id = admit(&pool, clock.as_ref(), &i, PROFILE, "tr_SOURCE_BINDING").await;
    for field in ["profile", "provider", "received"] {
        let mut tx = pool.begin().await.expect("isolated import damage");
        tx.execute("ALTER TABLE callback_intakes DISABLE TRIGGER USER")
            .await
            .expect("fixture bypass");
        match field {
            "profile" => {
                tx.execute("UPDATE callback_intakes SET profile_id='pfl_OTHER'")
                    .await
                    .expect("coherent profile change");
            }
            "provider" => {
                tx.execute("UPDATE callback_intakes SET provider_id='tr_OTHER'")
                    .await
                    .expect("coherent identity change");
            }
            _ => {
                tx.execute("UPDATE callback_intakes SET received_at=1499")
                    .await
                    .expect("coherent time change");
            }
        }
        tx.execute("ALTER TABLE callback_intakes ENABLE TRIGGER USER")
            .await
            .expect("guard restored");
        tx.commit().await.expect("damaged import");
        assert_eq!(
            read_callback_job(&pool, &id).await.err(),
            Some(PaymentStoreError::InvalidStoredData)
        );
        let scope = if field == "profile" {
            "pfl_OTHER"
        } else {
            PROFILE
        };
        assert_eq!(
            store::jobs::claim(&pool, clock.as_ref(), &i, scope)
                .await
                .err(),
            Some(PaymentStoreError::InvalidStoredData)
        );
        assert_eq!(
            store::jobs::sweep(&pool, clock.as_ref(), scope).await.err(),
            Some(PaymentStoreError::InvalidStoredData)
        );
        assert_eq!(counts(&pool).await, (0, 0, 0, 0, 0));
        let mut tx = pool.begin().await.expect("exact fixture repair");
        tx.execute("ALTER TABLE callback_intakes DISABLE TRIGGER USER")
            .await
            .expect("fixture repair");
        sqlx::query("UPDATE callback_intakes SET profile_id=$1,provider_id='tr_SOURCE_BINDING',received_at=1500").bind(PROFILE).execute(&mut *tx).await.expect("exact original headers");
        tx.execute("ALTER TABLE callback_intakes ENABLE TRIGGER USER")
            .await
            .expect("guard restored");
        tx.commit().await.expect("repaired fixture");
        assert!(
            read_callback_job(&pool, &id)
                .await
                .expect("paired healthy read")
                .is_some()
        );
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn native_intake_generation_and_processing_custody_cannot_be_deleted_rebound_or_rewound(
    pool: PgPool,
) {
    let (clock, f) = prepared(&pool, true).await;
    let i = ids();
    let id = admit(&pool, clock.as_ref(), &i, PROFILE, "tr_NATIVE_INBOX").await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, response(f.snapshot(), "tr_NATIVE_INBOX", "open")),
    ]);
    let c = client(&server).await;
    let claim = claimed_receipt(&pool, clock.as_ref(), &i, &c).await;
    store::drain::process_operation(&pool, clock.as_ref(), &i, &claim.operation_id)
        .await
        .expect("healthy original processing");
    for query in [
        "UPDATE callback_intakes SET provider_id=provider_id",
        "DELETE FROM callback_intakes",
        "UPDATE callback_intake_events SET payload=payload",
        "DELETE FROM callback_intake_events",
        "UPDATE callback_inbox_generations SET operation_id=operation_id",
        "DELETE FROM callback_inbox_generations",
        "UPDATE callback_receipt_processing SET outcome=outcome",
        "DELETE FROM callback_receipt_processing",
        "DELETE FROM callback_inbox_jobs",
        "UPDATE callback_inbox_jobs SET generation=0",
        "UPDATE callback_inbox_jobs SET generation=6",
        "UPDATE callback_inbox_jobs SET settled=true",
        "UPDATE callback_inbox_jobs SET updated_at=1499",
    ] {
        assert!(
            pool.execute(query).await.is_err(),
            "native custody rejected: {query}"
        );
    }
    let state = read_callback_job(&pool, &id)
        .await
        .expect("healthy after native controls")
        .expect("job");
    assert_eq!(state.generation, 1);
    assert!(state.state == JobState::Leased);
    store::jobs::finish(&pool, clock.as_ref(), &claim, JobState::Done, None)
        .await
        .expect("healthy checkpoint");
    store::jobs::sweep(&pool, clock.as_ref(), PROFILE)
        .await
        .expect("settle complete source");
    assert!(
        read_callback_job(&pool, &id)
            .await
            .expect("read")
            .expect("job")
            .settled
    );
    assert!(
        pool.execute("UPDATE callback_inbox_jobs SET settled=false")
            .await
            .is_err(),
        "settlement cannot rewind"
    );
}

#[sqlx::test(migrations = false)]
async fn schema29_upgrade_preserves_callback_receipts_without_installing_intake_or_worker_authority(
    pool: PgPool,
) {
    MIGRATOR
        .run_to(29, &pool)
        .await
        .expect("accepted callback custody schema");
    let (clock, f) = prepared(&pool, true).await;
    let i = ids();
    let operation = operation(&pool, clock.as_ref(), &i, "tr_PRIOR_CALLBACK").await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, response(f.snapshot(), "tr_PRIOR_CALLBACK", "open")),
    ]);
    let c = client(&server).await;
    let source = fetch_lookup(&pool, clock.as_ref(), &i, &operation.snapshot().id, &c)
        .await
        .expect("accepted prior receipt");
    let prior = counts(&pool).await;
    MIGRATOR.run(&pool).await.expect("forward upgrade");
    MIGRATOR.run(&pool).await.expect("idempotent upgrade");
    assert_eq!(intake_counts(&pool).await, (0, 0, 0));
    assert_eq!(counts(&pool).await, prior);
    assert!(
        read_lookup(&pool, &operation.snapshot().id)
            .await
            .expect("prior receipt preserved")
            .expect("source")
            .snapshot
            == source.snapshot
    );
}
