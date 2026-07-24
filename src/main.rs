mod auth;
mod calculator;
mod cli;
mod data;
mod esi;
mod optimizer;
mod parser;

use anyhow::{Context, Result};
use clap::Parser;
use std::io::{self, Read};

fn main() -> Result<()> {
    // Install crypto provider before any TLS code runs (both aws-lc-rs and ring resolve)
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cli = cli::Cli::parse();

    match cli.command {
        cli::Commands::Optimize(args) => cmd_optimize(&args),
        cli::Commands::Login(args) => cmd_login(&args),
        cli::Commands::Logout(args) => cmd_logout(&args),
        cli::Commands::Accounts => cmd_accounts(),
    }
}

// ── Optimize ─────────────────────────────────────────────────────────────

fn cmd_optimize(args: &cli::OptimizeArgs) -> Result<()> {
    let skills_db = data::load_skills().context("Failed to load skill database")?;
    let implants = data::load_implants().context("Failed to load implant database")?;

    // ── Fetch ESI data if --character specified ────────────────────────
    let esi_data = if let Some(char_query) = &args.character {
        eprintln!("Resolving character '{}'", char_query);
        let account = resolve_character(char_query)?;
        let rt = tokio::runtime::Runtime::new().context("failed to create tokio runtime")?;
        Some(rt.block_on(esi::fetch_character_data(&account))?)
    } else {
        None
    };

    // ── Resolve base + implant bonuses ────────────────────────────────
    let (base_attrs, source_label, implant_bonus) =
        resolve_attributes(&args, &esi_data, &implants)?;

    let effective_attrs = data::models::EffectiveAttributes::from(base_attrs.add(&implant_bonus));

    eprintln!(
        "Base     PER={} MEM={} WIL={} INT={} CHA={}",
        base_attrs.perception, base_attrs.memory, base_attrs.willpower,
        base_attrs.intelligence, base_attrs.charisma,
    );
    eprintln!(
        "Implant  +{} +{} +{} +{} +{}",
        implant_bonus.perception, implant_bonus.memory, implant_bonus.willpower,
        implant_bonus.intelligence, implant_bonus.charisma,
    );
    eprintln!(
        "Effective PER={} MEM={} WIL={} INT={} CHA={}",
        effective_attrs.perception, effective_attrs.memory, effective_attrs.willpower,
        effective_attrs.intelligence, effective_attrs.charisma,
    );
    // ── Build queued skills ────────────────────────────────────────────
    let (queued_skills, queue_label) = if let Some(path) = &args.queue {
        let content = read_queue_content(path)?;
        let skills = parser::parse_queue(&content, &skills_db, &effective_attrs, path)?;
        (skills, format!("queue file '{}'", path))
    } else if let Some(data) = &esi_data {
        let skills = build_queued_from_esi(data, &skills_db, &effective_attrs)?;
        (skills, "ESI /skillqueue/".to_string())
    } else {
        anyhow::bail!(
            "--queue is required when --character is not specified. Use --character to auto-fetch from ESI."
        )
    };

    // ── Bonus remaps: CLI > ESI > None ─────────────────────────────────
    let bonus_remaps = args
        .bonus_remaps
        .or_else(|| esi_data.as_ref().and_then(|d| d.bonus_remaps));

    // ── Normal remap available: CLI duration > ESI cooldown date > 0 ───
    let remap_available_secs = match (&args.remap_available, &esi_data) {
        (Some(dur), _) => calculator::parse_duration(dur).with_context(|| {
            format!(
                "Invalid --remap-available '{}': expected '0d' or '30d'",
                dur
            )
        })?,
        (_, Some(data)) => {
            parse_cooldown_to_offset(&data.accrued_remap_cooldown_date).unwrap_or(0.0)
        }
        _ => 0.0,
    };

    if bonus_remaps.is_some() {
        eprintln!(
            "Bonus remaps: {} (from {})",
            bonus_remaps.unwrap(),
            source_label
        );
    }
    eprintln!("Queue from {}: {} skills", queue_label, queued_skills.len());

    let char_state = data::models::CharacterState {
        base_attributes: base_attrs,
        active_implant_ids: esi_data
            .as_ref()
            .map(|d| d.active_implant_ids.clone())
            .unwrap_or_default(),
        implant_bonus,
        queued_skills,
        bonus_remaps,
        normal_remap_available_in_secs: remap_available_secs,
    };

    let result = run_optimizer_with_state(&char_state, &skills_db)?;

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
// ── Auth Commands ────────────────────────────────────────────────────────

fn cmd_login(args: &cli::LoginArgs) -> Result<()> {
    let rt = tokio::runtime::Runtime::new().context("failed to create tokio runtime")?;
    rt.block_on(async {
        let scopes_vec: Vec<&str> = args.scopes.iter().map(|s| s.as_str()).collect();
        let account =
            auth::login(&args.client_id, &scopes_vec, args.port, args.http_callback).await?;

        // Save token to disk
        let mut store = auth::AccountsStore::load()?;
        store.upsert(account.clone());
        store.save().context("failed to save tokens")?;

        println!(
            "Authenticated as {} (ID: {}). Tokens saved.",
            account.character_name, account.character_id
        );
        Ok::<_, anyhow::Error>(())
    })
}

fn cmd_logout(args: &cli::LogoutArgs) -> Result<()> {
    let mut store = auth::AccountsStore::load()?;

    if let Some(name) = &args.name {
        // Try parsing as character ID first, then fall back to name match
        if let Ok(id) = name.parse::<u64>() {
            if store.remove(id) {
                eprintln!("Logged out character ID {}.", id);
            } else {
                anyhow::bail!("No account found for character ID {}.", id);
            }
        } else {
            let len_before = store.accounts.len();
            store.accounts.retain(|a| a.character_name != *name);
            if store.accounts.len() < len_before {
                eprintln!("Logged out '{}'.", name);
            } else {
                anyhow::bail!("No account found matching '{}'.", name);
            }
        }
    } else {
        // No name specified — remove all accounts
        let count = store.accounts.len();
        store.accounts.clear();
        eprintln!("Removed {} saved account(s).", count);
    }

    store.save().context("failed to save accounts")?;
    Ok(())
}
fn cmd_accounts() -> Result<()> {
    let store = auth::AccountsStore::load()?;

    if store.accounts.is_empty() {
        println!("No saved characters. Run 'eve-remap login' to authenticate.");
        return Ok(());
    }

    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();

    // Header
    println!("{:<24} {:<12} {}", "Character", "ID", "Token");
    println!("{}", "-".repeat(60));

    for acc in &store.accounts {
        let status = if acc.is_expired() {
            format!("expired")
        } else {
            let remaining_min = ((acc.expires_at - now) / 60.0).ceil() as u64;
            if remaining_min > 60 {
                format!("{:.0}h", remaining_min as f64 / 60.0)
            } else {
                format!("{}m", remaining_min)
            }
        };
        println!(
            "{:<24} {:<12} {}",
            acc.character_name,
            acc.character_id,
            format!("valid {} left", status)
        );
    }
    Ok(())
}
/// Resolve base attributes and implant bonuses from CLI overrides or ESI data.
/// Returns `(base_attrs, source_label, implant_bonus)`.
fn resolve_attributes(
    args: &cli::OptimizeArgs,
    esi_data: &Option<esi::CharacterData>,
    implants: &[data::models::ImplantRecord],
) -> Result<(data::models::BaseAttributes, &'static str, data::models::BaseAttributes)> {
    let default_base = data::models::BaseAttributes {
        perception: 17, memory: 17, willpower: 17, intelligence: 17, charisma: 17,
    };
    let zero_implant = data::models::BaseAttributes {
        perception: 0, memory: 0, willpower: 0, intelligence: 0, charisma: 0,
    };

    // ── Implant bonuses (resolved first — needed for ESI back-calculation) ──
    // CLI --implant-bonuses overrides everything; otherwise resolve from ESI implant IDs.
    let mut implant_bonus: data::models::BaseAttributes = zero_implant;
    if let Some(impl_str) = &args.implant_bonuses {
        implant_bonus = parser::parse_implant_bonuses(impl_str)?;
    } else if let Some(data) = esi_data {
        for impl_id in &data.active_implant_ids {
            if let Some(rec) = implants.iter().find(|r| r.type_id == *impl_id) {
                for (attr, val) in &rec.bonuses {
                    match attr {
                        data::models::Attribute::Perception => implant_bonus.perception += *val as u32,
                        data::models::Attribute::Memory => implant_bonus.memory += *val as u32,
                        data::models::Attribute::Willpower => implant_bonus.willpower += *val as u32,
                        data::models::Attribute::Intelligence => implant_bonus.intelligence += *val as u32,
                        data::models::Attribute::Charisma => implant_bonus.charisma += *val as u32,
                    }
                }
            }
        }
    }

    // ── Base attributes ────────────────────────────────────────────────
    let (base_attrs, source_label): (data::models::BaseAttributes, &'static str) =
        if let Some(attrs_str) = &args.attributes {
            (parser::parse_attributes(attrs_str)?, "CLI override")
        } else if let Some(data) = esi_data {
            if let Some(eff) = &data.base_attributes {
                // ESI /attributes/ returns effective (includes implants). Back-calculate neural interface.
                (eff.sub(&implant_bonus), "ESI /attributes/")
            } else {
                (default_base, "default (ESI attributes unavailable)")
            }
        } else {
            (default_base, "default")
        };

    Ok((base_attrs, source_label, implant_bonus))
}

/// Resolve a character lookup (name or ID string) to a StoredAccount.
fn resolve_character(query: &str) -> Result<auth::StoredAccount> {
    let store = auth::AccountsStore::load()?;

    let account = if let Ok(id) = query.parse::<u64>() {
        store.get(id)
    } else {
        // Case-insensitive name match — first partial hit wins
        store.accounts.iter().find(|a| {
            a.character_name
                .to_lowercase()
                .contains(&query.to_lowercase())
        })
    };

    let account = account.ok_or_else(|| {
        anyhow::anyhow!(
            "No saved character matching '{}'. Run 'eve-remap accounts' to see options.",
            query
        )
    })?;
    Ok(account.clone())
}

/// Convert an ISO-8601 cooldown date string to seconds from now.
/// Returns None if parsing fails or the date is in the past (i.e., already available).
fn parse_cooldown_to_offset(cooldown_date: &Option<String>) -> Option<f64> {
    let s = cooldown_date.as_ref()?;
    // Parse RFC3339 — try chrono first, fall back to manual parsing for common formats.
    let dt = chrono::DateTime::parse_from_rfc3339(s).ok();
    let secs = match dt {
        Some(d) => d.signed_duration_since(chrono::Utc::now()).num_seconds() as f64,
        None => return None,
    };
    if secs <= 0.0 {
        return Some(0.0); // Already available
    }
    Some(secs)
}

/// Convert ESI skill queue entries into queue-file text and parse via the shared path.
fn build_queued_from_esi(
    data: &esi::CharacterData,
    skills_db: &[data::models::SkillRecord],
    effective_attrs: &data::models::EffectiveAttributes,
) -> Result<Vec<data::models::QueuedSkill>> {
    use std::collections::HashMap;

    let id_map: HashMap<u32, &data::models::SkillRecord> =
        skills_db.iter().map(|r| (r.id, r)).collect();

    // Sort by queue_position to ensure order
    let mut queue = data.skill_queue.clone();
    queue.sort_by_key(|e| e.queue_position);

    // Render each entry as "Skill Name <level>@<sp_trained>" or "Skill Name <level>".
    let mut lines = Vec::with_capacity(queue.len());
    for entry in &queue {
        let target_level = entry.finished_level as u8;
        if !(1..=5).contains(&target_level) {
            continue;
        }

        let record = id_map.get(&(entry.skill_id as u32)).ok_or_else(|| {
            anyhow::anyhow!(
                "Skill ID {} from ESI not found in local database",
                entry.skill_id
            )
        })?;

        // Only include SP progress if training has actually started.
        if entry.training_start_sp > entry.level_start_sp {
            lines.push(format!("{} {}@{}", record.name, target_level as i32, entry.training_start_sp as i64));
        } else {
            lines.push(format!("{} {}", record.name, target_level as i32));
        }
    }

    let content = lines.join("\n");
    parser::parse_queue(&content, skills_db, effective_attrs, "ESI /skillqueue/")
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

/// Run the optimizer engine against a character state and return results.
fn run_optimizer_with_state(
    char_state: &data::models::CharacterState,
    skills_db: &[data::models::SkillRecord],
) -> Result<data::models::OptimizationResult> {
    let result = optimizer::optimize(char_state, skills_db);
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
    let len = s.len();
    for (i, &b) in s.as_bytes().iter().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            result.push(',');
        }
        result.push(b as char);
    }
    result
}

fn print_table_output(result: &data::models::OptimizationResult) {
    println!("\n{}", "=".repeat(72));
    println!("REMAP OPTIMIZATION PLAN");
    println!("{}\n", "-".repeat(72));

    // Fixed display order matching table header.
    const DISPLAY_ORDER: [data::models::Attribute; 5] = [
        data::models::Attribute::Perception,
        data::models::Attribute::Memory,
        data::models::Attribute::Willpower,
        data::models::Attribute::Intelligence,
        data::models::Attribute::Charisma,
    ];

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
        let pri_vals: Vec<f64> = DISPLAY_ORDER
            .iter()
            .map(|a| epoch.sp_summary.primary.get(a).copied().unwrap_or(0.0))
            .collect();
        let sec_vals: Vec<f64> = DISPLAY_ORDER
            .iter()
            .map(|a| epoch.sp_summary.secondary.get(a).copied().unwrap_or(0.0))
            .collect();

        if pri_vals.iter().sum::<f64>() > 0.0 || sec_vals.iter().sum::<f64>() > 0.0 {
            let fmt = |v: f64| -> String {
                if v == 0.0 {
                    "-".into()
                } else {
                    format_sp(v)
                }
            };
            println!(
                "  {:>4} {:>7} {:>7} {:>7} {:>7} {:>7}",
                "", "PER", "MEM", "WIL", "INT", "CHA"
            );
            println!(
                "  Pri  {:>7} {:>7} {:>7} {:>7} {:>7}",
                fmt(pri_vals[0]),
                fmt(pri_vals[1]),
                fmt(pri_vals[2]),
                fmt(pri_vals[3]),
                fmt(pri_vals[4])
            );
            println!(
                "  Sec  {:>7} {:>7} {:>7} {:>7} {:>7}",
                fmt(sec_vals[0]),
                fmt(sec_vals[1]),
                fmt(sec_vals[2]),
                fmt(sec_vals[3]),
                fmt(sec_vals[4])
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
    let total_sp: f64 = result
        .epochs
        .iter()
        .flat_map(|e| e.sp_summary.primary.values())
        .sum();

    println!("{}", "-".repeat(72));
    println!("Total training time: {:.1} days", total_days);
    println!("Total SP in queue: {}", format_number(total_sp));
    println!("Epochs: {}", result.epochs.len());
    if result.baseline_wall_clock_seconds > 0.0 {
        let baseline_days = result.baseline_wall_clock_seconds / 86_400.0;
        println!("Baseline (no remaps): {:.1} days", baseline_days);
        if result.epochs.len() <= 1 {
            println!("  (Remapping did not improve training time over current attributes.)");
        }
    }
    println!();
    println!("Note: This plan uses a greedy heuristic and is not guaranteed optimal.");
    println!("      Results may vary by a few percent from the true minimum.");
    println!();
}

fn print_json_output(result: &data::models::OptimizationResult) {
    let json = serde_json::to_string_pretty(result).expect("Failed to serialize result");
    println!("{}", json);
}

/// Write the optimized skill queue order to a file in "Skill Name <level>" format.
fn write_queue_file(path: &str, result: &data::models::OptimizationResult) -> Result<()> {
    let mut lines = vec![String::from(
        "# Optimized by eve-remap — skill order reordered for attribute locality",
    )];
    let mut skill_count = 0usize;
    for epoch in &result.epochs {
        for (_skill_id, skill_name, target_level, _train_secs) in &epoch.completed_skills {
            lines.push(format!("{} {}", skill_name, target_level));
            skill_count += 1;
        }
    }

    use std::io::{self, Write};
    let content = lines.join("\n") + "\n";
    if path == "-" {
        io::stdout()
            .write_all(content.as_bytes())
            .context("Failed to write to stdout")?;
        io::stdout().flush()?;
        eprintln!(
            "[+] Optimized queue written to stdout ({} skills)",
            skill_count
        );
    } else {
        let mut file = std::fs::File::create(path)
            .context(format!("Failed to create output queue file '{}'", path))?;
        file.write_all(content.as_bytes())
            .context("Failed to write output queue file")?;
        eprintln!(
            "[+] Optimized queue written to '{}' ({} skills)",
            path, skill_count
        );
    }
    Ok(())
}
