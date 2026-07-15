# EVE Remap — Project Plan

## Problem Statement

EVE Online players invest hundreds of hours training skills on their characters. When they use a Neural Interface to remap (reallocate attribute points between Intelligence, Memory, Processing, Perception, Willpower), it resets all currently training skills back to their starting SP levels. Players have a timed remap available every 365 days plus any bonus remaps they've purchased.

The optimizer should answer:

> Given my character's current attributes, skill queue, and available remaps — how should I order my skills across remap epochs to minimize total completion time?

Output: the user gets a phased plan telling them which skills to train under each allocation and when to hit remap.

## Scope

### In Scope (MVP)

1. **Skill duration calculator** — compute exact training time for any skill→level transition given an attribute allocation, using SDE-derived skill data (baseTime, primaryAttribute + modifier, secondaryAttribute + modifier).
2. **Multi-epoch optimizer** — partition target skills into sequential "epochs" separated by remaps. Each epoch has its own optimal allocation. The optimizer finds the allocation per epoch that minimizes total wall-clock time from now through all epochs.
3. **Data layer** — parse SDE JSON dump once into a compact `skills.json`; query ESI for live character state (current attributes, skill levels, queue).
4. **CLI interface** — user provides remap info via CLI args; tool outputs phased plan with allocations, skill ordering, and dates.

### Out of Scope (for now)

- GUI / web frontend
- Real-time ESI polling or live progress tracking
- Implant bonuses on effective attributes (per-character, optional)
- Multi-character fleet optimization
- Prerequisite graph between skills (flat priority list first)

## Tech Stack

| Layer | Choice | Rationale |
|-------|--------|-----------|
| Language | **Rust** | Fast computation, strong typing, great CLI ergonomics via clap, native binary distribution |
| CLI | **clap** | Standard Rust CLI framework |
| Skill data | **JSON file** (`assets/skills.json`) | Only ~400 skills × 7 fields each; loads in microseconds with serde |
| HTTP | **reqwest** | Fetch ESI character data |
| Config | **env vars** | ESI token in env, no runtime config files |
| Testing | **cargo test** with proptest | Deterministic, fast |

## Architecture

```
┌───────────────────────────────────────┐
│              CLI (clap)               │
│    optimize --remaps N                │
│             --last-remap-date D       │
│             --character-id ID         │
│             --targets file            │
└──────────────┬────────────────────────┘
               │
┌──────────────▼────────────────────────┐
│          Application Layer            │
│                                       │
│  ┌─────────────────────────────────┐  │
│  │      Multi-Epoch Optimizer       │  │
│  │                                  │  │
│  │  For each epoch (N+1 total):     │  │
│  │    - find best allocation        │  │
│  │    - assign skill group          │  │
│  │    - compute duration            │  │
│  │                                  │  │
│  │  Output: phased plan with dates  │  │
│  └──────────┬──────────────────────┘  │
│             │                         │
│  ┌──────────▼──────────────────────┐  │
│  │      Duration Calculator         │  │
│  └──────────┬──────────────────────┘  │
└─────────────┼─────────────────────────┘
              │
┌─────────────▼─────────────────────────┐
│           Data Layer                  │
│  skills.json   ESI HTTP Client        │
└───────────────────────────────────────┘
```

### Project Structure

```
eve-remap/
├── Cargo.toml
├── src/
│   ├── main.rs           — CLI entrypoint (clap commands)
│   ├── cli.rs             — clap argument definitions
│   ├── calculator.rs      — skill duration formula + bucket builder
│   ├── optimizer.rs       — multi-epoch allocation search
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

### Remap Mechanics (confirmed from CCP support docs)

- **Timed remap**: available every 365 days after last use. Consumed first if both timed and bonus are available.
- **Bonus remaps**: purchased separately, usable anytime alongside the timed cooldown.
- User provides: number of bonus remaps available + date of last remap (to compute when next timed one unlocks).
- ESI does NOT expose the neural interface cooldown or bonus remap count — these come from CLI args.

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

### Multi-Epoch Optimization Strategy

The optimizer partitions target skills into sequential epochs separated by remaps:

```
Epoch 0 (now → remap 1):   allocation A0, skills S0
Epoch 1 (remap 1 → remap2): allocation A1, skills S1
...
Epoch N (remap N → end):    allocation AN, skills SN
```

For each epoch, skills are bucketed by their dominant attribute affinity. The strategy:

1. **Epoch 0**: train skills that already benefit from current attributes (no remap cost yet). These are the "free" skills.
2. **Subsequent epochs**: assign the biggest remaining bucket to the next epoch's optimal allocation. Each epoch's allocation is optimized for that epoch's skill group.

This avoids the combinatorial explosion of trying every possible partition across all allocations. Instead it follows a greedy heuristic grounded in the intuition that you want to maximize throughput within each epoch before paying the remap penalty.

### Bucketed Scoring (within an epoch)

Precompute once per epoch candidate. Each skill transition contributes rawSP to a bucket keyed by `(primaryAttrId, primaryModifier, secondaryAttrId, secondaryModifier)`:

```
For each skill in the epoch:
    rawSP = baseTime × sum(levelMultiplier[currentLevel..desiredLevel-1])
    key   = (skill.primaryAttrId, skill.primaryMod,
             skill.secondaryAttrId, skill.secondaryMod)
    bucket[key] += rawSP
