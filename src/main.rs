mod calculator;
mod cli;
mod auth;
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
    if args.browser {
        return cmd_login_browser();
    }
    if args.sso {
        eprintln!("Note: PKCE server-based SSO (--sso) requires port forwarding from WSL.");
        eprintln!("Use '--browser' instead for a simpler flow that works cross-platform.\n");
        return tokio::runtime::Runtime::new()?.block_on(async {
            let entry = auth::run_pkce_flow().await?;
            auth::save_account(entry.clone())?;
            println!("\nAuthenticated as {} (ID: {}).", entry.character_name, entry.character_id);
            Ok::<_, anyhow::Error>(())
        });
    }

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

    match auth::decode_jwt_token(&token) {
        Some(claims) => {
            let entry = auth::StoredAccountEntry {
                character_id: claims.owner_character_id,
                character_name: claims.character_name.clone(),
                access_token: token.clone(),
                refresh_token: String::new(),
                expires_at: claims.expires_at,
                scopes: claims.scopes,
                created_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            };

            auth::save_account(entry)?;
            println!("\nToken saved for {} (ID: {}).", claims.character_name, claims.owner_character_id);
        }
        None => {
            data::esi::save_tokens(&token)?;
            println!("\nToken saved (could not decode JWT — use --browser next time for full info).");
        }
    }

    println!("Run 'eve-remap optimize' to start optimizing your skill queue.");
    Ok(())
}

// ── Browser login (implicit grant — no port forwarding) ────────────────────

