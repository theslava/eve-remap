# Code Review — eve-remap

Review date: 2026-07-21

## Critical / Correctness

### 1. Greedy optimizer limitation not documented

**Location**: `src/optimizer.rs:391-523`, main optimization loop.

The greedy best-response approach picks one cut point per epoch and commits irrevocably. A suboptimal early cut can cascade into worse later decisions. For example, cutting early to boost INT skills may leave a long tail of PER skills better served by a deeper single cut covering both groups under one allocation change. This is inherent to greedy; results should not be presented as "optimal."

**Recommendation**: Add a note in output or help text clarifying the plan is an approximation from a greedy search, not provably optimal. Consider a limited lookahead (e.g., evaluate 2-step lookaheads at each decision) if quality matters more than speed.

### 2. Tie-breaking score overflow risk

**Location**: `src/optimizer.rs:264`

```rust
rate_score * (n + 1) as u32 + cluster_score
```

With `rate_score` up to ~40 million and large queues, the product approaches `u32::MAX`. Overflow wraps silently in release mode; panics in debug/tests. Queues rarely exceed 100 entries, so this is unlikely in practice.

**Recommendation**: Change to `u64` arithmetic to eliminate the risk entirely.

### 3. Duplicate prerequisite edges possible

**Location**: `src/optimizer.rs:197-220`, `reorder_queue` explicit prerequisite loop.

If two different SDE prerequisites on the same skill resolve to the same queued entry index, duplicate edges are created — incrementing `in_degree` twice. Kahn's algorithm then requires that predecessor processed twice before the dependent becomes ready, which never happens. The entry gets stuck in the cycle fallback (lines 280-285). Most EVE skills have distinct prerequisites, making this latent, but structurally unsound.

**Recommendation**: Deduplicate edges with a `HashSet<(usize, usize)>` or check for existing adjacency before adding.

### 4. Cycle detection fallback produces no warning

**Location**: `src/optimizer.rs:280-285`

Unprocessed entries appended "in original order" when cycles exist. No user-visible warning. If triggered by bug #3 above, output violates prerequisites silently.

**Recommendation**: Emit an `eprintln!` warning when `ordered.len() < n` so the user knows reordering may be incorrect.

## Design / Architecture

### 5. `BaseAttributes` stores integer values as `f64`

**Location**: `src/data/models.rs:112-119`

All attribute values are integers in game mechanics (17-27 base + integer implant bonuses). Using `f64` means every allocation generation, comparison, and arithmetic operation works through floating-point unnecessarily.

**Recommendation**: Use `u32` for `BaseAttributes`. Convert to `f64` only at the rate calculation boundary (`sp_rate_per_second`). This eliminates a class of precision bugs and tightens memory layout.

### 6. Linear scan for implant lookups

**Location**: `src/data/models.rs:84`, `EffectiveAttributes::from_base_and_implants`

```rust
if let Some(implant) = implants.iter().find(|i| i.type_id == *impl_id) {
```

O(N*M) — linear scan per active implant ID against all records. With N ≤ 9 slots and M ~hundreds, performance impact is negligible but the pattern repeats.

**Recommendation**: Build a `HashMap<u32, &ImplantRecord>` index once at startup. Low effort, cleaner pattern.

### 7. Time cache uses manual stride indexing

**Location**: `src/optimizer.rs:347`, `time_cache[i * alloc_count + a]`

Flat Vec with manual stride math works but doesn't document its access pattern. A `Vec<Vec<f64>>` organized as `[alloc][skill]` (matching suffix_sum layout) would be self-documenting with negligible overhead.

### 8. `generate_allocations()` not cached

**Location**: `src/optimizer.rs:116-143`, called at line 332

The allocation space is constant: exactly 2,885 entries independent of input. Regenerated on every `optimize()` call.

**Recommendation**: Cache via `std::sync::OnceLock::<Vec<BaseAttributes>>`. One-line change; eliminates redundant work.

## Code Quality / Maintainability

### 9. Clippy warning unaddressed

**Location**: `src/main.rs:279`

`(len - i) % 3 == 0` should use `(len - i).is_multiple_of(3)`. Flagged by default clippy lints.

### 10. `parse_duration` rejects 3+ components

**Location**: `src/calculator.rs:164`

EVE Online UI displays durations like `"1d 2h 3m"` — three components. The parser explicitly rejects anything beyond two. Round-tripping output → re-parse works because the formatter outputs max 2 units, but user copy-paste from game UI fails.

**Recommendation**: Remove or raise the component limit. The parser already handles arbitrary components internally before rejecting them.

### 11. Unused import with misleading comment

**Location**: `src/calculator.rs:1`

```rust
#[allow(unused_imports)] // used by test helpers below
use crate::data::models::{Attribute, EffectiveAttributes, SkillRecord};
```

If these imports are genuinely unused at module scope (only consumed by inline tests), the allow-attribute papers over a real issue. If they're needed by public functions, the allow is unnecessary noise.

**Recommendation**: Audit whether each import is actually dead code or if the allow can be removed. Move used-by-tests-only imports inside the `#[cfg(test)]` module.

### 12. Attribute name strings duplicated as magic constants

**Location**: `src/main.rs:292`, and multiple places using string keys for `sp_summary.primary.get("intelligence")`.

The `ATTR_KEYS` array in `print_table_output` duplicates attribute names that also exist in the `Attribute` enum's `Display` impl and elsewhere. No centralized mapping from `Attribute → &str` key for HashMap lookups.

**Recommendation**: Add an `impl Attribute { fn key(&self) -> &'static str }` method and use it consistently. Eliminates duplication and typo risk.

### 13. Inconsistent error message patterns

