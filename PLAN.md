# EVE Remap — Project Plan

## Problem Statement

EVE Online players invest hundreds of hours training skills on their characters. When they use a Neural Interface to remap (reallocate attribute points between Intelligence, Memory, Processing, Perception, Willpower), currently training skills keep their accumulated SP but switch to the new generation rate immediately. Players have a timed remap available every 365 days plus any bonus remaps they've purchased. Active implants add +1 to +5 per slot to specific attributes.

The optimizer should answer:

> Given my character's current state and queued skills — how should I sequence my allocations across remap epochs to minimize total wall-clock time until everything finishes?

Output: phased plan telling the user what allocation to set at each epoch, which skills will finish by then, and projected completion dates.

## Scope

### In Scope (MVP)

1. **Interactive SSO authentication** — CLI opens browser for EVE login, catches callback on localhost, stores tokens locally. Supports multiple characters; prompts to select if none specified via `--character-id`.
2. **Skill duration calculator** — compute exact training time for any skill→level transition given an effective attribute allocation (base + implants), using SDE-derived skill data (baseTime, primaryAttribute + modifier, secondaryAttribute + modifier).
3. **Multi-epoch optimizer** — simulate all target skills training in parallel under each epoch's allocation; find the allocation per epoch that minimizes when the last skill finishes. Skills carry progress forward across epochs with no rollback. Target skills come from ESI `/skillqueue` directly.
4. **Data layer** — parse SDE JSON dump once into a compact `skills.json`; query ESI for live character state (attributes, implant IDs, skill levels, queue).
5. **CLI interface** — minimal args: `eve-remap optimize [--character-id ID]`. Remap config uses defaults (1 bonus remap now, next timed at 365 days).

### Out of Scope (for now)

- GUI / web frontend
- Real-time ESI polling or live progress tracking
- Multi-character fleet optimization (multi-char is only auth/storage support)
- Prerequisite graph between skills (flat priority list first)
- Copy/paste queue input (future feature)

## Tech Stack

| Layer | Choice | Rationale |
|-------|--------|-----------|
| Language | **Rust** | Fast computation, strong typing, great CLI ergonomics via clap, native binary distribution |
| CLI | **clap** | Standard Rust CLI framework |
| Skill data | **JSON file** (`assets/skills.json`) | Only ~400 skills × 7 fields each; loads in microseconds with serde |
| HTTP | **reqwest** | Fetch ESI character data + OAuth token exchange |
| Token storage | **JSON file** (`~/.config/eve-remap/tokens.json`) | Per-character tokens with auto-refresh on expiry (~20 min access, long-lived refresh) |
| Testing | **cargo test** with proptest | Deterministic, fast |

## Architecture

```
┌───────────────────────────────────────┐
│              CLI (clap)               │
│    login                              │
│    optimize [--character-id ID]       │
│    logout                             │
│    accounts list                      │
└──────────────┬────────────────────────┘
               │
┌──────────────▼────────────────────────┐
│          Application Layer            │
│                                       │
│  ┌─────────────────────────────────┐  │
│  │      Multi-Epoch Optimizer       │  │
│  │                                  │  │
│  │  Simulate all queue skills       │  │
│  │  under allocation per epoch.     │  │
│  │  Score = when last skill done.   │  │
│  │  Greedy: fix epoch0=current,     │  │
│  │  then pick each next alloc to    │  │
│  │  minimize projected finish.      │  │
│  └──────────┬──────────────────────┘  │
│             │                         │
│  ┌──────────▼──────────────────────┐  │
│  │      Duration Calculator         │  │
│  │  effectiveAttr = base + implants │  │
│  └──────────┬──────────────────────┘  │
└─────────────┼─────────────────────────┘
              │
┌─────────────▼─────────────────────────┐
│           Data Layer                  │
│                                       │
│  skills.json   ESI HTTP Client        │
│  tokens.json   SSO Auth (PKCE)        │
└───────────────────────────────────────┘
```

### Project Structure

```
eve-remap/
├── Cargo.toml
├── src/
│   ├── main.rs           — CLI entrypoint (clap commands)
│   ├── cli.rs             — clap argument definitions
│   ├── calculator.rs      — duration formula, rate computation, implant resolution
│   ├── optimizer.rs       — multi-epoch allocation search with simulation
│   ├── auth/
│   │   ├── mod.rs         — auth facade
│   │   ├── sso.rs         — EVE SSO OAuth2 PKCE flow (authorize, token exchange)
│   │   └── store.rs       — local token persistence (~/.config/eve-remap/tokens.json)
│   ├── data/
│   │   ├── mod.rs         — data layer facade
│   │   ├── sde.rs         — SDE JSONL → skills.json parser
│   │   ├── esi.rs         — ESI client (reqwest wrapper, auto-refreshes tokens)
│   │   └── models.rs      — shared domain types
│   └── config.rs          — app paths, SSO client ID config
├── assets/
│   ├── skills.json        — pre-parsed skill data (~400 entries, ~150KB)
│   └── implants.json      — implant type → attribute bonus mapping
├── tests/
│   ├── calculator_test.rs — duration formula against known values
│   └── optimizer_test.rs  — small character scenarios
```

