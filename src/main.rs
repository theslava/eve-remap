mod calculator;
mod cli;
mod data;
mod optimizer;

use anyhow::{Context, Result};
use clap::Parser;

fn main() -> Result<()> {
    let cli = cli::Cli::parse();

    match &cli.command {
        cli::Commands::Optimize(args) => cmd_optimize(&args),
    }
}

// ── Optimize ─────────────────────────────────────────────────────────────

fn cmd_optimize(args: &cli::OptimizeArgs) -> Result<()> {
    let skills_db = data::load_skills().context("Failed to load skill database")?;
    let implants = data::load_implants().context("Failed to load implant database")?;

    // Parse --remap-available: "0d", "30d", etc. → seconds from now.
    let remap_available_str = args.remap_available.trim();
    let remap_available_secs = if let Some(num) = remap_available_str.strip_suffix('d') {
        num.parse::<f64>()
            .with_context(|| format!("Invalid --remap-available '{}': expected a number followed by 'd'", remap_available_str))?
            * 86_400.0
    } else {
        anyhow::bail!(
            "Invalid --remap-available '{}': expected a value like '0d' or '30d'",
            remap_available_str
        );
    };

    let result = run_optimizer_from_queue_file(
        &args.attributes,
        &args.implant_bonuses,
        args.bonus_remaps,
        &args.queue,
        &skills_db,
        &implants,
        remap_available_secs,
    )?;

    if args.json {
        print_json_output(&result);
    } else {
        print_table_output(&result);
    }

    if let Some(out_path) = &args.queue_out {
        write_queue_file(out_path, &result)?;
    }

    Ok(())
}

/// Parse a queue file and optimize directly without ESI.
fn run_optimizer_from_queue_file(
    attrs_str: &str,
    implant_bonuses_str: &str,
    bonus_remaps: Option<u32>,
    path: &str,
    skills_db: &[data::models::SkillRecord],
    implants: &[data::models::ImplantRecord],
    remap_available_secs: f64,
) -> Result<data::models::OptimizationResult> {
    use data::models::{BaseAttributes, EffectiveAttributes, QueuedSkill};

    // Parse attributes string like "17:17:17:17:17" (PER:MEM:WIL:INT:CHA).
    let parts: Vec<f64> = attrs_str.split(':')
        .map(|s| s.trim().parse::<f64>().with_context(|| format!("Invalid attribute value: {}", s)))
        .collect::<Result<Vec<_>>>()?;
    if parts.len() != 5 {
        anyhow::bail!("--attributes must have exactly 5 values (PER:MEM:WIL:INT:CHA), got {}", parts.len());
    }
    let base_attrs = BaseAttributes {
        perception: parts[0],
        memory: parts[1],
        willpower: parts[2],
        intelligence: parts[3],
        charisma: parts[4],
    };

    // Parse implant bonus string like "0:0:0:0:0" (PER:MEM:WIL:INT:CHA).
    let ib_parts: Vec<f64> = implant_bonuses_str.split(':')
        .map(|s| s.trim().parse::<f64>().with_context(|| format!("Invalid implant bonus value: {}", s)))
        .collect::<Result<Vec<_>>>()?;
    if ib_parts.len() != 5 {
        anyhow::bail!("--implant-bonuses must have exactly 5 values (PER:MEM:WIL:INT:CHA), got {}", ib_parts.len());
    }
    let implant_bonus = BaseAttributes {
        perception: ib_parts[0],
        memory: ib_parts[1],
        willpower: ib_parts[2],
        intelligence: ib_parts[3],
        charisma: ib_parts[4],
    };

    // Read and parse queue file.
    let content = std::fs::read_to_string(path).context("Failed to read queue file")?;
    let mut queued_skills = Vec::new();
    for (line_num, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Parse "Skill Name <level>" — level is the last token, skill name is everything before it.
        let tokens: Vec<&str> = trimmed.rsplitn(2, |c: char| c.is_whitespace()).collect();
        if tokens.len() != 2 {
            anyhow::bail!("Line {}: expected 'Skill Name <level>', got '{}'", line_num + 1, trimmed);
        }

        let level: u8 = tokens[0].parse::<u8>().with_context(|| format!(
            "Line {}: invalid level '{}', must be 1-5", line_num + 1, tokens[0]
        ))?;
        if level < 1 || level > 5 {
            anyhow::bail!("Line {}: level {} out of range (must be 1-5)", line_num + 1, level);
        }

        let skill_name = tokens[1];
        let record = skills_db.iter()
            .find(|s| s.name.eq_ignore_ascii_case(skill_name))
            .ok_or_else(|| anyhow::anyhow!(
                "Line {}: skill '{}' not found in database", line_num + 1, skill_name
            ))?;

        // level N means "train from level N-1 to N". Level 1 = from nothing.
        let from_level = if level <= 1 { 0u8 } else { level - 1 };
        let sp_to_next = calculator::sp_for_level(record, from_level, level);
        let effective_attrs = data::models::EffectiveAttributes::from(base_attrs);
        let duration_secs = calculator::duration_seconds(
            record, from_level, level, &effective_attrs,
        );

        queued_skills.push(QueuedSkill {
            id: record.id,
            level: from_level, // current trained level; optimizer adds +1 for target
            sp: sp_to_next as u64,
            duration: duration_secs.max(1.0) as u64,
            remaining_sec: duration_secs.max(1.0) as u64,
            is_active: queued_skills.is_empty(),
        });
    }

    if queued_skills.is_empty() {
        anyhow::bail!("No valid skills found in '{}'. Format each line as 'Skill Name <level>'.", path);
    }
    eprintln!(
        "Queue file '{}' — {} skills, effective PER={} MEM={} WIL={} INT={} CHA={}",
        path,
        queued_skills.len(),
        (base_attrs.perception + implant_bonus.perception) as u32,
        (base_attrs.memory + implant_bonus.memory) as u32,
        (base_attrs.willpower + implant_bonus.willpower) as u32,
        (base_attrs.intelligence + implant_bonus.intelligence) as u32,
        (base_attrs.charisma + implant_bonus.charisma) as u32,
    );

    let char_state = data::models::CharacterState {
        base_attributes: base_attrs,
        active_implant_ids: vec![],
        implant_bonus,
        queued_skills,
        effective_attributes: EffectiveAttributes::from(base_attrs),
        bonus_remaps,
        normal_remap_available_in_secs: remap_available_secs,
    };
    run_optimizer_with_state(&char_state, skills_db, implants)
}

