//! Explicit hold clocks and transitions. Late payment requires P3 reallocation.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoldState {
    Active,
    Released,
    Consumed,
    Expired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoldAction {
    Release,
    Consume,
    Expire,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoldError {
    InvalidWindow,
    InvalidClock,
    Expired,
    NotExpired,
    InvalidTransition,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HoldWindow {
    created: i64,
    expires: i64,
}

impl HoldWindow {
    pub fn new(created: i64, expires: i64) -> Result<Self, HoldError> {
        if created < 0 || expires <= created || expires - created > 1800 {
            return Err(HoldError::InvalidWindow);
        }
        Ok(Self { created, expires })
    }

    pub fn transition(
        self,
        state: HoldState,
        action: HoldAction,
        now: i64,
    ) -> Result<HoldState, HoldError> {
        if now < self.created {
            return Err(HoldError::InvalidClock);
        }
        let target = match action {
            HoldAction::Release => HoldState::Released,
            HoldAction::Consume => HoldState::Consumed,
            HoldAction::Expire => HoldState::Expired,
        };
        if state != HoldState::Active {
            if state == target {
                return Ok(state);
            }
            return Err(HoldError::InvalidTransition);
        }
        match action {
            HoldAction::Consume if now >= self.expires => Err(HoldError::Expired),
            HoldAction::Expire if now < self.expires => Err(HoldError::NotExpired),
            _ => Ok(target),
        }
    }
}
