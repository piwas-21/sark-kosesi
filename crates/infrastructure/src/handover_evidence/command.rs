use super::*;
use crate::{packing_custody, staff_access::StaffTransaction};
use std::collections::BTreeSet;

async fn claim(access: &mut StaffTransaction, order: &str) -> Result<String, Error> {
    let attempt = crate::order_stock::source::attempt(&mut access.tx, order)
        .await
        .map_err(|_| Error::Unavailable)?;
    crate::order_stock::source::claim_attempt(&mut access.tx, &attempt)
        .await
        .map_err(|_| Error::Database)?;
    Ok(attempt)
}

fn advance(watermark: &mut i64, time: impl Iterator<Item = i64>) {
    *watermark = (*watermark).max(time.max().unwrap_or(0));
}

async fn basis(
    access: &mut StaffTransaction,
    order: &str,
    attempt: &str,
    parent: &str,
    watermark: &mut i64,
) -> Result<Vec<packing_custody::Record>, Error> {
    let packs = packing_custody::store::history(&mut access.tx, order)
        .await
        .map_err(packing)?;
    advance(watermark, packs.iter().map(|p| p.recorded_at));
    validate_basis(access, order, attempt, parent, &packs).await?;
    Ok(packs)
}

async fn validate_basis(
    access: &mut StaffTransaction,
    order: &str,
    attempt: &str,
    parent: &str,
    packs: &[packing_custody::Record],
) -> Result<(), Error> {
    let pack = packs
        .iter()
        .find(|p| p.id == parent && p.action == "packed")
        .ok_or(Error::Conflict)?;
    if pack.attempt_id != attempt {
        return Err(Error::InvalidStoredData);
    }
    let src = crate::shipment_planning::source(
        &mut access.tx,
        order,
        attempt,
        Some(&pack.recognition_id),
    )
    .await
    .map_err(source)?;
    packing_custody::model::validate(&src, packs).map_err(packing)?;
    Ok(())
}

pub(super) async fn execute(
    access: &mut StaffTransaction,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
    req: &Request<'_>,
    digest: &str,
    watermark: &mut i64,
) -> Result<Receipt, Error> {
    store::lock_key(&mut access.tx, req.effect_key).await?;
    let existing = store::lookup(&mut access.tx, req.effect_key).await?;
    let order = existing
        .as_ref()
        .map_or(req.order_id, |(order, _)| order.as_str());
    let attempt = claim(access, order).await?;
    if existing.as_ref().is_some_and(|(_, a)| a != &attempt) {
        return Err(Error::InvalidStoredData);
    }
    access.check_time(clock).map_err(staff)?;
    let history = store::history(&mut access.tx, order).await?;
    advance(watermark, history.iter().map(|r| r.recorded_at));
    let old = history.iter().find(|r| r.effect_key == req.effect_key);
    if existing.is_some() && old.is_none() {
        return Err(Error::InvalidStoredData);
    }
    let parent = old.map_or(req.packing_id, |r| r.packing_id.as_str());
    let packs = basis(access, order, &attempt, parent, watermark).await?;
    if let Some(old) = old {
        if order != req.order_id
            || old.actor_id != access.actor
            || old.packing_id != req.packing_id
            || old.evidence_sha256 != digest
        {
            return Err(Error::Conflict);
        }
        return Ok(Receipt {
            submission: old.clone(),
            replayed: true,
        });
    }
    if packs
        .iter()
        .any(|p| p.action == "released" && p.target_id.as_deref() == Some(req.packing_id))
        || history.len() >= 64
        || history
            .iter()
            .filter(|r| r.packing_id == req.packing_id)
            .count()
            >= 4
        || history
            .iter()
            .any(|r| r.packing_id == req.packing_id && r.evidence_sha256 == digest)
    {
        return Err(Error::Conflict);
    }
    let at = access.authorized_at(clock).map_err(staff)?;
    if at < *watermark {
        return Err(Error::ClockUnavailable);
    }
    let submission = Submission {
        id: fresh(ids)?,
        order_id: req.order_id.into(),
        attempt_id: attempt,
        packing_id: req.packing_id.into(),
        actor_id: access.actor.clone(),
        effect_key: req.effect_key.into(),
        generation: i16::try_from(history.len() + 1).map_err(|_| Error::InvalidStoredData)?,
        evidence_sha256: digest.into(),
        recorded_at: at,
        event_id: fresh(ids)?,
    };
    store::insert(&mut access.tx, &submission).await?;
    let written = store::history(&mut access.tx, req.order_id).await?;
    advance(watermark, written.iter().map(|r| r.recorded_at));
    if written.last() != Some(&submission) {
        return Err(Error::InvalidStoredData);
    }
    Ok(Receipt {
        submission,
        replayed: false,
    })
}

pub(super) async fn inspect(
    access: &mut StaffTransaction,
    order: &str,
    watermark: &mut i64,
) -> Result<Vec<(Submission, bool)>, Error> {
    let attempt = claim(access, order).await?;
    let submissions = store::history(&mut access.tx, order).await?;
    advance(watermark, submissions.iter().map(|s| s.recorded_at));
    let Some(first) = submissions.first() else {
        return Ok(Vec::new());
    };
    let packs = basis(access, order, &attempt, &first.packing_id, watermark).await?;
    let mut recognitions = BTreeSet::new();
    let mut result = Vec::with_capacity(submissions.len());
    for submission in submissions {
        let parent = packs
            .iter()
            .find(|p| p.id == submission.packing_id && p.action == "packed")
            .ok_or(Error::InvalidStoredData)?;
        if recognitions.insert(parent.recognition_id.as_str()) && recognitions.len() > 1 {
            let (frozen, recognition) =
                crate::payment_attempts::recognition::historical_in_transaction(
                    &mut access.tx,
                    &attempt,
                    &parent.recognition_id,
                )
                .await
                .map_err(|e| match e {
                    crate::payment_attempts::PaymentStoreError::Database
                    | crate::payment_attempts::PaymentStoreError::Boundary(
                        crate::checkout_orders::OrderStoreError::Database,
                    ) => Error::Database,
                    _ => Error::InvalidStoredData,
                })?;
            if recognition.state != crate::payment_attempts::recognition::State::Received
                || recognition.order_id != order
                || frozen.snapshot().binding.order_id != order
            {
                return Err(Error::InvalidStoredData);
            }
        }
        let released = packs.iter().any(|p| {
            p.action == "released" && p.target_id.as_deref() == Some(&submission.packing_id)
        });
        result.push((submission, released));
    }
    Ok(result)
}
