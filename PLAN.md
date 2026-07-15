# EVE Remap — Project Plan

## Problem Statement

EVE Online players invest hundreds of hours training skills on their characters. When they use a Neural Interface to remap (reallocate attribute points between Intelligence, Memory, Processing, Perception, Willpower), it resets all currently training skills back to their starting SP levels. The optimizer should answer:

> Given my character's current state and a target set of skills I want to train, what attribute allocation minimizes total training time?

Secondary question: given a fixed remap, what is the optimal skill training order?

## Scope

### In Scope (MVP)

1. **Skill duration calculator** — compute exact training time for any skill→level transition given an attribute allocation, using SDE-derived skill data (baseTime, primaryAttribute + modifier, secondaryAttribute + modifier).
2. **Optimizer core** — given a character snapshot + target skill list → find the attribute allocation (remap) that minimizes total queue duration. Brute-force over valid allocations (~12K combos max); tractable thanks to bucketed scoring.
3. **Queue scheduler** — given a fixed remap + target skills, determine optimal ordering respecting brain size limits and prerequisite chains.
4. **Data layer** — parse SDE JSON dump once into a compact `skills.json` (one record per skill: id, name, baseTime, primaryAttrId, primaryModifier, secondaryAttrId, secondaryModifier); query ESI for live character state.
5. **CLI interface** — `eve-remap optimize --character-id <id> --targets <file>` produces the recommended remap + ordered queue.

### Out of Scope (for now)

- GUI / web frontend
- Real-time ESI polling or live progress tracking
- Implant bonuses (those modify effective attributes but are per-character and optional)
- Multi-character fleet optimization

## Tech Stack

| Layer | Choice | Rationale |
|-------|--------|-----------|
| Language | **Rust** | Fast computation, strong typing, great CLI ergonomics via clap, native binary distribution |
| CLI | **clap** | Standard Rust CLI framework |
| Skill data | **JSON file** (`assets/skills.json`) | Only ~400 skills × 6 fields each; no DB needed. Generated once from SDE at build time or via `download` command. |
| HTTP | **reqwest** | Fetch ESI character data |
| Config | **env vars** | ESI token in env, no runtime config files |
| Testing | **cargo test** with proptest | Deterministic, fast |

## Architecture

```
┌─────────────────────────────────┐
│           CLI (clap)            │
│  Commands:                      │
│    optimize   — full pipeline   │
│    download   — fetch + parse SDE│
│    char-show  — inspect char     │
└──────────────┬──────────────────┘
               │
┌──────────────▼──────────────────┐
│        Application Layer        │
│                                 │
│  ┌───────────┐  ┌────────────┐ │
│  │ Optimizer  │  │ Scheduler  │ │
│  │ (remap +   │  │ (brain-    │ │
│  │  ordering) │  │  aware)    │ │
│  └─────┬─────┘  └─────┬──────┘ │
│        │              │         │
│  ┌─────▼──────────────▼──────┐  │
│  │      Duration Calculator   │  │
│  │  f(baseTime, primaryAttr,  │  │
│  │     secondaryAttr, level)  │  │
│  └──────────────┬─────────────┘  │
└─────────────────┼────────────────┘
                  │
┌─────────────────▼────────────────┐
│          Data Layer              │
│                                  │
│  ┌──────────────┐  ┌──────────┐ │
│  │ skills.json  │  │ ESI HTTP │ │
│  │ (pre-parsed  │  │ Client   │ │
│  │  SDE extract)│  │          │ │
│  └──────────────┘  └──────────┘ │
└──────────────────────────────────┘
```

### Project Structure

```
eve-remap/
├── Cargo.toml
├── src/
│   ├── main.rs           — CLI entrypoint (clap commands)
│   ├── cli.rs             — clap argument definitions
│   ├── calculator.rs      — skill duration formula + bucket builder
│   ├── optimizer.rs       — allocation enumeration + scoring
│   ├── scheduler.rs       — queue ordering with brain size
│   ├── data/
│   │   ├── mod.rs         — data layer facade
│   │   ├── sde.rs         — SDE JSONL → skills.json parser
│   │   ├── esi.rs         — ESI client (reqwest wrapper)
│   │   └── models.rs      — shared domain types
│   └── config.rs          — token / path configuration
├── assets/
│   └── skills.json        — pre-parsed skill data (~400 entries, ~150KB)
├── tests/
│   ├── calculator_test.rs — duration formula against known values
│   └── optimizer_test.rs  — small character scenarios
└── .env                   — gitignored; ESI credentials
```

## Domain Model

### Skill Duration Formula

From EVE mechanics, each skill has a **primary** and **secondary** attribute:

