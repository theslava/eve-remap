# eve-remap

EVE Online skill queue remap optimizer. Determines the best attribute allocation at each neural interface cooldown to minimize total wall-clock time for training your queued skills.

## Quick Start

### Offline Mode (queue file)

```bash
# Basic usage with a queue file
echo -e "Gunnery 3\nNavigation 5" > my_queue.txt
cargo run --release -- optimize -q my_queue.txt --attributes 27:22:17:17:16

# Read from stdin instead of a file
echo "Gunnery 3" | cargo run --release -- optimize -q - --attributes 27:22:17:17:16

# Write optimized order to stdout
cargo run --release -- optimize -q my_queue.txt --queue-out -
```

### ESI Mode (live character data)

```bash
# Authenticate once
cargo run --release -- login

# Fetch queue and attributes from EVE, then optimize
cargo run --release -- optimize --character "Your Character Name"

# Override specific attributes while keeping ESI queue
cargo run --release -- optimize --character "Your Character Name" \
  --implant-bonuses 5:0:0:0:0
```

## Example Output

Running the optimizer produces a phased plan showing attribute allocations, training durations, SP summaries, and completed skills per epoch:

```text
========================================================================
REMAP OPTIMIZATION PLAN
------------------------------------------------------------------------

Epoch 1: Initial allocation
  Attributes: PER=27 MEM=22 WIL=17 INT=16 CHA=16
  Duration: 45 days 8 hours (45.3 days)
  Pri        -     2.1M       -       -       -
    - Gunnery 3 - 45 days 8 hours

Epoch 2: Remap
  Attributes: PER=27 MEM=22 WIL=17 INT=16 CHA=16
  Duration: 12 days 3 hours (12.1 days)
    - Navigation 2 - 12 days 3 hours

------------------------------------------------------------------------
Total training time: 57.4 days
Epochs: 2
```

## Installation

Requires Rust 1.75+ (edition 2021). No system dependencies — uses rustls for TLS without OpenSSL.

```bash
git clone <repo-url>
cd eve-remap
cargo build --release
```

The binary ends up at `target/release/eve-remap`. Or install locally:

```bash
cargo install --path .
```

This installs the `eve-remap` binary to `$CARGO_HOME/bin` (typically `~/.cargo/bin`).

## Commands

### optimize

Outputs a phased plan showing which allocation to set at each epoch, which skills complete, and projected finish times. Accepts input from a queue file (`--queue`) or live ESI data (`--character`).

| Flag | Description |
|---|---|
| `-q FILE`, `--queue FILE` | Path to queue file (optional if `--character` used). Use `-` for stdin |
| `--character NAME` | Fetch queue/attributes from ESI for the named character (requires `login`) |
| `--attributes PER:MEM:WIL:INT:CHA` | Base remapped attribute values excluding implants (default: `17:17:17:17:17`). Overridden by ESI when `--character` is set unless explicitly provided |
| `--implant-bonuses PER:MEM:WIL:INT:CHA` | Implant bonuses persisting across remaps (default: `0:0:0:0:0`). Overrides ESI implant lookup |
| `--bonus-remaps N` | Number of bonus neural interface remaps available (optional — unlimited timed epochs if omitted) |
| `--remap-available Dd` | When the normal remap cooldown expires (e.g., `0d` = now, `30d` = in 30 days; default: `0d`) |
| `--json` | Output results as JSON instead of table |
| `--queue-out FILE` | Write optimized skill order to a file. Use `-` for stdout |

### login / logout / accounts

Manage EVE Online SSO authentication tokens stored locally:

```bash
# Authenticate with EVE SSO (opens browser for PKCE flow)
cargo run --release -- login

# List saved characters and token status
cargo run --release -- accounts

# Remove a saved character
cargo run --release -- logout "Character Name"
```

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

### Partial Progress

For skills already being trained, include remaining time or SP earned:

| Syntax | Meaning |
|---|---|
| `"SkillName 3 @7d"` | Target L3, 7 days of training already completed |
| `"SkillName 3 @12345 SP"` | Target L3, 12345 SP already earned |

ESI-fetched queues use the SP-trained format automatically, preserving exact partial progress from the API.

## Attribute Resolution

Attributes and implant bonuses follow a single priority chain via `resolve_attributes()`:

| Source | Priority |
|---|---|
| Base attributes | CLI `--attributes` → ESI `/characters/me/attributes/` (back-calculated) → default `17:17:17:17:17` |
| Implant bonuses | CLI `--implant-bonuses` → ESI active implant IDs → local SDE lookup → `0:0:0:0:0` |

The optimizer receives only resolved effective values — raw implant IDs never pass downstream, preventing double-counting bugs.

### Bonus Remaps and Cooldown Delay

Use `--bonus-remaps` to limit extra neural interface uses, and `--remap-available` to specify when your normal cooldown expires:

```bash
# With bonus remaps and delayed normal cooldown
cargo run --release -- optimize -q queue.txt \
  --attributes 27:22:17:17:16 \
  --bonus-remaps 2 \
  --remap-available 90d
```
