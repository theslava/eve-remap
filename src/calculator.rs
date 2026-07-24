use crate::data::models::{EffectiveAttributes, SkillRecord};

/// Cumulative SP required at each skill level for STC=1 (rank 1).
/// Source: EVE Online forums archive — verified canonical values.
pub const CUMULATIVE_SP: [f64; 6] = [0.0, 250.0, 1_414.0, 8_000.0, 45_255.0, 256_000.0]; // levels 0..5

/// Compute the SP required to go from current_level to target_level for a given skill.
pub fn sp_for_level(skill: &SkillRecord, from_level: u8, to_level: u8) -> f64 {
    debug_assert!(
        (from_level <= 5) && (to_level <= 5),
        "levels should be validated before calling sp_for_level"
    );
    let from_idx = from_level as usize;
    let to_idx = to_level as usize;
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

/// Parse a skill-point value string into f64.
///
/// Accepts plain integers/floats with optional comma thousands separators.
/// No SI-style suffixes. Examples: `"12000"`, `"1,000,000"`, `"50000"`
pub fn parse_sp_value(input: &str) -> anyhow::Result<f64> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        anyhow::bail!("empty SP value");
    }

    // Strip commas for thousands separators.
    let stripped: String = trimmed.chars().filter(|c| *c != ',').collect();

    let value: f64 = stripped.parse::<f64>().map_err(|_| {
        anyhow::anyhow!(
            "invalid SP value '{}', expected a number (commas allowed)",
            trimmed
        )
    })?;

    if value < 0.0 {
        anyhow::bail!("SP values must not be negative (got {})", value);
    }

    Ok(value)
}

/// Parse a human-readable duration string into total seconds.
///
/// Accepts up to two components, each consisting of a numeric value followed by a unit
/// suffix (`d`, `h`, `m`, `s`). Components may be separated by a space or concatenated.
/// This is the inverse of [`format_duration`].
///
/// Maximum of two time units is enforced — this matches the EVE Online client UI
/// which only ever displays durations in two components (e.g., "5d 13h"). Accepting
/// more would be meaningless as users cannot enter such values in-game.
///
/// Valid examples: `"5d"`, `"3d 12h"`, `"3d12h"`, `"5h30m"`, `"90s"`, `"0s"`.
pub fn parse_duration(input: &str) -> anyhow::Result<f64> {
    let input = input.trim();
    if input.is_empty() {
        anyhow::bail!("duration string is empty");
    }

    // Extract individual <number><unit> tokens from the input.
    // We scan character-by-character, collecting digit/dot runs and expecting a unit after each.
    let mut total_secs: f64 = 0.0;
    let mut component_count = 0usize;
    let mut chars = input.chars().peekable();

    loop {
        // Skip whitespace between components.
        while let Some(&c) = chars.peek() {
            if c.is_whitespace() {
                chars.next();
            } else {
                break;
            }
        }

        // If no more characters, we're done.
        if chars.peek().is_none() {
            break;
        }

        // Collect the numeric value (digits and at most one decimal point).
        let mut num_str = String::new();
        let mut has_dot = false;
        while let Some(&c) = chars.peek() {
            if c.is_ascii_digit() || (c == '.' && !has_dot) {
                num_str.push(c);
                chars.next();
                if c == '.' {
                    has_dot = true;
                }
            } else {
                break;
            }
        }

        if num_str.is_empty() {
            anyhow::bail!(
                "expected a number in duration string '{}' but found non-numeric character",
                input
            );
        }

        // Next character must be the unit suffix.
        let unit_char = match chars.next() {
            Some(c) => c,
            None => anyhow::bail!(
                "duration component '{}' is missing a unit suffix (expected d, h, m, or s)",
                num_str
            ),
        };

        let value: f64 = num_str.parse::<f64>().map_err(|_| {
            anyhow::anyhow!(
                "invalid numeric value '{}' in duration '{}'",
                num_str,
                input
            )
        })?;

        if value < 0.0 {
            anyhow::bail!("duration values must not be negative (got {})", value);
        }

        component_count += 1;
        if component_count > 2 {
            anyhow::bail!(
                "too many time components in duration '{}' (max 2 allowed, matching EVE Online UI)",
                input
            );
        }

        match unit_char {
            'd' => total_secs += value * 86_400.0,
            'h' => total_secs += value * 3_600.0,
            'm' => total_secs += value * 60.0,
            's' => total_secs += value,
            other => anyhow::bail!(
                "unknown duration unit '{}' after value {}; expected d, h, m, or s",
                other,
                value
            ),
        }
    }

    Ok(total_secs)
}

