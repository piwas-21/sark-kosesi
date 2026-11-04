//! Reassess retained scarcity using the same frozen physical allocation primitive.
use super::disposition::{DecisionOutcome, DecisionReceipt};
use super::*;

pub(super) async fn execute(
    tx: &mut Transaction<'_, Postgres>,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
    decision: &mut DecisionReceipt,
) -> Result<(), LateStockError> {
    let context = current(tx, &decision.order_id, &decision.attempt_id).await?;
    let original = source::original(tx, &context.order, true).await?;
    if original
        .iter()
        .any(|m| !matches!(m.state, OriginalState::Active | OriginalState::Expired))
    {
        return Err(LateStockError::NotApplicable);
    }
    let initial = clock
        .unix_seconds()
        .map_err(|_| LateStockError::ClockUnavailable)?;
    let clock = ObservedClock::new(clock, initial);
    let started = now(&clock)?;
    check_time(
        started,
        context.order.identity.created_at,
        context.received.recorded_at,
    )?;
    if started < context.order.identity.hold_expires_at {
        return Err(LateStockError::NotApplicable);
    }
    let expires = started
        .checked_add(WINDOW_SECONDS)
        .filter(|v| *v <= commerce_domain::checkout_quotes::MAX_QUOTE_UNIX_SECONDS)
        .ok_or(LateStockError::ClockUnavailable)?;
    let mut savepoint = tx.begin().await?;
    let group = fresh(ids)?;
    let plan = engine::prepare(
        &mut savepoint,
        ids,
        &context,
        &original,
        &group,
        started,
        expires,
    )
    .await?;
    let footprint = ReviewFootprint {
        schema_version: 1,
        original: original.clone(),
        components: source::assess(&mut savepoint, &original).await?,
    };
    if ReviewReason::classify(&footprint)? == Some(ReviewReason::ComponentScarcity) {
        savepoint.rollback().await?;
        let refreshed = current(tx, &decision.order_id, &decision.attempt_id).await?;
        let actual = source::original(tx, &refreshed.order, true).await?;
        source::validate_same_assessment(
            &original,
            &actual,
            &context.received.id,
            &refreshed.received.id,
        )?;
        let footprint = ReviewFootprint {
            schema_version: 1,
            components: source::assess(tx, &actual).await?,
            original: actual,
        };
        if ReviewReason::classify(&footprint)? != Some(ReviewReason::ComponentScarcity) {
            return Err(LateStockError::StateChanged);
        }
        let at = now(&clock)?;
        check_time(
            at,
            refreshed.order.identity.created_at,
            refreshed.received.recorded_at,
        )?;
        decision.recorded_at = at;
        decision.recognition_id = refreshed.received.id;
        decision.outcome = DecisionOutcome::StillScarce;
        disposition::insert(tx, decision, Some(&footprint), &fresh(ids)?).await?;
        check_time(now(&clock)?, at, at)?;
        return Ok(());
    }
    let allocated = now(&clock)?;
    check_time(
        allocated,
        context.order.identity.created_at,
        context.received.recorded_at,
    )?;
    check_window(allocated, expires)?;
    let stages = footprint
        .components
        .iter()
        .map(|c| StageComponent::from_assessment(c)?.ok_or(PolicyError::InvalidSnapshot))
        .collect::<Result<Vec<_>, _>>()?;
    let allocation = LateAllocation {
        id: fresh(ids)?,
        order_id: decision.order_id.clone(),
        attempt_id: decision.attempt_id.clone(),
        recognition_id: context.received.id.clone(),
        original_group_id: context.order.identity.stock_group_id,
        fresh_group_id: group,
        actor: ACTOR.into(),
        allocated_at: allocated,
        expires_at: expires,
        footprint: LateFootprint {
            schema_version: 1,
            original: original.clone(),
            fresh: plan.members()?,
            components: stages,
        },
    };
    allocation.validate()?;
    decision.recognition_id = context.received.id;
    decision.recorded_at = allocated;
    decision.outcome = DecisionOutcome::Allocated {
        allocation_id: allocation.id.clone(),
    };
    disposition::insert(&mut savepoint, decision, None, &fresh(ids)?).await?;
    engine::allocate(&mut savepoint, &clock, ids, &allocation, &plan).await?;
    savepoint.commit().await?;
    Ok(())
}
