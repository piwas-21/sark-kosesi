//! Private restock opt-in custody. No notification delivery or public account route.
use crate::{
    customer_sessions::{self, CustomerCsrfToken, CustomerSessionToken},
    published_catalogue,
};
use commerce_application::{
    Locale,
    request_context::{Clock, ObservedClock},
};
use commerce_domain::catalogue_authoring::merchant_code;
use sqlx::PgPool;
mod state;
use state::{Membership, bucket, failure, now, saved, session_failure};

const CAPACITY: usize = 25;

#[derive(Debug, PartialEq, Eq)]
pub enum RestockError {
    Denied,
    Unavailable,
    Conflict,
    Capacity,
    InvalidRequest,
}
#[derive(Debug, PartialEq, Eq)]
pub struct RestockItem {
    pub sku: String,
    pub publication_revision: i32,
    pub generation: i64,
}
#[derive(Debug, PartialEq, Eq)]
pub struct RestockList {
    pub revision: i64,
    pub items: Vec<RestockItem>,
}
/// List only current published identities. Hidden opt-ins can still be cleared.
pub async fn list(
    pool: &PgPool,
    token: &CustomerSessionToken,
    clock: &dyn Clock,
    locale: Locale,
) -> Result<RestockList, RestockError> {
    let observed = ObservedClock::new(clock, 0);
    now(&observed)?;
    let mut tx = customer_sessions::begin(pool)
        .await
        .map_err(session_failure)?;
    let session = customer_sessions::claim_session(&mut tx, token, &observed)
        .await
        .map_err(session_failure)?;
    let revision = bucket(&mut tx, session.account_id()).await?;
    let entries = saved(&mut tx, session.account_id(), revision).await?;
    let skus: Vec<String> = entries.iter().map(|entry| entry.sku.clone()).collect();
    let products = published_catalogue::load_eligible(&mut tx, &observed, locale, &skus)
        .await
        .map_err(failure)?;
    let captured = now(&observed)?;
    session.check_at(captured).map_err(session_failure)?;
    let mut items = Vec::new();
    for eligible in products.into_iter().filter(|p| p.rights_active(captured)) {
        let generation = entries
            .iter()
            .find(|entry| entry.sku == eligible.product.sku)
            .ok_or(RestockError::Unavailable)?
            .generation;
        items.push(RestockItem {
            sku: eligible.product.sku,
            publication_revision: eligible.product.publication_revision,
            generation,
        });
    }
    tx.commit().await.map_err(failure)?;
    Ok(RestockList { revision, items })
}

/// Explicitly opt in to one current published merchant variant.
pub async fn subscribe(
    pool: &PgPool,
    token: &CustomerSessionToken,
    clock: &dyn Clock,
    locale: Locale,
    sku: &str,
    expected_revision: i64,
) -> Result<i64, RestockError> {
    set(
        pool,
        token,
        None,
        clock,
        Some(locale),
        sku,
        expected_revision,
    )
    .await
}
/// Cancel one opt-in, including a now-unpublished variant known to the owner.
pub async fn unsubscribe(
    pool: &PgPool,
    token: &CustomerSessionToken,
    clock: &dyn Clock,
    sku: &str,
    expected_revision: i64,
) -> Result<i64, RestockError> {
    set(pool, token, None, clock, None, sku, expected_revision).await
}
/// Opt in only under the exact live session's bound synchronizer.
pub async fn subscribe_with_csrf(
    pool: &PgPool,
    token: &CustomerSessionToken,
    csrf: &CustomerCsrfToken,
    clock: &dyn Clock,
    locale: Locale,
    sku: &str,
    expected_revision: i64,
) -> Result<i64, RestockError> {
    set(
        pool,
        token,
        Some(csrf),
        clock,
        Some(locale),
        sku,
        expected_revision,
    )
    .await
}

/// Cancel an opt-in, including a hidden SKU, with the bound synchronizer.
pub async fn unsubscribe_with_csrf(
    pool: &PgPool,
    token: &CustomerSessionToken,
    csrf: &CustomerCsrfToken,
    clock: &dyn Clock,
    sku: &str,
    expected_revision: i64,
) -> Result<i64, RestockError> {
    set(pool, token, Some(csrf), clock, None, sku, expected_revision).await
}

