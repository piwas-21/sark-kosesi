//! One durable local start followed by a trusted synchronous in-process opening.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenOutcome {
    Opened,
    AlreadyStarted,
}

mod validation;
use validation::{Envelope, Proof};

#[derive(PartialEq, Eq)]
struct Source {
    basket: String,
    order: String,
    identity: Zeroizing<String>,
    created_at: i64,
    expires_at: i64,
    envelope: Envelope,
}
impl Source {
    fn live(&self, clock: &dyn Clock) -> Result<(), LinkError> {
        let at = now(clock)?;
        if at < self.created_at {
            return Err(LinkError::Unavailable);
        }
        if at >= self.expires_at {
            return Err(LinkError::Denied);
        }
        Ok(())
    }
}

async fn source(
    tx: &mut Transaction<'_, Postgres>,
    verifier: &[u8; 32],
    key: &DeliveryKey,
    clock: &dyn Clock,
) -> Result<Source, LinkError> {
    // Redemption locks proof before order. Re-read joined custody only after waits.
    let p: Option<Proof> = sqlx::query_as("SELECT left(order_id,33) AS order_id,left(basket_id,33) AS basket_id,created_at,expires_at,consumed_at,consumed_account_id,consumed_xid IS NULL AS unconsumed_xid FROM guest_order_link_proofs WHERE verifier=$1 FOR UPDATE")
        .bind(verifier.as_slice()).fetch_optional(&mut **tx).await.map_err(|_| LinkError::Unavailable)?;
    let p = p.ok_or(LinkError::Denied)?;
    p.validate()?;
    require_unlinked_order(tx, &p.basket_id, &p.order_id).await?;
    let snapshot = checkout_orders::guest_snapshot_in_transaction(tx, &p.basket_id, &p.order_id)
        .await
        .map_err(order_error)?;
    let identity = Zeroizing::new(normalize_email(&snapshot.buyer.email).ok_or(LinkError::Denied)?);
    let e: Option<Envelope> = sqlx::query_as("SELECT left(e.key_id,65) AS key_id,e.nonce_counter,k.key_fingerprint AS fingerprint,e.ciphertext,e.envelope_version AS version,p.delivery_transaction_id::text AS proof_xid,e.transaction_id::text AS envelope_xid,n.transaction_id::text AS nonce_xid,n.verifier AS nonce_verifier,n.reserved_at,k.registered_at FROM guest_order_link_proofs p JOIN guest_link_delivery_envelopes e ON e.verifier=p.verifier JOIN guest_link_delivery_nonces n ON n.key_id=e.key_id AND n.nonce_counter=e.nonce_counter JOIN guest_link_delivery_keys k ON k.key_id=e.key_id WHERE p.verifier=$1 AND p.delivery_mode='envelope'")
        .bind(verifier.as_slice()).fetch_optional(&mut **tx).await.map_err(|_| LinkError::Unavailable)?;
    let e = e.ok_or(LinkError::Denied)?;
    e.validate(key, verifier, p.created_at)?;
    let source = Source {
        basket: p.basket_id,
        order: p.order_id,
        identity,
        created_at: p.created_at,
        expires_at: p.expires_at,
        envelope: e,
    };
    source.live(clock)?;
    Ok(source)
}

#[derive(sqlx::FromRow)]
struct Started {
    key_id: String,
    nonce_counter: i64,
    fingerprint: Vec<u8>,
    started_at: i64,
    transaction_id: String,
    current_transaction: bool,
    valid_source: bool,
}

