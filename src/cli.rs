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
}

#[derive(clap::Args)]
pub struct OptimizeArgs {
    /// Path to a file listing target skills, one per line.
    /// Formats: "SkillName \<level>", "SkillName \<level>@\<duration>" (e.g., @3d12h), or "SkillName \<level>@\<sp_trained>" (e.g., @12000, @1,000,000). SP is cumulative from blank. Use "-" for stdin.
    #[arg(long, short = 'q')]
    pub queue: String,
    /// Base remapped attribute values (excluding implants).
    /// Format: PER:MEM:WIL:INT:CHA (e.g., 27:20:17:17:18). Defaults to 17:17:17:17:17.
    #[arg(long, default_value = "17:17:17:17:17")]
    pub attributes: String,

    /// Output results as JSON instead of human-readable table.
    #[arg(long)]
    pub json: bool,

    /// Number of bonus neural interface remaps available (in addition to timed cooldown).
    #[arg(long)]
    pub bonus_remaps: Option<u32>,

    /// When the normal neural interface remap becomes available.
    /// '0d' means available now; '30d' means available 30 days from start of training.
    /// Defaults to '0d'. Useful when your last remap was recent and you're waiting out cooldown.
    #[arg(long, default_value = "0d")]
    pub remap_available: String,

    /// Implant attribute bonuses that persist across remaps.
    /// Format: PER:MEM:WIL:INT:CHA (e.g., 0:1:2:0:1).
    /// If omitted, defaults to zero — meaning --attributes are treated as raw base values.
    #[arg(long, default_value = "0:0:0:0:0")]
    pub implant_bonuses: String,

    /// Write the optimized skill training order to a file in the same format
    /// as the input queue (one "Skill Name <level>" per line). Use "-" for stdout.
    #[arg(long)]
    pub queue_out: Option<String>,
}
