# EVE Remap — Project Plan

## Problem Statement

EVE Online players invest hundreds of hours training skills on their characters. When they use a Neural Interface to remap (reallocate attribute points between Intelligence, Memory, Processing, Perception, Willpower), it resets all currently training skills back to their starting SP levels. The optimizer should answer:

> Given my character's current state and a target set of skills I want to train, what attribute allocation minimizes total training time?

Secondary question: given a fixed remap, what is the optimal skill training order?

## Scope

### In Scope (MVP)

1. **Skill duration calculator** — compute exact training time for any skill→level transition given an attribute allocation, using SDE data (baseTime, governingAttributeID, attributeModifier).
2. **Optimizer core** — given a character snapshot + target skill list → find the attribute allocation (remap) that minimizes total queue duration. Brute-force over valid allocations (max ~560 combinations for 25 points across 5 attributes with min 1 each); this is tractable.
3. **Queue scheduler** — given a fixed remap + target skills, determine optimal ordering respecting brain size limits and prerequisite chains.
4. **Data layer** — ingest SDE JSON dump (types.jsonl, typeDogma.jsonl, dogmaAttributes.jsonl, characterAttributes.jsonl) into a local SQLite database; query ESI for character state.
5. **CLI interface** — `eve-remap optimize --character-id <id> --skills <file>` produces the recommended remap + ordered queue.

### Out of Scope (for now)

- GUI / web frontend
- Real-time ESI polling or live progress tracking
- Implant bonuses (those modify effective attributes but are per-character and optional)
- Multi-character fleet optimization

## Tech Stack

| Layer | Choice | Rationale |
|-------|--------|-----------|
| Language | **Rust** | Fast computation (optimizer runs many combos), strong typing, great CLI ergonomics via clap, excellent ecosystem for data work |
| CLI | **clap** | Standard Rust CLI framework |
| Data format | **SQLite** (via rusqlite) | SDE fits in ~50MB on disk; perfect for joins between types/dogma/attributes |
| HTTP | **reqwest** | Fetch ESI character data |
| Config | **toml** (.env-style env vars for tokens) | Simple, no runtime config file needed |
| Testing | **cargo test** with proptest for fuzzing duration formulas | Deterministic, fast |

## Architecture

```
┌─────────────────────────────────┐
│           CLI (clap)            │
│  Commands:                      │
│    optimize   — full pipeline   │
│    download   — fetch SDE       │
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
│  │  f(baseTime, attrVal,      │  │
│  │     modifier, level) → ms  │  │
│  └──────────────┬─────────────┘  │
└─────────────────┼────────────────┘
                  │
┌─────────────────▼────────────────┐
│          Data Layer              │
│                                  │
│  ┌──────────────┐  ┌──────────┐ │
│  │ SDE SQLite DB │  │ ESI HTTP │ │
│  │ (types, dogma)│  │ Client   │ │
│  └──────────────┘  └──────────┘ │
└──────────────────────────────────┘
```

### Crate Structure

```
eve-remap/
├── Cargo.toml
├── src/
│   ├── main.rs           — CLI entrypoint (clap commands)
│   ├── cli.rs             — clap argument definitions
│   ├── calculator.rs      — skill duration formula
│   ├── optimizer.rs       — remap search + scoring
│   ├── scheduler.rs       — queue ordering with brain size
│   ├── data/
│   │   ├── mod.rs         — data layer facade
│   │   ├── sde.rs         — SDE ingestion → SQLite
│   │   ├── esi.rs         — ESI client (reqwest wrapper)
│   │   └── models.rs      — shared domain types
│   └── config.rs          — token / path configuration
├── tests/
│   ├── calculator_test.rs — duration formula against known values
│   └── optimizer_test.rs  — small character scenarios
├── data/                  — gitignored; holds SDE SQLite DB
│   └── sde.db
└── .env                   — gitignored; ESI credentials
```

## Domain Model

### Skill Duration Formula

From EVE mechanics:

```
duration(skill, level) = baseTime × levelMultiplier[level] / (attrValue ^ modifier)

levelMultiplier = [1, 4, 20, 80, 360]  // for levels 1-5
```

Where:
- `baseTime` — dogma attribute of the skill type (in seconds)
- `attributeValue` — character's value in the governing attribute (1..25)
- `modifier` — dogma attribute of the skill type (varies per skill)
- Each skill has exactly one governing attribute ID (also from dogma)

### Bucketed Scoring (avoids per-allocation re-scan)

Instead of iterating every target skill for every candidate allocation, precompute **once**:

```
For each target skill transition (currentLevel → desiredLevel):
    rawSP = baseTime × sum(levelMultiplier[currentLevel..desiredLevel-1])
    governingAttr = skill's governing attribute
    modifier     = skill's training modifier
    bucket[governingAttr][modifier] += rawSP
```

Then scoring any allocation `(a1, a2, a3, a4, a5)` is just:

```
totalDuration = Σ_attr Σ_modifier bucket[attr][mod] / (alloc[attr] ^ mod)
```

This reduces the inner loop from `O(numSkills)` to `O(distinctModifiers × 5)` — typically ~25 divisions instead of hundreds of multiplications + exponentiations. The buckets are computed once and reused across all ~12K allocations.

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
   - Parse and load into SQLite with indexed tables
   - Pre-compute a `skills` view joining types + dogma for quick lookups

2. **Character Fetch** (`eve-remap char-show --character-id <id>`)
   - Auth via ESI token (stored in env var or .env file)
   - Fetch `/characters/{id}/skills` (current skill levels, SP totals)
   - Fetch `/characters/{id}/skillqueue` (what's currently queued)
   - Display summary

3. **Optimization** (`eve-remap optimize --character-id <id> --targets <file>`)
   - Load character state + target skills
   - Pre-compute per-(attribute, modifier) SP buckets from target queue
   - For each valid remap allocation:
     - Score using bucketed formula (~25 divisions per allocation)
   - Return top-N allocations with projected queue
   - Output ordered skill list respecting brain size

## Implementation Plan

### Phase 1 — Foundation
- Scaffold Rust project with Cargo workspace
- Implement SDE download + SQLite ingestion pipeline
- Build `calculator.rs` with the duration formula; test against known values
- Add `.gitignore` for data/ and .env

### Phase 2 — Data Layer
- ESI client: authenticated requests to `/skills` and `/skillqueue`
- Domain models mapping API responses → internal types
- Character state snapshot type combining ESI + SDE lookups

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

1. **Rust over Python**: The optimizer's inner loop is tight but not GPU-bound; Rust gives us speed without complexity, plus we get a native binary distribution. No venv, no dependency hell.

2. **SQLite over flat files for SDE**: We need joins between types ↔ dogmaAttributes ↔ attributes. SQLite handles this natively and the DB fits in memory easily (~50MB).

3. **Brute-force over heuristic search**: The allocation space is small enough that exhaustive search is correct by construction. No risk of local optima.

4. **No prerequisite graph in MVP**: Skill prerequisites are complex and many players already know them. The scheduler can work on a flat priority list first; prerequisites can be added later when we parse the SDE's skill group dependencies.
