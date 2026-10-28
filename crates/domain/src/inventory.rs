//! Conserved physical balances. Refunds never imply a stock receipt.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StockBalance {
    on_hand: i64,
    quarantine: i64,
    reserved: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StockError {
    InvalidBalance,
    InvalidQuantity,
    Overflow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WarehouseChange {
    Receive(u32),
    Quarantine(u32),
    ReleaseQuarantine(u32),
    DisposeQuarantine(u32),
    Count(i64),
}

impl WarehouseChange {
    pub fn descriptor(self) -> (&'static str, i64) {
        match self {
            Self::Receive(quantity) => ("receive", i64::from(quantity)),
            Self::Quarantine(quantity) => ("quarantine", i64::from(quantity)),
            Self::ReleaseQuarantine(quantity) => ("release-quarantine", i64::from(quantity)),
            Self::DisposeQuarantine(quantity) => ("dispose-quarantine", i64::from(quantity)),
            Self::Count(quantity) => ("count", quantity),
        }
    }
}

impl StockBalance {
    pub fn new(on_hand: i64, quarantine: i64, reserved: i64) -> Result<Self, StockError> {
        if on_hand < 0 || quarantine < 0 || reserved < 0 {
            return Err(StockError::InvalidBalance);
        }
        let unavailable = quarantine
            .checked_add(reserved)
            .ok_or(StockError::Overflow)?;
        if unavailable > on_hand {
            return Err(StockError::InvalidBalance);
        }
        Ok(Self {
            on_hand,
            quarantine,
            reserved,
        })
    }

    pub fn on_hand(self) -> i64 {
        self.on_hand
    }
    pub fn quarantine(self) -> i64 {
        self.quarantine
    }
    pub fn reserved(self) -> i64 {
        self.reserved
    }
    pub fn available(self) -> i64 {
        self.on_hand - self.quarantine - self.reserved
    }

    pub fn reserve(self, quantity: i64) -> Result<Self, StockError> {
        if quantity <= 0 {
            return Err(StockError::InvalidQuantity);
        }
        let reserved = self
            .reserved
            .checked_add(quantity)
            .ok_or(StockError::Overflow)?;
        Self::new(self.on_hand, self.quarantine, reserved)
    }

    pub fn release(self, quantity: i64) -> Result<Self, StockError> {
        if quantity <= 0 {
            return Err(StockError::InvalidQuantity);
        }
        Self::new(self.on_hand, self.quarantine, self.reserved - quantity)
    }

    pub fn consume(self, quantity: i64) -> Result<Self, StockError> {
        if quantity <= 0 {
            return Err(StockError::InvalidQuantity);
        }
        Self::new(
            self.on_hand - quantity,
            self.quarantine,
            self.reserved - quantity,
        )
    }

    pub fn warehouse_change(self, change: WarehouseChange) -> Result<Self, StockError> {
        let (_, quantity) = change.descriptor();
        if quantity < 0 || (quantity == 0 && !matches!(change, WarehouseChange::Count(_))) {
            return Err(StockError::InvalidQuantity);
        }
        let add = |value: i64| value.checked_add(quantity).ok_or(StockError::Overflow);
        match change {
            WarehouseChange::Receive(_) => {
                Self::new(add(self.on_hand)?, self.quarantine, self.reserved)
            }
            WarehouseChange::Quarantine(_) => {
                Self::new(self.on_hand, add(self.quarantine)?, self.reserved)
            }
            WarehouseChange::ReleaseQuarantine(_) => {
                Self::new(self.on_hand, self.quarantine - quantity, self.reserved)
            }
            WarehouseChange::DisposeQuarantine(_) => Self::new(
                self.on_hand - quantity,
                self.quarantine - quantity,
                self.reserved,
            ),
            WarehouseChange::Count(_) => Self::new(quantity, self.quarantine, self.reserved),
        }
    }
}
