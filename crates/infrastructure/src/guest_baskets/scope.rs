//! Crate-private composition guard; caller rechecks every retained source after writes.
use super::*;
use commerce_application::request_context::ObservedClock;
pub(crate) struct BasketScope {
    access: access::Access,
    revision: i32,
    lines: Vec<BasketLine>,
    clock: Arc<dyn Clock>,
    identifiers: Arc<dyn Identifiers>,
}
impl BasketScope {
    pub(crate) fn basket_id(&self) -> &str {
        &self.access.identity.basket_id
    }
    pub(crate) fn revision(&self) -> i32 {
        self.revision
    }
    pub(crate) fn lines(&self) -> &[BasketLine] {
        &self.lines
    }
    pub(crate) fn clock(&self) -> Arc<dyn Clock> {
        self.clock.clone()
    }
    pub(crate) fn new_id(&self) -> Result<String, GuestError> {
        Ok(format!(
            "{:032x}",
            u128::from_be_bytes(self.identifiers.next()?)
        ))
    }
    pub(crate) fn transaction(&mut self) -> &mut Transaction<'static, Postgres> {
        &mut self.access.tx
    }
    pub(crate) fn checked_at(&mut self) -> Result<i64, GuestError> {
        self.access.checked_at(self.clock.as_ref())
    }
    /// No reads/writes may follow the caller's final combined source/authority clock check.
    pub(crate) async fn commit(self) -> Result<(), GuestError> {
        self.access.tx.commit().await?;
        Ok(())
    }
    pub(crate) async fn rollback(self) -> Result<(), GuestError> {
        self.access.tx.rollback().await?;
        Ok(())
    }
}
async fn claim(
    store: &GuestSessionStore,
    key: &GuestCredential,
    csrf: Option<&str>,
    write: bool,
) -> Result<BasketScope, GuestError> {
    let mut access = access::lock(store, key, csrf).await?;
    let (revision, lines, window) =
        basket::locked_intent(&mut access.tx, &access.identity.basket_id, write).await?;
    let at = access.checked_at(store.clock.as_ref())?;
    if window > at / 60 {
        return Err(GuestError::ClockUnavailable);
    }
    let minimum = u64::try_from(at).map_err(|_| GuestError::ClockUnavailable)?;
    Ok(BasketScope {
        access,
        revision,
        lines,
        clock: Arc::new(ObservedClock::new(store.clock.clone(), minimum)),
        identifiers: store.identifiers.clone(),
    })
}
pub(crate) async fn mutation_scope(
    store: &GuestSessionStore,
    key: &GuestCredential,
    csrf: &str,
) -> Result<BasketScope, GuestError> {
    claim(store, key, Some(csrf), true).await
}
pub(crate) async fn read_scope(
    store: &GuestSessionStore,
    key: &GuestCredential,
) -> Result<BasketScope, GuestError> {
    claim(store, key, None, false).await
}
