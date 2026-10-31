use super::*;
use commerce_application::guest_access::MAX_GUEST_LIFETIME_SECONDS;
use tower_sessions::Expiry;
fn library_error(error: tower_sessions::session::Error) -> GuestError {
    if matches!(error,tower_sessions::session::Error::Store(session_store::Error::Backend(ref code)) if code=="guest_issue_rate_limited")
    {
        GuestError::RateLimited
    } else {
        GuestError::SessionUnavailable
    }
}
/// Call only after same-origin validation and abuse controls at the HTTP boundary.
/// A valid supplied session is reused. Unknown credentials never select a basket ID.
pub async fn ensure_session(
    store: &GuestSessionStore,
    session: &Session,
) -> Result<BasketView, GuestError> {
    if let Ok(key) = GuestCredential::from_session(session) {
        match read_basket(store, &key).await {
            Ok(view) => return Ok(view),
            Err(GuestError::Denied) => session.flush().await.map_err(library_error)?,
            Err(error) => return Err(error),
        }
    }
    let created_at = now(store.clock.as_ref())?;
    let expires_at = created_at
        .checked_add(MAX_GUEST_LIFETIME_SECONDS)
        .ok_or(GuestError::ClockUnavailable)?;
    let basket_id = format!("{:032x}", u128::from_be_bytes(store.identifiers.next()?));
    let first = u128::from_be_bytes(store.identifiers.next()?);
    let second = u128::from_be_bytes(store.identifiers.next()?);
    let identity = GuestIdentity {
        basket_id,
        csrf_token: format!("{first:032x}{second:032x}"),
        created_at,
        expires_at,
    };
    identity.validate()?;
    session.set_expiry(Some(Expiry::AtDateTime(identity.expiry()?)));
    session
        .insert("guest", identity)
        .await
        .map_err(library_error)?;
    if let Err(error) = session.save().await {
        // Do not let response middleware retry a rejected creation or replace its status.
        session.clear().await;
        return Err(library_error(error));
    }
    read_basket(store, &GuestCredential::from_session(session)?).await
}
pub async fn revoke_session(
    store: &GuestSessionStore,
    session: &Session,
    csrf: &str,
) -> Result<(), GuestError> {
    let key = GuestCredential::from_session(session)?;
    let mut access = access::lock_mode(store, &key, Some(csrf), true).await?;
    // An exclusive credential claim serializes revocations against every live operation.
    sqlx::query("DELETE FROM guest_sessions WHERE token_hash=$1")
        .bind(key.digest().as_slice())
        .execute(&mut *access.tx)
        .await?;
    access.checked_at(store.clock.as_ref())?;
    access.tx.commit().await?;
    session.flush().await.map_err(library_error)?;
    Ok(())
}
