use anyhow::{bail, Context, Result};

use crate::calculator;
use crate::data::models::*;

/// Parse "PER:MEM:WIL:INT:CHA" string into base attributes (e.g., "27:22:17:17:16").
pub fn parse_attributes(input: &str) -> Result<BaseAttributes> {
    let parts: Vec<u32> = input
        .split(':')
        .map(|s| s.trim().parse::<u32>().with_context(|| format!("Invalid attribute value: {}", s)))
        .collect::<Result<Vec<_>>>()?;
    if parts.len() != 5 {
        bail!(
            "--attributes must have exactly 5 values (PER:MEM:WIL:INT:CHA), got {}",
            parts.len()
        );
    }
    let names = ["PER", "MEM", "WIL", "INT", "CHA"];
    for (i, &val) in parts.iter().enumerate() {
        if !(17..=27).contains(&val) {
            bail!("{}={} is out of valid range (17-27)", names[i], val);
        }
    }
    Ok(BaseAttributes {
        perception: parts[0],
        memory: parts[1],
        willpower: parts[2],
        intelligence: parts[3],
        charisma: parts[4],
    })
}

/// Parse implant bonus string "PER:MEM:WIL:INT:CHA" (e.g., "0:1:2:0:1").
pub fn parse_implant_bonuses(input: &str) -> Result<BaseAttributes> {
    let parts: Vec<u32> = input
        .split(':')
        .map(|s| s.trim().parse::<u32>().with_context(|| format!("Invalid implant bonus value: {}", s)))
        .collect::<Result<Vec<_>>>()?;
    if parts.len() != 5 {
        bail!(
            "--implant-bonuses must have exactly 5 values (PER:MEM:WIL:INT:CHA), got {}",
            parts.len()
        );
    }
    let names = ["PER", "MEM", "WIL", "INT", "CHA"];
    for (i, &val) in parts.iter().enumerate() {
        if !(0..=10).contains(&val) {
            bail!(
                "{}={} is out of valid range for implant bonus (0-10)",
                names[i],
                val
            );
        }
    }
    Ok(BaseAttributes {
        perception: parts[0],
        memory: parts[1],
        willpower: parts[2],
        intelligence: parts[3],
        charisma: parts[4],
    })
}

