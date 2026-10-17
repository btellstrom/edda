use serde::{Deserialize, Serialize};

/// A count of limited uses: spell slots, Action Surge, Ki points, etc.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(default)]
pub struct Pool {
    pub max: u16,
    pub used: u16,
}

impl Pool {
    /// A pool of `max` uses, none spent.
    pub fn new(max: u16) -> Self {
        Self { max, used: 0 }
    }

    /// How many uses are left.
    pub fn remaining(&self) -> u16 {
        self.max.saturating_sub(self.used)
    }

    /// Spends `amount` uses, or does nothing and returns `false` if there
    /// aren't enough remaining.
    pub fn expend(&mut self, amount: u16) -> bool {
        if amount > self.remaining() {
            return false;
        }
        self.used += amount;
        true
    }

    /// Regains up to `amount` uses (never beyond `max`).
    pub fn restore(&mut self, amount: u16) {
        self.used = self.used.saturating_sub(amount);
    }

    /// Regains every use.
    pub fn reset(&mut self) {
        self.used = 0;
    }

    /// Whether no more are spent than there are.
    pub fn is_valid(&self) -> bool {
        self.used <= self.max
    }
}

/// When a resource comes back on its own.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Recharge {
    /// Recovers on a short or long rest (Action Surge, Ki, Channel Divinity).
    ShortRest,
    /// Recovers on a long rest only (Rage, Arcane Recovery).
    #[default]
    LongRest,
    /// Never recovers automatically (item charges, homebrew).
    Manual,
}

/// A short or a long rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum RestKind {
    Short,
    Long,
}

impl Recharge {
    /// Whether a resource with this recharge comes back on `rest`.
    pub fn recovers_on(self, rest: RestKind) -> bool {
        match self {
            Recharge::ShortRest => true,
            Recharge::LongRest => rest == RestKind::Long,
            Recharge::Manual => false,
        }
    }
}

/// A named, limited-use class feature or ability, e.g. Action Surge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Resource {
    pub name: String,
    #[serde(flatten)]
    pub uses: Pool,
    #[serde(default)]
    pub recharge: Recharge,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_never_overspends_or_overrestores() {
        let mut p = Pool::new(2);
        assert!(p.expend(2));
        assert!(!p.expend(1));
        assert_eq!(p.used, 2);
        p.restore(5);
        assert_eq!(p.used, 0);
    }

    #[test]
    fn recharge_rules() {
        assert!(Recharge::ShortRest.recovers_on(RestKind::Short));
        assert!(Recharge::ShortRest.recovers_on(RestKind::Long));
        assert!(!Recharge::LongRest.recovers_on(RestKind::Short));
        assert!(!Recharge::Manual.recovers_on(RestKind::Long));
    }
}
