# ESI Integration Research

## Library Recommendation: `rfesi` (v0.50.2)

### Comparison

| | `rfesi` | `eve_esi` |
|---|---|---|
| **Latest version** | 0.50.2 (Jun 2026) | 0.5.0-rc.2 (Mar 2026) |
| **Maintenance** | Active — regular releases through June 2026 | **No longer maintained**. Author explicitly says "use https://github.com/celeo/rfesi" |
| **Stars / Downloads** | 28 stars, 65K downloads | 5 stars, 10K downloads |
| **Auth flow** | PKCE (application/auth_code_without_pkce), client_secret, refresh token — all three | OAuth2 with JWT validation, but requires `client_secret`. No PKCE support for public apps |
| **TLS backend** | `default-tls` or `rustls-tls` feature flags | OpenSSL (via reqwest default) |
| **Pure Rust option** | Yes — disable `default-tls`, enable `rustls-tls` + disable `validate_jwt` | No — depends on system TLS |
| **Async runtime** | tokio (required) | tokio (required) |
| **Endpoint coverage** | Macro-generated from swagger spec; not all endpoints implemented yet | Partial manual implementation (~40 endpoints across categories listed in README) |

### Why rfesi

1. **Actively maintained** with frequent releases tracking ESI spec updates. The previous crate (`eve_esi`) is dead.
2. **PKCE built-in** via `enable_application_authentication(true)` — EVE Online public applications don't have a `client_secret`, so this is the only viable auth flow. Our old hand-rolled PKCE code becomes unnecessary.
3. **Feature flags** allow stripping JWT validation and using rustls for a pure-Rust, zero-system-dependency build if desired.
4. **Swagger-driven**: endpoints are generated from the live ESI spec at build time, keeping them in sync with CCP's API changes.

### What rfesi Does NOT Cover Yet

The `rfesi` README notes: *"not all of them have been implemented."* For our use case:

| Endpoint | Implemented in rfesi? | Notes |
|---|---|---|
| `/characters/{id}/skills/` | ✅ `skills().get_skills(id)` | Returns trained skills list |
| `/characters/{id}/implants/` | ✅ `clones().get_clone_implants(id)` | Returns active clone implant type IDs |
| `/universe/types/{type_id}/` | ❌ Not yet | Needed to resolve implant type ID → attribute bonus mapping |
| `/characters/{id}/skillqueue/` | ❌ Not yet | Need to add to `src/groups/skills.rs` or request upstream |
| `/characters/{id}/attributes/` | ❌ Not yet | Need to add to `src/groups/character.rs` — but scope is `esi-skills.read_skills.v1`, so it lives under skills conceptually |

**Action items for missing endpoints:** Either contribute PRs to `rfesi` (author welcomes them), or implement the missing calls as raw authenticated requests through `rfesi`'s client, deserializing manually. The effort is minimal — each is a single typed GET with a known response schema.

### Dependencies Added by rfesi

```toml
# Core (always pulled in)
reqwest = "0.12"          # HTTP client (json, http2 features)
tokio = "1"               # async runtime
serde / serde_json        # already dependencies of eve-remap
base64, sha2, rand        # PKCE + auth internals
thiserror                 # error types
log                       # logging facade

# Optional (default features)
jsonwebtoken = "10"       # JWT validation — can disable via feature flag
```

This adds `tokio` and `reqwest` back — the same crates we had before the offline cleanup. No OpenSSL if using `rustls-tls`.

---

## ESI Endpoints Relevant to eve-remap

All endpoints verified against the live OpenAPI 3.1 spec (`https://esi.evetech.net/meta/openapi.json`). This is a newer format than the Swagger 2.0 spec at `/latest/swagger.json` — the two differ in schema detail, and only the OpenAPI 3.1 version includes newly added remap-related fields on the attributes endpoint.

### 1. Character Attributes (with Neural Remap Data)

**GET** `/characters/{character_id}/attributes/`

- **Scope required:** `esi-skills.read_skills.v1`
- **Auth:** Authenticated
- **Caching:** Up to 3600 seconds
- **Returns current remapped attribute values plus neural interface state**

Response (OpenAPI 3.1 schema):
```json
{
    "charisma": 20,
    "intelligence": 20,
    "memory": 20,
    "perception": 20,
    "willpower": 20,
    "accrued_remap_cooldown_date": "2025-07-23T12:00:00Z",
    "bonus_remaps": 2,
    "last_remap_date": "2025-04-15T08:30:00Z"
}
```