```
duration(skill, level) = baseTime × levelMultiplier[level]
    / (primaryAttrValue ^ primaryModifier)
    / (secondaryAttrValue ^ secondaryModifier)

levelMultiplier = [1, 4, 20, 80, 360]  // for levels 1-5
```

Where per skill:
- `baseTime` — base training time in seconds
- `primaryAttrId` / `primaryModifier` — governing attribute and its exponent
- `secondaryAttrId` / `secondaryModifier` — secondary attribute and its exponent

### Bucketed Scoring

Precompute once from the target queue. Each skill transition contributes to exactly one bucket keyed by `(primaryAttrId, primaryModifier, secondaryAttrId, secondaryModifier)`:

```
For each target skill transition (currentLevel → desiredLevel):
    rawSP = baseTime × sum(levelMultiplier[currentLevel..desiredLevel-1])
    key   = (skill.primaryAttrId, skill.primaryMod,
             skill.secondaryAttrId, skill.secondaryMod)
    bucket[key] += rawSP
```

Scoring any allocation `(a1..a5)` is then:

```
totalDuration = Σ_keys bucket[k] / (alloc[k.pAttr]^k.pMod) / (alloc[k.sAttr]^k.sMod)
```

With ~400 skills collapsing into a handful of distinct (attr×mod) pairs, this is O(bucketCount) per allocation instead of O(numSkills).

### Attribute Allocation Space

Valid remaps distribute points across 5 attributes with constraints:
- Each attribute must be ≥ 1
- Total points depends on character's SP investment in attributes (typically 25 base + unallocated SP can buy more, up to 25 per attribute max)
- For brute-force: enumerate all integer partitions of N into 5 bins with bounds [minAttr, maxAttr]. At N=25, that's C(24,4) = 12,650 combinations — very fast even in Rust without optimization. With actual min/max constraints from character data, it's typically fewer (~560 realistic combos).

### Brain Size

Brain size determines how many skills can train simultaneously:
- Base brain size grows with total trained skill levels
- Max ~25 concurrent training slots at high brain size
- The scheduler fills parallel slots before sequencing

## Data Flow

1. **SDE Ingestion** (`eve-remap download`)
   - Download SDE zip from CCP → extract relevant JSONL files
   - Parse types.jsonl + typeDogma.jsonl → compact `assets/skills.json`
   - Each record: `{ id, name, baseTime, primaryAttrId, primaryModifier, secondaryAttrId, secondaryModifier }`

2. **Character Fetch** (`eve-remap char-show --character-id <id>`)
   - Auth via ESI token (stored in env var or .env file)
   - Fetch `/characters/{id}/skills` (current skill levels, SP totals)
   - Fetch `/characters/{id}/skillqueue` (what's currently queued)
   - Display summary

3. **Optimization** (`eve-remap optimize --character-id <id> --targets <file>`)
   - Load character state + target skills
   - Pre-compute per-(attr, modifier) SP buckets from target queue
   - For each valid remap allocation:
     - Score using bucketed formula (~25 divisions per allocation)
   - Return top-N allocations with projected queue
   - Output ordered skill list respecting brain size

## Implementation Plan

### Phase 1 — Foundation
- Scaffold Rust project with Cargo
- Implement SDE parser: download JSONL → extract skill records → write `skills.json`
- Build `calculator.rs` with the duration formula; test against known values
- Add `.gitignore` for .env

### Phase 2 — Data Layer
- ESI client: authenticated requests to `/skills` and `/skillqueue`
- Domain models mapping API responses → internal types
- Character state snapshot type combining ESI + skills.json lookups

### Phase 3 — Optimizer Core
- Enumerate valid attribute allocations given character constraints
- Build SP buckets from target skill set (one-time precomputation)
- Score each allocation against bucketed sums; return ranked results

### Phase 4 — Scheduler
- Brain-size-aware parallel scheduling
- Prerequisite resolution (some skills require others as pre-requisites — stored in SDE dogma effects or computed from game knowledge)
- Generate final ordered queue output

### Phase 5 — CLI Polish
- All commands wired up with clap subcommands
- JSON and pretty-print output formats
- Configuration via environment variables

## Key Decisions

1. **Rust over Python**: Fast computation for the optimizer's tight loop, native binary distribution, no venv or dependency hell.

2. **JSON file over SQLite**: The data we need per skill is 6 scalar fields (~400 skills). A flat JSON file loads in microseconds with serde — no DB library, no schema migrations, no runtime dependency.

3. **Brute-force over heuristic search**: ~12K allocations max × O(1) bucketed scoring = sub-second runtime. Exhaustive search is correct by construction.

4. **No prerequisite graph in MVP**: Skill prerequisites are complex. The scheduler works on a flat priority list first; prerequisites can be added later from SDE group dependencies.
