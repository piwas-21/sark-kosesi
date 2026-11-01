//! The real private API must commit durable custody before the edge acknowledges.
use super::*;
use commerce_api::{AppState, callbacks::CallbackProfile, router_with_options};
use commerce_infrastructure::{
    MIGRATOR,
    runtime::{SecureIdentifiers, SystemClock},
};
use sqlx::{Executor, PgPool};
use tokio::sync::Semaphore;

async fn counts(pool: &PgPool) -> (i64, i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM callback_intakes),(SELECT count(*) FROM callback_intake_events),(SELECT count(*) FROM callback_inbox_jobs),(SELECT count(*) FROM callback_inbox_active)").fetch_one(pool).await.expect("actual complete custody counts")
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn real_private_api_and_postgres_commit_precede_the_empty_edge_acknowledgement(pool: PgPool) {
    pool.execute("CREATE FUNCTION edge_commit_barrier_fixture() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(hashtext(current_database())::bigint); RETURN NULL; END $$; CREATE CONSTRAINT TRIGGER edge_commit_barrier_fixture AFTER INSERT ON callback_intakes DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION edge_commit_barrier_fixture()")
        .await.expect("actual deferred commit barrier");
    let state = AppState {
        pool: pool.clone(),
        preview_token: Arc::from(TOKEN),
        capacity: Arc::new(Semaphore::new(32)),
        clock: Arc::new(SystemClock),
        identifiers: Arc::new(SecureIdentifiers),
    };
    let api = Server::start(
        router_with_options(
            state,
            None,
            Some(CallbackProfile::parse("pfl_EDGE_REAL_API").expect("server configured profile")),
        )
        .expect("private API router"),
    )
    .await;
    let direct = post(api.address, b"id=tr_DIRECT_DENIED").await;
    assert_eq!(direct.status, 401, "private API remains Bearer protected");
    let catalogue = exchange(
        api.address,
        head("GET", "/api/catalogue/nl", "api.invalid", None, &[], 0),
    )
    .await;
    assert_eq!(
        catalogue.status, 401,
        "existing catalogue protection remains intact"
    );
    let edge = Server::start(router(configured(&values(api.address)))).await;
    let mut blocker = pool.begin().await.expect("owned transaction barrier");
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext(current_database())::bigint)")
        .execute(&mut *blocker)
        .await
        .expect("hold commit completion");
    let address = edge.address;
    let task = tokio::spawn(async move {
        let body = b"id=tr_EDGE_REAL_RECEIPT";
        let mut raw = head(
            "POST",
            PATH,
            "edge.invalid",
            Some("application/x-www-form-urlencoded"),
            &[
                ("Authorization", "Bearer untrusted-client-token"),
                ("X-Mollie-Profile", "pfl_UNTRUSTED_BODY"),
            ],
            body.len(),
        );
        raw.extend_from_slice(body);
        exchange(address, raw).await
    });
    tokio::time::timeout(Duration::from_millis(700),async {
        loop {
            let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid() AND wait_event_type='Lock' AND query LIKE 'COMMIT%')").fetch_one(&pool).await.expect("actual commit wait");
            if waiting {break;}
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.expect("private API reaches commit barrier within its one-second HTTP budget");
    assert!(
        !task.is_finished(),
        "neither private API nor edge can acknowledge before commit"
    );
    assert_eq!(counts(&pool).await, (0, 0, 0, 0));
    blocker.rollback().await.expect("release actual commit");
    let response = task.await.expect("edge HTTP task");
    assert_eq!(response.status, 200);
    assert!(response.body.is_empty());
    assert_eq!(counts(&pool).await, (1, 1, 1, 1));
    let exact:i64=sqlx::query_scalar("SELECT count(*) FROM callback_intakes i JOIN callback_intake_events e ON e.intake_id=i.id JOIN callback_inbox_jobs j ON j.intake_id=i.id JOIN callback_inbox_active a ON a.intake_id=i.id WHERE i.profile_id='pfl_EDGE_REAL_API' AND i.provider_id='tr_EDGE_REAL_RECEIPT' AND e.created_at=i.received_at AND e.kind='payment.callback_received.v1' AND e.payload=jsonb_build_object('schema_version',1,'intake_id',i.id,'profile_id',i.profile_id,'provider_id',i.provider_id,'received_at',i.received_at) AND j.source_event_id=e.id AND j.state='pending' AND j.generation=0 AND NOT j.settled AND a.profile_id=i.profile_id AND a.created_at=i.received_at")
        .fetch_one(&pool).await.expect("independent exact atomic source proof");
    assert_eq!(exact, 1);
    let effects:(i64,i64,i64,i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM checkout_orders),(SELECT count(*) FROM payment_attempts),(SELECT count(*) FROM payment_observations),(SELECT count(*) FROM stock_movements),(SELECT count(*) FROM callback_lookup_operations),(SELECT count(*) FROM commerce_outbox)")
        .fetch_one(&pool).await.expect("actual financial aggregates remain empty");
    assert_eq!(effects, (0, 0, 0, 0, 0, 0));
    edge.stop().await;
    api.stop().await;
}