async fn set(
    pool: &PgPool,
    token: &CustomerSessionToken,
    csrf: Option<&CustomerCsrfToken>,
    clock: &dyn Clock,
    locale: Option<Locale>,
    sku: &str,
    expected_revision: i64,
) -> Result<i64, RestockError> {
    if !merchant_code(sku) || expected_revision < 0 {
        return Err(RestockError::InvalidRequest);
    }
    let observed = ObservedClock::new(clock, 0);
    now(&observed)?;
    let mut tx = customer_sessions::begin(pool)
        .await
        .map_err(session_failure)?;
    let session = match csrf {
        Some(csrf) => {
            customer_sessions::claim_session_with_csrf(&mut tx, token, csrf, &observed).await
        }
        None => customer_sessions::claim_session(&mut tx, token, &observed).await,
    }
    .map_err(session_failure)?;
    let account = session.account_id();
    let revision = bucket(&mut tx, account).await?;
    let mut expected = saved(&mut tx, account, revision).await?;
    session.check_at(now(&observed)?).map_err(session_failure)?;
    let position = expected.iter().position(|entry| entry.sku == sku);
    let desired = locale.is_some();
    let eligible = if let Some(locale) = locale {
        let products =
            published_catalogue::load_eligible(&mut tx, &observed, locale, &[sku.to_owned()])
                .await
                .map_err(failure)?;
        session.check_at(now(&observed)?).map_err(session_failure)?;
        if products.len() != 1 {
            return Err(RestockError::Unavailable);
        }
        products.into_iter().next()
    } else {
        None
    };
    let changed = position.is_some() != desired;
    if changed {
        if revision != expected_revision {
            return Err(RestockError::Conflict);
        }
        if desired && expected.len() >= CAPACITY {
            return Err(RestockError::Capacity);
        }
        let next = revision.checked_add(1).ok_or(RestockError::Unavailable)?;
        if desired {
            let at = now(&observed)?;
            session.check_at(at).map_err(session_failure)?;
            let result = sqlx::query("INSERT INTO customer_restock_subscriptions(account_id,sku,generation,opted_in_at) VALUES ($1,$2,NULL,$3)")
                .bind(account).bind(sku).bind(at).execute(&mut *tx).await.map_err(failure)?;
            if result.rows_affected() != 1 {
                return Err(RestockError::Unavailable);
            }
            expected.push(Membership {
                sku: sku.to_owned(),
                generation: next,
                opted_in_at: at,
            });
            expected.sort_by(|a, b| a.sku.cmp(&b.sku));
        } else if let Some(index) = position {
            let result = sqlx::query(
                "DELETE FROM customer_restock_subscriptions WHERE account_id=$1 AND sku=$2",
            )
            .bind(account)
            .bind(sku)
            .execute(&mut *tx)
            .await
            .map_err(failure)?;
            if result.rows_affected() != 1 {
                return Err(RestockError::Unavailable);
            }
            expected.remove(index);
        } else {
            return Err(RestockError::Unavailable);
        }
    }
    let wanted_revision = revision
        .checked_add(i64::from(changed))
        .ok_or(RestockError::Unavailable)?;
    let actual_revision = bucket(&mut tx, account).await?;
    if actual_revision != wanted_revision
        || saved(&mut tx, account, actual_revision).await? != expected
    {
        return Err(RestockError::Unavailable);
    }
    let captured = now(&observed)?;
    session.check_at(captured).map_err(session_failure)?;
    if eligible.is_some_and(|p| !p.rights_active(captured)) {
        return Err(RestockError::Unavailable);
    }
    tx.commit().await.map_err(failure)?;
    Ok(actual_revision)
}

/// Cancel all active opt-ins, including currently hidden identities.
pub async fn clear_all(
    pool: &PgPool,
    token: &CustomerSessionToken,
    clock: &dyn Clock,
    expected_revision: i64,
) -> Result<i64, RestockError> {
    if expected_revision < 0 {
        return Err(RestockError::InvalidRequest);
    }
    let observed = ObservedClock::new(clock, 0);
    now(&observed)?;
    let mut tx = customer_sessions::begin(pool)
        .await
        .map_err(session_failure)?;
    let session = customer_sessions::claim_session(&mut tx, token, &observed)
        .await
        .map_err(session_failure)?;
    let account = session.account_id();
    let revision = bucket(&mut tx, account).await?;
    let entries = saved(&mut tx, account, revision).await?;
    session.check_at(now(&observed)?).map_err(session_failure)?;
    if !entries.is_empty() {
        if revision != expected_revision {
            return Err(RestockError::Conflict);
        }
        let deleted = sqlx::query("DELETE FROM customer_restock_subscriptions WHERE account_id=$1")
            .bind(account)
            .execute(&mut *tx)
            .await
            .map_err(failure)?
            .rows_affected();
        if deleted != entries.len() as u64 {
            return Err(RestockError::Unavailable);
        }
    }
    let wanted_revision = revision
        .checked_add(entries.len() as i64)
        .ok_or(RestockError::Unavailable)?;
    let actual_revision = bucket(&mut tx, account).await?;
    if actual_revision != wanted_revision
        || !saved(&mut tx, account, actual_revision).await?.is_empty()
    {
        return Err(RestockError::Unavailable);
    }
    session.check_at(now(&observed)?).map_err(session_failure)?;
    tx.commit().await.map_err(failure)?;
    Ok(actual_revision)
}
