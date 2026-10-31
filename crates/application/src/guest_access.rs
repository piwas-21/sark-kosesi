//! Guest lifetime/resource policy, separate from staff MFA and verified account authority.
pub const MAX_GUEST_LIFETIME_SECONDS: i64 = 604800;
#[derive(Clone, Copy)]
pub struct GuestLifetime {
    pub created_at: i64,
    pub expires_at: i64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuestPolicyError {
    InvalidState,
    Expired,
}
impl GuestLifetime {
    pub fn valid(self) -> bool {
        self.created_at >= 0
            && self.expires_at > self.created_at
            && self.expires_at - self.created_at <= MAX_GUEST_LIFETIME_SECONDS
    }
    pub fn check(self, now: i64) -> Result<(), GuestPolicyError> {
        if !self.valid() {
            return Err(GuestPolicyError::InvalidState);
        }
        if now < self.created_at || now >= self.expires_at {
            return Err(GuestPolicyError::Expired);
        }
        Ok(())
    }
}
#[derive(Clone, Copy)]
pub struct GuestLimits {
    pub issue_per_minute: u16,
    pub mutations_per_minute: u16,
}
impl Default for GuestLimits {
    fn default() -> Self {
        Self {
            issue_per_minute: 64,
            mutations_per_minute: 60,
        }
    }
}
impl GuestLimits {
    pub fn valid(self) -> bool {
        (1..=1000).contains(&self.issue_per_minute)
            && (1..=1000).contains(&self.mutations_per_minute)
    }
}