## Domain Model

### Remap Mechanics (confirmed from CCP support docs)

- **Timed remap**: available every 365 days after last use. Consumed first if both timed and bonus are available.
- **Bonus remaps**: purchased separately, usable anytime alongside the timed cooldown.
- **No SP rollback**: actively training skills keep their accumulated SP and immediately switch to the new rate. Only future SP generation is affected.
- **Default assumption**: 1 bonus remap available now; next timed remap at 365 days from today. No CLI args needed for remap configuration.
- ESI does NOT expose the neural interface cooldown or bonus remap count — defaults apply unless user customizes later.

### Skill Duration Formula

From EVE mechanics, each skill has a **primary** and **secondary** attribute:

```
duration(skill, level) = baseTime × levelMultiplier[level]
    / (effectivePrimaryAttrValue ^ primaryModifier)
    / (effectiveSecondaryAttrValue ^ secondaryModifier)

levelMultiplier = [1, 4, 20, 80, 360]  // for levels 1-5
```

Where per skill:
- `baseTime` — base training time in seconds
- `primaryAttrId` / `primaryModifier` — governing attribute and its exponent
- `secondaryAttrId` / `secondaryModifier` — secondary attribute and its exponent
- `effectiveAttrValue = baseRemappedValue + sum(implantBonuses for that attr)`

### Effective Attributes

ESI returns data separately:
- `/characters/{id}/attributes/` → base remapped values (what you set with your neural interface), keys: `intelligence`, `memory`, `processing`, `perception`, `willpower`
- `/characters/{id}/implants/` → list of active implant type IDs; cross-reference SDE to get which attribute each boosts and by how much (+1 to +5 per slot)
- Effective value used in duration formula = base + total implant bonus per attribute

SDE needs to also store implant→attribute bonus mapping. This means `skills.json` gets a companion `implants.json`: `{ typeId, attributeName, bonus }`.

### Multi-Epoch Optimization (no rollback model)

Because skills keep their SP across remaps, the optimizer simulates all target skills training continuously through sequential epochs:

```
Epoch 0 (now → 365 days):        allocation A0 (= current attrs, fixed)
Epoch 1 (365 days → end):         allocation A1
```

Each skill tracks remaining time toward its target level. At each remap boundary, rates switch but progress carries forward. The score is **wall-clock time until the last unfinished skill completes**.

**Greedy strategy:**
1. Epoch 0 uses current attributes — no reason to waste a remap immediately.
2. For epoch 1, simulate forward under every candidate allocation; pick the one that minimizes projected finish time of the slowest remaining skill.

This avoids exhaustive search over `allocations^epochs` by greedily optimizing each step. With ~4 max epochs and ~560-12K allocations per greedy pass, it runs in milliseconds.

Brain size (~25 concurrent slots) means many skills train simultaneously within each epoch. The simulator advances them all at once: each skill progresses independently at its own rate under the current allocation.

### Attribute Allocation Space

Valid remaps distribute points across 5 attributes with constraints:
- Each attribute must be ≥ 1
- Total points depends on character's SP investment (typically 25 base + unallocated SP can buy more, up to 25 per attribute max)
- At N=25: C(24,4) = 12,650 combinations. With actual min/max constraints from character data, typically fewer (~560 realistic combos).

### SSO Authentication & Token Lifecycle