fn cmd_login_browser() -> Result<()> {
    let client_id = std::env::var("ESI_CLIENT_ID").context(
        "ESI_CLIENT_ID not set.\n\
         Register an app at https://developers.eveonline.com/applications/\n\
         Then export ESI_CLIENT_ID=<your-client-id>"
    )?;

    let redirect_uri = "https://127.0.0.1/callback";
    let state = rand::random::<u64>().to_string();
    let scopes = "esi-skills.read_skills.v1 esi-skills.read_skillqueue.v1 esi-clones.read_implants.v1";

    let auth_url = format!(
        "https://login.eveonline.com/v2/oauth/authorize?\
         response_type=token&\
         client_id={}&\
         redirect_uri={}&\
         state={}&\
         scope={}",
        urlencoding::encode(&client_id),
        urlencoding::encode(redirect_uri),
        urlencoding::encode(&state),
        urlencoding::encode(scopes),
    );

    println!("Opening browser for EVE SSO authorization...");
    println!("If it doesn't open, visit:\n{}\n", auth_url);

    let _ = open_browser(&auth_url);

    print!("Paste the redirected URL from your browser: ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let callback_url = input.trim().to_string();

    if callback_url.is_empty() {
        anyhow::bail!("No URL provided. Aborting.");
    }

    let token = parse_implicit_grant_callback(&callback_url, &state)?;

    match auth::decode_jwt_token(&token) {
        Some(claims) => {
            let entry = auth::StoredAccountEntry {
                character_id: claims.owner_character_id,
                character_name: claims.character_name.clone(),
                access_token: token,
                refresh_token: String::new(),
                expires_at: claims.expires_at,
                scopes: claims.scopes,
                created_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            };

            auth::save_account(entry)?;
            println!("\nToken saved for {} (ID: {}).", claims.character_name, claims.owner_character_id);
        }
        None => {
            data::esi::save_tokens(&token)?;
            println!("\nToken saved (could not decode JWT).");
        }
    }

    println!("Run 'eve-remap optimize' to start optimizing your skill queue.");
    Ok(())
}

fn open_browser(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    std::process::Command::new("xdg-open").arg(url).spawn()?;
    #[cfg(target_os = "macos")]
    std::process::Command::new("open").arg(url).spawn()?;
    #[cfg(target_os = "windows")]
    std::process::Command::new("cmd")
        .args(["/C", "start"])
        .arg(url)
        .spawn()?;
    Ok(())
}

/// Parse the callback URL from an implicit grant flow and extract the access token.
fn parse_implicit_grant_callback(callback_url: &str, expected_state: &str) -> Result<String> {
    let fragment_start = callback_url.find('#')
        .ok_or_else(|| anyhow::anyhow!("No fragment (#) in callback URL"))?;

    for param in callback_url[fragment_start + 1..].split('&') {
        if param.starts_with("error=") {
            return Err(anyhow::anyhow!(
                "Authorization error: {}",
                param.trim_start_matches("error=")
            ));
        }
    }

    let has_valid_state = callback_url.contains(&format!("state={}", expected_state));
    if !has_valid_state {
        return Err(anyhow::anyhow!(
            "State mismatch — this might be a CSRF attempt or the wrong auth URL.\n\
             Make sure you used the URL printed above."
        ));
    }

    for param in callback_url[fragment_start + 1..].split('&') {
        if let Some((key, value)) = param.split_once('=') {
            if key == "access_token" && !value.is_empty() {
                return Ok(value.to_string());
            }
        }
    }

    Err(anyhow::anyhow!(
        "No access_token found in callback URL fragment.\n\
         The browser may have shown an error page."
    ))
}

// ── Logout ───────────────────────────────────────────────────────────────

fn cmd_logout() -> Result<()> {
    let accounts = auth::load_accounts()?;
    if accounts.is_empty() {
        let path = data::esi::token_path();
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        println!("No stored accounts found (already logged out).");
    } else {
        for acct in &accounts {
            auth::remove_account(acct.character_id)?;
        }
        let path = data::esi::token_path();
        if path.exists() {
            std::fs::remove_file(&path).ok();
        }
        println!("Logged out {} account(s).", accounts.len());
    }
    Ok(())
}

// ── Accounts ─────────────────────────────────────────────────────────────

fn cmd_accounts(args: &cli::AccountsArgs) -> Result<()> {
    let accounts = auth::load_accounts()?;
    let chars = auth::list_characters()?;

    if chars.is_empty() {
        match data::esi::load_saved_token() {
            Some(_) => {
                println!("Status: authenticated (legacy token — run 'login' again to migrate)");
            }
            None => {
                println!("No authenticated accounts.");
                println!("Run 'eve-remap login --sso' or 'eve-remap login -t TOKEN' to authenticate.");
            }
        }
        return Ok(());
    }

    println!("Authenticated accounts:");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    for acct in &accounts {
        let expired = acct.expires_at <= now;
        let status = if expired { "[expired]" } else { "[active]" };
        println!(
            "  {} - {} ({}) {}",
            status,
            acct.character_name,
            acct.character_id,
            if args.verbose {
                format!("expires in {} min", ((acct.expires_at as i64 - now as i64).max(0)) / 60)
            } else {
                String::new()
            }
        );
    }
    Ok(())
}

// ── Download ─────────────────────────────────────────────────────────────

async fn cmd_download(_args: &cli::DownloadArgs) -> Result<()> {
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

    let result = if let Some(queue_path) = &args.queue {
        // Parse queue file locally — no ESI needed.
        run_optimizer_from_queue_file(
            &args.attributes,
            &args.implant_bonuses,
            args.bonus_remaps,
            queue_path,
            &skills_db,
            &implants,
        )?
    } else {
        match try_fetch_and_optimize(&skills_db, &implants).await {
            Ok(r) => r,
            Err(e) => {
                eprintln!("\nNote: could not fetch character data:\n  {}", e);
                println!("Use '--queue FILE' with a list of 'Skill Name <level>' lines instead.\n");
                return Ok(());
            }
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
    skills_db: &[data::models::SkillRecord],
    implants: &[data::models::ImplantRecord],
) -> anyhow::Result<data::models::OptimizationResult> {
    let (_token, char_id) = auth::find_valid_token()?.ok_or_else(|| anyhow::anyhow!(
        "No authenticated accounts found.\n\
         Run 'eve-remap login --browser' or pass --queue."
    ))?;

    let client = data::esi::EsIClient::from_env()?;
    let char_state = client.fetch_character_state(char_id, skills_db, implants).await?;
    run_optimizer_with_state(&char_state, skills_db, implants)
}

/// Parse a queue file and optimize directly without ESI.
fn run_optimizer_from_queue_file(
    attrs_str: &str,
    implant_bonuses_str: &str,
    bonus_remaps: Option<u32>,
    path: &str,
    skills_db: &[data::models::SkillRecord],
    implants: &[data::models::ImplantRecord],
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

    println!(
        "Queue file '{}' — {} skills, attributes INT={} CHA={} PER={} MEM={} WIL={}, bonus remaps={}",
        path,
        queued_skills.len(),
        base_attrs.intelligence as u32,
        base_attrs.charisma as u32,
        base_attrs.perception as u32,
        base_attrs.memory as u32,
        base_attrs.willpower as u32,
        bonus_remaps.map_or("not set".into(), |n| n.to_string()),
    );

    let char_state = data::models::CharacterState {
        base_attributes: base_attrs,
        active_implant_ids: vec![],
        implant_bonus,
        queued_skills,
        effective_attributes: EffectiveAttributes::from(base_attrs),
        bonus_remaps,
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
    if result.epochs.is_empty() {
        println!("No skills to optimize — queue is empty or all at max level.");
        return;
    }

    println!();
    println!(
        "═ Repaired Optimization Plan ({} total time)",
        calculator::format_duration(result.total_wall_clock_seconds)
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
            "│ Start: {} from now",
            calculator::format_duration(epoch.start_offset_secs)
        );
        println!(
            "│ Effective:   INT={} CHA={} PER={} MEM={} WIL={}",
            epoch.effective_attributes.intelligence as u32,
            epoch.effective_attributes.charisma as u32,
            epoch.effective_attributes.perception as u32,
            epoch.effective_attributes.memory as u32,
            epoch.effective_attributes.willpower as u32,
        );
        if !epoch.completed_skills.is_empty() {
            println!("│ Skills completing this epoch:");
            for (id, name, secs) in &epoch.completed_skills {
                let label = calculator::format_duration(*secs);
                println!("│   • {} [{}] — {}", name, id, label);
            }
        } else {
            println!("│ No skills completed in this epoch.");
        }

        println!(
            "│ Projected finish: {} from now",
            calculator::format_duration(epoch.projected_finish_secs)
        );
        println!("└──────────────────\n");
    }

    println!(
        "Total training time: {}",
        calculator::format_duration(result.total_wall_clock_seconds)
    );
}

fn print_json_output(result: &data::models::OptimizationResult) {
    let mut epochs = Vec::new();
    for epoch in &result.epochs {
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
            .map(|(id, name, secs)| serde_json::json!({ "skill_id": id, "name": name, "training_seconds": *secs }))
            .collect();

        epochs.push(serde_json::json!({
            "start_offset_days": epoch.start_offset_secs / 86_400.0,
            "base_attributes": {
                "intelligence": epoch.attributes.intelligence as u32,
                "charisma": epoch.attributes.charisma as u32,
                "perception": epoch.attributes.perception as u32,
                "memory": epoch.attributes.memory as u32,
                "willpower": epoch.attributes.willpower as u32,
            },
            "effective_attributes": attrs_map,
            "completed_skills": completed,
            "projected_finish_days": epoch.projected_finish_secs / 86_400.0,
        }));
    }

    let output = serde_json::json!({
        "total_epochs": result.epochs.len(),
        "total_days": result.total_wall_clock_seconds / 86_400.0,
        "total_wall_clock_seconds": result.total_wall_clock_seconds,
        "epochs": epochs,
    });

    println!("{}", serde_json::to_string_pretty(&output).unwrap());
}