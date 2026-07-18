# EVE Remap — Project Plan

## Problem Statement

EVE Online players invest hundreds of hours training skills on their characters. When they use a Neural Interface to remap (reallocate attribute points between Intelligence, Charisma, Perception, Memory, Willpower), currently training skills keep their accumulated SP but switch to the new generation rate immediately. Players have a timed remap available every 365 days plus any bonus remaps they've purchased. Active implants add +1 to +5 per slot to specific attributes.

The optimizer should answer:

> Given my character's current state and queued skills — how should I sequence my allocations across remap epochs to minimize total wall-clock time until everything finishes?

Output: phased plan telling the user what allocation to set at each epoch, which skills will finish by then, and projected completion dates.

## Current State

Offline-only CLI tool. No authentication, no ESI integration. Users supply their skill queue via `--queue FILE` with explicit attributes and implant bonuses. The optimizer runs entirely locally against pre-parsed SDE assets shipped in the repo.

### Planned Work

- **Read from stdin / write to stdout** — pipe-friendly mode for scripting (`cat queue.txt | eve-remap optimize - > plan.json`)
- **Export modified queue** — produce an EVE Online-importable queue file based on optimized epoch ordering
- **ESI authentication & live data fetch** — PKCE/implicit grant flows, `/skillqueue`, `/attributes`, `/implants` endpoints (deferred)
- **SDE download** — fetch and parse latest CCP SDE JSONL into `assets/` (deferred)
- **Token refresh** — wire actual `/oauth/token` refresh call on expiry (deferred)
- **Colored terminal output** — colored output and progress bars during optimization
- **Save/load plans** — persist optimization results to/from files
- **Multi-select character prompt** — when multiple accounts are stored

## Tech Stack

| Layer | Choice |
|---|---|
| Language | Rust 2021 edition (Rust 1.75 compatible) |
| CLI | clap derive subcommands |
| Data | Flat JSON assets (`assets/skills.json`, `assets/implants.json`) |
| Testing | `cargo test` — unit tests across calculator, optimizer |

No async runtime, no HTTP client, no system dependencies. Four crates: `serde`, `serde_json`, `clap`, `anyhow`.

## Architecture

```
┌──────────────────────────────┐
│         CLI (clap)           │
│   optimize --queue FILE      │
└──────────┬───────────────────┘
           │
┌──────────▼───────────────────┐
│    Multi-Epoch Optimizer     │
│                              │
│  Simulate skills one-by-one  │
│  in queue order under each   │
│  epoch's allocation. Greedy  │
│  best-response per epoch.    │
└──────────┬───────────────────┘
           │
┌──────────▼───────────────────┐
│    Duration Calculator       │
│  SP = cumulative delta × STC │
│  rate = (P + S/2) / 60 SP/s │
└──────────┬───────────────────┘
           │
┌──────────▼───────────────────┐
│       Data Layer             │
│  skills.json                 │
│  implants.json               │
└──────────────────────────────┘
```

### Project Structure

```
eve-remap/
├── Cargo.toml
├── src/
│   ├── main.rs         — CLI entrypoint, command dispatch, output formatters, queue file parser
│   ├── cli.rs          — clap derive argument definitions (--queue, --attributes, --implant-bonuses, etc.)
│   ├── calculator.rs   — SP formula, rate computation, duration helpers, format_duration
│   ├── optimizer.rs    — multi-epoch allocation search with simulation engine
│   └── data/
│       ├── mod.rs      — load_skills(), load_implants() facades
│       └── models.rs   — SkillRecord, QueuedSkill, CharacterState, EffectiveAttributes, etc.
├── assets/
│   ├── skills.json     — ~400 skill records from SDE
│   └── implants.json   — implant type -> attribute bonus mapping
```

## Domain Model

### Remap Mechanics (confirmed from CCP support docs)

