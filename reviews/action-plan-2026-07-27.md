# Code Review Action Plan (2026-07-27)

Derived from `code-review-2026-07-26.md`. Items marked "no change" are dropped.

---

## Documentation Fixes

### B2 — Fix doc on `CharacterData.base_attributes`

**File**: `esi.rs`, line 178–179

Change doc comment from *"Base attributes from neural interface (not including implants)"* to clarify that the field stores raw effective values from ESI `/characters/me/attributes/` (which include active implants). Implant subtraction happens later in `resolve_attributes()` (`main.rs`).

Example: `"Raw effective attributes from ESI /characters/me/attributes/. Includes active implant bonuses; subtracted later by resolve_attributes()."`

### B3 — Fix doc on `EsSkillQueueEntry.training_start_sp`

**File**: `esi.rs`, lines 35–36

Update doc comment to clarify this is not an immutable snapshot but tracks current progress during active training. For queued-but-not-yet-started entries it equals `level_start_sp`.

### D3 — Normalize field naming suffix

**File**: `src/data/models.rs`, lines 137–138

Rename `remaining_sec` → `remaining_secs` for consistency with `total_duration_secs` (plural `_secs` throughout). Update all callers.

---

## Bug & Logic Fixes

### M1 — Add URL percent-decoding to OAuth callback parser

**File**: `auth.rs`, lines 455–463

Decode both `code` and `state` query parameter values before use. Authorization codes are alphanumeric today, but state tokens or future CCP changes may contain encoded bytes. Apply percent-decoding after `split_once('=')`, before storing into `code`/`state`.

### M2 — Warn if refresh token is missing at login

**File**: `auth.rs`, line 315

When `esi.refresh_token` is `None`, emit a warning (`eprintln!`) that the account won't support auto-refresh. Store empty string as-is (don't block login).

Example: `"Warning: No refresh token received for '{}'. The session will expire and cannot be refreshed automatically."`

### M3 — Replace EPSILON comparison with integer equality check

**File**: `parser.rs`, line 174

Replace:
```rust
if (sp_trained - cum_to).abs() < f64::EPSILON || sp_trained > cum_to {
```

With integer-based equality. Both operands represent SP totals that should be integers (from controlled sources). Truncate/floor both sides and compare with `==`:
```rust
let sp_int = sp_trained.floor();
let to_int = cum_to.floor();
if sp_int == to_int || sp_trained > cum_to {
```

### M4 — Cache `cum_before` in best cut tuple to avoid redundant O(k) pass

**File**: `optimizer.rs`, lines 495–508

Store `cum_before` as a fourth element alongside `(cut, total_finish, chosen_a)` in `best_cut_info`. Use the cached value at line 508 instead of recomputing via `cum_before_for(&stay_times, cut - remaining_start)`.

---

## UX & Consistency Fixes

### C1 — Remove deprecated `gnome-open` browser launcher fallback

**File**: `auth.rs`, lines 488–493

Drop strategy 3 entirely. Fallback chain becomes: wslview → xdg-open → open (macOS).

### C3 — Fix port help text + reject `--port 0`

**Files**: `cli.rs` lines 84–86; `auth.rs` lines 509–514

- Update CLI help text for `--port` from "If omitted, an available ephemeral port will be selected automatically" to "Defaults to 9090 if omitted."
- Reject `Some(0)` in `find_available_port` with an error explaining that EVE SSO requires exact callback URI matching, so OS-assigned ports won't work.

### C5 — Make character lookup and logout both case-sensitive

**Files**: `main.rs`, line 176 (logout); line 310 (`resolve_character`)

Currently `resolve_character` uses `eq_ignore_ascii_case` but logout uses exact match. Make both case-sensitive for consistency. Update any relevant documentation or help text to reflect this requirement.

### C6 — Display remap type per epoch in table output

**File**: `main.rs`, lines 447–465

Add a display line under each epoch header showing whether the remap consumed a bonus slot or used a normal cooldown remap. For the initial allocation (epoch 0), omit or show "N/A".

Example:
```
Epoch 2: Remap
  Remap type: bonus    # or "normal"
  Attributes: ...
```

---

## Summary

| Severity | Items | Count |
|----------|-------|-------|
| Documentation | B2, B3, D3 | 3 |
| Bug / Logic | M1, M2, M3, M4 | 4 |
| UX / Consistency | C1, C3, C5, C6 | 4 |
| **Total** | | **11** |

Items dropped (no change): B1 (correct behavior), C2 (negligible), C4 (single callsite, input always non-negative). D1 covered by C3 fix. D2 had no issue.
