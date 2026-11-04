//! One bounded fresh attempt. Scarcity is rechecked after child rollback releases locks.
use super::*;
use crate::holds::basket::{
    BasketHoldLine, BasketHoldRequest, FreshPreparation, prepare_fresh, reserve_claimed,
};
pub(super) async fn recover(
    tx: &mut Transaction<'_, Postgres>,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
    order: &str,
    attempt: &str,
) -> Result<Outcome, LateStockError> {
    let context = current(tx, order, attempt).await?;
    let original = source::original(tx, &context.order, true).await?;
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
    if original.iter().any(|member| {
        matches!(
            member.state,
            OriginalState::Released | OriginalState::Consumed
        )
    }) {
        let components = source::assess(tx, &original).await?;
        return review(tx, &clock, ids, &context, original, components).await;
    }
    if started < context.order.identity.hold_expires_at {
        source::assess(tx, &original).await?;
        let checked = now(&clock)?;
        check_time(
            checked,
            context.order.identity.created_at,
            context.received.recorded_at,
        )?;
        if checked >= context.order.identity.hold_expires_at {
            return Err(LateStockError::StateChanged);
        }
        return Err(LateStockError::NotApplicable);
    }
    let mut savepoint = tx.begin().await?;
    let expires = started
        .checked_add(WINDOW_SECONDS)
        .filter(|value| *value <= commerce_domain::checkout_quotes::MAX_QUOTE_UNIX_SECONDS)
        .ok_or(LateStockError::ClockUnavailable)?;
    let group = fresh(ids)?;
    let fresh_plan = prepare(
        &mut savepoint,
        ids,
        &context,
        &original,
        &group,
        started,
        expires,
    )
    .await?;
    let components = source::assess(&mut savepoint, &original).await?;
    let footprint = ReviewFootprint {
        schema_version: 1,
        original: original.clone(),
        components,
    };
    if ReviewReason::classify(&footprint)? == Some(ReviewReason::ComponentScarcity) {
        savepoint.rollback().await?;
        check_time(
            now(&clock)?,
            context.order.identity.created_at,
            context.received.recorded_at,
        )?;
        // The rollback released component claims: refresh source, finance and time.
        let refreshed = current(tx, order, attempt).await?;
        let actual = source::original(tx, &refreshed.order, true).await?;
        source::validate_same_assessment(
            &original,
            &actual,
            &context.received.id,
            &refreshed.received.id,
        )?;
        let components = source::assess(tx, &actual).await?;
        check_time(
            now(&clock)?,
            refreshed.order.identity.created_at,
            refreshed.received.recorded_at,
        )?;
        let footprint = ReviewFootprint {
            schema_version: 1,
            original: actual.clone(),
            components: components.clone(),
        };
        if ReviewReason::classify(&footprint)? != Some(ReviewReason::ComponentScarcity) {
            return Err(LateStockError::StateChanged);
        }
        return review(tx, &clock, ids, &refreshed, actual, components).await;
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
        .map(|component| {
            StageComponent::from_assessment(component)?.ok_or(PolicyError::InvalidSnapshot)
        })
        .collect::<Result<Vec<_>, PolicyError>>()?;
    let receipt = LateAllocation {
        id: fresh(ids)?,
        order_id: order.into(),
        attempt_id: attempt.into(),
        recognition_id: context.received.id,
        original_group_id: context.order.identity.stock_group_id,
        fresh_group_id: group,
        actor: ACTOR.into(),
        allocated_at: allocated,
        expires_at: expires,
        footprint: LateFootprint {
            schema_version: 1,
            original: original.clone(),
            fresh: fresh_plan.members()?,
            components: stages,
        },
    };
    receipt.validate()?;
    allocate(&mut savepoint, &clock, ids, &receipt, &fresh_plan).await?;
    savepoint.commit().await?;
    Ok(Outcome::Late(receipt))
}
/// Commit the exact frozen allocation using the shared physical hold primitives.
pub(super) async fn allocate(
    tx: &mut Transaction<'_, Postgres>,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
    receipt: &LateAllocation,
    plan: &FreshPreparation,
) -> Result<(), LateStockError> {
    rows::insert_allocation(tx, receipt).await?;
    for member in &receipt.footprint.original {
        if member.state == OriginalState::Active {
            crate::holds::expire_claimed(tx, &member.member.hold_id, receipt.allocated_at).await?;
        }
    }
    reserve_claimed(tx, clock, plan).await?;
    for member in &receipt.footprint.fresh {
        crate::holds::consume_claimed(tx, &member.hold_id, &receipt.order_id, receipt.allocated_at)
            .await?;
    }
    rows::allocation_event(tx, &receipt.id, &fresh(ids)?).await?;
    let final_at = now(clock)?;
    check_time(final_at, receipt.allocated_at, receipt.allocated_at)?;
    check_window(final_at, receipt.expires_at)?;
    Ok(())
}
pub(super) async fn prepare(
    tx: &mut Transaction<'_, Postgres>,
    ids: &dyn Identifiers,
    context: &Context,
    original: &[OriginalMember],
    group: &str,
    created: i64,
    expires: i64,
) -> Result<FreshPreparation, LateStockError> {
    let lines = original
        .iter()
        .map(|member| {
            Ok(BasketHoldLine {
                id: fresh(ids)?,
                sku: member.member.sku.clone(),
                revision: member.member.revision,
                units: member.member.units,
            })
        })
        .collect::<Result<Vec<_>, LateStockError>>()?;
    Ok(prepare_fresh(
        tx,
        BasketHoldRequest {
            id: group,
            owner_id: &context.order.identity.id,
            created_at: created,
            expires_at: expires,
            lines: &lines,
        },
    )
    .await?)
}
async fn review(
    tx: &mut Transaction<'_, Postgres>,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
    context: &Context,
    original: Vec<OriginalMember>,
    components: Vec<AssessedComponent>,
) -> Result<Outcome, LateStockError> {
    let at = now(clock)?;
    check_time(
        at,
        context.order.identity.created_at,
        context.received.recorded_at,
    )?;
    let footprint = ReviewFootprint {
        schema_version: 1,
        original,
        components,
    };
    let reason = ReviewReason::classify(&footprint)?.ok_or(LateStockError::StateChanged)?;
    let case = ReviewCase {
        id: fresh(ids)?,
        order_id: context.order.identity.id.clone(),
        attempt_id: context.received.attempt_id.clone(),
        recognition_id: context.received.id.clone(),
        group_id: context.order.identity.stock_group_id.clone(),
        actor: REVIEW_ACTOR.into(),
        reason,
        assessed_at: at,
        footprint,
    };
    case.validate()?;
    rows::insert_review(tx, &case).await?;
    rows::review_event(tx, &case.id, &fresh(ids)?).await?;
    check_time(now(clock)?, at, at)?;
    Ok(Outcome::NeedsReview(case))
}