- **Timed remap**: available every 365 days after last use. Consumed first if both timed and bonus are available.
- **No SP rollback**: actively training skills keep their accumulated SP and immediately switch to the new rate. Only future SP generation is affected.
- **Bonus remaps**: Optional — if `--bonus-remaps N` is not specified, the optimizer runs unlimited timed epochs until the queue empties. Configurable via `--bonus-remaps N`.

### Skill Duration Formula

From EVE mechanics, each skill has a **primary** and **secondary** attribute. SP costs use an authoritative cumulative table (rank 1):

| Level | Cumulative SP | Incremental SP |
|---|---|---|
| L1    | 250          | 250            |
| L2    | 1,414        | 1,164          |
| L3    | 8,000        | 6,586          |
| L4    | 45,255       | 37,255         |
| L5    | 256,000      | 210,745        |

```
SP for transition = (CUMULATIVE[to_level] - CUMULATIVE[from_level]) * skillTimeConstant
rate_per_second   = (effectivePrimaryAttr + effectiveSecondaryAttr / 2.0) / 60.0
duration_seconds  = SP_for_transition / rate_per_second
```

Where per skill:
- `skillTimeConstant` — multiplier from SDE type data (typically 1.0–4.0)
- `primaryAttribute` / `secondaryAttribute` — governing attributes by name
- `effectiveAttrValue = baseRemappedValue + sum(implantBonuses for that attr)`

### Effective Attributes

For offline mode (`--queue FILE`), effective attributes default to the base values provided via `--attributes`. Implant bonuses supplied via `--implant-bonuses` are preserved across post-remap epochs.

Effective value used in duration formula = base + total implant bonus per attribute.

### Multi-Epoch Optimization (no rollback model)

Skills train **sequentially** in queue order — only one skill earns SP at any given moment. On completion, the next queued skill starts. Skills keep their SP across remaps; only the future training rate changes. Lower levels of a skill must complete before higher ones (Gunnery 1 → Gunnery 2), but cross-skill prerequisites are ignored for now.

Primary attribute points are worth exactly **twice** as much as secondary (`+1 primary = +2 secondary`), because the rate formula is linear.

#### Greedy Best-Response per Epoch

The optimizer uses a greedy approach: epoch 0 is fixed to current effective attributes, then each subsequent epoch picks the allocation that minimizes the projected finish time of the last queued skill. This avoids exhaustive search over `allocations^epochs`.

Precomputed time caches store training durations for every skill-allocation pair, enabling fast evaluation during the greedy search. Suffix sums accelerate multi-skill projections under each candidate allocation.

#### Allocation Space

A remap distributes **14** free points above a hard floor of **17** across 5 attributes. Per-attribute cap is **+10** (max value = 27). This yields **2,885 valid distributions**. No single-attribute dump is possible since 14 > 10, so every allocation touches at least 2 attributes. Distribution by boosted attribute count: 2 attrs (70), 3 attrs (690), 4 attrs (1,410), 5 attrs (715).

## Input Parameters (CLI)

| Flag | Description |
|---|---|
| `-q FILE`, `--queue FILE` | Path to queue file (required). Use `-` to read from stdin |
| `--attributes PER:MEM:WIL:INT:CHA` | Base remapped attribute values excluding implants (default: 17:17:17:17:17) |
| `--implant-bonuses PER:MEM:WIL:INT:CHA` | Implant bonuses persisting across remaps (default: 0:0:0:0:0) |
| `--bonus-remaps N` | Number of bonus neural interface remaps (optional — unlimited timed epochs if omitted) |
| `--remap-available Dd` | When normal remap cooldown expires, e.g. `0d` = now, `30d` = in 30 days (default: 0d) |
| `--json` | Output results as JSON instead of human-readable table |
| `--queue-out FILE` | Write optimized skill order to a file. Use `-` for stdout |

### Queue File Format

One skill per line in the format `"Skill Name <level>"`. Lines starting with `#` are comments; blank lines are ignored. Skill names must match entries in `assets/skills.json` (case-insensitive). Level must be 1–5. Skills at level 5 are skipped by the optimizer (already maxed).