OAuth 2.0 Authorization Code flow with PKCE — no client secret needed. Requires a registered app on [developers.eveonline.com](https://developers.eveonline.com/applications/) with a `http://127.0.0.1/callback` redirect URI.

**Flow:**
1. CLI spins up local HTTP listener on `127.0.0.1:<port>` (random available port).
2. Opens browser to `login.eveonline.com/v2/oauth/authorize?code_challenge=...&redirect_uri=http://127.0.0.1:<port>/callback`.
3. User logs into EVE, selects character, consents to scopes.
4. EVE redirects back to localhost callback with authorization code.
5. CLI exchanges the code for access token + refresh token at `login.eveonline.com/v2/oauth/token`.
6. Access token is a JWT containing `owner_character_id` and `character_owner_email` — gives us the character ID automatically.
7. Store `{ characterId, characterName, accessToken, refreshToken, expiresAt }` in `~/.config/eve-remap/tokens.json`.
8. On subsequent runs, check if stored access token is still valid; auto-refresh via POST to `/token` with `grant_type=refresh_token` if expired (~20 min lifetime). Refresh tokens are long-lived unless revoked by user.

**Scopes requested:**
- `esi-skills.read_skills.v1` — current skill levels and SP totals
- `esi-skills.read_skillqueue.v1` — queued skills and training progress
- `esi-clones.read_clones.v1` — home clone location (optional context)

### Multi-Character Support

Token store maps character ID → credentials. When multiple characters are logged in:

| Scenario | Behavior |
|----------|----------|
| `--character-id` specified | Use that character directly; error if not found in store |
| No flag, single character in store | Use it silently |
| No flag, multiple characters in store | Print numbered list, prompt for selection |
| No characters in store | Run `login` flow first, then proceed |

## Input Parameters (CLI)

No CLI args needed for remap configuration — defaults apply: 1 bonus remap now, next timed at 365 days from today.

| Parameter | Source | Description |
|-----------|--------|-------------|
| `--character-id` | CLI (optional) | Override auto-selection; must match a previously authorized character |

No `--targets` flag — the optimizer reads the current skill queue directly from ESI `/characters/{id}/skillqueue/`. If users want skills beyond their queue accounted for, they add them in-game first and re-run.

## Data Flow

1. **SDE Ingestion** (`eve-remap download`)
   - Download SDE zip from CCP → extract relevant JSONL files
   - Parse types.jsonl + typeDogma.jsonl → compact `assets/skills.json`
     - Each record: `{ id, name, baseTime, primaryAttrId, primaryModifier, secondaryAttrId, secondaryModifier }`
   - Parse implant bonus data → `assets/implants.json`
     - Each record: `{ typeId, attributeName, bonus }`

2. **Authentication** (`eve-remap login`, or auto on first use)
   - PKCE flow: open browser → user authorizes → catch localhost callback → exchange code for tokens
   - Store tokens locally with expiry tracking
   - On subsequent runs: verify token validity via JWT inspection or `/oauth/verify`; refresh if expired

3. **Character Fetch** — fetch current state via ESI (auto-refreshes token if needed)
   - `/characters/{id}/attributes/` → base attribute values
   - `/characters/{id}/skills/` → trained skill levels, SP totals
   - `/characters/{id}/skillqueue/` → target skills and their training progress
   - `/characters/{id}/implants/` → active implant IDs → resolve to effective attributes

4. **Optimization Pipeline**
   - Compute epoch boundaries from today + 365-day interval
   - Resolve effective attributes = base + implants
   - Epoch 0 fixed to current effective attrs; simulate all queue skills forward
   - For epoch 1: greedy best-response allocation minimizing projected finish of bottleneck skill
   - Output phased plan with allocations, per-skill completion dates, and total duration

## Implementation Plan

### Phase 1 — Foundation
- Scaffold Rust project with Cargo
- Implement SDE parser: download JSONL → extract skill records + implant bonuses → write `skills.json` and `implants.json`
- Build `calculator.rs` with the duration formula; test against known values

### Phase 2 — Auth & Data Layer
- SSO module: PKCE flow (code verifier/challenge generation, browser open, localhost callback server, token exchange)
- Token store: persistent JSON file at `~/.config/eve-remap/tokens.json`; JWT expiry checking; refresh logic
- ESI client: authenticated requests to `/attributes`, `/skills`, `/skillqueue`, `/implants`; auto-refresh on 401
- Domain models mapping API responses → internal types
- Character state snapshot combining ESI data + assets lookups (effective attributes)

### Phase 3 — Multi-Epoch Optimizer
- Simulation engine: advance all skills through epochs at varying rates
- Greedy allocation search per epoch (minimize last-skill finish time)
- Output phased plan

### Phase 4 — CLI Polish
- All commands wired up with clap subcommands (`login`, `logout`, `accounts`, `download`, `optimize`)
- Human-readable output: table per epoch showing allocation, which skills complete, start/end dates
- JSON output format for scripting

## Key Decisions

1. **Rust**: Fast computation for the optimizer's tight loop, native binary distribution, no venv or dependency hell.

2. **JSON files over SQLite**: Skill and implant data are ~400+ entries × 7 fields each. Flat JSON loads in microseconds with serde — no DB library needed.

3. **Greedy epoch optimization over exhaustive search**: With N~4 max epochs and up to 12K allocations, exhaustive `allocations^epochs` is impossible. Greedy best-response per epoch runs instantly and produces near-optimal results because each epoch independently accelerates all remaining skills.

4. **Remap info via defaults**: Default assumption of 1 bonus remap now + next timed at 365 days from today. No CLI args needed for remap configuration unless user wants to customize.

5. **SSO login via PKCE**: No client secret to manage locally. Registered app on developers.eveonline.com with `http://127.0.0.1/callback` redirect URI. Local HTTP server catches the callback; tokens stored persistently with auto-refresh.

6. **Queue from ESI, not file input**: Target skills come directly from `/skillqueue/`. If users want different targets, they adjust their queue in-game first. Keeps the tool focused on optimization, not queue management.
