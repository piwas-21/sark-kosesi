use super::*;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewReason {
    PriorConsumptionWithoutAllocation,
    ReleaseIntentUnresolved,
    ComponentScarcity,
}
impl ReviewReason {
    pub fn code(self) -> &'static str {
        match self {
            Self::PriorConsumptionWithoutAllocation => "prior_consumption_without_allocation",
            Self::ReleaseIntentUnresolved => "release_intent_unresolved",
            Self::ComponentScarcity => "component_scarcity",
        }
    }
    pub fn classify(footprint: &ReviewFootprint) -> Result<Option<Self>, PolicyError> {
        footprint.validate()?;
        if footprint
            .original
            .iter()
            .any(|member| member.state == OriginalState::Consumed)
        {
            return Ok(Some(Self::PriorConsumptionWithoutAllocation));
        }
        if footprint
            .original
            .iter()
            .any(|member| member.state == OriginalState::Released)
        {
            return Ok(Some(Self::ReleaseIntentUnresolved));
        }
        for component in &footprint.components {
            if !component.can_allocate()? {
                return Ok(Some(Self::ComponentScarcity));
            }
        }
        Ok(None)
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LateAllocation {
    pub id: String,
    pub order_id: String,
    pub attempt_id: String,
    pub recognition_id: String,
    pub original_group_id: String,
    pub fresh_group_id: String,
    pub actor: String,
    pub allocated_at: i64,
    pub expires_at: i64,
    pub footprint: LateFootprint,
}
impl LateAllocation {
    pub fn validate(&self) -> Result<(), PolicyError> {
        if ![
            &self.id,
            &self.order_id,
            &self.attempt_id,
            &self.recognition_id,
            &self.original_group_id,
            &self.fresh_group_id,
        ]
        .iter()
        .all(|id| identifier(id))
            || self.original_group_id == self.fresh_group_id
            || self.actor != ACTOR
        {
            return Err(PolicyError::InvalidSnapshot);
        }
        check_window(self.allocated_at, self.expires_at)?;
        self.footprint.validate()
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewCase {
    pub id: String,
    pub order_id: String,
    pub attempt_id: String,
    pub recognition_id: String,
    pub group_id: String,
    pub actor: String,
    pub reason: ReviewReason,
    pub assessed_at: i64,
    pub footprint: ReviewFootprint,
}
impl ReviewCase {
    pub fn validate(&self) -> Result<(), PolicyError> {
        if ![
            &self.id,
            &self.order_id,
            &self.attempt_id,
            &self.recognition_id,
            &self.group_id,
        ]
        .iter()
        .all(|id| identifier(id))
            || self.actor != REVIEW_ACTOR
            || !(0..=crate::checkout_quotes::MAX_QUOTE_UNIX_SECONDS).contains(&self.assessed_at)
            || ReviewReason::classify(&self.footprint)? != Some(self.reason)
        {
            return Err(PolicyError::InvalidSnapshot);
        }
        Ok(())
    }
}