/// Parse a queue file into `QueuedSkill` entries.
///
/// Each non-blank, non-comment line has one of these forms:
/// - `"Skill Name <level>"` — full training ahead
/// - `"Skill Name <level>@<duration>"` — e.g., `@3d12h` time remaining
/// - `"Skill Name <level>@<sp_trained>"` — e.g., `@12000` cumulative SP earned
///
/// Skill matching is case-insensitive against `skills_db`.
pub fn parse_queue(
    content: &str,
    skills_db: &[SkillRecord],
    effective_attrs: &EffectiveAttributes,
    source_label: &str,
) -> Result<Vec<QueuedSkill>> {
    let mut queued_skills = Vec::new();
    for (line_num, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Split on '@' first — everything after is progress info.
        let (skill_level_part, remaining_info) = match trimmed.rsplit_once('@') {
            Some((before_at, after_at)) => (before_at, Some(after_at.trim())),
            None => (trimmed, None),
        };

        // Parse "Skill Name <level>".
        let tokens: Vec<&str> = skill_level_part
            .rsplitn(2, |c: char| c.is_whitespace())
            .collect();
        if tokens.len() != 2 {
            bail!(
                "Line {}: expected 'Skill Name <level>', got '{}'",
                line_num + 1,
                skill_level_part
            );
        }

        let level_str = tokens[0];
        let level: u8 =
            level_str
                .parse::<u8>()
                .with_context(|| format!("Line {}: invalid level '{}', must be 1-5", line_num + 1, level_str))?;
        if !(1..=5).contains(&level) {
            bail!(
                "Line {}: level {} out of range (must be 1-5)",
                line_num + 1,
                level
            );
        }

        let skill_name = tokens[1];
        let record = skills_db
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(skill_name))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Line {}: skill '{}' not found in database",
                    line_num + 1,
                    skill_name
                )
            })?;

        // Compute total SP and duration for this level transition.
        let from_level = level.saturating_sub(1);
        let duration_secs = calculator::duration_seconds(record, from_level, level, effective_attrs);

        // Disambiguate progress info: time-unit suffix → duration, else → SP amount.
        let queued_skill_remaining = match remaining_info {
            None => QueuedSkillRemaining::Duration {
                remaining_sec: duration_secs,
                total_duration_secs: duration_secs,
            },
            Some(info) if matches!(info.chars().next_back(), Some('d' | 'h' | 'm' | 's')) => {
                let remaining_sec =
                    calculator::parse_duration(info).with_context(|| {
                        format!("Line {}: invalid time-left duration '{}'", line_num + 1, info)
                    })?;
                // Clamp: user-provided time-left may exceed our computed total (e.g.,
                // they were training under different attributes/implants). Without the
                // cap, earned_fraction goes negative and remaining SP inflates past total.
                let capped = remaining_sec.min(duration_secs);
                QueuedSkillRemaining::Duration {
                    remaining_sec: capped.max(0.0),
                    total_duration_secs: duration_secs,
                }
            }
            Some(info) => {
                let sp_trained =
                    calculator::parse_sp_value(info).with_context(|| {
                        format!("Line {}: invalid SP value '{}'", line_num + 1, info)
                    })?;
                let cum_from =
                    calculator::CUMULATIVE_SP[from_level as usize] * record.skill_time_constant;
                let cum_to =
                    calculator::CUMULATIVE_SP[level as usize] * record.skill_time_constant;
                if (sp_trained - cum_to).abs() < f64::EPSILON || sp_trained > cum_to {
                    bail!(
                        "Line {}: '{}' has {} SP trained but '{}' at level {} requires less than {:.0} SP — skill is already complete",
                        line_num + 1,
                        info,
                        sp_trained as u64,
                        record.name,
                        level,
                        cum_to
                    );
                }
                if sp_trained < cum_from {
                    bail!(
                        "Line {}: '{}' has {} SP trained but '{}' at level {} needs at least {:.0} SP (level {} threshold)",
                        line_num + 1,
                        info,
                        sp_trained as u64,
                        record.name,
                        level,
                        cum_from,
                        from_level
                    );
                }
                QueuedSkillRemaining::SpTrained { sp_trained }
            }
        };

        queued_skills.push(QueuedSkill {
            id: record.id,
            current_level: from_level,
            remaining: queued_skill_remaining,
        });
    }

    if queued_skills.is_empty() {
        bail!(
            "No valid skills found in '{}'. Format each line as 'Skill Name <level>', 'Skill Name <level>@<time_left>' (e.g., @3d12h), or 'Skill Name <level>@<sp_trained>' (e.g., @12000 or @1,000,000). SP is cumulative from blank.",
            source_label
        );
    }
    Ok(queued_skills)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Helpers ────────────────────────────────────────────────────────────

    fn make_skill(id: u32, name: &str, primary: Attribute, secondary: Attribute, stc: f64) -> SkillRecord {
        SkillRecord {
            id,
            name: name.to_string(),
            primary_attribute: primary,
            secondary_attribute: secondary,
            skill_time_constant: stc,
            prerequisites: vec![],
        }
    }

    fn skills_db() -> Vec<SkillRecord> {
        vec![
            make_skill(1, "Gunnery", Attribute::Intelligence, Attribute::Memory, 1.0),
            make_skill(2, "Navigation", Attribute::Willpower, Attribute::Perception, 2.0),
            make_skill(3, "Shield Operation", Attribute::Charisma, Attribute::Intelligence, 1.5),
            make_skill(4, "Drone Navigation", Attribute::Memory, Attribute::Willpower, 2.5),
            make_skill(5, "Cargo Hold Loader II", Attribute::Perception, Attribute::Memory, 1.0),
            make_skill(6, "Targeting", Attribute::Perception, Attribute::Intelligence, 3.0),
        ]
    }

    /// Uniform attributes (all 27 = max remapped + no implants).
    fn uniform_attrs() -> EffectiveAttributes {
        EffectiveAttributes {
            intelligence: 27.0,
            charisma: 27.0,
            perception: 27.0,
            memory: 27.0,
            willpower: 27.0,
        }
    }

    // ── parse_attributes tests ─────────────────────────────────────────────

    #[test]
    fn test_parse_attributes_uniform_min() {
        let attrs = parse_attributes("17:17:17:17:17").unwrap();
        assert_eq!(attrs.perception, 17);
        assert_eq!(attrs.intelligence, 17);
    }

    #[test]
    fn test_parse_attributes_max_values() {
        let attrs = parse_attributes("27:27:27:27:27").unwrap();
        assert_eq!(attrs.charisma, 27);
    }

    #[test]
    fn test_parse_attributes_mixed() {
        // Note: 16 is out of range — should fail.
        assert!(parse_attributes("27:22:17:17:16").is_err());
    }

    #[test]
    fn test_parse_attributes_wrong_count() {
        assert!(parse_attributes("17:17:17").is_err());
        assert!(parse_attributes("17:17:17:17:17:17").is_err());
    }

    #[test]
    fn test_parse_attributes_out_of_range_low() {
        assert!(parse_attributes("16:17:17:17:17").is_err());
    }

    #[test]
    fn test_parse_attributes_out_of_range_high() {
        assert!(parse_attributes("28:17:17:17:17").is_err());
    }

    #[test]
    fn test_parse_attributes_whitespace_tolerant() {
        let attrs = parse_attributes(" 17 : 17 : 17 : 17 : 17 ").unwrap();
        assert_eq!(attrs.perception, 17);
    }

    // ── parse_implant_bonuses tests ────────────────────────────────────────

    #[test]
    fn test_parse_implant_bonuses_zero() {
        let bonuses = parse_implant_bonuses("0:0:0:0:0").unwrap();
        assert_eq!(bonuses.intelligence, 0);
    }

    #[test]
    fn test_parse_implant_bonuses_mixed() {
        let bonuses = parse_implant_bonuses("0:1:2:3:4").unwrap();
        assert_eq!(bonuses.perception, 0);
        assert_eq!(bonuses.memory, 1);
        assert_eq!(bonuses.willpower, 2);
        assert_eq!(bonuses.intelligence, 3);
        assert_eq!(bonuses.charisma, 4);
    }

    #[test]
    fn test_parse_implant_bonuses_out_of_range() {
        assert!(parse_implant_bonuses("0:0:0:0:11").is_err());
        assert!(parse_implant_bonuses("-1:0:0:0:0").is_err());
    }

    // ── parse_queue: basic format tests ────────────────────────────────────

    #[test]
    fn test_parse_queue_basic_single_skill() {
        let db = skills_db();
        let attrs = uniform_attrs();
        let skills = parse_queue("Gunnery 3", &db, &attrs, "test").unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].id, 1);
        assert_eq!(skills[0].current_level, 2); // from_level for target 3
        matches!(&skills[0].remaining, QueuedSkillRemaining::Duration { remaining_sec, total_duration_secs } if (remaining_sec - total_duration_secs).abs() < f64::EPSILON);
    }

    #[test]
    fn test_parse_queue_multiple_skills() {
        let db = skills_db();
        let attrs = uniform_attrs();
        let input = "Gunnery 3\nNavigation 5\nShield Operation 2";
        let skills = parse_queue(input, &db, &attrs, "test").unwrap();
        assert_eq!(skills.len(), 3);
        assert_eq!(skills[0].id, 1);
        assert_eq!(skills[1].id, 2);
        assert_eq!(skills[2].id, 3);
    }

    #[test]
    fn test_parse_queue_case_insensitive_matching() {
        let db = skills_db();
        let attrs = uniform_attrs();
        let skills = parse_queue("GUNNERY 3\ngunnery 1\nGunNeRy 5", &db, &attrs, "test").unwrap();
        assert_eq!(skills.len(), 3);
        for s in &skills {
            assert_eq!(s.id, 1); // all match same skill
        }
    }

    #[test]
    fn test_parse_queue_comments_and_blanks_ignored() {
        let db = skills_db();
        let attrs = uniform_attrs();
        let input = "# This is a comment\n\nGunnery 3\n   \n# Another comment\nNavigation 5";
        let skills = parse_queue(input, &db, &attrs, "test").unwrap();
        assert_eq!(skills.len(), 2);
    }

    #[test]
    fn test_parse_queue_level_range() {
        let db = skills_db();
        let attrs = uniform_attrs();
        // Level 1 → from_level 0
        let skills = parse_queue("Gunnery 1", &db, &attrs, "test").unwrap();
        assert_eq!(skills[0].current_level, 0);

        // Level 5 → from_level 4
        let skills = parse_queue("Gunnery 5", &db, &attrs, "test").unwrap();
        assert_eq!(skills[0].current_level, 4);
    }

    // ── parse_queue: duration progress tests ───────────────────────────────

    #[test]
    fn test_parse_queue_duration_progress_days_hours() {
        let db = skills_db();
        let attrs = uniform_attrs();
        // Use a short duration well within Gunnery L2->L3 total (~2h42m at 27/27 attrs).
        let skills = parse_queue("Gunnery 3@2h", &db, &attrs, "test").unwrap();
        assert_eq!(skills.len(), 1);
        match &skills[0].remaining {
            QueuedSkillRemaining::Duration { remaining_sec, .. } => {
                assert!((remaining_sec - (2.0 * 3_600.0)).abs() < 1.0);
            }
            _ => panic!("expected Duration variant"),
        }
    }

    #[test]
    fn test_parse_queue_duration_progress_with_space() {
        let db = skills_db();
        let attrs = uniform_attrs();
        let skills = parse_queue("Navigation 5@5h 30m", &db, &attrs, "test").unwrap();
        match &skills[0].remaining {
            QueuedSkillRemaining::Duration { remaining_sec, .. } => {
                assert!((remaining_sec - (5.0 * 3_600.0 + 30.0 * 60.0)).abs() < 1.0);
            }
            _ => panic!("expected Duration variant"),
        }
    }

    #[test]
    fn test_parse_queue_duration_zero_remaining() {
        // @0s means skill is essentially complete; total still computed from attrs.
        let db = skills_db();
        let attrs = uniform_attrs();
        let skills = parse_queue("Gunnery 3@0s", &db, &attrs, "test").unwrap();
        match &skills[0].remaining {
            QueuedSkillRemaining::Duration { remaining_sec, total_duration_secs } => {
                assert!((remaining_sec - 0.0).abs() < f64::EPSILON);
                assert!(*total_duration_secs > 0.0);
            }
            _ => panic!("expected Duration variant"),
        }
    }

    #[test]
    fn test_parse_queue_duration_equals_total_no_progress() {
        // When remaining equals total, earned_fraction = 0 → full SP remaining in optimizer.
        let db = skills_db();
        let attrs = uniform_attrs();
        // First compute what the duration would be for Gunnery L2→L3 at uniform 27/27 attrs:
        // rate = (27 + 27/2)/60 = 40.5/60 = 0.675 SP/s
        // sp_needed = (8000 - 1414) * 1.0 = 6586.0
        // duration ≈ 9757.0 s ≈ 2h42m
        let input = "Gunnery 3@2h42m";
        let skills = parse_queue(input, &db, &attrs, "test").unwrap();
        match &skills[0].remaining {
            QueuedSkillRemaining::Duration { remaining_sec, total_duration_secs } => {
                // Should be approximately equal — within a few seconds of rounding.
                assert!((remaining_sec - total_duration_secs).abs() < 120.0);
            }
            _ => panic!("expected Duration variant"),
        }
    }

    #[test]
    fn test_parse_queue_duration_minutes_seconds() {
        let db = skills_db();
        let attrs = uniform_attrs();
        let skills = parse_queue("Gunnery 1@1m30s", &db, &attrs, "test").unwrap();
        match &skills[0].remaining {
            QueuedSkillRemaining::Duration { remaining_sec, .. } => {
                assert!((remaining_sec - 90.0).abs() < 1.0);
            }
            _ => panic!("expected Duration variant"),
        }
    }

    // ── parse_queue: SP-trained progress tests ─────────────────────────────

    #[test]
    fn test_parse_queue_sp_trained_bare_number() {
        let db = skills_db();
        let attrs = uniform_attrs();
        // Gunnery STC=1.0, L1→L2 needs (1414-250)=1164 SP.
        // @500 means 500 SP already earned toward this transition.
        let skills = parse_queue("Gunnery 2@500", &db, &attrs, "test").unwrap();
        match &skills[0].remaining {
            QueuedSkillRemaining::SpTrained { sp_trained } => {
                assert_eq!(*sp_trained, 500.0);
            }
            _ => panic!("expected SpTrained variant"),
        }
    }

    #[test]
    fn test_parse_queue_sp_trained_with_commas() {
        let db = skills_db();
        let attrs = uniform_attrs();
        // Navigation STC=2.0, L4→L5 needs (256000-45255)*2.0 = 421490 SP.
        // @200,000 — halfway there with commas.
        let skills = parse_queue("Navigation 5@200,000", &db, &attrs, "test").unwrap();
        match &skills[0].remaining {
            QueuedSkillRemaining::SpTrained { sp_trained } => {
                assert_eq!(*sp_trained, 200_000.0);
            }
            _ => panic!("expected SpTrained variant"),
        }
    }

    #[test]
    fn test_parse_queue_sp_trained_too_high_rejected() {
        let db = skills_db();
        let attrs = uniform_attrs();
        // Gunnery L2: cum_to = CUMULATIVE_SP[2]*1.0 = 1414.0
        // Providing @2000 exceeds the total for this level transition.
        let err = parse_queue("Gunnery 2@2000", &db, &attrs, "test").unwrap_err();
        assert!(err.to_string().contains("already complete"));
    }

    #[test]
    fn test_parse_queue_sp_trained_exact_threshold_accepted() {
        let db = skills_db();
        let attrs = uniform_attrs();
        // Gunnery L3: from_level=2, cum_from = CUMULATIVE_SP[2]*1.0 = 1414
        // cum_to = CUMULATIVE_SP[3]*1.0 = 8000
        // @1500 is in range [1414, 8000).
        let skills = parse_queue("Gunnery 3@1500", &db, &attrs, "test").unwrap();
        assert_eq!(skills.len(), 1);
        match &skills[0].remaining {
            QueuedSkillRemaining::SpTrained { sp_trained } => {
                assert_eq!(*sp_trained, 1500.0);
            }
            _ => panic!("expected SpTrained variant"),
        }
    }

    #[test]
    fn test_parse_queue_sp_trained_below_threshold_rejected() {
        let db = skills_db();
        let attrs = uniform_attrs();
        // Gunnery L3→L5: from_level=2, cum_from = CUMULATIVE_SP[2]*1.0 = 1414
        // @500 < 1414 → below level 2 threshold.
        let err = parse_queue("Gunnery 5@500", &db, &attrs, "test").unwrap_err();
        assert!(err.to_string().contains("needs at least"));
    }

    // ── parse_queue: error cases ───────────────────────────────────────────

    #[test]
    fn test_parse_queue_empty_input_errors() {
        let db = skills_db();
        let attrs = uniform_attrs();
        let err = parse_queue("", &db, &attrs, "stdin").unwrap_err();
        assert!(err.to_string().contains("No valid skills found"));
    }

    #[test]
    fn test_parse_queue_only_comments_errors() {
        let db = skills_db();
        let attrs = uniform_attrs();
        let err = parse_queue("# just a comment\n# another", &db, &attrs, "q.txt").unwrap_err();
        assert!(err.to_string().contains("No valid skills found"));
    }

    #[test]
    fn test_parse_queue_unknown_skill_error() {
        let db = skills_db();
        let attrs = uniform_attrs();
        let err = parse_queue("Nonexistent Skill 3", &db, &attrs, "test").unwrap_err();
        assert!(err.to_string().contains("not found in database"));
    }

    #[test]
    fn test_parse_queue_invalid_level_zero() {
        let db = skills_db();
        let attrs = uniform_attrs();
        let err = parse_queue("Gunnery 0", &db, &attrs, "test").unwrap_err();
        assert!(err.to_string().contains("out of range"));
    }

    #[test]
    fn test_parse_queue_invalid_level_six() {
        let db = skills_db();
        let attrs = uniform_attrs();
        let err = parse_queue("Gunnery 6", &db, &attrs, "test").unwrap_err();
        assert!(err.to_string().contains("out of range"));
    }

    #[test]
    fn test_parse_queue_missing_level_error() {
        let db = skills_db();
        let attrs = uniform_attrs();
        let err = parse_queue("Gunnery", &db, &attrs, "test").unwrap_err();
        assert!(err.to_string().contains("expected 'Skill Name <level>'"));
    }

    #[test]
    fn test_parse_queue_non_numeric_level_error() {
        let db = skills_db();
        let attrs = uniform_attrs();
        let err = parse_queue("Gunnery x", &db, &attrs, "test").unwrap_err();
        assert!(err.to_string().contains("invalid level"));
    }

    #[test]
    fn test_parse_queue_bad_duration_format() {
        let db = skills_db();
        let attrs = uniform_attrs();
        // Ends with 'd' so treated as duration — but invalid format (no number).
        let err = parse_queue("Gunnery 3@d", &db, &attrs, "test").unwrap_err();
        assert!(err.to_string().contains("invalid time-left duration"));
    }

    #[test]
    fn test_parse_queue_negative_sp_rejected() {
        let db = skills_db();
        let attrs = uniform_attrs();
        use std::error::Error;
        let err = parse_queue("Gunnery 2@-100", &db, &attrs, "test").unwrap_err();
        assert!(err.root_cause().to_string().contains("must not be negative"));
    }

    #[test]
    fn test_parse_queue_line_numbers_in_errors() {
        let db = skills_db();
        let attrs = uniform_attrs();
        let input = "Gunnery 3\nBadSkillName 5";
        let err = parse_queue(input, &db, &attrs, "test").unwrap_err();
        assert!(err.to_string().starts_with("Line 2:"));
    }

    // ── parse_queue: disambiguation tests ──────────────────────────────────

    #[test]
    fn test_parse_queue_at_value_ending_in_s_is_duration() {
        // Value ending in 's' is treated as duration (seconds).
        let db = skills_db();
        let attrs = uniform_attrs();
        let skills = parse_queue("Gunnery 2@90s", &db, &attrs, "test").unwrap();
        match &skills[0].remaining {
            QueuedSkillRemaining::Duration { remaining_sec, .. } => {
                assert!((remaining_sec - 90.0).abs() < 1.0);
            }
            _ => panic!("expected Duration — bare numbers ending in 's' are durations"),
        }
    }

    #[test]
    fn test_parse_queue_at_bare_number_is_sp_trained() {
        // Number without time suffix → SP trained.
        let db = skills_db();
        let attrs = uniform_attrs();
        let skills = parse_queue("Gunnery 2@500", &db, &attrs, "test").unwrap();
        match &skills[0].remaining {
            QueuedSkillRemaining::SpTrained { sp_trained } => {
                assert_eq!(*sp_trained, 500.0);
            }
            _ => panic!("expected SpTrained variant"),
        }
    }

    // ── parse_queue: multi-level same skill ────────────────────────────────

    #[test]
    fn test_parse_queue_same_skill_multiple_levels() {
        let db = skills_db();
        let attrs = uniform_attrs();
        let input = "Gunnery 2\nGunnery 3\nGunnery 4";
        let skills = parse_queue(input, &db, &attrs, "test").unwrap();
        assert_eq!(skills.len(), 3);
        for s in &skills {
            assert_eq!(s.id, 1);
        }
        assert_eq!(skills[0].current_level, 1); // L1->L2
        assert_eq!(skills[1].current_level, 2); // L2->L3
        assert_eq!(skills[2].current_level, 3); // L3->L4
    }

    // ── parse_queue: source_label in error messages ────────────────────────

    #[test]
    fn test_parse_queue_source_label_in_empty_error() {
        let db = skills_db();
        let attrs = uniform_attrs();
        let err = parse_queue("", &db, &attrs, "-").unwrap_err();
        assert!(err.to_string().contains("in '-'"));
    }
}
