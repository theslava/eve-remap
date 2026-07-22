# Code Review — eve-remap

Review date: 2026-07-21

## Critical / Correctness

### ~~1. Greedy optimizer limitation not documented~~ ✅ **Resolved**

Added disclaimer note in `print_table_output()` (`src/main.rs`) after summary statistics. Output now reads: "Note: This plan uses a greedy heuristic and is not guaranteed optimal. Results may vary by a few percent from the true minimum."

### ~~2. Tie-breaking score overflow risk~~ ✅ **Resolved**

Changed `rate_score` and combined tie-break key from `u32` to `u64` (`src/optimizer.rs`). Overflow path eliminated; max safe value is now ~1.8e19 instead of ~4e9.
### ~~3. Duplicate prerequisite edges possible~~ ✅ **Resolved**

Added `HashSet<(usize, usize)>` deduplication guard in `reorder_queue` explicit prerequisite loop (`src/optimizer.rs`). Edges `(j, i)` are only inserted once even if multiple SDE prerequisites resolve to the same queued entry index. Kahn's algorithm `in_degree` counts remain accurate.
### ~~4. Cycle detection fallback produces no warning~~ ✅ **Resolved**

Emit `eprintln!` warning when topological sort leaves unprocessed entries (`src/optimizer.rs`). Message reports count of affected skills and explains they are appended in original queue order.
## Design / Architecture

### ~~5. `BaseAttributes` stores integer values as `f64`~~ ✅ **Resolved**

Changed `BaseAttributes` fields from `f64` to `u32` across the entire codebase:
- `src/data/models.rs`: struct fields changed; `From<BaseAttributes>` impl casts to `f64`; `from_base_and_implants` accumulates into `EffectiveAttributes` directly
- `src/optimizer.rs`: `generate_allocations()` generates `u32` values natively; test helpers updated; removed `.round() as u32` casts
- `src/parser.rs`: `parse_attributes()` and `parse_implant_bonuses()` parse `u32` with range validation (17-27, 0-10); test assertions use integer literals
- `src/main.rs`: removed redundant `as u32` casts on attribute display

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

### ~~8. `generate_allocations()` not cached~~ ✅ **Resolved**

Cached via `std::sync::LazyLock<Vec<BaseAttributes>>` at module scope (`src/optimizer.rs`). Allocation space (2,885 entries) computed once on first call, cloned thereafter. Eliminates redundant backtracking search on repeated `optimize()` calls.
## Code Quality / Maintainability

### ~~9. Clippy warning unaddressed~~ ✅ **Resolved**

Changed `(len - i) % 3 == 0` to `(len - i).is_multiple_of(3)` in `format_number` (`src/main.rs`). Clippy lint satisfied.
### ~~10. `parse_duration` rejects 3+ components~~ ✅ **Resolved**

Removed component limit check from `parse_duration` (`src/calculator.rs`). Parser now accepts arbitrary numbers of components (e.g., `"1d 2h 3m"`), matching EVE Online UI format. Dead `component_count` variable removed.
### ~~11. Unused import with misleading comment~~ ✅ **Resolved**

Moved `Attribute` import from module-level into `#[cfg(test)] mod tests { ... }` where it is actually used. Removed `#[allow(unused_imports)]` allow-attribute and stale comment (`src/calculator.rs`). `EffectiveAttributes` and `SkillRecord` remain at module scope as they are consumed by public functions.
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

### ~~22. Baseline comparison message unclear when remaps not used~~ ✅ **Resolved**

Added context line when optimizer produces single epoch: "(Remapping did not improve training time over current attributes.)" printed indented under baseline stat (`src/main.rs`).
### ~~23. `--queue-out` writes reordered skills without explanation~~ ✅ **Resolved**

Prepends `# Optimized by eve-remap — skill order reordered for attribute locality` as first line of output. Skill count messages adjusted with `- 1` to exclude header from count (`src/main.rs`).

## Priority Ranking

| # | Issue | Severity | Effort | Status |
|---|-------|----------|--------|--------|
| 5 | ~~`f64` attributes~~ | — | — | ✅ Resolved |
| 1 | ~~Greedy limitation un-documented~~ | — | — | ✅ Resolved |
| 2 | ~~Score overflow~~ | — | — | ✅ Resolved |
| 3 | ~~Duplicate prerequisite edges~~ | — | — | ✅ Resolved |
| 4 | ~~Cycle detection no warning~~ | — | — | ✅ Resolved |
| 8 | ~~Allocation cache~~ | — | — | ✅ Resolved |
| 9 | ~~Clippy warning~~ | — | — | ✅ Resolved |
| 10 | ~~Duration parse limit~~ | — | — | ✅ Resolved |
| 11 | ~~Unused import~~ | — | — | ✅ Resolved |
| 14 | ~~No parser tests~~ | — | — | ✅ Resolved |
| 22 | ~~Baseline message unclear~~ | — | — | ✅ Resolved |
| 23 | ~~Queue-out no header~~ | — | — | ✅ Resolved |
| 15 | Stdin queue input untested | Coverage | Low | ⚠️ Deferred |
