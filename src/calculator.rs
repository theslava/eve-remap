#[allow(unused_imports)] // used by test helpers below
use crate::data::models::{Attribute, EffectiveAttributes, SkillRecord};

/// Level multipliers for EVE Online skill training (SP required per level).
/// Source: EVE wiki — these are well-established constants.
const LEVEL_MULTIPLIERS: [f64; 5] = [1.0, 4.0, 20.0, 80.0, 360.0];

/// Compute the SP required to go from current_level to target_level for a given skill.
pub fn sp_for_level(skill: &SkillRecord, from_level: u8, to_level: u8) -> f64 {
    let mut total_sp = 0.0;
    for lvl in from_level..to_level.min(5) {
        let idx = (lvl - 1) as usize; // levels 1-5 map to indices 0-4
        if idx < LEVEL_MULTIPLIERS.len() {
            total_sp += LEVEL_MULTIPLIERS[idx] * skill.skill_time_constant;
        }
    }
    total_sp
}

/// Compute the rate of SP generation per second for a skill under given effective attributes.
///
/// The formula is derived from EVE mechanics:
/// ```text
/// rate_per_sec = (primary_attr_value + secondary_attr_value / 2.0) / 60.0
/// ```
///
/// Where attribute values are the effective values after implants.
pub fn sp_rate_per_second(skill: &SkillRecord, attrs: &EffectiveAttributes) -> f64 {
    let primary_val = attrs.get(skill.primary_attribute);
    let secondary_val = attrs.get(skill.secondary_attribute);
    
    // Primary contributes full value, secondary contributes half.
    // Rate is measured in SP per minute conceptually, but we convert to per-second.
    (primary_val + secondary_val / 2.0) / 60.0
}

/// Compute training duration in seconds for a skill level transition.
pub fn duration_seconds(
    skill: &SkillRecord,
    from_level: u8,
    to_level: u8,
    attrs: &EffectiveAttributes,
) -> f64 {
    let sp_needed = sp_for_level(skill, from_level, to_level);
    let rate = sp_rate_per_second(skill, attrs);
    if rate <= 0.0 {
        return f64::INFINITY;
    }
    sp_needed / rate
}

/// Format seconds into a human-readable duration string.
pub fn format_duration(seconds: f64) -> String {
    let total_days = seconds / 86_400.0;
    
    if total_days >= 365.0 {
        let years = total_days / 365.0;
        format!("{:.1}y {:.0}d", 
            years.floor(), 
            ((years.fract() * 365.0).floor()))
    } else if total_days >= 30.0 {
        let weeks = total_days / 7.0;
        format!("{:.1}w {:.0}d",
            weeks.floor(),
            ((weeks.fract() * 7.0).floor()))
    } else if total_days >= 1.0 {
        format!("{:.1}d {:.0}h",
            total_days.floor(),
            ((total_days.fract() * 24.0).floor()))
    } else {
        let hours = total_days * 24.0;
        format!("{:.1}h {:.0}m",
            hours.floor(),
            ((hours.fract() * 60.0).floor()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_attrs(int: f64, cha: f64, per: f64, mem: f64, wil: f64) -> EffectiveAttributes {
        EffectiveAttributes {
            intelligence: int,
            charisma: cha,
            perception: per,
            memory: mem,
            willpower: wil,
        }
    }

    #[test]
    fn test_sp_rate_basic() {
        // Primary=Intelligence(5), Secondary=Memory(3) → rate = (5 + 3/2)/60 = 6.5/60 ≈ 0.1083 SP/s
        let skill = SkillRecord {
            id: 999,
            name: "Test".to_string(),
            primary_attribute: Attribute::Intelligence,
            secondary_attribute: Attribute::Memory,
            skill_time_constant: 1.0,
        };
        let attrs = test_attrs(5.0, 1.0, 1.0, 3.0, 1.0);
        
        let rate = sp_rate_per_second(&skill, &attrs);
        assert!((rate - 0.10833).abs() < 0.001);
    }

    #[test]
    fn test_sp_for_level_1_to_2() {
        // Level 1→2 uses multiplier[0]=1.0 * timeConstant
        let skill = SkillRecord {
            id: 999,
            name: "Test".to_string(),
            primary_attribute: Attribute::Intelligence,
            secondary_attribute: Attribute::Memory,
            skill_time_constant: 4.0,
        };
        
        let sp = sp_for_level(&skill, 1, 2);
        assert_eq!(sp, 4.0); // 1.0 * 4.0
    }

    #[test]
    fn test_sp_for_level_3_to_5() {
        // Level 3→4 uses multiplier[2]=20.0; Level 4→5 uses multiplier[3]=80.0
        let skill = SkillRecord {
            id: 999,
            name: "Test".to_string(),
            primary_attribute: Attribute::Intelligence,
            secondary_attribute: Attribute::Perception,
            skill_time_constant: 3.0,
        };
        
        let sp = sp_for_level(&skill, 3, 5);
        assert_eq!(sp, (20.0 + 80.0) * 3.0); // 300.0
    }

    #[test]
    fn test_duration_formula() {
        let skill = SkillRecord {
            id: 999,
            name: "Test".to_string(),
            primary_attribute: Attribute::Intelligence,
            secondary_attribute: Attribute::Memory,
            skill_time_constant: 2.0,
        };
        let attrs = test_attrs(10.0, 1.0, 1.0, 6.0, 1.0);
        
        // SP for level 1→2 = 1.0 * 2.0 = 2.0
        // Rate = (10 + 6/2)/60 = 13/60 ≈ 0.2167 SP/s
        // Duration = 2.0 / (13/60) = 120/13 ≈ 9.23 seconds
        let dur = duration_seconds(&skill, 1, 2, &attrs);
        assert!((dur - 9.23).abs() < 0.1);
    }

    #[test]
    fn test_format_duration_days() {
        let s = format_duration(86_400.0 * 15.5); // 15.5 days
        assert!(s.contains("d"));
        assert!(!s.contains("y") && !s.contains("w"));
    }

    #[test]
    fn test_format_duration_years() {
        let s = format_duration(86_400.0 * 400.0); // ~1.1 years
        assert!(s.contains("y"));
    }
}
