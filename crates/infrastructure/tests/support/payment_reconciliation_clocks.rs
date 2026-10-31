use super::*;
use commerce_application::request_context::{Clock, ContextError};
struct Samples(std::sync::Mutex<std::collections::VecDeque<u64>>);
impl Samples {
    fn new(values: &[u64]) -> Self {
        Self(std::sync::Mutex::new(values.iter().copied().collect()))
    }
}
impl Clock for Samples {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        self.0
            .lock()
            .expect("clock")
            .pop_front()
            .ok_or(ContextError::ClockUnavailable)
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn scheduling_and_claim_clock_failures_roll_back_before_any_network_or_lease(pool: PgPool) {
    let (time, f) = prepared(&pool, true).await;
    let a = &f.snapshot().binding.id;
    let server = Server::start(vec![profile()]);
    let client = read_client(&server).await;
    assert_eq!(
        run_batch(
            &pool,
            &Samples::new(&[1500, 1500, 1499]),
            &ids(),
            &client,
            1
        )
        .await
        .err(),
        Some(RecoveryError::ClockUnavailable)
    );
    assert!(read_job(&pool, a).await.expect("rolled back").is_none());
    assert_eq!(
        jobs::enqueue(&pool, &Samples::new(&[1500, 1499]), client.profile_id())
            .await
            .err(),
        Some(RecoveryError::ClockUnavailable)
    );
    jobs::enqueue(&pool, time.as_ref(), client.profile_id())
        .await
        .expect("healthy schedule");
    assert_eq!(
        jobs::sweep(&pool, &Samples::new(&[1500, 1499]), client.profile_id())
            .await
            .err(),
        Some(RecoveryError::ClockUnavailable)
    );
    for values in [&[1500, 1499][..], &[1500, 1560][..]] {
        assert_eq!(
            jobs::claim(&pool, &Samples::new(values), client.profile_id())
                .await
                .err(),
            Some(RecoveryError::ClockUnavailable)
        );
        let job = read_job(&pool, a).await.expect("job").expect("found");
        assert_eq!(job.generation, 0);
        assert!(job.lease_until.is_none());
    }
    let claim = jobs::claim(&pool, time.as_ref(), client.profile_id())
        .await
        .expect("healthy claim")
        .expect("claim");
    assert_eq!(
        jobs::finish(&pool, &Samples::new(&[1499]), &claim, &claim.job)
            .await
            .err(),
        Some(RecoveryError::ClockUnavailable)
    );
    assert_eq!(
        jobs::finish(&pool, &Samples::new(&[1500, 1499]), &claim, &claim.job)
            .await
            .err(),
        Some(RecoveryError::LeaseLost)
    );
    jobs::finish(&pool, time.as_ref(), &claim, &claim.job)
        .await
        .expect("healthy checkpoint");
    assert_eq!(
        read_job(&pool, a)
            .await
            .expect("job")
            .expect("found")
            .operation,
        1
    );
    assert_eq!(server.count(), 1);
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn final_checkpoint_clock_after_real_write_wait_fences_expired_workers_and_cancellation(
    pool: PgPool,
) {
    let (time, f) = prepared(&pool, true).await;
    let a = &f.snapshot().binding.id;
    let profile = &f.snapshot().binding.profile.id;
    jobs::enqueue(&pool, time.as_ref(), profile)
        .await
        .expect("schedule");
    for cancel in [false, true] {
        let claim = jobs::claim(&pool, time.as_ref(), profile)
            .await
            .expect("claim")
            .expect("available");
        let mut blocker = pool.begin().await.expect("blocker");
        blocker
            .execute("LOCK TABLE payment_reconciliation_jobs IN SHARE MODE")
            .await
            .expect("write gate");
        let p = pool.clone();
        let t = time.clone();
        let task =
            tokio::spawn(async move { jobs::finish(&p, t.as_ref(), &claim, &claim.job).await });
        tokio::time::timeout(Duration::from_secs(1),async {loop {let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'UPDATE payment_reconciliation_jobs SET phase=%')").fetch_one(&pool).await.expect("actual checkpoint wait");if waiting {break;}tokio::time::sleep(Duration::from_millis(5)).await;}}).await.expect("reached actual write");
        if cancel {
            task.abort();
            assert!(task.await.expect_err("cancelled").is_cancelled());
            blocker.commit().await.expect("release");
        } else {
            time.0.store(1560, Ordering::SeqCst);
            blocker.commit().await.expect("release");
            assert_eq!(
                task.await.expect("task").err(),
                Some(RecoveryError::LeaseLost)
            );
        }
        let job = read_job(&pool, a).await.expect("read").expect("found");
        assert_eq!(job.operation, 0, "checkpoint rollback");
        time.0.store(
            u64::try_from(job.lease_until.expect("still leased")).expect("time"),
            Ordering::SeqCst,
        );
    }
}
