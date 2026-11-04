use super::*;
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssessedComponent {
    pub component_code: String,
    pub demand: i64,
    pub active: i64,
    pub before: Balance,
}
impl AssessedComponent {
    pub fn available(&self) -> Result<i64, PolicyError> {
        self.before.stock()?;
        if self.demand <= 0
            || self.active < 0
            || self.active > self.demand
            || self.before.reserved < self.active
        {
            return Err(PolicyError::InvalidSnapshot);
        }
        self.before
            .on_hand
            .checked_sub(self.before.quarantine)
            .and_then(|value| value.checked_sub(self.before.reserved))
            .and_then(|value| value.checked_add(self.active))
            .ok_or(PolicyError::InvalidSnapshot)
    }
    pub fn can_allocate(&self) -> Result<bool, PolicyError> {
        Ok(self.available()? >= self.demand)
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StageComponent {
    pub component_code: String,
    pub demand: i64,
    pub active: i64,
    pub before: Balance,
    pub expired: Balance,
    pub reserved: Balance,
    pub after: Balance,
}
impl StageComponent {
    pub fn from_assessment(component: &AssessedComponent) -> Result<Option<Self>, PolicyError> {
        if !component.can_allocate()? {
            return Ok(None);
        }
        let expired = Balance {
            on_hand: component.before.on_hand,
            quarantine: component.before.quarantine,
            reserved: component
                .before
                .reserved
                .checked_sub(component.active)
                .ok_or(PolicyError::InvalidSnapshot)?,
        };
        let reserved = Balance {
            reserved: expired
                .reserved
                .checked_add(component.demand)
                .ok_or(PolicyError::InvalidSnapshot)?,
            ..expired.clone()
        };
        reserved.stock()?;
        let after = reserved.consume(component.demand)?;
        Ok(Some(Self {
            component_code: component.component_code.clone(),
            demand: component.demand,
            active: component.active,
            before: component.before.clone(),
            expired,
            reserved,
            after,
        }))
    }
    fn assessment(&self) -> AssessedComponent {
        AssessedComponent {
            component_code: self.component_code.clone(),
            demand: self.demand,
            active: self.active,
            before: self.before.clone(),
        }
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewFootprint {
    pub schema_version: u16,
    pub original: Vec<OriginalMember>,
    pub components: Vec<AssessedComponent>,
}
impl ReviewFootprint {
    pub fn validate(&self) -> Result<(), PolicyError> {
        let (mut full, active) = summarize(&self.original)?;
        if self.schema_version != 1
            || self.components.len() != full.len()
            || self.components.len() > crate::order_stock::MAX_PAIRS
        {
            return Err(PolicyError::InvalidSnapshot);
        }
        for component in &self.components {
            if full.remove(&component.component_code) != Some(component.demand)
                || component.active != active.get(&component.component_code).copied().unwrap_or(0)
            {
                return Err(PolicyError::InvalidSnapshot);
            }
            component.available()?;
        }
        Ok(())
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LateFootprint {
    pub schema_version: u16,
    pub original: Vec<OriginalMember>,
    pub fresh: Vec<Member>,
    pub components: Vec<StageComponent>,
}
impl LateFootprint {
    pub fn validate(&self) -> Result<(), PolicyError> {
        ReviewFootprint {
            schema_version: self.schema_version,
            original: self.original.clone(),
            components: self
                .components
                .iter()
                .map(StageComponent::assessment)
                .collect(),
        }
        .validate()?;
        Footprint::demand(&self.fresh)?;
        if self.fresh.len() != self.original.len() {
            return Err(PolicyError::InvalidSnapshot);
        }
        let ids: BTreeSet<_> = self
            .original
            .iter()
            .map(|member| member.member.hold_id.as_str())
            .collect();
        for member in &self.original {
            if !matches!(member.state, OriginalState::Active | OriginalState::Expired) {
                return Err(PolicyError::InvalidSnapshot);
            }
            let fresh = self
                .fresh
                .iter()
                .find(|fresh| fresh.sku == member.member.sku)
                .ok_or(PolicyError::InvalidSnapshot)?;
            if ids.contains(fresh.hold_id.as_str())
                || fresh.revision != member.member.revision
                || fresh.units != member.member.units
                || fresh.components != member.member.components
            {
                return Err(PolicyError::InvalidSnapshot);
            }
        }
        for component in &self.components {
            if StageComponent::from_assessment(&component.assessment())?.as_ref() != Some(component)
            {
                return Err(PolicyError::InvalidSnapshot);
            }
        }
        Ok(())
    }
}