```

Scoring any allocation `(a1..a5)` for this epoch:

```
epochDuration = Σ_keys bucket[k] / (alloc[k.pAttr]^k.pMod) / (alloc[k.sAttr]^k.sMod)
```

O(bucketCount) per allocation — typically ~25 divisions.

### Attribute Allocation Space

Valid remaps distribute points across 5 attributes with constraints:
- Each attribute must be ≥ 1
- Total points depends on character's SP investment (typically 25 base + unallocated SP can buy more, up to 25 per attribute max)
- At N=25: C(24,4) = 12,650 combinations. With actual min/max constraints from character data, typically fewer (~560 realistic combos).

### Brain Size

Brain size determines how many skills can train simultaneously:
- Base brain size grows with total trained skill levels
- Max ~25 concurrent training slots at high brain size
- Within each epoch, skills fill parallel slots before sequencing

## Input Parameters (CLI)

| Parameter | Source | Description |
|-----------|--------|-------------|
| `--character-id` | CLI | ESI character ID |
| `--remaps` | CLI | Number of **bonus** remaps available (timed ones are computed from date) |
| `--last-remap-date` | CLI | Date of last remap; used to compute when next timed remap unlocks (+365 days) |
| `--targets` | CLI | File listing target skills and desired levels |
| ESI token | env var `EVE_ESI_TOKEN` | Authenticated API access |

## Data Flow

1. **SDE Ingestion** (`eve-remap download`)
   - Download SDE zip from CCP → extract relevant JSONL files
   - Parse types.jsonl + typeDogma.jsonl → compact `assets/skills.json`
   - Each record: `{ id, name, baseTime, primaryAttrId, primaryModifier, secondaryAttrId, secondaryModifier }`

2. **Character Fetch** — fetch current state via ESI
   - `/characters/{id}/attributes/` → current attribute values
   - `/characters/{id}/skills/` → trained skill levels, SP totals
   - `/characters/{id}/skillqueue/` → what's currently queued

3. **Optimization Pipeline**
   - Compute remap dates from `--last-remap-date` + 365-day intervals + bonus count
   - Group target skills by dominant attribute affinity
   - Epoch 0: assign skills matching current attributes; find best allocation = current attrs (fixed)
   - Remaining epochs: greedily assign biggest remaining bucket; optimize allocation for that bucket
   - Output phased plan with allocations, skill groups, and projected completion dates

## Implementation Plan

### Phase 1 — Foundation
- Scaffold Rust project with Cargo
- Implement SDE parser: download JSONL → extract skill records → write `skills.json`
- Build `calculator.rs` with the duration formula; test against known values

### Phase 2 — Data Layer
- ESI client: authenticated requests to `/attributes`, `/skills`, `/skillqueue`
- Domain models mapping API responses → internal types
- Character state snapshot combining ESI data + skills.json lookups

### Phase 3 — Multi-Epoch Optimizer
- Remap date computation from user input
- Skill grouping by attribute affinity
- Greedy epoch assignment: current-attrs first, then biggest buckets
- Per-epoch allocation optimization using bucketed scoring
- Output phased plan

### Phase 4 — CLI Polish
- All commands wired up with clap subcommands
- Human-readable output: table per epoch showing allocation, skills, start/end dates
- JSON output format for scripting

## Key Decisions

1. **Rust**: Fast computation for the optimizer's tight loop, native binary distribution, no venv or dependency hell.

2. **JSON file over SQLite**: The data we need per skill is 7 scalar fields (~400 skills). A flat JSON file loads in microseconds with serde — no DB library, no schema migrations, no runtime dependency.

3. **Greedy epoch assignment over exhaustive search**: Trying every possible partition of skills across epochs × allocations would be combinatorially explosive. The greedy heuristic (current-attr skills first, then assign biggest remaining bucket) matches how players actually think about remaps and runs instantly.

4. **Remap info via CLI args**: ESI doesn't expose neural interface cooldown or bonus remap count. User provides `--remaps` (bonus count) and `--last-remap-date`; timed remaps are computed as +365 day intervals.