/// Constructed only after this call's fresh start was durably committed.
pub(super) struct Permit {
    pub(super) verifier: [u8; 32],
    key_id: String,
    counter: i64,
    fingerprint: Vec<u8>,
    started_at: i64,
    transaction_id: String,
    ciphertext_sha256: [u8; 32],
}
impl Permit {
    pub(super) async fn matches_start(
        &self,
        tx: &mut Transaction<'_, Postgres>,
    ) -> Result<bool, LinkError> {
        let exact: Option<bool> = sqlx::query_scalar("SELECT transaction_id::text=$2 AND started_at=$3 AND key_id=$4 AND nonce_counter=$5 AND key_fingerprint=$6 FROM guest_link_delivery_starts WHERE verifier=$1 FOR SHARE")
            .bind(self.verifier.as_slice()).bind(&self.transaction_id).bind(self.started_at)
            .bind(&self.key_id).bind(self.counter).bind(&self.fingerprint)
            .fetch_optional(&mut **tx).await.map_err(|_| LinkError::Unavailable)?;
        Ok(exact == Some(true))
    }
    pub(super) fn transaction_id(&self) -> &str {
        &self.transaction_id
    }
    pub(super) fn ciphertext_sha256(&self) -> &[u8; 32] {
        &self.ciphertext_sha256
    }
}
async fn read_start(
    tx: &mut Transaction<'_, Postgres>,
    verifier: &[u8; 32],
) -> Result<Option<Started>, LinkError> {
    sqlx::query_as("SELECT left(s.key_id,65) AS key_id,s.nonce_counter,s.key_fingerprint AS fingerprint,s.started_at,s.transaction_id::text AS transaction_id,coalesce(s.transaction_id=pg_current_xact_id_if_assigned(),false) AS current_transaction,coalesce(guest_link_delivery_start_source(s),false) AS valid_source FROM guest_link_delivery_starts s WHERE s.verifier=$1")
        .bind(verifier.as_slice()).fetch_optional(&mut **tx).await.map_err(|_| LinkError::Unavailable)
}
fn check_start(
    start: &Started,
    source: &Source,
    key: &DeliveryKey,
    clock: &dyn Clock,
) -> Result<(), LinkError> {
    if start.key_id != key.id
        || start.nonce_counter != source.envelope.nonce_counter
        || start.fingerprint.as_slice() != key.fingerprint
        || !start.valid_source
        || start.started_at < source.created_at
        || start.started_at >= source.expires_at
        || start.started_at > now(clock)?
    {
        return Err(LinkError::Unavailable);
    }
    source.live(clock)
}

fn check_new_start(start: &Started, at: i64) -> Result<(), LinkError> {
    if !start.current_transaction || start.started_at != at {
        return Err(LinkError::Unavailable);
    }
    Ok(())
}

fn check_same_source(current: &Source, original: &Source) -> Result<(), LinkError> {
    if current != original {
        return Err(LinkError::Unavailable);
    }
    Ok(())
}

fn check_same_start(saved: &Started, original: &Started) -> Result<(), LinkError> {
    if saved.transaction_id != original.transaction_id || saved.started_at != original.started_at {
        return Err(LinkError::Unavailable);
    }
    Ok(())
}

/// Commit a single local start, then authenticate its live envelope and call the
/// trusted synchronous consumer once. Neither start nor callback proves delivery.
/// State can change after the final liveness check; no DB lock spans the callback.
pub async fn claim_and_open(
    pool: &PgPool,
    verifier: &[u8; 32],
    clock: &dyn Clock,
    key: &DeliveryKey,
    accept: impl FnOnce(&str, &GuestOrderLinkToken),
) -> Result<OpenOutcome, LinkError> {
    let observed = ObservedClock::new(clock, 0);
    match open(pool, verifier, &observed, key, None).await? {
        Some(opened) => {
            accept(&opened.identity, &opened.token);
            Ok(OpenOutcome::Opened)
        }
        None => Ok(OpenOutcome::AlreadyStarted),
    }
}

/// Record only this fresh opening's closed synthetic callback classification.
/// A result never establishes provider or mailbox acceptance.
pub async fn claim_open_and_record(
    pool: &PgPool,
    verifier: &[u8; 32],
    clock: &dyn Clock,
    key: &DeliveryKey,
    accept: impl FnOnce(&str, &GuestOrderLinkToken) -> super::outcome::TransportOutcome,
) -> Result<OpenOutcome, LinkError> {
    let observed = ObservedClock::new(clock, 0);
    match open(pool, verifier, &observed, key, None).await? {
        Some(opened) => {
            let result = accept(&opened.identity, &opened.token);
            let FreshOpening {
                identity,
                token,
                permit,
            } = opened;
            drop((identity, token));
            super::outcome::record(pool, &permit, &observed, result).await?;
            Ok(OpenOutcome::Opened)
        }
        None => Ok(OpenOutcome::AlreadyStarted),
    }
}

