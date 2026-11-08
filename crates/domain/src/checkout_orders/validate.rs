use super::*;
impl OrderSnapshot {
    /// Reconstruct immutable accepted history. Current quote expiry never erases an order.
    pub fn validate(&self) -> Result<(), OrderError> {
        let id = &self.identity;
        let binding = &self.quote.binding;
        if self.schema_version
            != if self.promoted_quote.is_some() {
                2
            } else {
                ORDER_SNAPSHOT_VERSION
            }
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
            || self.payable_total().gross_minor <= 0
        {
            return Err(OrderError::InvalidSnapshot);
        }
        self.validate_promotion()?;
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

impl OrderSnapshot {
    pub fn payable_total(&self) -> &crate::checkout_quotes::CapturedAmount {
        self.promoted_quote
            .as_ref()
            .map_or(&self.quote.total, |p| &p.total)
    }
    pub fn payable_line_amounts(
        &self,
        index: usize,
    ) -> Option<&crate::checkout_quotes::CapturedAmount> {
        match &self.promoted_quote {
            Some(p) => p.lines.get(index).map(|line| &line.amounts),
            None => self.quote.lines.get(index).map(|line| &line.amounts),
        }
    }
    fn validate_promotion(&self) -> Result<(), OrderError> {
        let Some(p) = &self.promoted_quote else {
            return Ok(());
        };
        if p.schema_version != 2
            || p.original != self.quote
            || p.allocation_version != 1
            || p.promotion.revision <= 0
            || p.promotion.review_revision <= 0
            || !crate::catalogue_authoring::merchant_code(&p.promotion.id)
            || p.shipping != self.quote.shipping
            || p.lines.len() != self.quote.lines.len()
            || p.discount_minor <= 0
        {
            return Err(OrderError::InvalidSnapshot);
        }
        let mut gross = p.shipping.gross_minor;
        let mut net = p.shipping.net_minor;
        let mut vat = p.shipping.vat_minor;
        let mut discount = 0_i64;
        for (line, original) in p.lines.iter().zip(&self.quote.lines) {
            if line.sku != original.sku
                || line.discount_minor < 0
                || original
                    .amounts
                    .gross_minor
                    .checked_sub(line.discount_minor)
                    != Some(line.amounts.gross_minor)
            {
                return Err(OrderError::InvalidSnapshot);
            }
            let discounted = crate::Money::eur(line.amounts.gross_minor)
                .map_err(|_| OrderError::InvalidSnapshot)?;
            let calculated = crate::quotes::split(discounted, original.vat_basis_points)
                .map_err(|_| OrderError::InvalidSnapshot)?;
            if crate::checkout_quotes::CapturedAmount::from(&calculated) != line.amounts {
                return Err(OrderError::InvalidSnapshot);
            }
            gross = gross
                .checked_add(line.amounts.gross_minor)
                .ok_or(OrderError::InvalidSnapshot)?;
            net = net
                .checked_add(line.amounts.net_minor)
                .ok_or(OrderError::InvalidSnapshot)?;
            vat = vat
                .checked_add(line.amounts.vat_minor)
                .ok_or(OrderError::InvalidSnapshot)?;
            discount = discount
                .checked_add(line.discount_minor)
                .ok_or(OrderError::InvalidSnapshot)?;
        }
        if discount != p.discount_minor
            || (gross, net, vat) != (p.total.gross_minor, p.total.net_minor, p.total.vat_minor)
        {
            return Err(OrderError::InvalidSnapshot);
        }
        Ok(())
    }
}
