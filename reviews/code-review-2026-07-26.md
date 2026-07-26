# Code Review Report: eve-remap (2026-07-26)

## BUGS / LOGIC ERRORS

### B1. `format_duration` silently drops days when result rounds back to zero components

**Location**: calculator.rs, line 236

When all four units `[d, h, m, s]` are non-zero and seconds ≥ 30 trigger a cascade carry (`m→h→d`), the code filters out zeroed components from `(v_d, v_h, v_m)`. If rounding causes `v_h` or `v_m` to become exactly 60/24 and roll over into `v_d`, those slots drop to 0 and get filtered. The output may show only `"Xd"` instead of three components, losing information about the original magnitude of hours/minutes.

Example: input producing `(d=1, h=23, m=59, s=30)` rounds to `(d=2, h=0, m=0)` → displays as `"2d"`, dropping the fact that it was nearly two full days with significant sub-day components. Misleading for durations near day boundaries.

---

### B2. `CharacterData.base_attributes` doc comment contradicts actual content

**Location**: esi.rs, lines 178–179

The field is documented as *"Base attributes from neural interface (not including implants)"* but the assignment at lines 224–230 copies directly from `EsCharacterAttributes` which returns **effective values INCLUDING active implants**. The subtraction (`eff.sub(&implant_bonus)`) happens later in `resolve_attributes()` (main.rs line 291).

The code path is correct end-to-end, but the struct documentation on `CharacterData::base_attributes` describes what the value becomes after processing, not what it actually contains. A reader assuming the invariant holds at the point of use would reason incorrectly.

---

### B3. `EsSkillQueueEntry.training_start_sp` doc comment is misleading

**Location**: esi.rs, lines 35–36

Documented as *"Cumulative SP where training actually started"* — this reads like a static snapshot of when training began. In practice, CCP's API updates this field to reflect **current progress** during active training. For queued-but-not-yet-started skills it equals `level_start_sp`. The name and description suggest immutability; the semantics are "current cumulative SP toward target level."

---

## MODERATE ISSUES

### M1. No URL decoding in OAuth callback parser

**Location**: auth.rs, lines 455–463

`parse_callback` splits query parameters with raw string operations (`split('&')`, `split_once('=')`) without percent-decoding values. Authorization codes from EVE SSO are typically alphanumeric, so this works today. But if CCP ever encodes special characters (or if state tokens contain encoded bytes), the code sent to `esi.authenticate()` will be malformed. A URL-decode step on both `code` and `value` would make this robust.

---

### M2. Empty refresh token silently stored

**Location**: auth.rs, line 315

```rust
let refresh_token = esi.refresh_token.clone().unwrap_or_default();
```

If the token exchange doesn't return a refresh token, an empty string is persisted. Subsequent calls to `ensure_fresh_token()` will fail with *"token refresh failed"* — but there's no indication that the root cause was a missing refresh token at login time. Should either error out or log a warning that the account won't support auto-refresh.

---

### M3. SP equality check uses `f64::EPSILON` for large-number comparison

**Location**: parser.rs, line 174

```rust
if (sp_trained - cum_to).abs() < f64::EPSILON || sp_trained > cum_to {
```

`f64::EPSILON` ≈ 2.2×10⁻¹⁶ is machine epsilon at value 1.0. For SP values in the range of thousands to hundreds of thousands, meaningful floating-point noise is orders of magnitude larger (~10⁻¹¹ for 256,000). This check works correctly today because both operands come from controlled sources (integer-parsed user input and exact integer multiplications), so differences are always ≥ 1.0 when they differ. But it's technically incorrect usage of EPSILON and would break if either source introduced fractional SP values. A relative tolerance like `(a - b).abs() / ((a + b).abs()) < 1e-9` or an absolute threshold like `< 0.5` would be more robust.

---

### M4. `cum_before_for` recomputes a sum already available

**Location**: optimizer.rs, line 508

After finding the best cut point, the code recomputes `epoch_end` via `cum_before_for(&stay_times, cut - remaining_start)` instead of using the `cum_before` accumulator from the winning iteration. The variable holds the value from the *last* loop iteration, not necessarily the winner — so the recompute is necessary. However, storing `cum_before` alongside `best_cut_info` (as a fourth tuple element) would eliminate this redundant O(k) pass over stay_times. Minor performance issue; the loop runs ~2885×N times total in worst case but each inner sum is small.