/// Format seconds as up to the three most significant time units (e.g., "5d 13h", "1d 2h 45m").
/// Rounds the least-significant displayed unit when a fourth unit would be discarded.
pub fn format_duration(seconds: f64) -> String {
    let secs = seconds.max(0.0);

    // Decompose top-down into integer components, keeping floating-point remainders.
    let d_i = (secs / 86_400.0).floor() as u64;
    let rem_d = secs - d_i as f64 * 86_400.0;
    let h_i = (rem_d / 3_600.0).floor() as u64;
    let rem_h = rem_d - h_i as f64 * 3_600.0;
    let m_i = (rem_h / 60.0).floor() as u64;
    let rem_m = rem_h - m_i as f64 * 60.0;
    let s_i = rem_m.ceil() as u64;

    if d_i == 0 && h_i == 0 && m_i == 0 && s_i == 0 {
        return "0s".to_string();
    }

    let vals = [(d_i, "d"), (h_i, "h"), (m_i, "m"), (s_i, "s")];
    let nz: Vec<usize> = (0..4).filter(|&i| vals[i].0 > 0).collect();

    match nz.len() {
        1 => format!("{}{}", vals[nz[0]].0, vals[nz[0]].1),
        n if n <= 3 => {
            // Display all non-zero units — nothing to discard, no rounding needed.
            nz.iter()
                .map(|&i| format!("{}{}", vals[i].0, vals[i].1))
                .collect::<Vec<_>>()
                .join(" ")
        }
        4 => {
            // All four units non-zero: show top-3, round the third based on discarded fourth.
            // Units are always [d=0, h=1, m=2, s=3] when len==4.
            let mut v_d = d_i;
            let mut v_h = h_i;
            let mut v_m = m_i;

            // Round minutes up if seconds >= 30.
            if s_i >= 30 {
                v_m += 1;
                if v_m >= 60 {
                    v_m -= 60;
                    v_h += 1;
                    if v_h >= 24 {
                        v_h -= 24;
                        v_d += 1;
                    }
                }
            }

            // Build result from non-zero components of the top-3 (d, h, m).
            let parts: Vec<String> = [(v_d, "d"), (v_h, "h"), (v_m, "m")]
                .into_iter()
                .filter_map(|(v, u)| {
                    if v > 0 {
                        Some(format!("{}{}", v, u))
                    } else {
                        None
                    }
                })
                .collect();
            match parts.len() {
                0 => unreachable!(), // impossible: started with all four non-zero
                _ => parts.join(" "),
            }
        }
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::models::Attribute;

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
    #[test]
    fn test_format_duration_zero() {
        assert_eq!(format_duration(0.0), "0s");
    }

    #[test]
    fn test_format_duration_sub_second() {
        // Sub-second rounds up via ceil to 1 second.
        assert_eq!(format_duration(0.5), "1s");
    }

    #[test]
    fn test_format_duration_exact_minute_boundary() {
        // Exactly 60 seconds → "1m" with no trailing "0s".
        let s = format_duration(60.0);
        assert_eq!(s, "1m", "exact minute boundary should not show '0s'");
    }

    #[test]
    fn test_format_duration_just_over_minute_boundary() {
        // 61 seconds → "1m 1s".
        assert_eq!(format_duration(61.0), "1m 1s");
    }

    #[test]
    fn test_format_duration_exact_hour_boundary() {
        // Exactly 3600 seconds → "1h" with no trailing minutes/seconds.
        assert_eq!(format_duration(3600.0), "1h");
    }

    #[test]
    fn test_format_duration_exact_day_boundary() {
        // Exactly 86400 seconds → "1d".
        assert_eq!(format_duration(86_400.0), "1d");
    }

    #[test]
    fn test_format_duration_two_years_shows_days() {
        // ~730 days (2 years) — should display as days only, not weeks or years.
        let s = format_duration(86_400.0 * 730.0);
        assert_eq!(s, "730d", "expected '730d' for two years but got '{}'", s);
        assert!(!s.contains("w") && !s.contains("y"));
    }

    #[test]
    fn test_format_duration_round_when_discarding() {
        // With up-to-3 display, 3-component values pass through unchanged.
        let s = format_duration(86_400.0 + 11.0 * 3_600.0 + 30.0 * 60.0);
        assert_eq!(s, "1d 11h 30m", "expected '1d 11h 30m' but got '{}'", s);

        // All four units non-zero → show d/h/m, round m if s >= 30.
        // 1d 5h 30m 45s → rounds m to 31 → "1d 5h 31m"
        let s = format_duration(86_400.0 + 5.0 * 3_600.0 + 30.0 * 60.0 + 45.0);
        assert_eq!(s, "1d 5h 31m", "expected '1d 5h 31m' but got '{}'", s);

        // 1d 5h 30m 29s → no round → "1d 5h 30m"
        let s = format_duration(86_400.0 + 5.0 * 3_600.0 + 30.0 * 60.0 + 29.0);
        assert_eq!(s, "1d 5h 30m", "expected '1d 5h 30m' but got '{}'", s);

        // Cascade: 1d 5h 59m 30s → rounds m→60, cascades h→6, drops m → "1d 6h"
        let s = format_duration(86_400.0 + 5.0 * 3_600.0 + 59.0 * 60.0 + 30.0);
        assert_eq!(s, "1d 6h", "expected '1d 6h' but got '{}'", s);
    }

    #[test]
    fn test_parse_duration_single_day() {
        assert_eq!(parse_duration("5d").unwrap(), 5.0 * 86_400.0);
    }

    #[test]
    fn test_parse_duration_two_components() {
        assert_eq!(
            parse_duration("3d 12h").unwrap(),
            3.0 * 86_400.0 + 12.0 * 3_600.0
        );
    }

    #[test]
    fn test_parse_duration_hours_minutes() {
        assert_eq!(
            parse_duration("5h 30m").unwrap(),
            5.0 * 3_600.0 + 30.0 * 60.0
        );
    }

    #[test]
    fn test_parse_duration_minutes_seconds() {
        assert_eq!(parse_duration("1m 30s").unwrap(), 90.0);
    }

    #[test]
    fn test_parse_duration_zero() {
        assert_eq!(parse_duration("0s").unwrap(), 0.0);
    }

    #[test]
    fn test_parse_duration_single_hour() {
        assert_eq!(parse_duration("90s").unwrap(), 90.0);
    }

    #[test]
    fn test_parse_duration_whitespace_tolerance() {
        assert_eq!(
            parse_duration("  3d 12h  ").unwrap(),
            3.0 * 86_400.0 + 12.0 * 3_600.0
        );
    }

    #[test]
    fn test_parse_duration_three_components_rejected() {
        let err = parse_duration("1d 2h 3m").unwrap_err();
        assert!(err.to_string().contains("too many time components"));
    }

    #[test]
    fn test_parse_duration_empty_string() {
        assert!(parse_duration("").is_err());
    }

    #[test]
    fn test_parse_duration_bad_unit() {
        assert!(parse_duration("5w").is_err());
    }

    #[test]
    fn test_parse_duration_missing_value() {
        assert!(parse_duration("d").is_err());
    }

    #[test]
    fn test_parse_duration_concatenated_days_hours() {
        // "3d12h" without space — common in game UI copy-paste.
        assert_eq!(
            parse_duration("3d12h").unwrap(),
            3.0 * 86_400.0 + 12.0 * 3_600.0
        );
    }

    #[test]
    fn test_parse_duration_concatenated_hours_minutes() {
        assert_eq!(
            parse_duration("5h30m").unwrap(),
            5.0 * 3_600.0 + 30.0 * 60.0
        );
    }

    #[test]
    fn test_parse_format_roundtrip_concatenated() {
        let original = 3.0 * 86_400.0 + 7.0 * 3_600.0; // 3d 7h
        let formatted = format_duration(original); // "3d 7h" with space
                                                   // Re-parse the spaced output as if it were concatenated.
        let compact = formatted.replace(' ', ""); // "3d7h"
        let parsed = parse_duration(&compact).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn test_parse_format_roundtrip_single_unit() {
        let original = 86_400.0; // 1 day exact
        let formatted = format_duration(original);
        let parsed = parse_duration(&formatted).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn test_parse_format_roundtrip_two_units() {
        let original = 3.0 * 86_400.0 + 7.0 * 3_600.0; // 3d 7h
        let formatted = format_duration(original);
        let parsed = parse_duration(&formatted).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn test_parse_format_roundtrip_hours_minutes() {
        let original = 2.0 * 3_600.0 + 45.0 * 60.0; // 2h 45m
        let formatted = format_duration(original);
        let parsed = parse_duration(&formatted).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn test_parse_format_roundtrip_minutes_seconds() {
        let original = 12.0 * 60.0 + 30.0; // 12m 30s
        let formatted = format_duration(original);
        let parsed = parse_duration(&formatted).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn test_format_duration_hour_cascade_to_day() {
        // 3 non-zero units (h/m/s) → shows all three with no cascade to day.
        // 23h 59m 30s has exactly 3 components → "23h 59m 30s".
        let secs = 23.0 * 3_600.0 + 59.0 * 60.0 + 30.0;
        assert_eq!(format_duration(secs), "23h 59m 30s");
    }
}