/// Run the optimizer engine against a character state and return results.
fn run_optimizer_with_state(
    char_state: &data::models::CharacterState,
    skills_db: &[data::models::SkillRecord],
    implants: &[data::models::ImplantRecord],
) -> Result<data::models::OptimizationResult> {
    let result = optimizer::optimize(char_state, skills_db, implants);
    Ok(result)
}

// ── Output formatters ────────────────────────────────────────────────────

fn print_table_output(result: &data::models::OptimizationResult) {
    println!("\n{}", "=".repeat(72));
    println!("REMAP OPTIMIZATION PLAN");
    println!("{}\n", "-".repeat(72));

    // Attribute key order matches display elsewhere (PER:MEM:WIL:INT:CHA).
    const ATTR_KEYS: [&str; 5] = ["perception", "memory", "willpower", "intelligence", "charisma"];

    for (i, epoch) in result.epochs.iter().enumerate() {
        let epoch_type = match i {
            0 => "Initial allocation",
            _ => "Remap",
        };
        println!("Epoch {}: {}", i + 1, epoch_type);
        println!(
            "  Attributes: PER={} MEM={} WIL={} INT={} CHA={}",
            epoch.effective_attributes.perception,
            epoch.effective_attributes.memory,
            epoch.effective_attributes.willpower,
            epoch.effective_attributes.intelligence,
            epoch.effective_attributes.charisma,
        );
        println!(
            "  Duration: {} ({:.1} days)",
            calculator::format_duration(epoch.projected_finish_secs - epoch.start_offset_secs),
            (epoch.projected_finish_secs - epoch.start_offset_secs) / 86_400.0,
        );

        for (_skill_id, skill_name, _target_level, train_secs) in &epoch.completed_skills {
            println!(
                "    - {} ({})",
                skill_name,
                calculator::format_duration(*train_secs),
            );
        }

        // SP breakdown by role and attribute
        let pri_vals: Vec<f64> = ATTR_KEYS.iter()
            .map(|k| epoch.sp_summary.primary.get(*k).copied().unwrap_or(0.0))
            .collect();
        let sec_vals: Vec<f64> = ATTR_KEYS.iter()
            .map(|k| epoch.sp_summary.secondary.get(*k).copied().unwrap_or(0.0))
            .collect();

        if pri_vals.iter().sum::<f64>() > 0.0 || sec_vals.iter().sum::<f64>() > 0.0 {
            println!("  {:<12} {:>8} {:>8} {:>8} {:>8} {:>8}", "", "PER", "MEM", "WIL", "INT", "CHA");
            println!(
                "  {:<12} {:>8.0} {:>8.0} {:>8.0} {:>8.0} {:>8.0}",
                "Primary:",
                pri_vals[0], pri_vals[1], pri_vals[2], pri_vals[3], pri_vals[4]
            );
            println!(
                "  {:<12} {:>8.0} {:>8.0} {:>8.0} {:>8.0} {:>8.0}",
                "Secondary:",
                sec_vals[0], sec_vals[1], sec_vals[2], sec_vals[3], sec_vals[4]
            );
        }
        println!();
    }

    let total_days = result.total_wall_clock_seconds / 86_400.0;
    println!("{}", "-".repeat(72));
    println!("Total training time: {:.1} days", total_days);
    println!("Epochs: {}", result.epochs.len());
    if result.baseline_wall_clock_seconds > 0.0 {
        let baseline_days = result.baseline_wall_clock_seconds / 86_400.0;
        println!("Baseline (no remaps): {:.1} days", baseline_days);
    }
    println!();
}

fn print_json_output(result: &data::models::OptimizationResult) {
    let json = serde_json::to_string_pretty(result).expect("Failed to serialize result");
    println!("{}", json);
}

/// Write the optimized skill queue order to a file in "Skill Name <level>" format.
fn write_queue_file(
    path: &str,
    result: &data::models::OptimizationResult,
) -> Result<()> {
    let mut seen = std::collections::HashSet::<u32>::new();
    let mut lines = Vec::new();
    for epoch in &result.epochs {
        for (skill_id, skill_name, target_level, _train_secs) in &epoch.completed_skills {
            if seen.insert(*skill_id) {
                lines.push(format!("{} {}", skill_name, target_level));
            }
        }
    }

    use std::io::Write;
    let content = lines.join("\n") + "\n";
    let mut file = std::fs::File::create(path).context("Failed to create output queue file")?;
    file.write_all(content.as_bytes())
        .context("Failed to write output queue file")?;

    eprintln!(
        "[+] Optimized queue written to '{}' ({} skills)",
        path,
        lines.len()
    );
    Ok(())
}
