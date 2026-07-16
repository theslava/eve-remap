mod calculator;
mod cli;
mod data;
mod optimizer;

use anyhow::{Context, Result};
use clap::Parser;
use std::collections::HashMap;
use std::io::{self, Write};

fn main() -> Result<()> {
    let cli = cli::Cli::parse();

    match &cli.command {
        cli::Commands::Login(args) => cmd_login(args),
        cli::Commands::Logout => cmd_logout(),
        cli::Commands::Accounts(args) => cmd_accounts(args),
        cli::Commands::Download(args) => tokio::runtime::Runtime::new()?.block_on(cmd_download(args)),
        cli::Commands::Verify => cmd_verify(),
        cli::Commands::Optimize(args) => tokio::runtime::Runtime::new()?.block_on(cmd_optimize(args)),
    }
}

// ── Login ────────────────────────────────────────────────────────────────

fn cmd_login(args: &cli::LoginArgs) -> Result<()> {
    let token = if let Some(t) = &args.token {
        t.clone()
    } else {
        print!("Enter EVE SSO bearer token: ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let trimmed = input.trim().to_string();
        if trimmed.is_empty() {
            anyhow::bail!("No token provided. Aborting.");
        }
        trimmed
    };

    data::esi::save_tokens(&token)?;
    println!("\nToken saved successfully.");
    println!("Run 'eve-remap optimize' to start optimizing your skill queue.");
    Ok(())
}

// ── Logout ───────────────────────────────────────────────────────────────

fn cmd_logout() -> Result<()> {
    let path = data::esi::token_path();
    if path.exists() {
        std::fs::remove_file(&path).context("Failed to remove tokens file")?;
        println!("Tokens removed. You are now logged out.");
    } else {
        println!("No stored tokens found (already logged out).");
    }
    Ok(())
}

// ── Accounts ─────────────────────────────────────────────────────────────

fn cmd_accounts(_args: &cli::AccountsArgs) -> Result<()> {
    match data::esi::load_saved_token() {
        Some(_) => {
            println!("Status: authenticated");
            println!("Run 'eve-remap optimize' with --character-id to select a character.");
        }
        None => {
            println!("Status: not authenticated");
            println!("Run 'eve-remap login' first, or set EVE_REMAP_TOKEN environment variable.");
        }
    }
    Ok(())
}

// ── Download ─────────────────────────────────────────────────────────────

async fn cmd_download(_args: &cli::DownloadArgs) -> Result<()> {
    // TODO: implement SDE download and parsing pipeline
    println!("SDE download not yet implemented. Assets already in repo.");
    Ok(())
}

// ── Verify ───────────────────────────────────────────────────────────────

fn cmd_verify() -> Result<()> {
    let skills = data::load_skills()?;
    let implants = data::load_implants()?;

    println!("Assets verified:");
    println!("  Skills: {} entries", skills.len());
    println!("  Implants: {} entries with attribute bonuses", implants.len());

    if let Some(skill) = skills.first() {
        println!(
            "  Sample: {} (primary={}, secondary={}, tc={})",
            skill.name,
            skill.primary_attribute,
            skill.secondary_attribute,
            skill.skill_time_constant
        );
    }
    Ok(())
}

// ── Optimize ─────────────────────────────────────────────────────────────

async fn cmd_optimize(args: &cli::OptimizeArgs) -> Result<()> {
    let skills_db = data::load_skills().context("Failed to load skill database")?;
    let implants = data::load_implants().context("Failed to load implant database")?;

    // Try ESI client first for live data, fall back to demo mode.
    let result = match try_fetch_and_optimize(&args.character_id, &skills_db, &implants).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Note: could not fetch character data ({e}). Running in demo mode.");
            run_demo_optimizer(&skills_db, &implants)?
        }
    };

    if args.json {
        print_json_output(&result);
    } else {
        print_table_output(&result);
    }

    Ok(())
}

/// Attempt to fetch real character state via ESI and optimize.
async fn try_fetch_and_optimize(
    _char_id: &Option<u64>,
    skills_db: &[data::models::SkillRecord],
    implants: &[data::models::ImplantRecord],
) -> anyhow::Result<data::models::OptimizationResult> {
    let client = data::esi::EsIClient::from_env()?;

    // For now we need a character ID — either from flag or from token introspection.
    // Without it, we can't proceed with live data.
    // TODO: extract owner_character_id from JWT payload when char_id is None.
    let char_id = 0u64; // placeholder until JWT parsing is wired
    if char_id == 0 {
        return Err(anyhow::anyhow!(
            "No character ID available. Pass --character-id or set up JWT introspection."
        ));
    }

    let char_state = client.fetch_character_state(char_id, skills_db, implants).await?;
    run_optimizer_with_state(&char_state, skills_db, implants)
}

