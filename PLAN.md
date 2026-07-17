# EVE Remap — Project Plan

## Problem Statement

EVE Online players invest hundreds of hours training skills on their characters. When they use a Neural Interface to remap (reallocate attribute points between Intelligence, Charisma, Perception, Memory, Willpower), currently training skills keep their accumulated SP but switch to the new generation rate immediately. Players have a timed remap available every 365 days plus any bonus remaps they've purchased. Active implants add +1 to +5 per slot to specific attributes.

The optimizer should answer:

> Given my character's current state and queued skills — how should I sequence my allocations across remap epochs to minimize total wall-clock time until everything finishes?

Output: phased plan telling the user what allocation to set at each epoch, which skills will finish by then, and projected completion dates.

## Scope

### In Scope (MVP)

1. **Interactive SSO authentication** — CLI opens browser for EVE login via implicit grant flow (works cross-platform including WSL). Supports multiple characters; token persistence with JWT introspection. Also supports pasting tokens directly (`login -t TOKEN`).
2. **Skill duration calculator** — compute exact training time for any skill-to-level transition given effective attributes and SDE-derived skill data. See [Skill Duration Formula](#skill-duration-formula).
3. **Multi-epoch optimizer** — simulate target skills training sequentially (one at a time, in queue order) under each epoch's allocation; find the allocation per epoch that minimizes total wall-clock time until everything finishes. Skills carry progress forward across epochs with no rollback. Lower skill levels must complete before higher ones of the same skill (e.g., Gunnery 1 before Gunnery 2), but cross-skill prerequisites are ignored for now. Target skills can come from ESI `/skillqueue` or a local queue file (`--queue FILE`).
4. **Data layer** — flat JSON assets (`assets/skills.json`, `assets/implants.json`) loaded once at startup; query ESI for live character state (attributes, implant IDs, skill levels, queue) when authenticated.
5. **CLI interface** — clap derive subcommands: `login`, `logout`, `accounts`, `download`, `verify`, `optimize`. Offline mode via `--queue FILE`, `--attributes PER:MEM:WIL:INT:CHA`, and `--implant-bonuses PER:MEM:WIL:INT:CHA` requires no authentication.

### Out of Scope (for now)

- GUI / web frontend
- Real-time ESI polling or live progress tracking
- Multi-character fleet optimization (multi-char is only auth/storage support)
- Prerequisite graph between skills (flat priority list first)

## Tech Stack

| Layer | Choice | Rationale |
|-------|--------|-----------|
| Language | **Rust 2021 edition** | Fast computation, strong typing, great CLI ergonomics via clap, native binary distribution. Edition pinned to 2021 for Rust 1.75 compatibility. |
| CLI | **clap (derive)** | Auto-generates help text, env var integration, consistent subcommand structure |
| Skill data | **JSON file** (`assets/skills.json`) | ~400 skills × 7 fields loads in microseconds with serde; no runtime DB dependency |
| HTTP | **reqwest + tokio** | Fetch ESI character data + OAuth token exchange |
| Token storage | **JSON file** (`~/.config/eve-remap/accounts.json`) | Per-character tokens with expiry tracking; legacy `tokens.json` still supported |
| Testing | **cargo test** | Deterministic unit tests across calculator, optimizer, ESI parsing, and auth modules |

## Architecture

```
┌───────────────────────────────────────┐
│              CLI (clap)               │
│    login [-t TOKEN] [--sso] [--browser]│
│    optimize [-q FILE] [--attributes A]│
│    logout                             │
│    accounts [--verbose]                │
│    download                           │
│    verify                             │
└──────────────┬────────────────────────┘
               │
┌──────────────▼────────────────────────┐
│          Application Layer            │
│                                       │
│  ┌─────────────────────────────────┐  │
│  │      Multi-Epoch Optimizer       │  │
│  │                                  │
│  │  Simulate skills one-by-one in   │
│  │  queue order under each epoch's  │
│  │  allocation. Score = when last   │
│  │  skill completes. Greedy: fix    │
│  │  epoch0=current, then pick each  │
│  │  next alloc to minimize finish.  │
│  └──────────┬──────────────────────┘  │
│             │                         │
│  ┌──────────▼──────────────────────┐  │
│  │      Duration Calculator         │  │
│  │  SP = cumulative delta × STC    │  │
│  │  rate = (P + S/2) / 60 SP/s     │  │
│  └──────────┬──────────────────────┘  │
└─────────────┼─────────────────────────┘
              │
┌─────────────▼─────────────────────────┐
│           Data Layer                  │
│                                       │
│  skills.json   ESI HTTP Client        │
│  implants.json JWT introspection      │
│  accounts.json PKCE / implicit grant  │
└───────────────────────────────────────┘
```

### Project Structure

```
eve-remap/
├── Cargo.toml
├── src/
│   ├── main.rs           — CLI entrypoint, command dispatch, output formatters
│   ├── cli.rs            — clap derive argument definitions
│   ├── calculator.rs     — SP formula, rate computation, duration helpers
│   ├── optimizer.rs      — multi-epoch allocation search with simulation engine
│   ├── auth/
│   │   ├── mod.rs        — auth facade: JWT decode, account store, token management
│   │   └── sso.rs        — EVE SSO flows (PKCE + implicit grant browser login)
│   └── data/
│       ├── mod.rs        — data layer facade (load_skills, load_implants)
│       ├── models.rs     — shared domain types (SkillRecord, CharacterState, etc.)
│       ├── esi.rs        — ESI client (reqwest wrapper, character state fetching)
│       └── sde.rs        — SDE JSONL parser: extract_skills(), extract_implants(), download_sde()
├── assets/
│   ├── skills.json       — pre-parsed skill data (~400 entries from SDE)
│   └── implants.json     — implant type → attribute bonus mapping
```

## Domain Model

### Remap Mechanics (confirmed from CCP support docs)

- **Timed remap**: available every 365 days after last use. Consumed first if both timed and bonus are available.
- **No SP rollback**: actively training skills keep their accumulated SP and immediately switch to the new rate. Only future SP generation is affected.
- **Bonus remaps**: Optional — if `--bonus-remaps N` is not specified, the optimizer runs unlimited timed epochs until the queue empties. Configurable via `--bonus-remaps N`.
- ESI does NOT expose the neural interface cooldown or bonus remap count — defaults apply unless user customizes via CLI.

### Skill Duration Formula
From EVE mechanics, each skill has a **primary** and **secondary** attribute. SP costs use an authoritative cumulative table (rank 1):

| Level | Cumulative SP | Incremental SP |
|-------|--------------|----------------|
| L1    | 250          | 250            |
| L2    | 1,414        | 1,164          |
| L3    | 8,000        | 6,586          |
| L4    | 45,255       | 37,255         |
| L5    | 256,000      | 210,745        |

```
SP for transition = (CUMULATIVE[to_level] - CUMULATIVE[from_level]) × skillTimeConstant
rate_per_second   = (effectivePrimaryAttr + effectiveSecondaryAttr / 2.0) / 60.0
duration_seconds  = SP_for_transition / rate_per_second
```

Where per skill:
- `skillTimeConstant` — multiplier from SDE type data (typically 1.0–4.0)
- `primaryAttribute` / `secondaryAttribute` — governing attributes by name
- `effectiveAttrValue = baseRemappedValue + sum(implantBonuses for that attr)`
### Effective Attributes

ESI returns data separately:
- `/characters/{id}/attributes/` → base remapped values (what you set with your neural interface), keys: `intelligence`, `charisma`, `perception`, `memory`, `willpower`
- `/characters/{id}/implants/` → list of active implant type IDs; cross-reference SDE to get which attribute each boosts and by how much (+1 to +5 per slot)
- Effective value used in duration formula = base + total implant bonus per attribute

For offline mode (`--queue FILE`), effective attributes default to the base values provided via `--attributes`.

SDE needs to also store implant→attribute bonus mapping. This means `skills.json` gets a companion `implants.json`: `{ typeId, attributeName, bonus }`.

### Multi-Epoch Optimization (no rollback model)

Skills train **sequentially** in queue order — only one skill earns SP at any given moment. On completion, the next queued skill starts. Skills keep their SP across remaps; only the future training rate changes. Lower levels of a skill must complete before higher ones (Gunnery 1 → Gunnery 2), but cross-skill prerequisites are ignored for now.

Primary attribute points are worth exactly **twice** as much as secondary (`+1 primary = +2 secondary`), because the rate formula is linear.

#### Greedy Best-Response per Epoch

The optimizer uses a greedy approach: epoch 0 is fixed to current effective attributes, then each subsequent epoch picks the allocation that minimizes the projected finish time of the last queued skill. This avoids exhaustive search over `allocations^epochs`.

Precomputed time caches store training durations for every skill-allocation pair, enabling fast evaluation during the greedy search. Suffix sums accelerate multi-skill projections under each candidate allocation.

#### Allocation Space

A remap distributes **14** free points above a hard floor of **17** across 5 attributes. Per-attribute cap is **+10** (max value = 27). This yields **2,885 valid distributions**. No single-attribute dump is possible since 14 > 10, so every allocation touches at least 2 attributes. Distribution by boosted attribute count: 2 attrs (70), 3 attrs (690), 4 attrs (1,410), 5 attrs (715).


### SSO Authentication & Token Lifecycle

Two login modes are supported:

**PKCE flow** (`--sso`): OAuth 2.0 Authorization Code flow with PKCE — no client secret needed. CLI spins up a local HTTP listener on `127.0.0.1:<port>`, opens browser for authorization, catches callback, exchanges code for tokens. Requires port forwarding on WSL.

**Implicit grant / browser mode** (`--browser`): Opens browser with implicit grant URL using `https://127.0.0.1/callback` redirect URI; user pastes the redirected URL back into the terminal. Works cross-platform without port forwarding.

Requires a registered app on [developers.eveonline.com](https://developers.eveonline.com/applications/).

**JWT introspection**: Access tokens are JWTs containing real claims:
- `sub`: `"CHARACTER:EVE:<id>"` (character ID extracted from this)
- `scp`: array of granted scope strings
- `name`: character name
- `exp`: Unix timestamp expiry

Token store maps character ID → credentials. When multiple characters are logged in:

| Scenario | Behavior |
|----------|----------|
| No flag, single character in store | Use it silently |
| No flag, multiple characters in store | Pick first valid token (multi-select not yet implemented) |
| No characters in store | Run `login` flow first, then proceed |

## Input Parameters (CLI)

| Command | Flags | Description |
|---------|-------|-------------|
| `optimize` | `-q FILE`, `--queue FILE` | Path to queue file with target skills (one per line as "Skill Name \<level>") |
| `optimize` | `--attributes PER:MEM:WIL:INT:CHA` | Effective attribute values including implants (default: 17:17:17:17:17) |
| `optimize` | `--implant-bonuses PER:MEM:WIL:INT:CHA` | Implant bonuses persisting across remaps (default: 0:0:0:0:0) |
| `optimize` | `--bonus-remaps N` | Number of bonus neural interface remaps (optional — unlimited timed epochs if omitted) |
| `optimize` | `--json` | Output results as JSON instead of human-readable table |
| `optimize` | `--remap-available Dd` | When normal remap cooldown expires, e.g. `0d` = now, `30d` = in 30 days (default: 0d) |
| `login` | `-t TOKEN` / `EVE_REMAP_TOKEN` env | Paste a JWT bearer token directly |
| `login` | `--browser` | Open browser for authorization (implicit grant, cross-platform) |
| `login` | `--sso` | PKCE server-based SSO flow (requires port forwarding on WSL) |
| `accounts` | `--verbose` | Show token expiry details |

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

### Offline Mode (`--queue FILE`)

1. Parse queue file into target skills and levels.
2. Look up each skill in `assets/skills.json` to get time constant and attributes.
3. Compute SP needed per transition using cumulative table lookup × skillTimeConstant.
4. Build character state from `--attributes` values. If `--implant-bonuses` is provided, those deltas are preserved across post-remap epochs.
5. Run multi-epoch optimizer — output phased plan.

### Online Mode (ESI authentication)

1. **Authentication** (`eve-remap login`, or auto on first use)
   - PKCE flow: open browser → user authorizes → catch localhost callback → exchange code for tokens
   - Or implicit grant: open browser → paste redirected URL back → extract token from fragment
   - Store tokens locally with expiry tracking
   - On subsequent runs: verify token validity via JWT inspection; refresh if expired

2. **Character Fetch** — fetch current state via ESI (auto-refreshes token if needed)
   - `/characters/{id}/attributes/` → base attribute values
   - `/characters/{id}/skills/` → trained skill levels, SP totals
   - `/characters/{id}/skillqueue/` → target skills and their training progress
   - `/characters/{id}/implants/` → active implant IDs → resolve to effective attributes

3. **Optimization Pipeline**
   - Resolve effective attributes = base + implants
   - Epoch 0 fixed to current effective attrs; simulate all queue skills forward
   - For epoch N: greedy best-response allocation minimizing projected finish of bottleneck skill
   - Output phased plan with allocations, per-skill completion dates, and total duration

## Implementation Status

### Phase 1 — Foundation ✅
- [x] Rust project scaffolded with Cargo, edition 2021
- [x] `calculator.rs` with correct duration formula using cumulative SP table lookup
- [x] SDE asset files (`assets/skills.json`, `assets/implants.json`) present in repo
- [x] Calculator tests covering SP rate, cumulative table, and duration helpers

### Phase 2 — Auth & Data Layer ✅
- [x] JWT introspection: decode payload extracting character ID from `sub` field
- [x] Account store at `~/.config/eve-remap/accounts.json` (multi-character support)
- [x] ESI client: authenticated requests to `/attributes`, `/skills`, `/skillqueue`, `/implants`
- [x] Domain models mapping API responses → internal types in `data/models.rs`
- [x] Character state snapshot combining ESI data + assets lookups
- [x] SDE JSONL parser (`sde.rs`): extract_skills(), extract_implants() from raw CCP data files
- [ ] Token refresh via `/oauth/token` endpoint (placeholder pending implementation)

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
- [x] PKCE SSO flow (`--sso`) and implicit grant browser login (`--browser`)
- [x] Queue file input (`--queue FILE`) with offline mode (`--attributes`, `--implant-bonuses`)
- [x] Graceful fallback when no token available suggests using `--queue`

### Phase 5 — Remaining Work
- [ ] Token refresh implementation: wire actual `/oauth/token` refresh call on expiry
- [ ] Auto-detect expired tokens on optimize run and prompt re-authentication
- [ ] Colored terminal output and progress bars during optimization
- [ ] Save/load optimization plans to/from files
- [ ] Multi-select character prompt when multiple accounts in store

## Key Decisions

1. **Rust**: Fast computation for the optimizer's tight loop, native binary distribution, no venv or dependency hell. Edition pinned to 2021 for Rust 1.75 compatibility on WSL.

2. **JSON files over SQLite**: Skill and implant data are ~400+ entries x 7 fields each. Flat JSON loads in microseconds with serde — no DB library needed.

3. **Greedy epoch optimization over exhaustive search**: With N~4 max epochs and up to 2,885 allocations per epoch, exhaustive `allocations^epochs` is impossible. Greedy best-response per epoch runs instantly and produces near-optimal results because each epoch independently accelerates all remaining skills.

4. **Remap info via CLI args**: ESI doesn't expose neural interface cooldown or bonus remap count; user provides `--bonus-remaps N` and `--remap-available Dd`. Optional — if omitted, optimizer runs unlimited timed epochs until queue empties.

5. **Queue from file OR ESI**: Target skills can come from `--queue FILE` (offline, copy/paste friendly) or fetched live from ESI `/skillqueue/` when authenticated. This removes the authentication barrier for initial use and testing.

6. **Browser login over PKCE for WSL**: Implicit grant flow avoids port forwarding issues between Windows host and WSL guest where `http://localhost:<port>` isn't routable. PKCE (`--sso`) still available as an option.

7. **Cumulative SP table over multiplier formula**: Authoritative values from EVE Online forums archive: L1=250, L2=1414, L3=8000, L4=45255, L5=256000. SP for transition = `(CUMULATIVE[to] - CUMULATIVE[from]) * STC`. This replaced the incorrect LEVEL_MULTIPLIERS*BASE_SP approach.

8. **EsIClient uses immutable tokens**: Replaced `Arc<Mutex<String>>` with plain `String` to avoid holding locks across `.await` points, eliminating deadlock risk in async context.