Schema from spec (`CharactersCharacterIdAttributesGet`):

| Field | Type | Required? | Description |
|---|---|---|---|
| `charisma` | int64 | Yes | Current charisma value |
| `intelligence` | int64 | Yes | Current intelligence value |
| `memory` | int64 | Yes | Current memory value |
| `perception` | int64 | Yes | Current perception value |
| `willpower` | int64 | Yes | Current willpower value |
| `accrued_remap_cooldown_date` | date-time | No | When timed remap becomes available again |
| `bonus_remaps` | int64 | No | Number of bonus neural remaps remaining |
| `last_remap_date` | date-time | No | Datetime of last remap (including bonus usage) |

The five attribute fields are marked required. The three remap fields are not — they may be absent for characters with no remap history or in edge cases. Treat them as optional at runtime.

**Replaces:** `--attributes PER:MEM:WIL:INT:CHA`, `--remap-available Dd`, and `--bonus-remaps N` CLI flags when fetching from API.

---

### 2. Trained Skills

**GET** `/characters/{character_id}/skills/`

- **Scope required:** `esi-skills.read_skills.v1`
- **Auth:** Authenticated
- **Caching:** Up to 3600 seconds
- **Returns all trained skills with current SP and levels**

Response:
```json
{
    "skills": [
        {
            "skill_id": 34,
            "trained_skill_level": 5,
            "active_skill_level": 3,
            "sp": 8000
        }
    ],
    "total_sp": 100000,
    "unallocated_sp": 0
}
```

Fields relevant to us:
- `skill_id` — maps to SDE type IDs in our `assets/skills.json`
- `trained_skill_level` — highest level reached (what we care about for prerequisite checking)
- `sp` — current SP invested (useful for computing progress within a level)

**Use case:** Cross-reference with skill queue to determine starting level for each queued training target. If a skill is not in the trained list, it starts from level 0.

---

### 3. Skill Queue

**GET** `/characters/{character_id}/skillqueue/`

- **Scope required:** `esi-skills.read_skillqueue.v1`
- **Auth:** Authenticated
- **Caching:** Up to 3600 seconds
- **Returns the configured training queue**

Response:
```json
[
    {
        "skill_id": 34,
        "finished_level": 5,
        "queue_position": 0,
        "start_date": null,
        "finish_date": "2025-12-15T10:47:00Z"
    },
    {
        "skill_id": 1978,
        "finished_level": 3,
        "queue_position": 1,
        "start_date": "2025-12-15T10:47:00Z",
        "finish_date": "2026-01-20T08:30:00Z"
    }
]
```

Fields relevant to us:
- `skill_id` — SDE type ID
- `finished_level` — level that will be reached when this entry completes (this is the target level)
- `queue_position` — order in queue (0 = currently training)
- `finish_date` — ISO-8601 timestamp; `null` for position 0 if actively training with no finish date set

**Replaces:** `--queue FILE` when fetching from API. The optimizer can consume this directly and compute SP deltas based on current trained levels vs `finished_level`.

---

### 4. Active Clone Implants

There are two endpoints for implant data:

#### 4a. Simple Implant List

**GET** `/characters/{character_id}/implants/`

- **Scope required:** `esi-clones.read_implants.v1`
- **Auth:** Authenticated
- **Caching:** Up to 3600 seconds
- **Returns array of implant type IDs on the active clone**

Response:
```json
[22118, 22119, 22120, ...]
```

This returns only type IDs. To get attribute bonuses, each must be resolved through `/universe/types/{type_id}/` or looked up against local SDE data.

**rfesi implementation:** `clones().get_clone_implants(character_id)`

#### 4b. Full Clones (includes implants per jump clone)

**GET** `/characters/{character_id}/clones/`

- **Scope required:** `esi-clones.read_clones.v1`
- **Auth:** Authenticated
- **Caching:** Up to 3600 seconds
- **Returns all clones with their implant lists and locations**

Response:
```json
{
    "home_location": { "location_id": 60003463, "location_type": "station" },
    "last_clone_jump_date": "2025-01-01T00:00:00Z",
    "jump_clones": [
        {
            "jump_clone_id": 12345,
            "name": "My Clone",
            "location_id": 60003463,
            "location_type": "station",
            "implants": [22118, 22119]
        }
    ]
}
```