Example:
```
# My training targets
Gunnery 3
Navigation 5
Drone Navigation 2
```

## Data Flow

1. Parse queue file into target skills and levels.
2. Look up each skill in `assets/skills.json` to get time constant and attributes.
3. Compute SP needed per transition using cumulative table lookup × skillTimeConstant.
4. Build character state from `--attributes` values. If `--implant-bonuses` is provided, those deltas are preserved across post-remap epochs.
5. Run multi-epoch optimizer — output phased plan.

## Implementation Status

### Phase 1 — Foundation ✅
- [x] Rust project scaffolded with Cargo, edition 2021
- [x] `calculator.rs` with correct duration formula using cumulative SP table lookup
- [x] SDE asset files (`assets/skills.json`, `assets/implants.json`) present in repo
- [x] Calculator tests covering SP rate, cumulative table, and duration helpers

### Phase 2 — Offline Optimizer ✅
- [x] Domain models mapping API responses to internal types in `data/models.rs`
- [x] Character state snapshot combining user input + assets lookups
- [x] Queue file parser (`main.rs`): parse "Skill Name <level>" format

### Phase 3 — Multi-Epoch Optimizer ✅
- [x] Simulation engine: advance queue sequentially through epochs at varying rates
- [x] Allocation generator: backtracking search producing valid attribute distributions
- [x] Greedy allocation search per epoch (minimize last-skill finish time)
- [x] Output phased plan with table and JSON formats
- [x] Optimizer tests covering allocation generation, epoch simulation, and greedy scheduling

### Phase 4 — CLI Polish ✅
- [x] All commands wired up with clap derive subcommands
- [x] Human-readable output: table per epoch showing allocation, which skills complete, projected dates
- [x] JSON output format for scripting (`--json`)
- [x] Queue file input (`--queue FILE`) with offline mode (`--attributes`, `--implant-bonuses`)

### Removed (deferred to later)
- ~~Auth & ESI integration~~ — PKCE/implicit grant SSO flows, JWT introspection, account store, ESI client (`eve_esi` crate), token persistence. Deferred to a future phase.
- ~~SDE download~~ — `download_sde()` and JSONL parser removed. Assets shipped statically in repo.

## Key Decisions

1. **Rust**: Fast computation for the optimizer's tight loop, native binary distribution, no venv or dependency hell. Edition pinned to 2021 for Rust 1.75 compatibility on WSL.

2. **JSON files over SQLite**: Skill and implant data are ~400+ entries x 7 fields each. Flat JSON loads in microseconds with serde — no DB library needed.

3. **Greedy epoch optimization over exhaustive search**: With N~4 max epochs and up to 2,885 allocations per epoch, exhaustive `allocations^epochs` is impossible. Greedy best-response per epoch runs instantly and produces near-optimal results because each epoch independently accelerates all remaining skills.

4. **Remap info via CLI args**: ESI doesn't expose neural interface cooldown or bonus remap count; user provides `--bonus-remaps N` and `--remap-available Dd`. Optional — if omitted, optimizer runs unlimited timed epochs until queue empties.

5. **Queue from file only (for now)**: Target skills come from `--queue FILE` (offline). ESI fetch deferred to a future phase when auth is re-added.

6. **Cumulative SP table over multiplier formula**: Authoritative values from EVE Online forums archive: L1=250, L2=1414, L3=8000, L4=45255, L5=256000. SP for transition = `(CUMULATIVE[to] - CUMULATIVE[from]) * STC`. This replaced the incorrect LEVEL_MULTIPLIERS*BASE_SP approach.

7. **Zero system dependencies**: No OpenSSL, no pkg-config, no vcpkg. Pure Rust dependency tree (`serde`, `serde_json`, `clap`, `anyhow`). Builds on any platform with Rust installed.
