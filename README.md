# eve-remap

EVE Online skill queue remap optimizer. Determines the best attribute allocation at each neural interface cooldown to minimize total wall-clock time for training your queued skills.

## Quick Start

```bash
# Offline mode — no authentication needed
echo -e "Gunnery 3\nNavigation 5" > my_queue.txt
cargo run --release -- optimize -q my_queue.txt --attributes 22:17:17:17:17

# With implant bonuses (+5 PER from implants)
cargo run --release -- optimize -q my_queue.txt \
  --attributes 22:17:17:17:17 --implant-bonuses 5:0:0:0:0

# Authenticate and fetch live character data
cargo run --release -- login --browser
cargo run --release -- optimize
```

## Installation

Requires Rust 1.75+ (edition 2021).

```bash
git clone https://github.com/<your>/eve-remap.git
cd eve-remap
cargo build --release
```

The binary is at `target/release/eve-remap`.

## Commands

### optimize

Core command. Outputs a phased plan showing which allocation to set at each epoch, which skills complete, and projected finish times.

| Flag | Description |
|------|-------------|
| `-q FILE`, `--queue FILE` | Path to queue file (see format below) |
| `--attributes PER:MEM:WIL:INT:CHA` | Effective attribute values for offline mode (default: `17:17:17:17:17`) |
| `--implant-bonuses PER:MEM:WIL:INT:CHA` | Implant bonuses that persist across remaps (default: `0:0:0:0:0`) |
| `--bonus-remaps N` | Number of bonus neural interface remaps available (optional) |
| `--json` | Output results as JSON instead of table |

**How it works:** When authenticated, fetches your live character state from ESI (`/skillqueue`, `/attributes`, `/implants`). When using `--queue FILE`, runs entirely offline using the attributes you supply.

If no token is available and no queue file is given, falls back to demo mode using sample SDE skills.

### login

Authenticate with EVE Single Sign-On.

| Flag | Description |
|------|-------------|
| `-t TOKEN` | Paste a JWT bearer token directly (also via `EVE_REMAP_TOKEN` env var) |
| `--sso` | Interactive PKCE flow — opens browser, catches localhost callback (requires port forwarding on WSL) |
| `--browser` | Opens browser for authorization; paste the redirected URL back into terminal. Works cross-platform without port forwarding |

Tokens are stored at `~/.config/eve-remap/accounts.json`. Multiple characters can be logged in simultaneously.

### logout

Remove all stored authentication tokens.

### accounts

List authenticated characters. Use `--verbose` to show token expiry details.

### download

Download and parse latest SDE data into `assets/`. Optionally specify output directory with `-d DIR`. Requires an active ESI token.

### verify

Verify that local asset files (`assets/skills.json`, `assets/implants.json`) are present and valid.

## Queue File Format

One skill per line as `"Skill Name <level>"`:

```
# My training targets
Gunnery 3
Navigation 5
Drone Navigation 2
```

Lines starting with `#` are comments. Blank lines are ignored. Skill names match case-insensitively against the SDE database. Level must be 1–5 (skills at level 5 are skipped).

**Level semantics:** `N` means "train from level N−1 to N". A value of `1` trains from nothing to level 1.

## Attribute Input

The `--attributes` flag takes your **effective** attribute values — i.e., what you see in-game including implant bonuses:

```
--attributes PER:MEM:WIL:INT:CHA
```

If you have implants installed, separate them using `--implant-bonuses` so the optimizer can preserve them across remap epochs:

```bash
# Raw base is all-17; implants give +5 PER, +2 MEM
# Effective = 17+5=22 PER, 17+2=19 MEM
cargo run --release -- optimize -q queue.txt \
  --attributes 22:19:17:17:17 \
  --implant-bonuses 5:2:0:0:0
```

Without `--implant-bonuses`, every post-remap epoch resets to base=17 and loses your implant delta.

## SP Calculation

Uses the authoritative EVE Online cumulative SP table (rank 1):

| Level | Cumulative SP | Incremental SP |
|-------|--------------|----------------|
| L1    | 250          | 250            |
| L2    | 1,414        | 1,164          |
| L3    | 8,000        | 6,586          |
| L4    | 45,255       | 37,255         |
| L5    | 256,000      | 210,745        |

SP for a transition = `(CUMULATIVE[to] − CUMULATIVE[from]) × skillTimeConstant`.

Training rate per second = `(primaryAttribute + secondaryAttribute / 2) / 60`.

Primary attribute points are worth exactly **twice** as much as secondary (`+1 primary ≡ +2 secondary`).

## Remap Constraints

- Base attribute value: **17** (hard floor)
- Free points per remap: **14** distributed above base
- Per-attribute cap: maximum **+10** add (max value = 27)
- Valid allocations: **2,886** distributions
- Timed cooldown: every 365 days; bonus remaps usable anytime

## Output

Table output shows each epoch with effective attributes, skills completing during that period, and projected finish times. JSON output is available via `--json` for scripting.

```
═ Repaired Optimization Plan (313.8d total time)

┌─ Current ──────────────
│ Start: 0s from now
│ Effective:   INT=17 CHA=17 PER=22 MEM=17 WIL=17
│ Skills completing this epoch:
│   • Gunnery [3300] — 5h
│ Projected finish: 5h from now
└──────────────────

┌─ Epoch 1 ──────────────
│ Start: 365.0d from now
│ Effective:   INT=17 CHA=17 PER=32 MEM=18 WIL=20
│ ...
```

## Architecture

```
CLI (clap) → Optimizer → Duration Calculator → Data Layer (JSON + ESI)
```

- **Optimizer**: Greedy best-response per epoch. Epoch 0 fixed to current effective attributes; each subsequent epoch picks the allocation minimizing last-skill finish time across ~2,900 valid distributions.
- **Training model**: Sequential — one skill at a time in queue order. Skills carry SP forward across remaps with no rollback.
- **Data**: Flat JSON assets (`assets/skills.json`, `assets/implants.json`) loaded once at startup. Live character state fetched via authenticated ESI requests.
