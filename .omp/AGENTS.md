# Agent Guidelines — Eve Remap

## Project Goal

Optimize **EVE Online** character attribute remaps across multiple epochs to minimize total wall-clock time for training target skills. Users provide their skill queue via `--queue FILE` along with base attributes and bonus remap count. The optimizer outputs a phased plan: what allocation to set at each epoch, which skills complete, and projected finish dates.

Offline-only CLI tool. No authentication, no ESI integration, no network dependencies. All data comes from pre-parsed SDE assets shipped in the repo.

## Current Phase

Post-cleanup. Auth, ESI, and SDE download removed. Core optimizer and CLI working. Remaining work: stdin/stdout pipe mode, export modified queue for EVE import, colored output, save/load plans, eventually re-add auth & live data fetch.

## Tech Stack

|Layer|Choice|
|---|---|
|Language|Rust 2021 edition (Rust 1.75 compatible)|
|CLI|clap derive subcommands|
|Data|Flat JSON assets (`assets/skills.json`, `assets/implants.json`)|
|Testing|`cargo test` — unit tests across calculator, optimizer|

Zero system dependencies. Four crates: `serde`, `serde_json`, `clap`, `anyhow`. No async runtime, no HTTP client, no OpenSSL.

## Build Commands

```bash
cargo build          # debug build
cargo run --release  # release binary
cargo test           # run all tests
```

## Test Commands

```bash
cargo test                    # full suite (calculator, optimizer, parser)
cargo test calculator::tests  # only calculator module
cargo test optimizer::tests   # only optimizer module
cargo test parser::tests      # only parser module
```

## Code Style

- **Naming**: snake_case for functions/variables, PascalCase for types/enums
- **Formatting**: `cargo fmt` defaults; no custom config
- **Error handling**: `anyhow::Result<()>` at CLI boundary
- **Modules**: flat structure under `src/`; `data/` subdir with `mod.rs` facade
- **Tests**: inline `#[cfg(test)] mod tests { ... }` in each source file; no separate integration test dir
- **Comments**: doc comments on public items; implementation details in code or PLAN.md
- **No emojis**, no marketing language in prose

## Key Architecture Facts

Full specification is in [PLAN.md](../PLAN.md). Essentials:

- **SP formula**: cumulative table lookup `(CUMULATIVE[to] - CUMULATIVE[from]) * STC`. See `calculator.rs:CUMULATIVE_SP`.
- **Rate formula**: `(primary + secondary / 2.0) / 60.0` SP/s. Primary worth exactly 2x secondary.
- **Optimizer**: greedy best-response per epoch over **2,885** valid allocations (base=17 + 14 free points, max +10/attr). Precomputed time caches and suffix sums accelerate evaluation.
- **Queue input**: `--queue FILE` or `-` for stdin. Format: one "Skill Name \<level>" per line; `#` comments and blanks ignored; case-insensitive matching. Level N means train from (N-1) to N.
- **CLI flags on `optimize`**: `--queue`, `--attributes`, `--implant-bonuses`, `--bonus-remaps`, `--remap-available Dd`, `--json`, `--queue-out FILE` (`-` for stdout).

## File Map

```
src/
├── main.rs           — CLI entrypoint, command dispatch, output formatters
├── cli.rs            — clap derive argument definitions (--queue, --attributes, --implant-bonuses, --remap-available, etc.)
├── calculator.rs     — SP formula, rate computation, duration helpers, format_duration
├── parser.rs         — queue file parser, attribute/implant string parsers (pure functions, tested)
├── optimizer.rs      — multi-epoch allocation search with simulation engine
└── data/
    ├── mod.rs        — load_skills(), load_implants() facades
    └── models.rs     — SkillRecord, QueuedSkill, CharacterState, EffectiveAttributes, etc.
assets/
├── skills.json       — ~400 skill records from SDE
└── implants.json     — implant type -> attribute bonus mapping
```
