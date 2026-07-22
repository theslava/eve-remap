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

### ~~6. Linear scan for implant lookups~~ ✅ **Resolved**

Added `EffectiveAttributes::from_base_and_implants_with_index()` accepting a pre-built `HashMap<u32, &ImplantRecord>`. Old method delegates to indexed version. `optimize()` builds single map at entry point (`src/optimizer.rs`), reused across all callers including initial effective attributes computation.

### ~~7. Time cache uses manual stride indexing~~ ❌ **Won't Fix**

Flat `Vec<f64>` with stride math is intentional — matches the layout we converted `suffix_sum` to in #18. Making both `Vec<Vec<f64>>` would undo that allocation reduction. The comment documents the access pattern.

### ~~8. `generate_allocations()` not cached~~ ✅ **Resolved**

Cached via `std::sync::LazyLock<Vec<BaseAttributes>>` at module scope (`src/optimizer.rs`). Allocation space (2,885 entries) computed once on first call, cloned thereafter. Eliminates redundant backtracking search on repeated `optimize()` calls.
## Code Quality / Maintainability

### ~~9. Clippy warning unaddressed~~ ✅ **Resolved**

Changed `(len - i) % 3 == 0` to `(len - i).is_multiple_of(3)` in `format_number` (`src/main.rs`). Clippy lint satisfied.
### ~~10. `parse_duration` rejects 3+ components~~ ✅ **Resolved**

Restored 2-component limit matching EVE Online client UI (displays max two components like "5d 13h"). Added rationale comment in doc string so this is not reverted again. Test renamed `three_components_accepted` → `three_components_rejected`.
### ~~11. Unused import with misleading comment~~ ✅ **Resolved**

Moved `Attribute` import from module-level into `#[cfg(test)] mod tests { ... }` where it is actually used. Removed `#[allow(unused_imports)]` allow-attribute and stale comment (`src/calculator.rs`). `EffectiveAttributes` and `SkillRecord` remain at module scope as they are consumed by public functions.
### ~~12. Attribute name strings duplicated as magic constants~~ ✅ **Resolved**

Changed `AttributeSpSummary` HashMaps from `HashMap<String, f64>` to `HashMap<Attribute, f64>`. Producer (`optimizer.rs`) inserts typed attributes directly — no `.to_string()`. Consumer (`main.rs::print_table_output`) iterates over a typed `DISPLAY_ORDER` const instead of string keys. JSON output unchanged via existing `#[serde(rename_all = "lowercase")]`.

### ~~13. Inconsistent error message patterns~~ ❌ **Won't Fix**

Existing conventions are self-documenting: line-numbered context for queue-file parsing (user-facing), bare errors for CLI args and data loading. Introducing a helper would add indirection without improving signal-to-noise.

## Testing

### ~~14. Queue file parser has zero test coverage~~ ✅ **Resolved**

Extracted into `src/parser.rs` with three pure functions:

| Function | Tests | Coverage |
|---|---|---|
| `parse_attributes()` | 7 | Valid inputs, range validation, wrong count, whitespace tolerance |
| `parse_implant_bonuses()` | 3 | Zero, mixed values, out-of-range |
| `parse_queue()` | 28 | Basic format, multiple skills, case-insensitive matching, comments/blanks, duration progress (`@3d12h`, `@5h 30m`, `@90s`, `@0s`), SP-trained progress (bare numbers, commas, too-high, below-threshold, exact-threshold), error cases (empty input, only comments, unknown skill, invalid levels 0/6/x, missing level, bad duration, negative SP, line-number accuracy in errors), disambiguation (`s` suffix → duration vs bare number → SP), multi-level same skill, source label propagation |

Refactored `main.rs::run_optimizer_from_queue_file` to delegate all parsing to the new module (~130 lines removed).

### ~~15. Stdin queue input untested~~ ❌ **Won't Fix**

Stdin (`-`) and file paths both call `parse_queue()` with a `String` — the parser module has full coverage (38 tests). The only difference is where the string comes from (`std::io::stdin().read_to_string()` vs `fs::read_to_string()`), which is stdlib I/O with no application logic to test.

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

### ~~18. Suffix sum table fragmentation~~ ✅ **Resolved**

Converted from `Vec<Vec<f64>>` (~2,885 allocations) to single flat buffer matching `time_cache` stride convention (`src/optimizer.rs`). Reduces allocation overhead and improves cache locality.

### ~~19. Reorder cluster scoring O(|ready|² per step)~~ ❌ **Won't Fix**

Queues are < 50 skills. This runs in sub-milliseconds. Incremental scoring optimization adds complexity with no measurable benefit at current scale.

### ~~20. `format_number` allocates intermediate vector~~ ✅ **Resolved**

Replaced `Vec<char>` collection with byte iteration via `as_bytes()` (`src/main.rs`). Zero-allocation path for ASCII digit input.

## UX / CLI

### ~~21. No progress indicator during precomputation~~ ❌ **Won't Fix**

User declined — optimizer completes in under a second for typical queues. Progress output would add noise, not signal.

### ~~22. Baseline comparison message unclear when remaps not used~~ ✅ **Resolved**

Added context line when optimizer produces single epoch: "(Remapping did not improve training time over current attributes.)" printed indented under baseline stat (`src/main.rs`).
### ~~23. `--queue-out` writes reordered skills without explanation~~ ✅ **Resolved**

Prepends `# Optimized by eve-remap — skill order reordered for attribute locality` as first line of output. Skill count messages adjusted with `- 1` to exclude header from count (`src/main.rs`).

## Priority Ranking

| # | Issue | Severity | Effort | Status |
|---|-------|----------|--------|--------|
| 5 | ~~`f64` attributes~~ | — | — | ✅ Resolved |
| 1 | ~~Greedy limitation un-documented~~ | — | — | ✅ Resolved |
| 6 | ~~Linear implant scan~~ | — | — | ✅ Resolved |
| 20 | ~~Vec\<char\> allocation~~ | — | — | ✅ Resolved |
| 2 | ~~Score overflow~~ | — | — | ✅ Resolved |
| 3 | ~~Duplicate prerequisite edges~~ | — | — | ✅ Resolved |
| 4 | ~~Cycle detection no warning~~ | — | — | ✅ Resolved |
| 8 | ~~Allocation cache~~ | — | — | ✅ Resolved |
| 9 | ~~Clippy warning~~ | — | — | ✅ Resolved |
| 10 | ~~Duration parse limit~~ | — | — | ✅ Resolved |
| 18 | ~~Suffix sum fragmentation~~ | — | — | ✅ Resolved |
| 11 | ~~Unused import~~ | — | — | ✅ Resolved |
| 14 | ~~No parser tests~~ | — | — | ✅ Resolved |
| 22 | ~~Baseline message unclear~~ | — | — | ✅ Resolved |
| 23 | ~~Queue-out no header~~ | — | — | ✅ Resolved |
| 7 | ~~Time cache stride~~ | — | — | ❌ Won't Fix |
| 12 | ~~Attribute name duplication~~ | — | — | ✅ Resolved |
| 13 | ~~Error message patterns~~ | — | — | ❌ Won't Fix |
| 15 | ~~Stdin queue untested~~ | Coverage | Low | ❌ Won't Fix |
| 19 | ~~Cluster scoring O(n²)~~ | — | — | ❌ Won't Fix |
| 21 | ~~Progress indicator~~ | UX | Low | ❌ Won't Fix |
