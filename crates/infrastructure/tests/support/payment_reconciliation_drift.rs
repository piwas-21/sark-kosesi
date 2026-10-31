use super::*;
#[sqlx::test(migrator = "MIGRATOR")]
async fn drifted_progress_cannot_rebind_page_receipts_or_refetch_an_existing_operation(
    pool: PgPool,
) {
    let (time, f) = prepared(&pool, true).await;
    let i = ids();
    let server = Server::start(vec![
        profile(),
        page(f.snapshot(), &[("tr_A", "paid")], None),
        fetched(f.snapshot(), "tr_A", "paid"),
    ]);
    let client = read_client(&server).await;
    jobs::enqueue(&pool, time.as_ref(), client.profile_id())
        .await
        .expect("schedule");
    let mut claim = jobs::claim(&pool, time.as_ref(), client.profile_id())
        .await
        .expect("claim")
        .expect("lease");
    let candidate = steps::advance(&pool, time.as_ref(), &i, &client, &claim)
        .await
        .expect("durable page");
    let mut drift = claim.job.clone();
    drift.page_id = candidate.page_id.clone();
    let bad = Claim {
        job: drift.clone(),
        until: claim.until,
    };
    assert!(
        steps::advance(&pool, time.as_ref(), &i, &client, &bad)
            .await
            .is_err(),
        "root receipt cannot be rebound to predecessor"
    );
    drift.operation += 1;
    let bad = Claim {
        job: drift,
        until: claim.until,
    };
    assert!(
        steps::advance(&pool, time.as_ref(), &i, &client, &bad)
            .await
            .is_err(),
        "terminal page has no continuation"
    );
    for (index, page_id, phase) in [
        (10, candidate.page_id.clone(), Phase::Candidates),
        (-1, candidate.page_id.clone(), Phase::Candidates),
        (0, None, Phase::Candidates),
        (0, None, Phase::NeedsReview),
    ] {
        let mut drift = candidate.clone();
        drift.candidate_index = index;
        drift.page_id = page_id;
        drift.phase = phase;
        let bad = Claim {
            job: drift,
            until: claim.until,
        };
        assert!(
            steps::advance(&pool, time.as_ref(), &i, &client, &bad)
                .await
                .is_err()
        );
    }
    claim.job = candidate;
    steps::advance(&pool, time.as_ref(), &i, &client, &claim)
        .await
        .expect("first fetched operation");
    steps::advance(&pool, time.as_ref(), &i, &client, &claim)
        .await
        .expect("same operation receipt avoids another fetch");
    assert_eq!(server.count(), 3);
    get_only(&server);
    // Missing original evidence fails the trusted step rather than exposing a callback surface.
    let mut missing = claim.job.clone();
    missing.attempt_id = "f".repeat(32);
    let bad = Claim {
        job: missing,
        until: claim.until,
    };
    assert!(
        steps::advance(&pool, time.as_ref(), &i, &client, &bad)
            .await
            .is_err()
    );
    pool.close().await;
    assert!(
        steps::advance(&pool, time.as_ref(), &i, &client, &claim)
            .await
            .is_err()
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn generation_fence_rejects_wrong_owner_even_while_deadline_is_fresh(pool: PgPool) {
    let (time, f) = prepared(&pool, true).await;
    let profile = &f.snapshot().binding.profile.id;
    jobs::enqueue(&pool, time.as_ref(), profile)
        .await
        .expect("schedule");
    let claim = jobs::claim(&pool, time.as_ref(), profile)
        .await
        .expect("lease")
        .expect("claim");
    let mut wrong = claim.job.clone();
    wrong.generation -= 1;
    let wrong = Claim {
        job: wrong,
        until: claim.until,
    };
    assert_eq!(
        jobs::finish(&pool, time.as_ref(), &wrong, &wrong.job)
            .await
            .err(),
        Some(RecoveryError::LeaseLost)
    );
    jobs::finish(&pool, time.as_ref(), &claim, &claim.job)
        .await
        .expect("exact healthy owner");
}
