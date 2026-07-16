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
    /// Authenticate with EVE SSO (set token for API access).
    Login(LoginArgs),

    /// Remove stored authentication tokens.
    Logout,

    /// List authenticated characters or show current account info.
    Accounts(AccountsArgs),

    /// Download and parse latest SDE data into assets/.
    Download(DownloadArgs),

    /// Verify that local asset files are present and valid.
    Verify,

    /// Optimize character's skill training queue across remap epochs.
    Optimize(OptimizeArgs),
}

#[derive(clap::Args)]
pub struct LoginArgs {
    /// Bearer token from EVE SSO (paste full JWT string).
    /// If omitted and --sso/--browser are not set, will prompt interactively.
    #[arg(short, long, env = "EVE_REMAP_TOKEN")]
    pub token: Option<String>,

    /// Use interactive browser-based SSO/PKCE flow instead of pasting a token.
    #[arg(long)]
    pub sso: bool,

    /// Open browser for authorization, then paste the callback URL back.
    /// Works cross-platform (WSL, macOS, Linux) without port forwarding.
    #[arg(long)]
    pub browser: bool,
}

#[derive(clap::Args)]
pub struct AccountsArgs {
    /// Show token details including expiry time.
    #[arg(short, long)]
    pub verbose: bool,
}

#[derive(clap::Args)]
pub struct DownloadArgs {
    /// Output directory for parsed assets (default: assets/ in repo root).
    #[arg(short, long)]
    pub dir: Option<String>,
}

#[derive(clap::Args)]
pub struct OptimizeArgs {
    /// Path to a file listing target skills, one per line as "Skill Name <level>".
    /// Example lines: "Gunnery 3", "Navigation 5"
    #[arg(long, short = 'q')]
    pub queue: Option<String>,
    /// Effective attribute values including implants for offline mode.
    /// Format: PER:MEM:WIL:INT:CHA (e.g., 22:19:17:17:17). Defaults to 17:17:17:17:17.
    #[arg(long, default_value = "17:17:17:17:17")]
    pub attributes: String,

    /// Output results as JSON instead of human-readable table.
    #[arg(long)]
    pub json: bool,

    /// Number of bonus neural interface remaps available (in addition to timed cooldown).
    #[arg(long)]
    pub bonus_remaps: Option<u32>,

    /// Implant attribute bonuses that persist across remaps.
    /// Format: PER:MEM:WIL:INT:CHA (e.g., 0:1:2:0:1).
    /// If omitted, defaults to zero — meaning --attributes are treated as raw base values.
    #[arg(long, default_value = "0:0:0:0:0")]
    pub implant_bonuses: String,
}
