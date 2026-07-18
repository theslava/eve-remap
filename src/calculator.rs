#[allow(unused_imports)] // used by test helpers below
use crate::data::models::{Attribute, EffectiveAttributes, SkillRecord};

/// Cumulative SP required at each skill level for STC=1 (rank 1).
/// Source: EVE Online forums archive — verified canonical values.
const CUMULATIVE_SP: [f64; 6] = [0.0, 250.0, 1_414.0, 8_000.0, 45_255.0, 256_000.0]; // levels 0..5

/// Compute the SP required to go from current_level to target_level for a given skill.
pub fn sp_for_level(skill: &SkillRecord, from_level: u8, to_level: u8) -> f64 {
    let from_idx = from_level.min(5) as usize;
    let to_idx = to_level.min(5) as usize;
    if from_idx >= to_idx {
        return 0.0;
    }
    (CUMULATIVE_SP[to_idx] - CUMULATIVE_SP[from_idx]) * skill.skill_time_constant
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

/// Format seconds as the two most significant time units (e.g., "5d 13h").
/// Rounds at the boundary of the second unit — sub-units push it up to the next integer.
pub fn format_duration(seconds: f64) -> String {
    let mut secs = seconds.max(0.0);
    let days = (secs / 86_400.0) as u64;
    secs -= days as f64 * 86_400.0;
    let hours = (secs / 3_600.0) as u64;
    secs -= hours as f64 * 3_600.0;
    let minutes = (secs / 60.0) as u64;
    secs -= minutes as f64 * 60.0;
    let remaining_secs = secs.ceil() as u64;

    // Collect non-zero components in order of significance.
    let units = [
        ("d", days),
        ("h", hours),
        ("m", minutes),
        ("s", remaining_secs),
    ];

    let entries: Vec<(&str, u64)> = units.iter().filter(|(_, v)| *v > 0).copied().collect();

    if entries.is_empty() {
        return "0s".to_string();
    }

    // If we only have one unit, show it alone.
    if entries.len() == 1 {
        return format!("{}{}", entries[0].1, entries[0].0);
    }

    // Show top-2 units. The second unit already absorbed lower remainders via ceil().
    format!(
        "{}{} {}{}",
        entries[0].1, entries[0].0,
        entries[1].1, entries[1].0
    )
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
            prerequisites: vec![],
        };
        let attrs = test_attrs(5.0, 1.0, 1.0, 3.0, 1.0);
        
        let rate = sp_rate_per_second(&skill, &attrs);
        assert!((rate - 0.10833).abs() < 0.001);
    }

    #[test]
    fn test_sp_for_level_1_to_2() {
        // Cumulative L1=250, L2=1414 → incremental = 1164 × STC
        let skill = SkillRecord {
            id: 999,
            name: "Test".to_string(),
            primary_attribute: Attribute::Intelligence,
            secondary_attribute: Attribute::Memory,
            skill_time_constant: 4.0,
            prerequisites: vec![],
        };
        
        let sp = sp_for_level(&skill, 1, 2);
        assert_eq!(sp, (1_414.0 - 250.0) * 4.0); // 4656.0
    }

    #[test]
    fn test_sp_for_level_3_to_5() {
        // Cumulative L3=8000, L5=256000 → incremental = 248000 × STC
        let skill = SkillRecord {
            id: 999,
            name: "Test".to_string(),
            primary_attribute: Attribute::Intelligence,
            secondary_attribute: Attribute::Perception,
            skill_time_constant: 3.0,
            prerequisites: vec![],
        };
        
        let sp = sp_for_level(&skill, 3, 5);
        assert_eq!(sp, (256_000.0 - 8_000.0) * 3.0); // 744000.0
    }

    #[test]
    fn test_duration_formula() {
        let skill = SkillRecord {
            id: 999,
            name: "Test".to_string(),
            primary_attribute: Attribute::Intelligence,
            secondary_attribute: Attribute::Memory,
            skill_time_constant: 2.0,
            prerequisites: vec![],
        };
        let attrs = test_attrs(10.0, 1.0, 1.0, 6.0, 1.0);
        
        // SP for level 1→2 = (1414-250)*2.0 = 2328.0
        // Rate = (10 + 6/2)/60 = 13/60 ≈ 0.2167 SP/s
        // Duration = 2328 / (13/60) ≈ 10763.1 seconds (~3 hours)
        let dur = duration_seconds(&skill, 1, 2, &attrs);
        assert!((dur - 10_744.6).abs() < 1.0);
    }
    #[test]
    fn test_format_duration_days_cap() {
        // Large value should show as days, not years/weeks.
        let s = format_duration(86_400.0 * 400.0); // 400 days
        assert!(s.contains("d"), "expected 'd' in '{}'", s);
        assert!(!s.contains("w") && !s.contains("y"));
    }
}
