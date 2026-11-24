//! Private encrypted guest-link custody and one in-process start.
use super::*;
use commerce_domain::{checkout_quotes::MAX_QUOTE_UNIX_SECONDS, email_identity::normalize_email};
use sha2::{Digest, Sha256};
mod crypto;
mod opening;
pub mod outcome;
#[cfg(feature = "test-verification-smtp")]
pub mod test_smtp;
pub use opening::{OpenOutcome, claim_and_open, claim_open_and_record};

/// Privately supplied root key, purpose-derived before encryption.
pub struct DeliveryKey {
    id: String,
    derived: Zeroizing<[u8; 32]>,
    fingerprint: [u8; 32],
}
impl DeliveryKey {
    pub fn capture(id: &str, bytes: [u8; 32]) -> Result<Self, LinkError> {
        let bytes = Zeroizing::new(bytes);
        if id.is_empty()
            || id.len() > 64
            || !id.as_bytes()[0].is_ascii_alphanumeric()
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            || bytes.iter().all(|b| *b == 0)
        {
            return Err(LinkError::Unavailable);
        }
        let derived = crypto::derive(&bytes)?;
        let mut hash = Sha256::new();
        hash.update(b"piwas:guest-order-link:delivery:key-fingerprint:v1\0");
        hash.update(derived.as_slice());
        Ok(Self {
            id: id.into(),
            derived,
            fingerprint: hash.finalize().into(),
        })
    }
}
/// Identifies the stored proof without exposing its bearer, ciphertext or recipient.
#[derive(Debug, PartialEq, Eq)]
pub struct DeliveryReceipt {
    pub verifier: [u8; 32],
    pub created_at: i64,
    pub expires_at: i64,
}

fn now(clock: &dyn Clock) -> Result<i64, LinkError> {
    clock
        .unix_seconds()
        .ok()
        .and_then(|v| i64::try_from(v).ok())
        .filter(|v| *v <= MAX_QUOTE_UNIX_SECONDS)
        .ok_or(LinkError::Unavailable)
}
async fn durable(tx: &mut Transaction<'_, Postgres>) -> Result<(), LinkError> {
    let ok: bool = sqlx::query_scalar(
        "SELECT current_setting('fsync')='on' AND current_setting('synchronous_commit')='on'",
    )
    .fetch_one(&mut **tx)
    .await
    .map_err(|_| LinkError::Unavailable)?;
    if !ok {
        return Err(LinkError::Unavailable);
    }
    Ok(())
}
async fn begin_durable(pool: &PgPool) -> Result<Transaction<'static, Postgres>, LinkError> {
    let mut tx = super::super::begin(pool).await.map_err(LinkError::from)?;
    sqlx::query("SET LOCAL synchronous_commit='on'")
        .execute(&mut *tx)
        .await
        .map_err(|_| LinkError::Unavailable)?;
    Ok(tx)
}
/// Register one immutable identifier and reject material aliases within this purpose.
pub async fn register_key(
    pool: &PgPool,
    clock: &dyn Clock,
    key: &DeliveryKey,
) -> Result<(), LinkError> {
    let observed = ObservedClock::new(clock, 0);
    let at = now(&observed)?;
    let mut tx = begin_durable(pool).await?;
    let inserted = sqlx::query("INSERT INTO guest_link_delivery_keys(key_id,key_fingerprint,registered_at) VALUES($1,$2,$3) ON CONFLICT (key_id) DO NOTHING")
        .bind(&key.id).bind(key.fingerprint.as_slice()).bind(at).execute(&mut *tx).await.map_err(|_| LinkError::Unavailable)?.rows_affected();
    let row: Option<(Vec<u8>, i64)> = sqlx::query_as("SELECT key_fingerprint,registered_at FROM guest_link_delivery_keys WHERE key_id=$1 FOR SHARE")
        .bind(&key.id).fetch_optional(&mut *tx).await.map_err(|_| LinkError::Unavailable)?;
    let (fingerprint, registered_at) = row.ok_or(LinkError::Unavailable)?;
    if fingerprint.as_slice() != key.fingerprint
        || registered_at < 0
        || (inserted == 1 && registered_at != at)
    {
        return Err(LinkError::Unavailable);
    }
    durable(&mut tx).await?;
    if now(&observed)? < registered_at {
        return Err(LinkError::Unavailable);
    }
    tx.commit().await.map_err(|_| LinkError::Unavailable)
}
struct NonceReservation {
    counter: i64,
    verifier: [u8; 32],
    key_id: String,
    fingerprint: [u8; 32],
}
async fn reserve(
    pool: &PgPool,
    clock: &dyn Clock,
    key: &DeliveryKey,
    verifier: &[u8; 32],
) -> Result<NonceReservation, LinkError> {
    let mut tx = begin_durable(pool).await?;
    let fingerprint: Option<Vec<u8>> = sqlx::query_scalar(
        "SELECT key_fingerprint FROM guest_link_delivery_keys WHERE key_id=$1 FOR UPDATE",
    )
    .bind(&key.id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| LinkError::Unavailable)?;
    if fingerprint.as_deref() != Some(key.fingerprint.as_slice()) {
        return Err(LinkError::Unavailable);
    }
    let at = now(clock)?;
    let counter: i64 = sqlx::query_scalar("INSERT INTO guest_link_delivery_nonces(key_id,verifier,reserved_at) VALUES($1,$2,$3) RETURNING nonce_counter")
        .bind(&key.id).bind(verifier.as_slice()).bind(at).fetch_one(&mut *tx).await.map_err(|_| LinkError::Unavailable)?;
    let exact: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM guest_link_delivery_nonces WHERE key_id=$1 AND nonce_counter=$2 AND verifier=$3 AND reserved_at=$4 AND transaction_id=pg_current_xact_id())")
        .bind(&key.id).bind(counter).bind(verifier.as_slice()).bind(at).fetch_one(&mut *tx).await.map_err(|_| LinkError::Unavailable)?;
    if !exact || counter <= 0 {
        return Err(LinkError::Unavailable);
    }
    durable(&mut tx).await?;
    now(clock)?;
    tx.commit().await.map_err(|_| LinkError::Unavailable)?;
    Ok(NonceReservation {
        counter,
        verifier: *verifier,
        key_id: key.id.clone(),
        fingerprint: key.fingerprint,
    })
}