pub(super) struct FreshOpening {
    pub(super) identity: Zeroizing<String>,
    pub(super) token: GuestOrderLinkToken,
    pub(super) permit: Permit,
}

pub(super) async fn open(
    pool: &PgPool,
    verifier: &[u8; 32],
    observed: &dyn Clock,
    key: &DeliveryKey,
    only_identity: Option<&str>,
) -> Result<Option<FreshOpening>, LinkError> {
    now(observed)?;
    let mut tx = begin_durable(pool).await?;
    let original = source(&mut tx, verifier, key, observed).await?;
    if only_identity.is_some_and(|identity| identity != original.identity.as_str()) {
        return Err(LinkError::Denied);
    }
    if let Some(start) = read_start(&mut tx, verifier).await? {
        check_start(&start, &original, key, observed)?;
        durable(&mut tx).await?;
        original.live(observed)?;
        tx.commit().await.map_err(|_| LinkError::Unavailable)?;
        return Ok(None);
    }
    let at = now(observed)?;
    sqlx::query("INSERT INTO guest_link_delivery_starts(verifier,key_id,nonce_counter,key_fingerprint,started_at) VALUES($1,$2,$3,$4,$5)")
        .bind(verifier.as_slice()).bind(&key.id).bind(original.envelope.nonce_counter)
        .bind(key.fingerprint.as_slice()).bind(at).execute(&mut *tx).await.map_err(|_| LinkError::Unavailable)?;
    sqlx::query("SET CONSTRAINTS require_guest_link_delivery_start IMMEDIATE")
        .execute(&mut *tx)
        .await
        .map_err(|_| LinkError::Unavailable)?;
    let start = read_start(&mut tx, verifier)
        .await?
        .ok_or(LinkError::Unavailable)?;
    check_start(&start, &original, key, observed)?;
    check_new_start(&start, at)?;
    durable(&mut tx).await?;
    original.live(observed)?;
    tx.commit().await.map_err(|_| LinkError::Unavailable)?;

    let mut tx = super::super::super::begin(pool)
        .await
        .map_err(LinkError::from)?;
    let current = source(&mut tx, verifier, key, observed).await?;
    check_same_source(&current, &original)?;
    let saved = read_start(&mut tx, verifier)
        .await?
        .ok_or(LinkError::Unavailable)?;
    check_start(&saved, &current, key, observed)?;
    check_same_start(&saved, &start)?;
    let context = crypto::Context {
        basket: &current.basket,
        order: &current.order,
        identity: &current.identity,
        verifier,
        created_at: current.created_at,
        expires_at: current.expires_at,
        key_id: &key.id,
        fingerprint: &key.fingerprint,
        counter: current.envelope.nonce_counter,
    };
    let token = crypto::open(key, &context, &current.envelope.ciphertext)?;
    // Seal only the ciphertext that this opening actually authenticated.
    let permit = Permit {
        verifier: *verifier,
        key_id: start.key_id.clone(),
        counter: start.nonce_counter,
        fingerprint: start.fingerprint.clone(),
        started_at: start.started_at,
        transaction_id: start.transaction_id.clone(),
        ciphertext_sha256: Sha256::digest(&current.envelope.ciphertext).into(),
    };
    current.live(observed)?;
    tx.commit().await.map_err(|_| LinkError::Unavailable)?;
    current.live(observed)?;
    Ok(Some(FreshOpening {
        identity: current.identity,
        token,
        permit,
    }))
}

#[cfg(test)]
#[path = "../../../tests/support/guest_link_open_unit.rs"]
mod tests;
