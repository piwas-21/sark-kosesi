use super::*;
use commerce_domain::{
    checkout_quotes::QuoteSnapshot,
    quotes::{BasketLine, Destination, QuoteRequest},
};
pub(super) struct Eligibility {
    sources: quotes::QuoteProof,
    reservation: holds::basket::ActiveBasket,
    seller_expires_at: i64,
}
impl Eligibility {
    pub(super) fn check_at(&mut self, at: i64) -> Result<(), PaymentStoreError> {
        self.sources.check_at(at)?;
        self.reservation.check_at(at)?;
        if at >= self.seller_expires_at {
            return Err(PaymentStoreError::Unavailable);
        }
        Ok(())
    }
}
/// Accepted order history is independent of today's cart and the old indicative TTL.
/// Current readiness and the exact owned reservation still govern NEW payment work.
pub(super) async fn claim(
    tx: &mut Transaction<'_, Postgres>,
    clock: &dyn commerce_application::request_context::Clock,
    order: &OrderSnapshot,
) -> Result<Eligibility, PaymentStoreError> {
    let locale = Locale::from_internal(&order.quote.binding.locale)
        .ok_or(PaymentStoreError::InvalidStoredData)?;
    let request = QuoteRequest {
        destination: Destination {
            country: order.shipping_address.country.clone(),
            postcode: order.shipping_address.postcode.clone(),
        },
        lines: order
            .quote
            .lines
            .iter()
            .map(|line| BasketLine {
                sku: line.sku.clone(),
                quantity: line.quantity,
            })
            .collect(),
    };
    let policy = market_policies::reviewed_policy_in_transaction(tx, &request).await?;
    if policy.policy != order.market
        || policy.review_revision != order.quote.binding.policy_review_revision
    {
        return Err(PaymentStoreError::Unavailable);
    }
    // Retain catalogue/asset claims BEFORE member and component claims to avoid publisher cycles.
    let sources = quotes::prepare_captured_sources_in_transaction(
        tx,
        clock,
        locale,
        &request,
        std::slice::from_ref(&policy.policy),
    )
    .await?;
    let current = QuoteSnapshot::capture(
        order.quote.binding.clone(),
        &sources.original.totals,
        &sources.bom_revisions(),
    )
    .map_err(|_| PaymentStoreError::InvalidStoredData)?;
    if current != order.quote || checkout_orders::product_snapshots(&sources) != order.products {
        return Err(PaymentStoreError::Unavailable);
    }
    let lines = order
        .quote
        .lines
        .iter()
        .map(|line| {
            Ok(holds::basket::ExistingBasketLine {
                sku: line.sku.as_str(),
                revision: line.bom_revision,
                units: u16::try_from(line.quantity)
                    .map_err(|_| PaymentStoreError::InvalidStoredData)?,
            })
        })
        .collect::<Result<Vec<_>, PaymentStoreError>>()?;
    let reservation = holds::basket::claim_active(
        tx,
        clock,
        holds::basket::ExistingBasketBinding {
            id: &order.identity.stock_group_id,
            owner_id: &order.identity.id,
            created_at: order.identity.created_at,
            expires_at: order.identity.hold_expires_at,
            lines: &lines,
        },
    )
    .await?;
    let at = i64::try_from(
        clock
            .unix_seconds()
            .map_err(|_| PaymentStoreError::ClockUnavailable)?,
    )
    .map_err(|_| PaymentStoreError::ClockUnavailable)?;
    order
        .seller
        .permits(locale.internal(), at)
        .map_err(|_| PaymentStoreError::Unavailable)?;
    let mut proof = Eligibility {
        sources,
        reservation,
        seller_expires_at: order.seller.expires_at,
    };
    proof.check_at(at)?;
    Ok(proof)
}

#[cfg(test)]
#[path = "../../tests/support/payment_eligibility.rs"]
mod tests;
