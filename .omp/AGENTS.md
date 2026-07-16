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

- **SP formula**: `SP = skillTimeConstant × levelMultiplier[level] × 20000`. The ×20000 base unit is critical — without it durations are off by 4 orders of magnitude.
- **Rate formula**: `(effectivePrimary + effectiveSecondary / 2.0) / 60.0` SP/s.
- **Optimizer**: greedy best-response per epoch. Epoch 0 fixed to current attributes; each subsequent epoch picks the allocation minimizing projected finish of the bottleneck skill. ~12K allocations searched per epoch via backtracking (C(24,4)).
- **Queue input**: two modes — ESI `/skillqueue` when authenticated, or `--queue FILE` for offline use. Queue files: one "Skill Name \<level>" per line; `#` comments and blank lines ignored; case-insensitive name matching against SDE data.
- **Auth**: PKCE (`--sso`) requires port forwarding on WSL; implicit grant (`--browser`) works cross-platform. JWT claims differ from docs: `sub: "CHARACTER:EVE:<id>"`, `scp` array for scopes, `name` for character name.
- **Token refresh**: currently a placeholder — `refresh_token` field stored but not yet wired to `/oauth/token` endpoint.

## File Map

```
src/
├── main.rs           — CLI entrypoint, command dispatch, output formatters, queue file parser
├── cli.rs            — clap derive argument definitions (OptimizeArgs has --queue, --attributes)
├── calculator.rs     — SP formula, rate computation, duration helpers, format_duration
├── optimizer.rs      — multi-epoch allocation search with simulation engine
├── auth/
│   ├── mod.rs        — JWT decode, account store CRUD, find_valid_token
│   └── sso.rs        — PKCE flow + browser implicit grant flow
└── data/
    ├── mod.rs        — load_skills(), load_implants() facades
    ├── models.rs     — SkillRecord, QueuedSkill, CharacterState, EffectiveAttributes, etc.
    ├── esi.rs        — EsIClient: authenticated ESI requests, character state fetching
    └── sde.rs        — SDE JSONL → skills.json parser (not yet implemented)
assets/
├── skills.json       — ~400 skill records from SDE
└── implants.json     — implant type → attribute bonus mapping
```