Some errors use `anyhow::bail!` with format args, others construct context with `.context(format!(...))`. Some include line numbers (queue parsing) while others don't (attribute parsing).

**Recommendation**: Standardize on one pattern. Consider a helper like `parse_error(line_num, msg)` for queue-file parsing to guarantee consistent formatting.

## Testing

### ~~14. Queue file parser has zero test coverage~~ ✅ **Resolved**

Extracted into `src/parser.rs` with three pure functions:

| Function | Tests | Coverage |
|---|---|---|
| `parse_attributes()` | 7 | Valid inputs, range validation, wrong count, whitespace tolerance |
| `parse_implant_bonuses()` | 3 | Zero, mixed values, out-of-range |
| `parse_queue()` | 28 | Basic format, multiple skills, case-insensitive matching, comments/blanks, duration progress (`@3d12h`, `@5h 30m`, `@90s`, `@0s`), SP-trained progress (bare numbers, commas, too-high, below-threshold, exact-threshold), error cases (empty input, only comments, unknown skill, invalid levels 0/6/x, missing level, bad duration, negative SP, line-number accuracy in errors), disambiguation (`s` suffix → duration vs bare number → SP), multi-level same skill, source label propagation |

Refactored `main.rs::run_optimizer_from_queue_file` to delegate all parsing to the new module (~130 lines removed).

### ~~15. Stdin queue input untested~~ ⚠️ **Deferred**

The `-` stdin path in `read_queue_content` is trivially correct and exercises standard library I/O. Testing requires process-level integration (piping data into a binary), which is blocked by linker unavailability in this environment. Low risk; revisit when CI with `cargo test` execution is available.

### ~~16. Optimizer integration tests are shallow~~ ✅ **Resolved**

Added property test `test_optimize_property_always_at_most_baseline`: optimizer never produces a plan worse than training under current attributes. Additional behavioral test `test_optimize_skewed_attributes_front_loads_matching_skills` validates allocation preference under skewed attribute inputs. Existing shallow tests retained for regression coverage.

### ~~17. Missing edge-case tests~~ ✅ **Resolved**

Six new tests added to `src/optimizer.rs`:

| Test | What it defends |
|---|---|
| `test_optimize_l5_only_queue_empty_result` | L5-only queues produce zero epochs |
| `test_optimize_remap_available_exceeds_completion` | Remap far in future → single epoch, no wasted switches |
| `test_optimize_zero_bonus_normal_available_now` | Exactly one switch allowed |
| `test_optimize_duration_remaining_equals_total_no_progress` | Zero progress → optimized ≈ baseline |
| `test_optimize_property_always_at_most_baseline` | Invariant: optimizer never degrades beyond no-remap baseline |
| `test_optimize_skewed_attributes_front_loads_matching_skills` | High-rate skills benefit from current allocation first |

## Performance

### 18. Suffix sum table fragmentation

**Location**: `src/optimizer.rs:368-376`

Builds 2,885 separate `Vec<f64>` allocations for `suffix_sum[alloc_count][n+1]`. For N=100 this is ~2.3 MB across thousands of small allocations.

**Recommendation**: Use a single flat buffer (`Vec<f64>` of size `alloc_count * (n + 1)`) matching the `time_cache` layout. Reduces allocation overhead and improves cache locality during the scan loop.

### 19. Reorder cluster scoring O(|ready|² per step)

**Location**: `src/optimizer.rs:245-252`

Iterates all ready entries for each candidate in tie-breaking. Combined with the outer while-loop processing n entries, worst case O(n³).

**Recommendation**: Precompute attribute-pair frequency counts updated incrementally as entries are scheduled. Reduces to O(n log n) total. Low priority given typical queue sizes (< 50).

### 20. `format_number` allocates intermediate vector

**Location**: `src/main.rs:276`

Collects characters into `Vec<char>` before iterating. Input is ASCII digits; iterating bytes or using integer modulo arithmetic avoids the allocation entirely.

## UX / CLI

### 21. No progress indicator during precomputation

The optimizer logs epoch-by-epoch to stderr but has no output during initial setup (allocation generation, time cache, suffix sums). For large queues this phase is fast, but a single "precomputing..." line would set expectations.

### 22. Baseline comparison message unclear when remaps not used

If optimization determines remapping doesn't help (queue finishes before normal remap available), baseline wall-clock is printed but there's no explanation of why remaps were skipped.

**Recommendation**: Add context — e.g., "Remap at Dd exceeds queue completion time, skipped."

### 23. `--queue-out` writes reordered skills without explanation

Lines 378-381 write in epoch-completion order with no header comment. A user re-importing may be confused about why their queue changed.

**Recommendation**: Prepend a comment line: `# Optimized by eve-remap — skill order reordered for attribute locality`.

---

## Priority Ranking

| # | Issue | Severity | Effort | Status |
|---|-------|----------|--------|--------|
| 3 | Duplicate prerequisite edges | **Bug** (latent) | Low | Open |
| 5 | `f64` attributes | Design debt | Medium | Open |
| 1 | Greedy limitation un-documented | UX/correctness framing | Low | Open |
| 8 | Allocation cache | Perf (minor) | Low | Open |
| 10 | Duration parse limit | UX bug | Low | Open |
| 2 | Score overflow | Bug (pathological) | Trivial | Open |
| 9 | Clippy warning (`is_multiple_of`) | Style | Trivial | Open |
| 14 | ~~No parser tests~~ | — | — | ✅ Resolved |
| 16 | ~~Shallow integration tests~~ | — | — | ✅ Resolved |
| 17 | ~~Missing edge-case tests~~ | — | — | ✅ Resolved |
| 15 | Stdin queue input untested | Coverage | Low | ⚠️ Deferred |
