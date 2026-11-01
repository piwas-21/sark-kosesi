//! Test-only pause keyed to one isolated database, reader backend and order.
use super::*;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, OnceLock},
};
use tokio::sync::Notify;
type Key = (String, i32, String);
static GATES: OnceLock<Mutex<BTreeMap<Key, Arc<Gate>>>> = OnceLock::new();
pub(crate) struct Gate {
    pub(crate) reached: Notify,
    pub(crate) release: Notify,
}
pub(crate) async fn register(tx: &mut Transaction<'_, Postgres>, order: &str) -> Arc<Gate> {
    let (database, pid): (String, i32) =
        sqlx::query_as("SELECT current_database(),pg_backend_pid()")
            .fetch_one(&mut **tx)
            .await
            .expect("specific isolated reader identity");
    let gate = Arc::new(Gate {
        reached: Notify::new(),
        release: Notify::new(),
    });
    assert!(
        GATES
            .get_or_init(|| Mutex::new(BTreeMap::new()))
            .lock()
            .expect("test registry")
            .insert((database, pid, order.into()), gate.clone())
            .is_none(),
        "one explicit reader gate"
    );
    gate
}
pub(super) async fn pause(
    tx: &mut Transaction<'_, Postgres>,
    order: &str,
) -> Result<(), SettlementError> {
    let gates = GATES.get_or_init(|| Mutex::new(BTreeMap::new()));
    if gates.lock().expect("test registry").is_empty() {
        return Ok(());
    }
    let (database, pid): (String, i32) =
        sqlx::query_as("SELECT current_database(),pg_backend_pid()")
            .fetch_one(&mut **tx)
            .await?;
    let gate = gates
        .lock()
        .expect("test registry")
        .remove(&(database, pid, order.into()));
    if let Some(gate) = gate {
        gate.reached.notify_one();
        gate.release.notified().await;
    }
    Ok(())
}