/// Run optimizer with a sample/demo character for verification without ESI access.
fn run_demo_optimizer(
    skills_db: &[data::models::SkillRecord],
    implants: &[data::models::ImplantRecord],
) -> Result<data::models::OptimizationResult> {
    use data::models::{BaseAttributes, EffectiveAttributes, QueuedSkill};

    // Build a realistic demo queue using actual SDE skill IDs.
    // Find some representative INT and MEM skills from the database.
    let int_skill = skills_db
        .iter()
        .find(|s| s.primary_attribute == data::models::Attribute::Intelligence)
        .ok_or_else(|| anyhow::anyhow!("No intelligence skill found in assets"))?;

    let mem_skill = skills_db
        .iter()
        .find(|s| s.primary_attribute == data::models::Attribute::Memory)
        .ok_or_else(|| anyhow::anyhow!("No memory skill found in assets"))?;

    println!(
        "Demo mode — optimizing {} (INT/TC={}) and {} (MEM/TC={})",
        int_skill.name,
        int_skill.skill_time_constant,
        mem_skill.name,
        mem_skill.skill_time_constant
    );

    let base_attrs = BaseAttributes {
        intelligence: 12.0,
        charisma: 3.0,
        perception: 4.0,
        memory: 4.0,
        willpower: 2.0,
    };

    let char_state = data::models::CharacterState {
        base_attributes: base_attrs,
        active_implant_ids: vec![],
        queued_skills: vec![
            QueuedSkill {
                id: int_skill.id,
                level: 1,
                sp: 4_000_000,
                duration: 86400 * 7, // ~1 week
                remaining_sec: 86400 * 7,
                is_active: true,
            },
            QueuedSkill {
                id: mem_skill.id,
                level: 1,
                sp: 4_000_000,
                duration: 86400 * 14, // ~2 weeks
                remaining_sec: 86400 * 14,
                is_active: false,
            },
        ],
        effective_attributes: EffectiveAttributes::from(base_attrs),
    };

    run_optimizer_with_state(&char_state, skills_db, implants)
}

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
    if result.epochs.is_empty() {
        println!("No skills to optimize — queue is empty or all at max level.");
        return;
    }

    println!();
    println!(
        "═ Repaired Optimization Plan ({:.1} total days)",
        result.total_days
    );
    println!();

    for (i, epoch) in result.epochs.iter().enumerate() {
        let label = if i == 0 {
            "Current"
        } else {
            format!("Epoch {}", i).leak() // safe: only printed immediately
        };

        println!("┌─ {} ──────────────", label);
        println!(
            "│ Start: {:.1} days from now",
            epoch.start_offset_days
        );
        println!(
            "│ Attributes: INT={} CHA={} PER={} MEM={} WIL={}",
            epoch.attributes.intelligence as u32,
            epoch.attributes.charisma as u32,
            epoch.attributes.perception as u32,
            epoch.attributes.memory as u32,
            epoch.attributes.willpower as u32,
        );

        if !epoch.completed_skills.is_empty() {
            println!("│ Skills completing this epoch:");
            for (id, name) in &epoch.completed_skills {
                println!("│   • {} [{}]", name, id);
            }
        } else {
            println!("│ No skills completed in this epoch.");
        }

        println!(
            "│ Projected finish: {:.1} days from now",
            epoch.projected_finish_days
        );
        println!("└──────────────────\n");
    }

    println!(
        "Total training time: {:.1} days ({:.0} years)",
        result.total_days,
        result.total_days / 365.0
    );
}

fn print_json_output(result: &data::models::OptimizationResult) {
    // Build a serializable structure.
    let mut epochs = Vec::new();
    for epoch in &result.epochs {
        use data::models::Attribute;

        let attrs_map: HashMap<String, f64> = HashMap::from([
            ("intelligence".to_string(), epoch.effective_attributes.intelligence),
            ("charisma".to_string(), epoch.effective_attributes.charisma),
            ("perception".to_string(), epoch.effective_attributes.perception),
            ("memory".to_string(), epoch.effective_attributes.memory),
            ("willpower".to_string(), epoch.effective_attributes.willpower),
        ]);

        let completed: Vec<_> = epoch
            .completed_skills
            .iter()
            .map(|(id, name)| serde_json::json!({ "skill_id": id, "name": name }))
            .collect();

        epochs.push(serde_json::json!({
            "start_offset_days": epoch.start_offset_days,
            "base_attributes": {
                "intelligence": epoch.attributes.intelligence as u32,
                "charisma": epoch.attributes.charisma as u32,
                "perception": epoch.attributes.perception as u32,
                "memory": epoch.attributes.memory as u32,
                "willpower": epoch.attributes.willpower as u32,
            },
            "effective_attributes": attrs_map,
            "completed_skills": completed,
            "projected_finish_days": epoch.projected_finish_days,
        }));
    }

    let output = serde_json::json!({
        "total_epochs": result.epochs.len(),
        "total_days": result.total_days,
        "total_wall_clock_seconds": result.total_wall_clock_seconds,
        "epochs": epochs,
    });

    println!("{}", serde_json::to_string_pretty(&output).unwrap());
}
