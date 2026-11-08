use super::*;
pub(super) struct Access {
    pub tx: Transaction<'static, Postgres>,
    pub identity: GuestIdentity,
    started: i64,
}
impl Access {
    pub fn checked_at(&mut self, clock: &dyn Clock) -> Result<i64, GuestError> {
        let at = now(clock)?;
        if at < self.started {
            return Err(GuestError::ClockUnavailable);
        }
        self.identity.check(at)?;
        self.started = at;
        Ok(at)
    }
}
pub(super) async fn lock_mode(
    store: &GuestSessionStore,
    key: &GuestCredential,
    csrf: Option<&str>,
    exclusive: bool,
) -> Result<Access, GuestError> {
    let tx = begin(&store.pool).await?;
    lock_in_transaction(key, csrf, exclusive, tx, store.clock.as_ref()).await
}

pub(super) async fn lock_in_transaction(
    key: &GuestCredential,
    csrf: Option<&str>,
    exclusive: bool,
    mut tx: Transaction<'static, Postgres>,
    clock: &dyn Clock,
) -> Result<Access, GuestError> {
    let identity = identity::load(&mut tx, &key.digest(), exclusive)
        .await?
        .ok_or(GuestError::Denied)?;
    let started = now(clock)?;
    identity.check(started)?;
    if let Some(csrf) = csrf
        && (!valid_hex(csrf, 64)
            || !bool::from(csrf.as_bytes().ct_eq(identity.csrf_token.as_bytes())))
    {
        return Err(GuestError::Csrf);
    }
    Ok(Access {
        tx,
        identity,
        started,
    })
}

pub(super) async fn lock(
    store: &GuestSessionStore,
    key: &GuestCredential,
    csrf: Option<&str>,
) -> Result<Access, GuestError> {
    lock_mode(store, key, csrf, false).await
}