/// Capture the current exact guest/order authority before reserving a nonce.
async fn preflight(
    store: &GuestSessionStore,
    guest: &GuestCredential,
    csrf: &str,
    order: &str,
    clock: &dyn Clock,
) -> Result<(String, Zeroizing<String>), LinkError> {
    let mut scope = access::lock(store, guest, Some(csrf)).await?;
    let basket = scope.identity.basket_id.clone();
    let result = async {
        require_unlinked_order(&mut scope.tx, &basket, order).await?;
        let snapshot =
            checkout_orders::guest_snapshot_in_transaction(&mut scope.tx, &basket, order)
                .await
                .map_err(order_error)?;
        normalize_email(&snapshot.buyer.email)
            .map(Zeroizing::new)
            .ok_or(LinkError::Denied)
    }
    .await;
    scope.checked_at(clock)?;
    let email = result?;
    scope
        .tx
        .rollback()
        .await
        .map_err(|_| LinkError::Unavailable)?;
    Ok((basket, email))
}

/// Persist a proof and encrypted envelope only for the live owner of its exact order.
pub async fn issue_for_delivery(
    store: &GuestSessionStore,
    guest: &GuestCredential,
    csrf: &str,
    order: &str,
    key: &DeliveryKey,
) -> Result<DeliveryReceipt, LinkError> {
    if !valid_hex(order, 32) {
        return Err(LinkError::Denied);
    }
    let clock = ObservedClock::new(store.clock.as_ref(), 0);
    now(&clock)?;
    let (basket, identity) = preflight(store, guest, csrf, order, &clock).await?;
    let token = GuestOrderLinkToken::random()?;
    let verifier = token.verifier();
    let reserved = reserve(&store.pool, &clock, key, &verifier).await;
    let mut scope = access::lock(store, guest, Some(csrf)).await?;
    scope.checked_at(&clock)?;
    let reservation = reserved?;
    let result: Result<DeliveryReceipt, LinkError> = async {
        if scope.identity.basket_id != basket {
            return Err(LinkError::Denied);
        }
        sqlx::query("SET LOCAL synchronous_commit='on'")
            .execute(&mut *scope.tx).await.map_err(|_| LinkError::Unavailable)?;
        require_unlinked_order(&mut scope.tx, &basket, order).await?;
        let snapshot = checkout_orders::guest_snapshot_in_transaction(&mut scope.tx, &basket, order)
            .await.map_err(order_error)?;
        if normalize_email(&snapshot.buyer.email).as_deref() != Some(identity.as_str()) {
            return Err(LinkError::Denied);
        }
        let created_at = scope.checked_at(&clock)?;
        let expires_at = created_at.checked_add(LIFETIME)
            .filter(|v| *v <= MAX_QUOTE_UNIX_SECONDS).ok_or(LinkError::Unavailable)?;
        let counter = reservation.counter;
        let context = crypto::Context {
            basket: &basket, order, identity: &identity, verifier: &verifier,
            created_at, expires_at, key_id: &key.id, fingerprint: &key.fingerprint, counter,
        };
        let sealed = crypto::seal(key, &context, &token, reservation)?;
        sqlx::query("INSERT INTO guest_order_link_proofs(verifier,order_id,basket_id,created_at,expires_at,delivery_mode) VALUES($1,$2,$3,$4,$5,'envelope')")
            .bind(verifier.as_slice()).bind(order).bind(&basket).bind(created_at).bind(expires_at)
            .execute(&mut *scope.tx).await.map_err(|_| LinkError::Unavailable)?;
        sqlx::query("INSERT INTO guest_link_delivery_envelopes(verifier,key_id,nonce_counter,envelope_version,ciphertext) VALUES($1,$2,$3,$4,$5)")
            .bind(verifier.as_slice()).bind(&key.id).bind(counter).bind(crypto::VERSION).bind(&sealed)
            .execute(&mut *scope.tx).await.map_err(|_| LinkError::Unavailable)?;
        sqlx::query("SET CONSTRAINTS require_guest_link_proof_envelope,require_guest_link_envelope IMMEDIATE")
            .execute(&mut *scope.tx).await.map_err(|_| LinkError::Unavailable)?;
        let exact: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM guest_order_link_proofs p JOIN guest_link_delivery_envelopes e ON e.verifier=p.verifier WHERE p.verifier=$1 AND p.order_id=$2 AND p.basket_id=$3 AND p.created_at=$4 AND p.expires_at=$5 AND p.delivery_mode='envelope' AND p.consumed_at IS NULL AND p.consumed_account_id IS NULL AND p.consumed_xid IS NULL AND p.delivery_transaction_id=pg_current_xact_id() AND e.key_id=$6 AND e.nonce_counter=$7 AND e.envelope_version=$8 AND e.ciphertext=$9 AND e.transaction_id=pg_current_xact_id())")
            .bind(verifier.as_slice()).bind(order).bind(&basket).bind(created_at).bind(expires_at)
            .bind(&key.id).bind(counter).bind(crypto::VERSION).bind(&sealed)
            .fetch_one(&mut *scope.tx).await.map_err(|_| LinkError::Unavailable)?;
        if !exact {
            return Err(LinkError::Unavailable);
        }
        durable(&mut scope.tx).await?;
        let at = scope.checked_at(&clock)?;
        if at >= expires_at {
            return Err(LinkError::Denied);
        }
        Ok(DeliveryReceipt { verifier, created_at, expires_at })
    }.await;
    let final_at = scope.checked_at(&clock)?;
    let receipt = result?;
    if final_at >= receipt.expires_at {
        return Err(LinkError::Denied);
    }
    let held_identity = scope.identity;
    if scope.tx.commit().await.is_err() {
        held_identity.check(now(&clock)?)?;
        return Err(LinkError::Unavailable);
    }
    Ok(receipt)
}

#[cfg(test)]
#[path = "../../../tests/support/guest_link_delivery_unit.rs"]
mod tests;
