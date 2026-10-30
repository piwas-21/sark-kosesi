//! Policy for server-side staff session records; no client-supplied role claims.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Capability {
    CatalogueManage,
    CatalogueReview,
    WarehouseManage,
    MarketManage,
    MarketReview,
}
impl Capability {
    pub fn code(self) -> &'static str {
        match self {
            Self::CatalogueManage => "catalogue.manage",
            Self::CatalogueReview => "catalogue.review",
            Self::WarehouseManage => "warehouse.manage",
            Self::MarketManage => "market.manage",
            Self::MarketReview => "market.review",
        }
    }
}
#[derive(Clone, Copy)]
pub struct SessionState {
    pub enabled: bool,
    pub revoked: bool,
    pub granted: bool,
    pub created_at: i64,
    pub expires_at: i64,
    pub mfa_verified_at: Option<i64>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AccessDenied;
pub fn check_session(state: SessionState, now: i64) -> Result<(), AccessDenied> {
    if !state.enabled
        || state.revoked
        || !state.granted
        || state.created_at < 0
        || state.expires_at <= state.created_at
        || state.expires_at - state.created_at > 43200
        || now < state.created_at
        || now >= state.expires_at
    {
        return Err(AccessDenied);
    }
    let Some(verified) = state.mfa_verified_at else {
        return Err(AccessDenied);
    };
    if verified < state.created_at || verified > now || now - verified > 900 {
        return Err(AccessDenied);
    }
    Ok(())
}
