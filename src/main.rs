mod calculator;
mod cli;
mod data;
mod optimizer;
mod parser;

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
    let base_attrs = parser::parse_attributes(attrs_str)?;
    let implant_bonus = parser::parse_implant_bonuses(implant_bonuses_str)?;
    let effective_attrs = data::models::EffectiveAttributes::from(base_attrs.add(&implant_bonus));

    let content = read_queue_content(path)?;
    let queued_skills = parser::parse_queue(&content, skills_db, &effective_attrs, path)?;

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
