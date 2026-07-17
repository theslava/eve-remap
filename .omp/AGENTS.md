# Agent Guidelines — Eve Remap

## Project Goal

Optimize **EVE Online** character attribute remaps across multiple epochs to minimize total wall-clock time for training target skills. Users provide their skill queue either via ESI authentication or a local text file (`--queue FILE`), along with base attributes and bonus remap count. The optimizer outputs a phased plan: what allocation to set at each epoch, which skills complete, and projected finish dates.

## Current Phase

Phase 5 — Remaining work: token refresh implementation, colored output, save/load plans, multi-select character prompt. Core optimizer, CLI, auth flows, and offline mode are all working.

## Tech Stack

| Layer | Choice |
|-------|--------|
| Language | Rust 2021 edition (Rust 1.75 compatible) |
| CLI | clap derive subcommands |
| Async | tokio runtime, reqwest HTTP client |
| Data | Flat JSON assets (`assets/skills.json`, `assets/implants.json`) |
| Auth | JWT introspection + PKCE / implicit grant SSO flows |
| Token storage | `~/.config/eve-remap/accounts.json` |
| Testing | `cargo test` — 34 tests across calculator, optimizer, ESI parsing, auth |

## Build Commands

```bash
cargo build          # debug build
cargo run --release  # release binary
cargo test           # run all 34 tests
```

## Test Commands

```bash
cargo test                          # full suite (calculator, optimizer, ESI, auth)
cargo test calculator::tests        # only calculator module
cargo test optimizer::tests         # only optimizer module
```

## Code Style

- **Naming**: snake_case for functions/variables, PascalCase for types/enums
- **Formatting**: `cargo fmt` defaults; no custom config
- **Error handling**: `anyhow::Result<()>` at CLI boundary, `thiserror` not used yet
- **Modules**: flat structure under `src/`; `auth/` and `data/` subdirs with `mod.rs` facades
- **Tests**: inline `#[cfg(test)] mod tests { ... }` in each source file; no separate integration test dir
- **Comments**: doc comments on public items; implementation details in code or PLAN.md
- **No emojis**, no marketing language in prose

## Key Architecture Facts

Full specification is in [PLAN.md](../PLAN.md). Essentials:

- **SP formula**: cumulative table lookup `(CUMULATIVE[to] - CUMULATIVE[from]) * STC`. See `calculator.rs:CUMULATIVE_SP`.
- **Rate formula**: `(primary + secondary / 2.0) / 60.0` SP/s. Primary worth exactly 2x secondary.
- **Optimizer**: greedy best-response per epoch over **2,885** valid allocations (base=17 + 14 free points, max +10/attr). Precomputed time caches and suffix sums accelerate evaluation.
- **Queue input**: ESI `/skillqueue` when authenticated, or `--queue FILE` offline. Format: one "Skill Name \<level>" per line; `#` comments and blanks ignored; case-insensitive matching. Level N means train from (N-1) to N.
- **Auth**: PKCE (`--sso`) needs port forwarding on WSL; implicit grant (`--browser`) works cross-platform. JWT claims: `sub: "CHARACTER:EVE:<id>"`, `scp` array, `name` string. Token refresh not yet wired.
- **CLI flags on `optimize`**: `--queue`, `--attributes`, `--implant-bonuses`, `--bonus-remaps`, `--remap-available Dd`, `--json`.

## File Map

```
src/
├── main.rs           — CLI entrypoint, command dispatch, output formatters, queue file parser
├── cli.rs            — clap derive argument definitions (--queue, --attributes, --implant-bonuses, --remap-available, etc.)
├── calculator.rs     — SP formula, rate computation, duration helpers, format_duration
├── optimizer.rs      — multi-epoch allocation search with simulation engine
├── auth/
│   ├── mod.rs        — JWT decode, account store CRUD, find_valid_token
│   └── sso.rs        — PKCE flow + browser implicit grant flow
└── data/
    ├── mod.rs        — load_skills(), load_implants() facades
    ├── models.rs     — SkillRecord, QueuedSkill, CharacterState, EffectiveAttributes, etc.
    ├── esi.rs        — EsIClient: authenticated ESI requests, character state fetching
    └── sde.rs        — SDE JSONL parser: extract_skills(), extract_implants(), download_sde()
assets/
├── skills.json       — ~400 skill records from SDE
└── implants.json     — implant type -> attribute bonus mapping
```
