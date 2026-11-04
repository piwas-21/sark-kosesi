//! Coherent bounded original custody and database-ordered current balance claims.
use super::*;
#[derive(sqlx::FromRow)]
struct Reference {
    hold_id: String,
    component_code: String,
    action: String,
    sequence: i64,
    digest: String,
}
pub(super) async fn original(
    tx: &mut Transaction<'_, Postgres>,
    order: &OrderSnapshot,
    update: bool,
) -> Result<Vec<OriginalMember>, LateStockError> {
    let source = crate::order_stock::source::members(tx, order, update).await?;
    let holds: Vec<_> = source
        .members
        .iter()
        .map(|member| member.hold_id.as_str())
        .collect();
    let references:Vec<Reference>=sqlx::query_as("SELECT left(m.hold_id,33) AS hold_id,left(m.component_code,66) AS component_code,left(m.action,20) AS action,m.sequence,late_stock_movement_digest(m) AS digest FROM stock_movements m WHERE m.hold_id=ANY($1) ORDER BY m.sequence LIMIT 2561")
        .bind(holds).fetch_all(&mut **tx).await?;
    if references.len() > commerce_domain::order_stock::MAX_PAIRS * 2 {
        return Err(LateStockError::InvalidStoredData);
    }
    let mut original = Vec::with_capacity(source.members.len());
    for (member, state) in source.members.into_iter().zip(source.states) {
        let (state, terminal) = match state.as_str() {
            "active" => (OriginalState::Active, None),
            "expired" => (OriginalState::Expired, Some("expire-hold")),
            "released" => (OriginalState::Released, Some("release-hold")),
            "consumed" => (OriginalState::Consumed, Some("consume-hold")),
            _ => return Err(LateStockError::InvalidStoredData),
        };
        let mut pairs = Vec::with_capacity(member.components.len());
        for component in &member.components {
            let reserve = references
                .iter()
                .find(|r| {
                    r.hold_id == member.hold_id
                        && r.component_code == component.component_code
                        && r.action == "reserve"
                })
                .ok_or(LateStockError::InvalidStoredData)?;
            let terminal = terminal
                .map(|action| {
                    references
                        .iter()
                        .find(|r| {
                            r.hold_id == member.hold_id
                                && r.component_code == component.component_code
                                && r.action == action
                        })
                        .ok_or(LateStockError::InvalidStoredData)
                })
                .transpose()?;
            pairs.push(PairReference {
                component_code: component.component_code.clone(),
                reserve_sequence: reserve.sequence,
                reserve_digest: reserve.digest.clone(),
                terminal_sequence: terminal.map(|r| r.sequence),
                terminal_digest: terminal.map(|r| r.digest.clone()),
            });
        }
        let member = OriginalMember {
            member,
            state,
            pairs,
        };
        member.validate()?;
        original.push(member);
    }
    summarize(&original)?;
    Ok(original)
}
#[derive(sqlx::FromRow)]
struct Physical {
    component_code: String,
    on_hand: i64,
    quarantine: i64,
    reserved: i64,
}
pub(super) async fn assess(
    tx: &mut Transaction<'_, Postgres>,
    original: &[OriginalMember],
) -> Result<Vec<AssessedComponent>, LateStockError> {
    let (mut full, active) = summarize(original)?;
    let codes: Vec<_> = full.keys().map(String::as_str).collect();
    let rows:Vec<Physical>=sqlx::query_as("SELECT left(component_code,66) AS component_code,on_hand,quarantine,reserved FROM stock_balances WHERE component_code=ANY($1) ORDER BY component_code LIMIT 1281 FOR UPDATE")
        .bind(codes).fetch_all(&mut **tx).await?;
    if rows.len() != full.len() {
        return Err(LateStockError::InvalidStoredData);
    }
    rows.into_iter()
        .map(|row| {
            let component = AssessedComponent {
                demand: full
                    .remove(&row.component_code)
                    .ok_or(LateStockError::InvalidStoredData)?,
                active: active.get(&row.component_code).copied().unwrap_or(0),
                component_code: row.component_code,
                before: commerce_domain::order_stock::Balance {
                    on_hand: row.on_hand,
                    quarantine: row.quarantine,
                    reserved: row.reserved,
                },
            };
            component.available()?;
            Ok(component)
        })
        .collect()
}
pub(super) fn historical(
    captured: &[OriginalMember],
    current: &[OriginalMember],
) -> Result<(), LateStockError> {
    if captured.len() != current.len() {
        return Err(LateStockError::InvalidStoredData);
    }
    for (captured, current) in captured.iter().zip(current) {
        captured.validate()?;
        if captured.member != current.member || captured.pairs.len() != current.pairs.len() {
            return Err(LateStockError::InvalidStoredData);
        }
        for (old, actual) in captured.pairs.iter().zip(&current.pairs) {
            if old.component_code != actual.component_code
                || old.reserve_sequence != actual.reserve_sequence
                || old.reserve_digest != actual.reserve_digest
                || (captured.state != OriginalState::Active
                    && (captured.state != current.state
                        || old.terminal_sequence != actual.terminal_sequence
                        || old.terminal_digest != actual.terminal_digest))
            {
                return Err(LateStockError::InvalidStoredData);
            }
        }
    }
    Ok(())
}

pub(super) fn validate_same_assessment(
    expected: &[OriginalMember],
    actual: &[OriginalMember],
    expected_recognition: &str,
    actual_recognition: &str,
) -> Result<(), LateStockError> {
    if actual != expected || actual_recognition != expected_recognition {
        return Err(LateStockError::StateChanged);
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/support/late_stock_source_unit.rs"]
mod source_tests;