---

## MINOR ISSUES & CODE QUALITY

### C1. `gnome-open` browser launcher is deprecated

**Location**: auth.rs, lines 488–493

`gnome-open` has been deprecated since GNOME 3.x in favor of `xdg-open`, which is already tried as strategy 2. This fallback path never fires on modern systems and adds dead code.

---

### C2. Unnecessary clone in `reorder_queue` return

**Location**: optimizer.rs, line 326

```rust
.map(|idx| entries[idx].clone())
```

The function owns `entries` by value but clones each entry into the result vector instead of rearranging in place. A single-pass shuffle or index-to-entry swap would avoid N allocations. Cosmetic inefficiency.

---

### C3. `find_available_port` ignores its hint when hint=Some(0)

**Location**: auth.rs, lines 509–514

```rust
fn find_available_port(hint: Option<u16>) -> Result<u16> {
    if let Some(port) = hint { Ok(port) } else { Ok(9090) }
}
```

Passing port 0 (OS-assigned ephemeral) via `--port 0` returns 0 directly rather than letting the OS pick one. Port 0 binding will succeed on most systems but conflicts with EVE SSO's requirement that callback URIs match exactly — the user registered a specific URI, not "some random port." The doc comment says "If omitted, an available ephemeral port will be selected automatically" which is false — it always defaults to 9090, never picks ephemeral. Either the default behavior should attempt 9090 then fall back to OS-assigned, or the documentation should accurately state it uses 9090.

---

### C4. `format_number` wraps negative floats to huge integers

**Location**: main.rs, line 420

```rust
let int = n as u64;
```

Negative SP values cast to `u64` produce very large numbers (wrap-around). SP shouldn't be negative in practice, but there's no assertion or guard. A `debug_assert!(n >= 0.0)` would catch programming errors early.

---

### C5. Logout by name uses case-sensitive comparison

**Location**: main.rs, line 176

```rust
store.accounts.retain(|a| a.character_name != *name);
```

But character lookup in `resolve_character` (line 310) uses `eq_ignore_ascii_case`. This inconsistency means a user who can look up their character with mixed-case input might fail to log out if the case doesn't match exactly.

---

### C6. `bonus_remaps_used` not displayed in table output

**Location**: main.rs lines 447–515

Each epoch tracks how many bonus remaps were consumed (`EpochPlan.bonus_remaps_used`), but the human-readable table output never prints this. Users seeing multiple epochs won't know which ones cost bonus remaps vs normal cooldown remaps. The JSON output includes it via serde serialization.

---

## DOCUMENTATION INCONSISTENCIES

### D1. `--port` help text claims ephemeral port selection

**Location**: cli.rs, lines 84–86

> "If omitted, an available ephemeral port will be selected automatically."

This is incorrect — omitting `--port` defaults to **9090**, not an OS-assigned ephemeral port. See C3 above.

---

### D2. AGENTS.md `--remap-available Dd` format example

**Location**: AGENTS.md line 57

The documented flag description says `'0d' means available now; '30d' means available 30 days from start`. This matches the implementation and the CLI help text at cli.rs lines 52–55. No discrepancy here. ✓

---

### D3. `QueuedSkillRemaining::Duration` field naming

**Location**: models.rs, lines 136–138

```rust
Duration {
    remaining_sec: f64,
    total_duration_secs: f64,
},
```

Inconsistent suffix style: `remaining_sec` (singular) vs `total_duration_secs` (plural). Minor but confusing when scanning API surface.

---

## SUMMARY

| Severity | Count | Items |
|----------|-------|-------|
| Bug / Logic Error | 3 | B1, B2, B3 |
| Moderate Issue | 4 | M1, M2, M3, M4 |
| Minor / Code Quality | 6 | C1–C6 |
| Documentation Inconsistency | 3 | D1–D3 |

No security vulnerabilities found. Token storage uses OS config directory with standard permissions. No hardcoded credentials. The optimizer's greedy heuristic is documented as non-optimal and includes a disclaimer in output.
