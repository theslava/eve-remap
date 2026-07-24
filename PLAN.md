# EVE Remap — Project Plan

## Problem Statement

EVE Online players invest hundreds of hours training skills on their characters. When they use a Neural Interface to remap (reallocate attribute points between Intelligence, Charisma, Perception, Memory, Willpower), currently training skills keep their accumulated SP but switch to the new generation rate immediately. Players have a timed remap available every 365 days plus any bonus remaps they've been granted. Active implants add +1 to +5 per slot to specific attributes.

The optimizer should answer:

> Given my character's current state and queued skills — how should I sequence my allocations across remap epochs to minimize total wall-clock time until everything finishes?

Output: phased plan telling the user what allocation to set at each epoch, which skills will finish by then, and projected completion dates.

## Current State

CLI tool with optional ESI integration. Users supply their skill queue via `--queue FILE` (offline) or fetch it from EVE Online SSO using `--character NAME` (ESI mode). In ESI mode, base attributes, implant bonuses, and SP-trained progress are resolved from the API; offline mode uses explicit CLI flags (`--attributes`, `--implant-bonuses`). The optimizer runs entirely against pre-parsed SDE assets shipped in the repo.

### Planned Work

- **Export modified queue** — produce an EVE Online-importable queue file based on optimized epoch ordering
- **Colored terminal output** — colored output and progress bars during optimization
- **Save/load plans** — persist optimization results to/from files

## Tech Stack

| Layer | Choice |
|---|---|
| Language | Rust 2021 edition (Rust 1.75 compatible) |
| CLI | clap derive subcommands |
| Data | Flat JSON assets (`assets/skills.json`, `assets/implants.json`) |
| Testing | `cargo test` — unit tests across calculator, optimizer |

Async runtime (tokio) for ESI HTTP client. Rustls for TLS without OpenSSL. Core crates: `serde`, `serde_json`, `clap`, `anyhow`; plus `reqwest`, `tokio`, `rustls`, `chrono`.

## Architecture

