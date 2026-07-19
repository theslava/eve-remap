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

### Example Output

Running the optimizer produces a phased plan showing attribute allocations, training durations, SP summaries, and completed skills per epoch. Here's what output looks like:

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

Requires Rust 1.75+ (edition 2021). Zero system dependencies — no OpenSSL, pkg-config, or other C libraries needed.

```bash
git clone <repo-url>
cd eve-remap
cargo build --release
```

The binary ends up at `target/release/eve-remap`.

Alternatively, install it locally with cargo:

```bash
cargo install --path .
```

This installs the `eve-remap` binary to `$CARGO_HOME/bin` (typically `~/.cargo/bin`).

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

Without `--implant-bonuses`, every post-remap epoch resets to base=17 and loses your implant delta.

### Bonus Remaps and Cooldown Delay

Use `--bonus-remaps` to limit extra neural interface uses, and `--remap-available` to specify when your normal cooldown expires:

```bash
# With bonus remaps and delayed normal cooldown
cargo run --release -- optimize -q queue.txt \
  --attributes 27:22:17:17:16 \
  --bonus-remaps 2 \
  --remap-available 90d
```
