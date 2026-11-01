//! Private inbox admission/supervision over isolated PostgreSQL and authenticated GETs.
use super::*;
use crate::payment_attempts::callbacks::inbox::{self as store, *};
use commerce_domain::payment_attempts::callback_inbox::{JobState, ReviewReason};
async fn intake_counts(pool: &PgPool) -> (i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM callback_intakes),(SELECT count(*) FROM callback_intake_events),(SELECT count(*) FROM callback_inbox_jobs)")
        .fetch_one(pool).await.expect("complete independent intake custody")
}
async fn admit(
    pool: &PgPool,
    clock: &dyn commerce_application::request_context::Clock,
    i: &dyn commerce_application::request_context::Identifiers,
    profile: &str,
    provider: &str,
) -> String {
    admit_callback(pool, clock, i, profile, provider)
        .await
        .expect("bounded admitted callback")
        .intake_id
}
async fn waiting_query(pool: &PgPool, query: &str) {
    tokio::time::timeout(Duration::from_millis(800),async {
        loop {
            let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE $1)").bind(query).fetch_one(pool).await.expect("actual intended write wait");
            if waiting {break;}
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.expect("reached actual intended statement lock");
}

#[path = "callback_inbox_admission.rs"]
mod admission;

#[path = "callback_inbox_supervisor.rs"]
mod supervisor;

#[path = "callback_inbox_recovery.rs"]
mod recovery;

#[path = "callback_inbox_native.rs"]
mod native;

#[path = "callback_inbox_clocks.rs"]
mod clocks;

#[path = "callback_inbox_worker.rs"]
mod worker;

#[path = "callback_inbox_processing.rs"]
mod processing;

#[path = "callback_inbox_source.rs"]
mod source;

#[path = "callback_inbox_quarantine.rs"]
mod quarantine;
