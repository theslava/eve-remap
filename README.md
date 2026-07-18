# eve-remap

EVE Online skill queue remap optimizer. Determines the best attribute allocation at each neural interface cooldown to minimize total wall-clock time for training your queued skills.

## Quick Start

```bash
# Basic usage with a queue file
echo -e "Gunnery 3\nNavigation 5" > my_queue.txt
cargo run --release -- optimize -q my_queue.txt --attributes 27:22:17:17:16

# Read from stdin instead of a file
echo "Gunnery 3" | cargo run --release -- optimize -q - --attributes 27:22:17:17:16

# Write optimized order to stdout
cargo run --release -- optimize -q my_queue.txt --queue-out -
```

## Installation

Requires Rust 1.75+ (edition 2021). Zero system dependencies — no OpenSSL, pkg-config, or other C libraries needed.

```bash
git clone https://github.com/<your>/eve-remap.git
cd eve-remap
cargo build --release
```

The binary is at `target/release/eve-remap`.

## Commands

### optimize

Single command. Outputs a phased plan showing which allocation to set at each epoch, which skills complete, and projected finish times.

| Flag | Description |
|---|---|
| `-q FILE`, `--queue FILE` | Path to queue file (required). Use `-` to read from stdin |
| `--attributes PER:MEM:WIL:INT:CHA` | Base remapped attribute values excluding implants (default: `17:17:17:17:17`) |
| `--implant-bonuses PER:MEM:WIL:INT:CHA` | Implant bonuses that persist across remaps (default: `0:0:0:0:0`) |
| `--bonus-remaps N` | Number of bonus neural interface remaps available (optional) |
| `--remap-available Dd` | When the normal remap cooldown expires (e.g., `0d` = now, `30d` = in 30 days; default: `0d`) |
| `--json` | Output results as JSON instead of table |
| `--queue-out FILE` | Write optimized skill order to a file. Use `-` for stdout |

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

The `--attributes` flag takes your **base** attribute values — what you set on the neural interface, excluding implants:

```
--attributes PER:MEM:WIL:INT:CHA
```

Implant bonuses are separate. Use `--implant-bonuses` so the optimizer can preserve them across remap epochs:

```bash
# Neural interface sets PER=22, MEM=17; implants give +5 PER, +2 MEM
# Effective = base + implants → PER=27, MEM=19
cargo run --release -- optimize -q queue.txt \
  --attributes 22:17:17:17:17 \
  --implant-bonuses 5:2:0:0:0
```

Without `--implant-bonuses`, every post-remap epoch resets to base=17 and loses your implant delta.</parameter>
</function>
</tool_call>
<tool_call>
<function=read>
<parameter=i>
find PLAN.md attributes references

## SP Calculation

Uses the authoritative EVE Online cumulative SP table (rank 1):

| Level | Cumulative SP | Incremental SP |
|---|---|---|
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
- Valid allocations: **2,885** distributions
- Timed cooldown: every 365 days; bonus remaps usable anytime

## Architecture

```
CLI (clap) -> Optimizer -> Duration Calculator -> Data Layer (JSON)
```

- **Optimizer**: Greedy best-response per epoch. Epoch 0 fixed to current effective attributes; each subsequent epoch picks the allocation minimizing last-skill finish time across 2,885 valid distributions.
- **Training model**: Sequential — one skill at a time in queue order. Skills carry SP forward across remaps with no rollback.
- **Data**: Flat JSON assets (`assets/skills.json`, `assets/implants.json`) loaded once at startup.
