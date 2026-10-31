use super::*;
impl OrderSnapshot {
    /// Reconstruct immutable accepted history. Current quote expiry never erases an order.
    pub fn validate(&self) -> Result<(), OrderError> {
        let id = &self.identity;
        let binding = &self.quote.binding;
        if self.schema_version != ORDER_SNAPSHOT_VERSION
            || !hex(&id.id, 32)
            || !hex(&id.stock_group_id, 32)
            || id.basket_id != binding.basket_id
            || id.quote_id != binding.quote_id
            || self.products.len() != self.quote.lines.len()
            || self.products.len() > 20
            || self.shipping_address.country != binding.country
            || self.shipping_address.postcode != binding.postcode
        {
            return Err(OrderError::InvalidSnapshot);
        }
        self.quote
            .check_at(id.created_at)
            .map_err(|_| OrderError::InvalidSnapshot)?;
        if self.market.id != binding.policy_id
            || self.market.revision != binding.policy_revision
            || self.market.shipping.gross.minor_units() != self.quote.shipping.gross_minor
            || self.market.shipping.vat_basis_points != self.quote.shipping_vat_basis_points
        {
            return Err(OrderError::InvalidSnapshot);
        }
        self.market
            .permits(
                &crate::quotes::Destination {
                    country: self.shipping_address.country.clone(),
                    postcode: self.shipping_address.postcode.clone(),
                },
                id.created_at,
            )
            .map_err(|_| OrderError::InvalidSnapshot)?;
        for line in &self.quote.lines {
            let rule = self
                .market
                .product(&line.sku, line.publication_revision, line.quantity)
                .map_err(|_| OrderError::InvalidSnapshot)?;
            if rule.vat_basis_points != line.vat_basis_points {
                return Err(OrderError::InvalidSnapshot);
            }
        }

        crate::holds::HoldWindow::new(id.created_at, id.hold_expires_at)
            .map_err(|_| OrderError::InvalidSnapshot)?;
        if id.hold_expires_at > crate::checkout_quotes::MAX_QUOTE_UNIX_SECONDS
            || self.quote.total.gross_minor <= 0
        {
            return Err(OrderError::InvalidSnapshot);
        }
        self.buyer
            .validate()
            .map_err(|_| OrderError::InvalidSnapshot)?;
        self.shipping_address
            .validate()
            .map_err(|_| OrderError::InvalidSnapshot)?;
        self.billing_address
            .validate()
            .map_err(|_| OrderError::InvalidSnapshot)?;
        self.seller
            .permits(&binding.locale, id.created_at)
            .map_err(|_| OrderError::InvalidSnapshot)?;
        for (product, line) in self.products.iter().zip(&self.quote.lines) {
            if product.sku != line.sku
                || product.publication_revision != line.publication_revision
                || product.bom_revision != line.bom_revision
                || !text(&product.title, 200)
                || !crate::catalogue_authoring::merchant_code(&product.family_code)
                || !text(&product.size_code, 80)
                || !text(&product.textile_code, 80)
                || !["covers-only", "with-foam"].contains(&product.fill.as_str())
                || product.footprint.validate().is_err()
            {
                return Err(OrderError::InvalidSnapshot);
            }
        }
        Ok(())
    }
}
