//! Audited merchant candidates, separate from fixture preview and publication.
use super::staff_access::{SessionToken, StaffError, lock_staff};
use commerce_application::{request_context::Clock, staff_access::Capability};
use commerce_domain::{
    catalogue_authoring::CatalogueCommand, media::valid_command_id, specifications::bounded_text,
};
use sqlx::{PgPool, Postgres, Transaction};
mod apply;
mod events;
pub use events::definition_event;

pub struct DefinitionRequest<'a> {
    pub effect_key: &'a str,
    pub command: &'a CatalogueCommand,
    pub source_ref: &'a str,
    pub reason: &'a str,
    pub confirmed: bool,
}
#[derive(Debug, PartialEq, Eq)]
pub struct DefinitionReceipt {
    pub bom_revision: Option<i32>,
    pub replayed: bool,
}
#[derive(Debug, PartialEq, Eq)]
pub struct DefinitionEvent {
    pub command: CatalogueCommand,
    pub bom_revision: Option<i32>,
    pub actor: String,
    pub source_ref: String,
    pub reason: String,
}
#[derive(Debug)]
pub enum DefinitionError {
    Staff(StaffError),
    Database(sqlx::Error),
    InvalidRequest,
    InvalidStoredData,
    NotFound,
    Conflict,
}
impl From<StaffError> for DefinitionError {
    fn from(value: StaffError) -> Self {
        Self::Staff(value)
    }
}
impl From<sqlx::Error> for DefinitionError {
    fn from(value: sqlx::Error) -> Self {
        Self::Database(value)
    }
}
fn text(value: &str) -> bool {
    bounded_text(value, 500) && !value.contains('\0')
}

pub async fn record_definition(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    request: DefinitionRequest<'_>,
) -> Result<DefinitionReceipt, DefinitionError> {
    let mut access = lock_staff(pool, token, clock, Capability::CatalogueManage).await?;
    if !request.confirmed
        || !valid_command_id(request.effect_key)
        || !text(request.source_ref)
        || !text(request.reason)
    {
        return Err(DefinitionError::InvalidRequest);
    }
    let command = request
        .command
        .normalized()
        .map_err(|_| DefinitionError::InvalidRequest)?;
    let payload = serde_json::to_string(&command).map_err(|_| DefinitionError::InvalidRequest)?;
    if payload.len() > 65536 {
        return Err(DefinitionError::InvalidRequest);
    }
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("catalogue-definition:{}", request.effect_key))
        .execute(&mut *access.tx)
        .await?;
    if let Some(prior) = events::load(&mut access.tx, request.effect_key).await? {
        if prior.command != command
            || prior.actor != access.actor
            || prior.source_ref != request.source_ref
            || prior.reason != request.reason
        {
            return Err(DefinitionError::Conflict);
        }
        access.check_time(clock)?;
        access.tx.rollback().await?;
        return Ok(DefinitionReceipt {
            bom_revision: prior.bom_revision,
            replayed: true,
        });
    }
    let bom_revision = command.bom_revision();
    sqlx::query("INSERT INTO catalogue_definition_events(effect_key,payload,bom_revision,actor,source_ref,reason) VALUES($1,$2::jsonb,$3,$4,$5,$6)")
        .bind(request.effect_key).bind(payload).bind(bom_revision).bind(&access.actor).bind(request.source_ref).bind(request.reason).execute(&mut *access.tx).await?;
    apply::command(&mut access.tx, &command, request.effect_key).await?;
    access.check_time(clock)?;
    access.tx.commit().await?;
    Ok(DefinitionReceipt {
        bom_revision,
        replayed: false,
    })
}
