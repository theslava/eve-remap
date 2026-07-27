# Clippy Fix Plan

Generated from `cargo clippy --all-targets` on 2026-07-27 (Rust 1.97).

## Error (must fix — blocks compilation)

### 1. `erasing_op` — `src/optimizer.rs:484`

```rust
let mut best_after = suffix_sum[0 * (n + 1) + cut];
```

`0 * (n + 1)` always evaluates to `0`. This is intentional (row 0 of the table), but clippy denies it by default because it usually masks a bug.

**Fix**: Replace `0 * (n + 1) + cut` with just `cut`, or add a comment explaining the intent and use `_ *` style if the pattern repeats. Since this is explicitly indexing row 0, simplify to `suffix_sum[cut]`.

---

## Warnings by file

### `src/auth.rs`

| # | Lint | Line | Description | Fix |
|---|------|------|-------------|-----|
| 2 | `unnecessary_fallible_conversions` | 136 | `PrivateKeyDer::try_from(PrivatePkcs8KeyDer::from(...))` — conversion cannot fail | Use `.into()` or `From::from()` instead; remove `.context(...)?` unwrap chain |
| 3 | `double_ended_iterator_last` | 304 | `.split(':').last()` iterates entire string iterator | Replace with `.next_back()` (O(1) on `DoubleEndedIterator`) |
| 4 | `redundant_pattern_matching` | 347 | `if let Ok(_) = ...` discards the value | Replace with `.is_ok()` |
| 5 | `redundant_pattern_matching` | 408 | Same as #4 in second listener handler | Replace with `.is_ok()` |

### `src/esi.rs`

| # | Lint | Line | Description | Fix |
|---|------|------|-------------|-----|
| 6 | `needless_question_mark` | 67-76 | Function returns `Ok(expr?)` wrapping a fallible expression | Remove outer `Ok(...)` and inner `?`, return the expression directly |

### `src/main.rs`

| # | Lint | Line | Description | Fix |
|---|------|------|-------------|-----|
| 7 | `option_if_let_else` / double-check | 110-113 | Checks `.is_some()`, then `.unwrap()` inside | Use `if let Some(n) = bonus_remaps` to bind once |

### `src/parser.rs`

| # | Lint | Line | Description | Fix |
|---|------|------|-------------|-----|
| 8 | `unused_imports` | 649 | `use std::error::Error;` imported but not used (`.root_cause()` works via method dispatch on `anyhow::Error`) | Remove the import |

### `src/optimizer.rs`

| # | Lint | Line | Description | Fix |
|---|------|------|-------------|-----|
| 9 | `needless_range_loop` | 317 | `for i in 0..n { ... seen[i] ... }` | Rewrite as `for (i, &seen_i) in seen.iter().enumerate().take(n)` — or keep index loop with `#[allow]` since it's a hot path and the iterator version is less clear here. **Decision**: allow lint, readability wins for this O(n) fallback path. |
| 10 | `needless_range_loop` | 1046 | Test code: `for i in int_end..primaries.len() { assert_eq!(primaries[i], ...) }` | Use `.iter().skip(int_end).enumerate()` or just `.iter().skip(int_end)` since only the value matters |
| 11 | `vec_init_then_push` | 941-942 | Test code: `let mut v = Vec::new(); v.push(...);` | Use `vec![...]` macro |
| 12 | `unused_variables` + `useless_vec` | 869 | Test code: `implants` variable created but never used; single-element `vec![]` | Prefix with `_`; replace `vec![...]` with `[...]` array literal |
| 13 | `unnecessary_cast` | 1180 | `(sp_needed / 0.5) as f64` — both operands are `f64` | Remove the cast |

---

## Execution Order

All fixes are independent (no cross-file dependencies). Apply in file order, then run `cargo clippy --all-targets` to verify zero warnings/errors, followed by `cargo test`.