```
┌──────────────────────────────┐
│         CLI (clap)           │
│   optimize [-q FILE|stdin]   │
│   login / logout / accounts  │
└──────────┬───────────────────┘
           │
     ┌─────┴─────┐
     │  Dual path │
     └─────┬─────┘
     ┌─────┼──────┐
     ▼     ▼      ▼
┌──────────┐  ┌──────────────────┐
│ File/    │  │ ESI auth + HTTP   │
│ Stdin    │  │ (/skillqueue,     │
│ parser   │  │  /attributes,     │
│          │  │  /characters/me/) │
└────┬─────┘  └────────┬─────────┘
     │                 │
     └────────┬────────┘
              ▼
┌──────────────────────────────┐
│    resolve_attributes()      │
│  CLI → ESI → defaults        │
│  Single source of truth for  │
│  base_attrs & implant_bonus  │
└──────────┬───────────────────┘
           │
┌──────────▼───────────────────┐
│    Multi-Epoch Optimizer     │
│                              │
│  Reorder queue for attribute │
│  locality (prereq-aware),    │
│  then simulate sequentially  │
│  under each epoch's alloc.   │
│  Greedy best-response per    │
│  epoch minimizes finish time.│
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
│   ├── main.rs         — CLI entrypoint, command dispatch, output formatters, resolve_attributes()
│   ├── cli.rs          — clap derive argument definitions (--queue, --attributes, --implant-bonuses, etc.)
│   ├── calculator.rs   — SP formula, rate computation, duration helpers, format_duration
│   ├── parser.rs       — queue file parser, attribute/implant string parsers (pure functions, tested)
│   ├── optimizer.rs    — multi-epoch allocation search with simulation engine
│   ├── auth.rs         — EVE SSO PKCE flow, token persistence, account store
│   ├── esi.rs          — ESI HTTP client models (/skillqueue, /attributes, /characters/me)
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

Effective value used in duration formula = base + total implant bonus per attribute.

Resolution is unified through `resolve_attributes()` (`src/main.rs`), which returns `(base_attrs, source_label, implant_bonus)` from a single priority chain:

| Priority | Source | Fallback |
|---|---|---|
| Base attrs | `--attributes` CLI override | ESI `/characters/{id}/attributes/` (back-calculated to exclude implants) | default 17:17:17:17:17 |
| Implant bonus | `--implant-bonuses` CLI override | ESI active implant IDs → local SDE lookup | 0:0:0:0:0 |

The optimizer receives only the resolved values — raw implant IDs are never passed downstream, preventing double-counting bugs.

### Multi-Epoch Optimization (no rollback model)

Skills train **sequentially** in queue order — only one skill earns SP at any given moment. On completion, the next queued skill starts. Skills keep their SP across remaps; only the future training rate changes. The optimizer reorders the queue for attribute locality while respecting both intra-skill ordering (Gunnery 1 before Gunnery 2) and cross-skill prerequisites from SDE data, using topological sort with attribute-aware tie-breaking.

Primary attribute points are worth exactly **twice** as much as secondary (`+1 primary = +2 secondary`), because the rate formula is linear.

#### Greedy Best-Response per Epoch

The optimizer uses a greedy approach: epoch 0 is fixed to current effective attributes, then each subsequent epoch picks the allocation that minimizes the projected finish time of the last queued skill. This avoids exhaustive search over `allocations^epochs`.

Precomputed time caches store training durations for every skill-allocation pair, enabling fast evaluation during the greedy search. Suffix sums accelerate multi-skill projections under each candidate allocation.

#### Allocation Space

A remap distributes **14** free points above a hard floor of **17** across 5 attributes. Per-attribute cap is **+10** (max value = 27). This yields **2,885 valid distributions**. No single-attribute dump is possible since 14 > 10, so every allocation touches at least 2 attributes. Distribution by boosted attribute count: 2 attrs (70), 3 attrs (690), 4 attrs (1,410), 5 attrs (715).

| Flag | Description |
|---|---|
| `-q FILE`, `--queue FILE` | Path to queue file (optional if `--character` used). Use `-` for stdin |
| `--character NAME` | Fetch queue/attributes from ESI for the named character (requires `eve-remap login`) |
| `--attributes PER:MEM:WIL:INT:CHA` | Base remapped attribute values excluding implants (default: 17:17:17:17:17). Overridden by ESI when `--character` is set unless explicitly provided |
| `--implant-bonuses PER:MEM:WIL:INT:CHA` | Implant bonuses persisting across remaps (default: 0:0:0:0:0). Overrides ESI implant lookup |
| `--bonus-remaps N` | Number of bonus neural interface remaps (optional — unlimited timed epochs if omitted) |
| `--remap-available Dd` | When normal remap cooldown expires, e.g. `0d` = now, `30d` = in 30 days (default: 0d) |
| `--json` | Output results as JSON instead of human-readable table |
| `--queue-out FILE` | Write optimized skill order to a file. Use `-` for stdout |

### Queue File Format

One skill per line. Lines starting with `#` are comments; blank lines are ignored. Skill names match case-insensitively against `assets/skills.json`. Level must be 1–5 (skills at level 5 are skipped).

**Basic format:** `"Skill Name <level>"` — trains from (level-1) to level.

**Progress formats** (for partially trained skills):
| Syntax | Meaning |
|---|---|
| `"SkillName 3 @7d"` | Target L3, 7 days of training already completed |
| `"SkillName 3 @12345 SP"` | Target L3, 12345 SP already earned |

ESI-fetched queues use the SP-trained format automatically, preserving exact partial progress from the API.

Example:
```
# My training targets
Gunnery 3
Navigation 5
Drone Navigation 2 @3d
```

## Data Flow

**Offline mode** (`--queue FILE`):
1. Parse queue file into target skills and levels.
2. Resolve attributes via CLI flags → `resolve_attributes()`.
3. Look up each skill in `assets/skills.json` for time constant and attributes.
4. Compute SP needed per transition using cumulative table lookup × skillTimeConstant.
5. Run multi-epoch optimizer — output phased plan.

