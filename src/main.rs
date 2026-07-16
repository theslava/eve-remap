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

fn cmd_login_browser() -> Result<()> {
    let client_id = std::env::var("ESI_CLIENT_ID").context(
        "ESI_CLIENT_ID not set.\n\
         Register an app at https://developers.eveonline.com/applications/\n\
         Then export ESI_CLIENT_ID=<your-client-id>"
    )?;

    // Use a dummy redirect that captures the token in the URL fragment.
    let redirect_uri = "https://127.0.0.1/callback";
    let state = rand::random::<u64>().to_string();
    let scopes = "esi-skills.read_skills.v1 esi-skills.read_skillqueue.v1";

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

    // Extract access_token and expires_in from the URL fragment.
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
/// The fragment looks like: #access_token=...&expires_in=3600&state=...
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

    // Verify state to prevent CSRF.
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
        // Also clean legacy tokens file.
        let path = data::esi::token_path();
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        println!("No stored accounts found (already logged out).");
    } else {
        for acct in &accounts {
            auth::remove_account(acct.character_id)?;
        }
        // Clean legacy tokens file too.
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
        // Check legacy token storage.
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
        println!("  {} - {} ({}) {}", status, acct.character_name, acct.character_id, {
            if args.verbose {
                format!("expires in {} min", ((acct.expires_at as i64 - now as i64).max(0)) / 60)
            } else {
                String::new()
            }
        });
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
    char_id_flag: &Option<u64>,
    skills_db: &[data::models::SkillRecord],
    implants: &[data::models::ImplantRecord],
) -> anyhow::Result<data::models::OptimizationResult> {
    // Resolve character ID: CLI flag > stored account > error.
    let char_id = match char_id_flag {
        Some(id) => *id,
        None => {
            match auth::find_valid_token() {
                Ok(Some((_token, id))) => id,
                Ok(None) => return Err(anyhow::anyhow!(
                    "No authenticated accounts found.\n\
                     Run 'eve-remap login --sso' or pass --character-id."
                )),
                Err(e) => return Err(anyhow::anyhow!("Failed to load accounts: {}", e)),
            }
        }
    };

    let client = data::esi::EsIClient::from_env()?;
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
