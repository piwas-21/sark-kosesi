//! Private, bounded customer favourites. No HTTP route or delivery is installed.
use crate::{
    customer_sessions::{self, CustomerSessionToken, SessionError},
    published_catalogue,
};
use commerce_application::{
    Locale,
    request_context::{Clock, ObservedClock},
};
use commerce_domain::catalogue_authoring::merchant_code;
use sqlx::{PgPool, Postgres, Transaction};

const CAPACITY: i64 = 25;

#[derive(Debug, PartialEq, Eq)]
pub enum FavouriteError {
    Denied,
    Unavailable,
    Conflict,
    Capacity,
    InvalidRequest,
}
#[derive(Debug, PartialEq, Eq)]
pub struct FavouriteItem {
    pub sku: String,
    pub publication_revision: i32,
}
#[derive(Debug, PartialEq, Eq)]
pub struct FavouriteList {
    pub revision: i64,
    pub items: Vec<FavouriteItem>,
}

fn failure(_: impl Sized) -> FavouriteError {
    FavouriteError::Unavailable
}
fn session_failure(error: SessionError) -> FavouriteError {
    match error {
        SessionError::Denied => FavouriteError::Denied,
        SessionError::Unavailable => FavouriteError::Unavailable,
    }
}
fn now(clock: &dyn Clock) -> Result<i64, FavouriteError> {
    clock
        .unix_seconds()
        .ok()
        .and_then(|v| i64::try_from(v).ok())
        .ok_or(FavouriteError::Unavailable)
}
async fn bucket(tx: &mut Transaction<'_, Postgres>, account: i64) -> Result<i64, FavouriteError> {
    sqlx::query(
        "INSERT INTO customer_favourite_buckets(account_id) VALUES ($1) ON CONFLICT DO NOTHING",
    )
    .bind(account)
    .execute(&mut **tx)
    .await
    .map_err(failure)?;
    sqlx::query_scalar(
        "SELECT revision FROM customer_favourite_buckets WHERE account_id=$1 FOR UPDATE",
    )
    .bind(account)
    .fetch_one(&mut **tx)
    .await
    .map_err(failure)
}
async fn saved(
    tx: &mut Transaction<'_, Postgres>,
    account: i64,
) -> Result<Vec<String>, FavouriteError> {
    let skus: Vec<String> = sqlx::query_scalar("SELECT sku FROM customer_favourites WHERE account_id=$1 ORDER BY sku COLLATE \"C\" LIMIT 26")
        .bind(account).fetch_all(&mut **tx).await.map_err(failure)?;
    if skus.len() > CAPACITY as usize || skus.iter().any(|sku| !merchant_code(sku)) {
        return Err(FavouriteError::Unavailable);
    }
    Ok(skus)
}

/// A list never returns stored identities without current public eligibility.
pub async fn list(
    pool: &PgPool,
    token: &CustomerSessionToken,
    clock: &dyn Clock,
    locale: Locale,
) -> Result<FavouriteList, FavouriteError> {
    let observed = ObservedClock::new(clock, 0);
    now(&observed)?;
    let mut tx = customer_sessions::begin(pool)
        .await
        .map_err(session_failure)?;
    let session = customer_sessions::claim_session(&mut tx, token, &observed)
        .await
        .map_err(session_failure)?;
    let revision = bucket(&mut tx, session.account_id()).await?;
    let skus = saved(&mut tx, session.account_id()).await?;
    let products = published_catalogue::load_eligible(&mut tx, &observed, locale, &skus)
        .await
        .map_err(failure)?;
    let captured = now(&observed)?;
    session.check_at(captured).map_err(session_failure)?;
    let items = products
        .into_iter()
        .filter(|p| p.rights_active(captured))
        .map(|p| FavouriteItem {
            sku: p.product.sku,
            publication_revision: p.product.publication_revision,
        })
        .collect();
    tx.commit().await.map_err(failure)?;
    Ok(FavouriteList { revision, items })
}

/// Set membership with optimistic revision. An already-satisfied request is a no-op;
/// a stale request cannot reverse a subsequent change.
pub async fn set(
    pool: &PgPool,
    token: &CustomerSessionToken,
    clock: &dyn Clock,
    locale: Locale,
    sku: &str,
    desired: bool,
    expected_revision: i64,
) -> Result<i64, FavouriteError> {
    if !merchant_code(sku) || expected_revision < 0 {
        return Err(FavouriteError::InvalidRequest);
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
    let skus = saved(&mut tx, account).await?;
    let present = skus.iter().any(|saved| saved == sku);
    let eligible = if desired {
        let products =
            published_catalogue::load_eligible(&mut tx, &observed, locale, &[sku.to_owned()])
                .await
                .map_err(failure)?;
        if products.len() != 1 {
            return Err(FavouriteError::Unavailable);
        }
        products.into_iter().next()
    } else {
        None
    };
    if present != desired {
        if revision != expected_revision {
            return Err(FavouriteError::Conflict);
        }
        if desired && skus.len() >= CAPACITY as usize {
            return Err(FavouriteError::Capacity);
        }
        let sql = if desired {
            "INSERT INTO customer_favourites(account_id,sku) VALUES ($1,$2)"
        } else {
            "DELETE FROM customer_favourites WHERE account_id=$1 AND sku=$2"
        };
        if sqlx::query(sql)
            .bind(account)
            .bind(sku)
            .execute(&mut *tx)
            .await
            .map_err(failure)?
            .rows_affected()
            != 1
        {
            return Err(FavouriteError::Unavailable);
        }
    }
    let expected = revision
        .checked_add(i64::from(present != desired))
        .ok_or(FavouriteError::Unavailable)?;
    let actual = bucket(&mut tx, account).await?;
    let after = saved(&mut tx, account).await?;
    if actual != expected || after.iter().any(|entry| entry == sku) != desired {
        return Err(FavouriteError::Unavailable);
    }
    let captured = now(&observed)?;
    session.check_at(captured).map_err(session_failure)?;
    if eligible.is_some_and(|p| !p.rights_active(captured)) {
        return Err(FavouriteError::Unavailable);
    }
    tx.commit().await.map_err(failure)?;
    Ok(actual)
}

/// Clear includes currently hidden entries, and retains the monotonic bucket revision.
pub async fn clear_all(
    pool: &PgPool,
    token: &CustomerSessionToken,
    clock: &dyn Clock,
    expected_revision: i64,
) -> Result<i64, FavouriteError> {
    if expected_revision < 0 {
        return Err(FavouriteError::InvalidRequest);
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
    let skus = saved(&mut tx, account).await?;
    if !skus.is_empty() {
        if revision != expected_revision {
            return Err(FavouriteError::Conflict);
        }
        let deleted = sqlx::query("DELETE FROM customer_favourites WHERE account_id=$1")
            .bind(account)
            .execute(&mut *tx)
            .await
            .map_err(failure)?
            .rows_affected();
        if deleted != skus.len() as u64 {
            return Err(FavouriteError::Unavailable);
        }
    }
    let expected = revision
        .checked_add(skus.len() as i64)
        .ok_or(FavouriteError::Unavailable)?;
    let actual = bucket(&mut tx, account).await?;
    if actual != expected || !saved(&mut tx, account).await?.is_empty() {
        return Err(FavouriteError::Unavailable);
    }
    session.check_at(now(&observed)?).map_err(session_failure)?;
    tx.commit().await.map_err(failure)?;
    Ok(actual)
}
