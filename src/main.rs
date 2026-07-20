mod calculator;
mod cli;
mod data;
mod optimizer;

use anyhow::{Context, Result};
use std::io::{self, Read};
use clap::Parser;

fn main() -> Result<()> {
    let cli = cli::Cli::parse();

    match cli.command {
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

/// Read content from a file or stdin (when path is "-").
fn read_queue_content(path: &str) -> Result<String> {
    if path == "-" {
        let mut buf = io::BufReader::new(io::stdin());
        let mut s = String::new();
        buf.read_to_string(&mut s)?;
        Ok(s)
    } else {
        std::fs::read_to_string(path).context(format!("Failed to read queue file '{}'", path))
    }
}

/// Parse a queue file and run the offline optimizer with user-provided attributes.
fn run_optimizer_from_queue_file(
    attrs_str: &str,
    implant_bonuses_str: &str,
    bonus_remaps: Option<u32>,
    path: &str,
    skills_db: &[data::models::SkillRecord],
    implants: &[data::models::ImplantRecord],
    remap_available_secs: f64,
) -> Result<data::models::OptimizationResult> {
    use data::models::{BaseAttributes, QueuedSkill};

    // Parse attributes string like "17:17:17:17:17" (PER:MEM:WIL:INT:CHA).
    let parts: Vec<f64> = attrs_str.split(':')
        .map(|s| s.trim().parse::<f64>().with_context(|| format!("Invalid attribute value: {}", s)))
        .collect::<Result<Vec<_>>>()?;
    if parts.len() != 5 {
        anyhow::bail!("--attributes must have exactly 5 values (PER:MEM:WIL:INT:CHA), got {}", parts.len());
    }
    // Validate attribute ranges (base remapped attributes are typically 17-27).
    {
        let names = ["PER", "MEM", "WIL", "INT", "CHA"];
        for (i, &val) in parts.iter().enumerate() {
            if !(17.0..=27.0).contains(&val) {
                anyhow::bail!("{}={} is out of valid range (17-27)", names[i], val);
            }
        }
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
    // Validate implant bonus ranges (typically 0-10 per attribute).
    {
        let names = ["PER", "MEM", "WIL", "INT", "CHA"];
        for (i, &val) in ib_parts.iter().enumerate() {
            if !(0.0..=10.0).contains(&val) {
                anyhow::bail!("{}={} is out of valid range for implant bonus (0-10)", names[i], val);
            }
        }
    }
    let implant_bonus = BaseAttributes {
        perception: ib_parts[0],
        memory: ib_parts[1],
        willpower: ib_parts[2],
        intelligence: ib_parts[3],
        charisma: ib_parts[4],
    };
    // Effective attributes including implants — used for duration calculations below.
    let effective_attrs = data::models::EffectiveAttributes::from(base_attrs.add(&implant_bonus));
    // Read from file or stdin (when path is "-").
    let content = read_queue_content(path)?;
    let mut queued_skills = Vec::new();
    for (line_num, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Split on '@' first — everything after is the optional time-left duration
        // (may contain spaces like "3d 12h"). The rest is "Skill Name <level>".
        let (skill_level_part, remaining_time_secs) = match trimmed.rsplit_once('@') {
            Some((before_at, dur_part)) => {
                let secs = calculator::parse_duration(dur_part).with_context(|| format!(
                    "Line {}: invalid time-left duration '{}'",
                    line_num + 1, dur_part.trim()
                ))?;
                (before_at, Some(secs))
            }
            None => (trimmed, None),
        };

        // Parse "Skill Name <level>" from the part before '@'.
        let tokens: Vec<&str> = skill_level_part.rsplitn(2, |c: char| c.is_whitespace()).collect();
        if tokens.len() != 2 {
            anyhow::bail!("Line {}: expected 'Skill Name <level>', got '{}'", line_num + 1, skill_level_part);
        }

        let level_str = tokens[0];
        let level: u8 = level_str.parse::<u8>().with_context(|| format!(
            "Line {}: invalid level '{}', must be 1-5", line_num + 1, level_str
        ))?;
        if !(1..=5).contains(&level) {
            anyhow::bail!("Line {}: level {} out of range (must be 1-5)", line_num + 1, level);
        }

        let skill_name = tokens[1];
        let record = skills_db.iter()
            .find(|s| s.name.eq_ignore_ascii_case(skill_name))
            .ok_or_else(|| anyhow::anyhow!(
                "Line {}: skill '{}' not found in database", line_num + 1, skill_name
            ))?;

        // level N means "train from level N-1 to N". Level 1 = from nothing.
        let from_level = level.saturating_sub(1);
        let duration_secs = calculator::duration_seconds(
            record, from_level, level, &effective_attrs,
        );

        // If no explicit time-left was given, the full duration remains.
        let remaining_sec = match remaining_time_secs {
            Some(secs) => secs.max(0.0),
            None => duration_secs,
        };

        queued_skills.push(QueuedSkill {
            id: record.id,
            level: from_level, // current trained level; optimizer adds +1 for target
            duration: duration_secs.max(1.0) as u64,
            remaining_sec: remaining_sec.max(1.0) as u64,
        });
    }

    if queued_skills.is_empty() {
        anyhow::bail!("No valid skills found in '{}'. Format each line as 'Skill Name <level>' or 'Skill Name <level>@<time_left>'.", path);
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

/// Format an SP value using SI-style suffixes (K, M).
fn format_sp(sp: f64) -> String {
    if sp >= 1_000_000.0 {
        format!("{:.1}M", sp / 1_000_000.0)
    } else if sp >= 1_000.0 {
        format!("{:.1}K", sp / 1_000.0)
    } else {
        format!("{:.0}", sp)
    }
}

/// Format a number with comma thousands separators (e.g., "217,300").
fn format_number(n: f64) -> String {
    let int = n as u64;
    let s = int.to_string();
    let mut result = String::with_capacity(s.len() + s.len() / 3);
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    for (i, &c) in chars.iter().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result
}

fn print_table_output(result: &data::models::OptimizationResult) {
    println!("\n{}", "=".repeat(72));
    println!("REMAP OPTIMIZATION PLAN");
    println!("{}\n", "-".repeat(72));

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

        // SP summary as an attribute matrix table
        let pri_vals: Vec<f64> = ATTR_KEYS.iter()
            .map(|k| epoch.sp_summary.primary.get(*k).copied().unwrap_or(0.0))
            .collect();
        let sec_vals: Vec<f64> = ATTR_KEYS.iter()
            .map(|k| epoch.sp_summary.secondary.get(*k).copied().unwrap_or(0.0))
            .collect();

        if pri_vals.iter().sum::<f64>() > 0.0 || sec_vals.iter().sum::<f64>() > 0.0 {
            let fmt = |v: f64| -> String { if v == 0.0 { "-".into() } else { format_sp(v) } };
            println!(
                "  {:>4} {:>7} {:>7} {:>7} {:>7} {:>7}",
                "", "PER", "MEM", "WIL", "INT", "CHA"
            );
            println!(
                "  Pri  {:>7} {:>7} {:>7} {:>7} {:>7}",
                fmt(pri_vals[0]), fmt(pri_vals[1]),
                fmt(pri_vals[2]), fmt(pri_vals[3]), fmt(pri_vals[4])
            );
            println!(
                "  Sec  {:>7} {:>7} {:>7} {:>7} {:>7}",
                fmt(sec_vals[0]), fmt(sec_vals[1]),
                fmt(sec_vals[2]), fmt(sec_vals[3]), fmt(sec_vals[4])
            );
        }

        for (_skill_id, skill_name, target_level, train_secs) in &epoch.completed_skills {
            println!(
                "    - {} {} - {}",
                skill_name,
                target_level,
                calculator::format_duration(*train_secs),
            );
        }
        println!();
    }

    let total_days = result.total_wall_clock_seconds / 86_400.0;
    // Sum SP from primary buckets — each skill contributes exactly once.
    let total_sp: f64 = result.epochs.iter()
        .flat_map(|e| e.sp_summary.primary.values()).sum();

    println!("{}", "-".repeat(72));
    println!("Total training time: {:.1} days", total_days);
    println!("Total SP in queue: {}", format_number(total_sp));
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
    let mut lines = Vec::new();
    for epoch in &result.epochs {
        for (_skill_id, skill_name, target_level, _train_secs) in &epoch.completed_skills {
            lines.push(format!("{} {}", skill_name, target_level));
        }
    }

    use std::io::{self, Write};
    let content = lines.join("\n") + "\n";
    if path == "-" {
        io::stdout().write_all(content.as_bytes()).context("Failed to write to stdout")?;
        io::stdout().flush()?;
        eprintln!("[+] Optimized queue written to stdout ({} skills)", lines.len());
    } else {
        let mut file = std::fs::File::create(path).context(format!("Failed to create output queue file '{}'", path))?;
        file.write_all(content.as_bytes())
            .context("Failed to write output queue file")?;
        eprintln!(
            "[+] Optimized queue written to '{}' ({} skills)",
            path,
            lines.len()
        );
    }
    Ok(())
}
