use serde::{Deserialize, Serialize};

/// The five EVE Online character attributes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Attribute {
    Intelligence,
    Charisma,
    Perception,
    Memory,
    Willpower,
}

impl Attribute {
    pub fn variants() -> [Attribute; 5] {
        [
            Attribute::Intelligence,
            Attribute::Charisma,
            Attribute::Perception,
            Attribute::Memory,
            Attribute::Willpower,
        ]
    }
}
impl std::fmt::Display for Attribute {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Attribute::Intelligence => write!(f, "intelligence"),
            Attribute::Charisma => write!(f, "charisma"),
            Attribute::Perception => write!(f, "perception"),
            Attribute::Memory => write!(f, "memory"),
            Attribute::Willpower => write!(f, "willpower"),
        }
    }
}

/// A single skill from SDE data (pre-parsed).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRecord {
    pub id: u32,
    pub name: String,
    #[serde(rename = "primaryAttribute")]
    pub primary_attribute: Attribute,
    #[serde(rename = "secondaryAttribute")]
    pub secondary_attribute: Attribute,
    #[serde(rename = "skillTimeConstant")]
    pub skill_time_constant: f64,
}

/// An implant with attribute bonuses (pre-parsed from SDE).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImplantRecord {
    #[serde(rename = "typeId")]
    pub type_id: u32,
    pub name: String,
    pub bonuses: std::collections::HashMap<Attribute, i32>,
}

/// Effective attributes after combining remapped base values with active implants.
#[derive(Debug, Clone, Copy)]
pub struct EffectiveAttributes {
    pub intelligence: f64,
    pub charisma: f64,
    pub perception: f64,
    pub memory: f64,
    pub willpower: f64,
}

impl EffectiveAttributes {
    /// Get the effective value for a given attribute.
    pub fn get(&self, attr: Attribute) -> f64 {
        match attr {
            Attribute::Intelligence => self.intelligence,
            Attribute::Charisma => self.charisma,
            Attribute::Perception => self.perception,
            Attribute::Memory => self.memory,
            Attribute::Willpower => self.willpower,
        }
    }

    /// Build from raw base values plus implant bonuses.
    pub fn from_base_and_implants(
        base: &BaseAttributes,
        active_implant_ids: &[u32],
        implants: &[ImplantRecord],
    ) -> Self {
        let mut attrs = *base;
        for impl_id in active_implant_ids {
            if let Some(implant) = implants.iter().find(|i| i.type_id == *impl_id) {
                for (attr, bonus) in &implant.bonuses {
                    match attr {
                        Attribute::Intelligence => attrs.intelligence += *bonus as f64,
                        Attribute::Charisma => attrs.charisma += *bonus as f64,
                        Attribute::Perception => attrs.perception += *bonus as f64,
                        Attribute::Memory => attrs.memory += *bonus as f64,
                        Attribute::Willpower => attrs.willpower += *bonus as f64,
                    }
                }
            }
        }
        EffectiveAttributes::from(attrs)
    }
}

impl From<BaseAttributes> for EffectiveAttributes {
    fn from(base: BaseAttributes) -> Self {
        EffectiveAttributes {
            intelligence: base.intelligence,
            charisma: base.charisma,
            perception: base.perception,
            memory: base.memory,
            willpower: base.willpower,
        }
    }
}

/// Base remapped attribute values from the neural interface (stored as f64 for arithmetic).
#[derive(Debug, Clone, Copy)]
pub struct BaseAttributes {
    pub intelligence: f64,
    pub charisma: f64,
    pub perception: f64,
    pub memory: f64,
    pub willpower: f64,
}

impl BaseAttributes {
    /// Total points distributed across all attributes.
    pub fn total(&self) -> f64 {
        self.intelligence + self.charisma + self.perception + self.memory + self.willpower
    }
}

/// A skill currently being trained by a character (from ESI /skillqueue).
#[derive(Debug, Clone)]
pub struct QueuedSkill {
    pub id: u32,
    pub level: u8,         // current trained level (1-5)
    pub sp: u64,           // SP earned so far toward next level
    pub duration: u64,     // total duration in seconds for this queue entry
    pub remaining_sec: u64, // seconds remaining until completion
    pub is_active: bool,   // true if this is the currently training skill
}

impl QueuedSkill {
    /// Fraction of progress through the current queue entry (0.0 to 1.0).
    pub fn progress_fraction(&self) -> f64 {
        if self.duration == 0 {
            return 1.0;
        }
        1.0 - (self.remaining_sec as f64 / self.duration as f64)
    }

    /// Remaining SP needed to complete this queue entry.
    pub fn remaining_sp(&self) -> u64 {
        let progress = self.progress_fraction();
        ((1.0 - progress) * self.sp as f64) as u64
    }
}

/// Full character state snapshot combining ESI data with resolved SDE lookups.
/// This is the single source of truth consumed by the optimizer.
#[derive(Debug, Clone)]
pub struct CharacterState {
    /// Current base remapped attribute values from neural interface.
    pub base_attributes: BaseAttributes,
    /// IDs of currently active implants providing attribute bonuses.
    pub active_implant_ids: Vec<u32>,
    /// Effective attributes after applying active implant bonuses.
    pub effective_attributes: EffectiveAttributes,
    /// Skills queued for training, ordered by position (first is active).
    pub queued_skills: Vec<QueuedSkill>,
    /// Number of bonus neural interface remaps available (timed cooldown separate).
    pub bonus_remaps: Option<u32>,
}

impl CharacterState {
    /// Derive effective attributes from base values plus active implants.
    pub fn recompute_effective(&mut self, implants: &[ImplantRecord]) {
        self.effective_attributes = EffectiveAttributes::from_base_and_implants(
            &self.base_attributes,
            &self.active_implant_ids,
            implants,
        );
    }
}

/// A single epoch in the optimizer plan.
#[derive(Debug, Clone)]
pub struct EpochPlan {
    pub start_offset_secs: f64,  // seconds from now when this epoch starts
    pub attributes: BaseAttributes,
    pub effective_attributes: EffectiveAttributes,
    pub completed_skills: Vec<(u32, String, f64)>, // (skill_id, skill_name, train_seconds)
    pub projected_finish_secs: f64,  // seconds from now when this epoch ends
}

/// Full optimization result across all epochs.
#[derive(Debug, Clone)]
pub struct OptimizationResult {
    pub epochs: Vec<EpochPlan>,
    // removed: derived from total_wall_clock_seconds at display time
    pub total_wall_clock_seconds: f64,
}