To determine which clone is *active*, the client must check the character's current location against clone locations. The simple `/implants/` endpoint (4a) already resolves this ambiguity server-side — it returns only implants on the active clone.

---

### 5. Universe Types (for resolving implant bonuses)

**GET** `/universe/types/{type_id}/`

- **Scope required:** None (public)
- **Auth:** Public
- **Caching:** Up to 3600 seconds

Response:
```json
{
    "type_id": 22118,
    "name": "Implant: Intelligence +1",
    "group_id": 786,
    "published": true,
    "description": "..."
}
```

This does NOT return attribute bonus data directly. The type name contains the bonus info as text ("+1"), but there's no structured field for it. To get proper attribute modifier data, you need SDE Dogma attributes (`/universe/types/{type_id}/dogma_attributes/`).

#### 5b. Type Dogma Attributes

**GET** `/universe/types/{type_id}/dogma_attributes/`

- **Scope required:** None (public)
- **Auth:** Public
- **Returns dogma attribute values for a type**

Response:
```json
[
    { "attribute_id": 984, "value": 1 },
    { "attribute_id": 985, "value": 0 }
]
```

Attribute IDs map to EVE's internal Dogma constants. For implants, these encode which game attribute is modified and by how much. However, mapping Dogma attribute IDs → character skill attributes (PER/MEM/WIL/INT/CHA) requires a lookup table that isn't part of ESI — it comes from the SDE.

**Recommendation:** Ship implant-to-bonus mapping as static `assets/implants.json` (already exists in repo). Use ESI only for fetching *which* implants are active on the clone, then resolve bonuses locally. This avoids N+1 API calls for each implant type.

---

## Neural Interface / Remap Cooldown — NOW Available via ESI (OpenAPI 3.1 only)

The OpenAPI 3.1 spec (`/meta/openapi.json`) adds three new fields to the attributes response that were **not present in the Swagger 2.0 spec** (`/latest/swagger.json`):

- `accrued_remap_cooldown_date` — when timed remap becomes available again
- `bonus_remaps` — number of bonus neural remaps remaining
- `last_remap_date` — datetime of last remap (including bonus usage)

This means PLAN.md Key Decision #4 ("ESI doesn't expose neural interface cooldown or bonus remap count") is **no longer true**. With full ESI integration, both `--remap-available Dd` and `--bonus-remaps N` can be auto-populated from a single `/attributes/` call. CLI flags serve as overrides when API data is unavailable.

---

## Required Scopes Summary

| Scope | Endpoints Covered | Needed For |
|---|---|---|
| `esi-skills.read_skills.v1` | `/attributes/`, `/skills/` | Current attributes + trained skill levels |
| `esi-skills.read_skillqueue.v1` | `/skillqueue/` | Active training queue |
| `esi-clones.read_implants.v1` | `/implants/` | Active clone implant list |

These are the same three scopes our previous implementation requested. No changes needed.

---

## Auth Flow (PKCE / Application Authentication)

EVE Online supports two OAuth flows:
1. **Authorization Code** — requires `client_id` + `client_secret` (confidential apps registered on developers.eveonline.com)
2. **Application Authentication (PKCE)** — only `client_id` required, no secret

For a public CLI tool distributed to end users, PKCE is the right choice. Users register their own app at <https://developers.eveonline.com/applications/> or use a shared client ID without a secret.

`rfesi` handles this via builder flags:

```rust
let esi = EsiBuilder::new()
    .user_agent("eve-remap/0.1.0")
    .client_id(env::var("ESI_CLIENT_ID").expect("set ESI_CLIENT_ID"))
    .callback_url("http://localhost:{port}/callback")
    .scope("esi-skills.read_skills.v1 esi-skills.read_skillqueue.v1 esi-clones.read_implants.v1")
    .enable_application_authentication(true)  // enables PKCE
    .build()?;

// Get authorization URL, open browser
let auth_info = esi.get_authorize_url()?;
open(&auth_info.authorization_url);

// Wait for callback with code, then exchange:
esi.authenticate_with_code(auth_code, &auth_info.pkce_verifier).await?;

// Now authenticated — access_token is set on the Esi instance
```

The old hand-rolled `src/auth/sso.rs` (~252 lines of PKCE + local TCP listener + token exchange) can be replaced entirely by `rfesi`.