**ESI mode** (`--character NAME`):
1. Authenticate with saved tokens from `eve-remap login`.
2. Fetch `/skillqueue`, `/characters/me/attributes/`, active implant IDs from ESI.
3. Convert ESI entries to parser text format (`"SkillName L@sp_trained"`) and delegate to `parse_queue()` — same code path as offline.
4. Resolve base attributes and implant bonuses via `resolve_attributes()` (back-calculates neural interface values from ESI effective attributes).
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
- [x] Queue file parser extracted to `parser.rs`: pure functions for attributes, implant bonuses, and queue parsing (~45 tests)

### Phase 3 — Multi-Epoch Optimizer ✅
- [x] Simulation engine: advance queue sequentially through epochs at varying rates
- [x] Allocation generator: backtracking search producing valid attribute distributions
- [x] Greedy allocation search per epoch (minimize last-skill finish time)
- [x] Output phased plan with table and JSON formats
- [x] Optimizer tests covering allocation generation, epoch simulation, greedy scheduling, edge cases (L5-only, remap cooldown), property invariant (optimized ≤ baseline)

### Phase 4 — CLI Polish ✅
- [x] All commands wired up with clap derive subcommands
- [x] Human-readable output: table per epoch showing allocation, which skills complete, projected dates
- [x] JSON output format for scripting (`--json`)
- [x] Queue file input (`--queue FILE`) with offline mode (`--attributes`, `--implant-bonuses`)

### Phase 5 — Test Coverage ✅
- [x] Extracted `src/parser.rs` with testable pure functions (`parse_attributes`, `parse_implant_bonuses`, `parse_queue`)
- [x] 45+ unit tests across parser module: format variants, progress disambiguation, error messages, boundary values
- [x] Edge-case optimizer tests: L5-only queues, remap-available-exceeds-completion, zero-bonus-single-switch, duration-no-progress
- [x] Property test: optimizer result never exceeds baseline wall-clock time

### Phase 6 — ESI Integration ✅
- [x] SSO PKCE auth flow with token persistence (`auth.rs`, `login/logout/accounts` commands)
- [x] ESI HTTP client fetching `/skillqueue`, `/characters/me/attributes/`, active implants (`esi.rs`)
- [x] Unified attribute resolution via `resolve_attributes()`: CLI override → ESI data → defaults
- [x] ESI queue entries converted to parser text format for identical SP math as offline mode
- [x] Single source of truth for implant bonuses prevents double-counting bugs
- [x] First-skill pinning fix preserves optimizer ordering in epoch-0 output

### Removed (deferred to later)
- ~~SDE download~~ — fetch and parse latest CCP SDE JSONL into `assets/`. Assets shipped statically in repo.

## Key Decisions

1. **Rust**: Fast computation for the optimizer's tight loop, native binary distribution, no venv or dependency hell. Edition pinned to 2021 for Rust 1.75 compatibility on WSL.

2. **JSON files over SQLite**: Skill and implant data are ~400+ entries x 7 fields each. Flat JSON loads in microseconds with serde — no DB library needed.

3. **Greedy epoch optimization over exhaustive search**: With N~4 max epochs and up to 2,885 allocations per epoch, exhaustive `allocations^epochs` is impossible. Greedy best-response per epoch runs instantly and produces near-optimal results because each epoch independently accelerates all remaining skills.

4. **Remap info via CLI args**: ESI doesn't expose neural interface cooldown or bonus remap count; user provides `--bonus-remaps N` and `--remap-available Dd`. Optional — if omitted, optimizer runs unlimited timed epochs until queue empties.

5. **Dual input paths converge at parser**: ESI-fetched queues render as `"SkillName L@sp_trained"` text and flow through `parser::parse_queue()`, ensuring offline and ESI modes share identical duration/SP computation logic.

6. **Cumulative SP table over multiplier formula**: Authoritative values from EVE Online forums archive: L1=250, L2=1414, L3=8000, L4=45255, L5=256000. SP for transition = `(CUMULATIVE[to] - CUMULATIVE[from]) * STC`. This replaced the incorrect LEVEL_MULTIPLIERS*BASE_SP approach.

7. **Single source of truth for attributes**: `resolve_attributes()` consolidates CLI overrides, ESI data, and defaults into one function returning `(base_attrs, source_label, implant_bonus)`. The optimizer receives only resolved values — raw implant IDs never pass downstream, preventing double-counting bugs.
