use super::*;
use crate::{packing_custody, staff_access::StaffTransaction};

async fn claim(access: &mut StaffTransaction, order: &str) -> Result<String, Error> {
    let attempt = crate::order_stock::source::attempt(&mut access.tx, order)
        .await
        .map_err(|_| Error::Unavailable)?;
    crate::order_stock::source::claim_attempt(&mut access.tx, &attempt)
        .await
        .map_err(|_| Error::Database)?;
    Ok(attempt)
}

async fn basis(
    access: &mut StaffTransaction,
    order: &str,
    attempt: &str,
    parent: &str,
) -> Result<Vec<packing_custody::Record>, Error> {
    let packs = packing_custody::store::history(&mut access.tx, order)
        .await
        .map_err(packing)?;
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
    packing_custody::model::validate(&src, &packs).map_err(packing)?;
    Ok(packs)
}

pub(super) async fn execute(
    access: &mut StaffTransaction,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
    req: &Request<'_>,
    digest: &str,
) -> Result<Receipt, Error> {
    store::lock_key(&mut access.tx, req.effect_key).await?;
    let existing = store::lookup(&mut access.tx, req.effect_key).await?;
    if existing
        .as_ref()
        .is_some_and(|(order, _)| order != req.order_id)
    {
        return Err(Error::Conflict);
    }
    let attempt = claim(access, req.order_id).await?;
    if existing.as_ref().is_some_and(|(_, a)| a != &attempt) {
        return Err(Error::InvalidStoredData);
    }
    access.check_time(clock).map_err(staff)?;
    let history = store::history(&mut access.tx, req.order_id).await?;
    let packs = basis(access, req.order_id, &attempt, req.packing_id).await?;
    if let Some(old) = history.iter().find(|r| r.effect_key == req.effect_key) {
        if old.actor_id != access.actor
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
    let parent = packs
        .iter()
        .find(|p| p.id == req.packing_id)
        .ok_or(Error::InvalidStoredData)?;
    if at < parent.recorded_at || history.last().is_some_and(|r| at < r.recorded_at) {
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
) -> Result<Vec<(Submission, bool)>, Error> {
    let attempt = claim(access, order).await?;
    let submissions = store::history(&mut access.tx, order).await?;
    let mut result = Vec::with_capacity(submissions.len());
    for submission in submissions {
        let packs = basis(access, order, &attempt, &submission.packing_id).await?;
        let released = packs.iter().any(|p| {
            p.action == "released" && p.target_id.as_deref() == Some(&submission.packing_id)
        });
        result.push((submission, released));
    }
    Ok(result)
}
