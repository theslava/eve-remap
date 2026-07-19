use std::collections::HashMap;

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
    /// Direct prerequisite skills: (skill_id, required_level).
    /// Level is 1-indexed (1..=5). Empty if no prerequisites.
    #[serde(default)]
    pub prerequisites: Vec<(u32, u8)>,
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
#[derive(Debug, Clone, Copy, Serialize)]
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

#[derive(Debug, Clone, Copy, Serialize)]
pub struct BaseAttributes {
    pub intelligence: f64,
    pub charisma: f64,
    pub perception: f64,
    pub memory: f64,
    pub willpower: f64,
}

impl BaseAttributes {
    /// Add another attribute set pointwise.
    pub fn add(&self, other: &Self) -> Self {
        BaseAttributes {
            intelligence: self.intelligence + other.intelligence,
            charisma: self.charisma + other.charisma,
            perception: self.perception + other.perception,
            memory: self.memory + other.memory,
            willpower: self.willpower + other.willpower,
        }
    }
}

/// A skill entry from a user-provided queue file (parsed from `"Skill Name <level>"`).
#[derive(Debug, Clone)]
pub struct QueuedSkill {
    pub id: u32,
    pub level: u8,         // current trained level (1-5)
    pub duration: u64,     // total duration in seconds for this queue entry
    pub remaining_sec: u64, // seconds remaining until completion
}

impl QueuedSkill {
    /// Fraction of progress through the current queue entry (0.0 to 1.0).
    pub fn progress_fraction(&self) -> f64 {
        if self.duration == 0 {
            return 1.0;
        }
        1.0 - (self.remaining_sec as f64 / self.duration as f64)
    }
}

/// Full character state snapshot built from CLI arguments and local asset lookups.
/// This is the single source of truth consumed by the optimizer.
#[derive(Debug, Clone)]
pub struct CharacterState {
    /// Current base remapped attribute values from neural interface.
    pub base_attributes: BaseAttributes,
    /// IDs of currently active implants providing attribute bonuses.
    pub active_implant_ids: Vec<u32>,
    /// Direct implant bonus values (for offline mode when --implant-bonuses is used).
    /// When non-zero, these are added back after each remap regardless of active_implant_ids.
    pub implant_bonus: BaseAttributes,
    /// Skills queued for training, ordered by position (first is active).
    pub queued_skills: Vec<QueuedSkill>,
    /// Number of bonus neural interface remaps available (timed cooldown separate).
    pub bonus_remaps: Option<u32>,
    /// Wall-clock seconds from training start when the normal remap becomes available.
    /// Defaults to 0 (available immediately); set via --remap-available.
    pub normal_remap_available_in_secs: f64,
}

/// SP accumulated per (role × attribute) pair for one epoch.
#[derive(Debug, Clone, Default, Serialize)]
pub struct AttributeSpSummary {
    /// SP earned while attributes served as **primary** for completed skills.
    pub primary: HashMap<String, f64>,
    /// SP earned while attributes served as **secondary** for completed skills.
    pub secondary: HashMap<String, f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EpochPlan {
    pub start_offset_secs: f64,  // seconds from now when this epoch starts
    pub attributes: BaseAttributes,
    pub effective_attributes: EffectiveAttributes,
    pub completed_skills: Vec<(u32, String, u8, f64)>, // (skill_id, skill_name, target_level, train_seconds)
    /// Total SP per (role × attribute) pair for skills completed this epoch.
    pub sp_summary: AttributeSpSummary,
    pub projected_finish_secs: f64,  // seconds from now when this epoch ends
    /// Number of bonus neural interface remaps consumed for this epoch.
    pub bonus_remaps_used: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct OptimizationResult {
    pub epochs: Vec<EpochPlan>,
    pub total_wall_clock_seconds: f64,
    /// Wall-clock seconds if no remaps were used (current attrs throughout).
    pub baseline_wall_clock_seconds: f64,
}
