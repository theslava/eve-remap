use clap::{Parser, Subcommand};

/// eve-remap — EVE Online skill queue remap optimizer
#[derive(Parser)]
#[command(name = "eve-remap", version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Optimize character's skill training queue across remap epochs.
    Optimize(OptimizeArgs),
    /// Authenticate with EVE Online SSO and save tokens locally.
    Login(LoginArgs),
    /// Remove a previously saved account by character name or ID.
    Logout(LogoutArgs),
    /// List all saved characters and their token status.
    Accounts,
}

#[derive(clap::Args)]
pub struct OptimizeArgs {
    /// Select a saved character by name or ID.
    /// When specified, attempts to load attributes, bonus remaps, and cooldown from stored tokens.
    /// Manual flags (--attributes, --bonus-remaps, --remap-available) still override.
    #[arg(long)]
    pub character: Option<String>,

    /// Path to a file listing target skills, one per line.
    /// Formats: "SkillName \<level>", "SkillName \<level>@\<duration>" (e.g., @3d12h), or "SkillName \<level>@\<sp_trained>". Use "-" for stdin.
    /// Omit when using --character to auto-fetch from ESI skill queue.
    #[arg(long, short = 'q')]
    pub queue: Option<String>,

    /// Base remapped attribute values (excluding implants).
    /// Format: PER:MEM:WIL:INT:CHA (e.g., 27:20:17:17:18). Defaults to 17:17:17:17:17.
    /// Overrides fetched value from --character if set explicitly.
    #[arg(long)]
    pub attributes: Option<String>,

    /// Output results as JSON instead of human-readable table.
    #[arg(long)]
    pub json: bool,

    /// Number of bonus neural interface remaps available (in addition to timed cooldown).
    /// Overrides fetched value from --character if set.
    #[arg(long)]
    pub bonus_remaps: Option<u32>,

    /// When the normal neural interface remap becomes available.
    /// '0d' means available now; '30d' means available 30 days from start of training.
    /// Overrides fetched value from --character if set.
    #[arg(long)]
    pub remap_available: Option<String>,

    /// Implant attribute bonuses that persist across remaps.
    /// Format: PER:MEM:WIL:INT:CHA (e.g., 0:1:2:0:1).
    /// If omitted with --character, implants are auto-fetched and resolved locally.
    /// Without --character, defaults to zero.
    #[arg(long)]
    pub implant_bonuses: Option<String>,

    /// Write the optimized skill training order to a file in the same format
    /// as the input queue (one "Skill Name <level>" per line). Use "-" for stdout.
    #[arg(long)]
    pub queue_out: Option<String>,
}
#[derive(clap::Args)]
pub struct LoginArgs {
    /// EVE Online SSO client ID (required).
    #[arg(long, short = 'c')]
    pub client_id: String,

    /// Space-separated list of ESI scopes to request.
    /// Defaults to the minimum needed for optimizer data: skills, skillqueue, implants.
    #[arg(
        long,
        default_value = "esi-skills.read_skills.v1 esi-skills.read_skillqueue.v1 esi-clones.read_implants.v1"
    )]
    pub scopes: Vec<String>,

    /// Port for the local OAuth callback listener.
    /// If omitted, an available ephemeral port will be selected automatically.
    #[arg(long)]
    pub port: Option<u16>,

    /// Use plain HTTP for the OAuth callback instead of HTTPS.
    /// Useful when browsers block self-signed cert redirects (common with EVE SSO).
    /// Requires http://localhost:PORT/callback registered on dev portal.
    #[arg(long)]
    pub http_callback: bool,
}

#[derive(clap::Args)]
pub struct LogoutArgs {
    /// Character name or character ID to log out. Omit to remove all accounts.
    #[arg(long, short = 'n')]
    pub name: Option<String>,
}
