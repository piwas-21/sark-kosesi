use super::*;
#[derive(sqlx::FromRow)]
struct StoredEvent {
    payload: String,
    bom_revision: Option<i32>,
    actor: String,
    source_ref: String,
    reason: String,
}
fn decode(row: StoredEvent) -> Result<DefinitionEvent, DefinitionError> {
    if row.payload.len() > 131072
        || !valid_command_id(&row.actor)
        || !text(&row.source_ref)
        || !text(&row.reason)
    {
        return Err(DefinitionError::InvalidStoredData);
    }
    let command: CatalogueCommand =
        serde_json::from_str(&row.payload).map_err(|_| DefinitionError::InvalidStoredData)?;
    let command = command
        .normalized()
        .map_err(|_| DefinitionError::InvalidStoredData)?;
    if command.bom_revision() != row.bom_revision {
        return Err(DefinitionError::InvalidStoredData);
    }
    Ok(DefinitionEvent {
        command,
        bom_revision: row.bom_revision,
        actor: row.actor,
        source_ref: row.source_ref,
        reason: row.reason,
    })
}
pub(super) async fn load(
    tx: &mut Transaction<'_, Postgres>,
    key: &str,
) -> Result<Option<DefinitionEvent>, DefinitionError> {
    let row=sqlx::query_as::<_,StoredEvent>("SELECT payload::text,bom_revision,actor,source_ref,reason FROM catalogue_definition_events WHERE effect_key=$1")
        .bind(key).fetch_optional(&mut **tx).await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let result = decode(row)?;
    let matches: bool = sqlx::query_scalar(
        "SELECT definition_event_matches(e) FROM catalogue_definition_events e WHERE effect_key=$1",
    )
    .bind(key)
    .fetch_one(&mut **tx)
    .await?;
    if !matches {
        return Err(DefinitionError::InvalidStoredData);
    }
    Ok(Some(result))
}
pub async fn definition_event(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    key: &str,
) -> Result<Option<DefinitionEvent>, DefinitionError> {
    let mut access = lock_staff(pool, token, clock, Capability::CatalogueManage).await?;
    if !valid_command_id(key) {
        return Err(DefinitionError::InvalidRequest);
    }
    let event = load(&mut access.tx, key).await?;
    access.check_time(clock)?;
    access.tx.rollback().await?;
    Ok(event)
}

#[cfg(test)]
#[path = "../../tests/support/definition_events.rs"]
mod tests;
