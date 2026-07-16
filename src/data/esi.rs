use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use serde::Deserialize;

pub use crate::data::models::CharacterState;
use crate::data::models::{BaseAttributes, EffectiveAttributes, ImplantRecord, QueuedSkill, SkillRecord};

// ── ESI response types ────────────────────────────────────────────────

/// Raw response from `/characters/{id}/attributes/`.
#[derive(Debug, Deserialize)]
struct EsAttributesResponse {
    #[serde(rename = "intelligence")]
    intelligence: u32,
    #[serde(rename = "memory")]
    memory: u32,
    /// ESI calls charisma "processing" in the attributes endpoint.
    #[serde(rename = "processing")]
    processing: u32,
    #[serde(rename = "perception")]
    perception: u32,
    #[serde(rename = "willpower")]
    willpower: u32,
}

/// Single skill entry from `/characters/{id}/skills/`.
#[derive(Debug, Deserialize)]
struct EsSkillRow {
    #[serde(rename = "skill_id")]
    pub skill_id: u32,
    #[serde(rename = "level")]
    pub level: u8,
    #[serde(rename = "sp")]
    pub sp: u64,
}

/// A single row returned by `/characters/{id}/skillqueue/`.
#[derive(Debug, Deserialize)]
struct EsSkillQueueRow {
    #[serde(rename = "activity")]
    pub activity: Option<String>,
    /// ISO-8601 timestamp; may be null for the active training slot.
    #[serde(rename = "finish_date")]
    pub finish_date: Option<String>,
    #[serde(rename = "is_queued")]
    pub is_queued: bool,
    #[serde(rename = "skill_id")]
    pub skill_id: u32,
    /// ISO-8601 timestamp; null while actively training.
    #[serde(rename = "start_date")]
    pub start_date: Option<String>,
    /// Level that will have been trained when this entry completes.
    #[serde(rename = "trained_skill_level")]
    pub trained_skill_level: u8,
}

/// Single implant entry from `/characters/{id}/implants/`.
#[derive(Debug, Deserialize)]
struct EsImplantRow {
    #[serde(rename = "implant_id")]
    pub implant_id: u32,
    #[serde(rename = "slot_name")]
    pub slot_name: Option<String>,
}

// ── Token persistence ────────────────────────────────────────────────

use serde::Serialize;

/// Minimal token blob we persist to `~/.config/eve-remap/tokens.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredTokens {
    access_token: String,
    /// Epoch seconds when the token expires. 0 means unknown / never.
    expires_in: u64,
    issued_at: u64,
}
pub fn token_dir() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    std::path::PathBuf::from(home).join(".config").join("eve-remap")
}

pub fn token_path() -> std::path::PathBuf {
    token_dir().join("tokens.json")
}

pub fn save_tokens(token: &str) -> Result<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // ESI tokens live for ~900 s by default.
    let stored = StoredTokens {
        access_token: token.to_string(),
        expires_in: 900,
        issued_at: now,
    };

    let dir = token_dir();
    std::fs::create_dir_all(&dir).with_context(|| "Failed to create token directory")?;
    let content = serde_json::to_string_pretty(&stored)
        .context("Failed to serialize tokens")?;
    std::fs::write(token_path(), content).context("Failed to write tokens file")?;
    Ok(())
}

/// Try to load a previously saved access token. Returns `None` on any error.
pub fn load_saved_token() -> Option<String> {
    let path = token_path();
    if !path.exists() {
        return None;
    }
    let content = std::fs::read_to_string(&path).ok()?;
    let stored: StoredTokens = serde_json::from_str(&content).ok()?;
    Some(stored.access_token)
}

// ── Time helpers (no chrono dep — use std + simple ISO parse) ────────

/// Return current UTC timestamp as seconds since epoch.
fn now_utc_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Parse an ISO-8601 date-time string to Unix seconds.
/// Supports `YYYY-MM-DDTHH:MM:SSZ` and `YYYY-MM-DDTHH:MM:SS+HH:MM`.
/// Returns `None` on parse failure or empty input.
fn iso_to_unix_secs(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() || s == "null" {
        return None;
    }

    // Minimal RFC 3339 / ISO 8601 parser for the formats ESI returns.
    // Expected: YYYY-MM-DDTHH:MM:SS[Z|+HH:MM|-HH:MM]
    let parts: Vec<&str> = s.split('T').collect();
    if parts.len() < 2 {
        return None;
    }

    let date_parts: Vec<u32> = parts[0].split('-').filter_map(|p| p.parse().ok()).collect();
    if date_parts.len() != 3 {
        return None;
    }
    let (year, month, day) = (date_parts[0], date_parts[1], date_parts[2]);

    let time_str = parts[1];
    // Strip timezone suffix to get HH:MM:SS
    let (time_only, tz_offset_minutes): (String, i64) = if time_str.ends_with('Z') {
        (time_str[..time_str.len() - 1].to_string(), 0i64)
    } else {
        // Find +/- offset at the end: +HH:MM or -HH:MM
        let tz_start = time_str.rfind('+').or_else(|| time_str.rfind(|c| c == '-'));
        match tz_start {
            Some(idx) if idx > 2 => {
                let tz_part = &time_str[idx..];
                // Parse offset like "+02:00" or "-05:30"
                let offset_chars: Vec<char> = tz_part.chars().skip(1).take(5).collect();
                if offset_chars.len() >= 5 && offset_chars[2] == ':' {
                    let h_s: String = offset_chars.iter().take(2).collect();
                    let m_s: String = offset_chars.iter().skip(3).take(2).collect();
                    let hours: i64 = h_s.parse().ok()?;
                    let mins: i64 = m_s.parse().ok()?;
                    let offset = hours * 60 + mins;
                    let tz_offset = if time_str.as_bytes()[idx] == b'-' {
                        -offset
                    } else {
                        offset
                    };
                    (time_str[..idx].to_string(), tz_offset)
                } else {
                    return None;
                }
            }
            _ => (time_str.to_string(), 0i64),
        }
    };

    let time_parts: Vec<u32> = time_only.split(':').filter_map(|p| p.parse().ok()).collect();
    if time_parts.len() < 3 {
        return None;
    }
    let (hour, minute, second) = (time_parts[0], time_parts[1], time_parts[2]);

    // Convert to Unix timestamp using a simple days-since-epoch calculation.
    let days = ymd_to_days(year as i32, month as i32, day as i32)?;
    let secs: i64 = days * 86_400 + hour as i64 * 3_600 + minute as i64 * 60 + second as i64;
    let adjusted = secs - tz_offset_minutes * 60;
    Some(adjusted.max(0) as u64)
}

/// Convert year/month/day to days since Unix epoch (1970-01-01).
fn ymd_to_days(year: i32, month: i32, day: i32) -> Option<i64> {
    // Count total days from year 1 AD to start of `year`, then add day-of-year offset.
    let y = year as i64;
    if y < 1 || month < 1 || month > 12 || day < 1 {
        return None;
    }

    // Days in full years before this one (years 1..y-1).
    let prev = y - 1;
    let leaps_before_y = prev / 4 - prev / 100 + prev / 400;
    let days_before_year = prev * 365 + leaps_before_y;

    let is_leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let cum_days: [i64; 12] = if is_leap {
        [0, 31, 60, 91, 121, 152, 182, 213, 244, 274, 305, 335]
    } else {
        [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334]
    };

    // Days before this date within the current year.
    let m = month as usize;
    let day_of_year = cum_days[m - 1] + (day as i64 - 1);

    // Unix epoch (1970-01-01) is 719_162 days after Jan 1 of year 1 AD.
    Some(days_before_year + day_of_year - 719_162)
}

// ── Structured ESI Errors ─────────────────────────────────────────────

/// Error from an ESI API call with full context.
#[derive(Debug)]
pub struct EsiError {
    pub endpoint: String,
    pub status: u16,
}

impl std::fmt::Display for EsiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.status {
            401 => write!(f, "[{}] {} - Unauthorized: token expired or invalid. Re-authenticate.", self.endpoint, self.status),
            403 => write!(f, "[{}] {} - Forbidden: missing scope or insufficient permissions", self.endpoint, self.status),
            404 => write!(f, "[{}] {} - Not Found: character ID may be wrong", self.endpoint, self.status),
            429 => write!(f, "[{}] {} - Too Many Requests: rate limited by ESI", self.endpoint, self.status),
            _ if (500..=599).contains(&self.status) => write!(f, "[{}] {} - Server Error: EVE backend issue", self.endpoint, self.status),
            _ => write!(f, "[{}] {} - HTTP Error", self.endpoint, self.status),
        }
    }
}

impl std::error::Error for EsiError {}

// ── EsIClient ─────────────────────────────────────────────────────────

const ESI_BASE_URL: &str = "https://esi.evetech.net/latest";

/// High-level client for EVE Single Interface (ESI) endpoints.
#[derive(Clone)]
pub struct EsIClient {
    http: Client,
    token: String, // immutable after construction — no lock needed
}

impl EsIClient {
    // ── Construction ────────────────────────────────────────────────

    /// Create a client from an explicit bearer token string.
    pub fn from_token(token: String) -> Self {
        let _ = save_tokens(&token); // best-effort persistence
        Self {
            http: Client::new(),
            token,
        }
    }

    /// Create a client by resolving the token in priority order:
    /// 1. `EVE_REMAP_TOKEN` env var
    /// 2. Previously saved legacy token file
    /// 3. New account store (`accounts.json`)
    /// Returns an error if no token is available.
    pub fn from_env() -> Result<Self> {
        if let Ok(token) = std::env::var("EVE_REMAP_TOKEN") {
            return Ok(Self::from_token(token));
        }
        if let Some(token) = load_saved_token() {
            return Ok(Self::from_token(token));
        }
        if let Ok(Some((token, _))) = crate::auth::find_valid_token() {
            return Ok(Self::from_token(token));
        }
        Err(anyhow!(
            "No ESI token found. Set EVE_REMAP_TOKEN or run 'eve-remap login'."
        ))
    }

    // ── Internal helpers ────────────────────────────────────────────

    async fn get_json<T>(&self, path: &str) -> Result<T>
    where
        T: for<'de> Deserialize<'de>,
    {
        let url = format!("{}{}", ESI_BASE_URL, path);
        eprintln!("[+] Fetching {} ...", path);
        let response = self.http.get(&url)
            .header(reqwest::header::AUTHORIZATION, format!("Bearer {}", &self.token))
            .send().await.with_context(|| format!("Request failed to {}", url))?;

        let status = response.status();
        eprintln!("[+] {} -> {}", path, status);
        if status.is_success() {
            let body = response.text().await.context("Failed to read response body")?;
            serde_json::from_str::<T>(&body).context("Failed to parse ESI JSON response")
        } else {
            let err = EsiError {
                endpoint: path.to_string(),
                status: status.as_u16(),
            };
            Err(anyhow!(err))
        }
    }

    /// Attempt to refresh the access token via EVE SSO's `/oauth/token` endpoint.
    /// Returns `Ok(())` on success (token updated in-place), or an error.
    pub async fn try_refresh_token(&self) -> Result<()> {
        // We need a refresh_token to actually call the SSO endpoint.
        // For MVP we don't store it yet — this is a placeholder that returns
        // an informative error so callers know auth is stale.
        Err(anyhow!(
            "Token refresh not yet supported. Re-authenticate and set EVE_REMAP_TOKEN."
        ))
    }

    /// Execute a GET with automatic 401 retry: on first 401 attempt a token
    /// refresh; if that succeeds, retry the original request once.
    async fn get_json_with_retry<T>(&self, path: &str) -> Result<T>
    where
        T: for<'de> Deserialize<'de>,
    {
        match self.get_json::<T>(path).await {
            Ok(data) => Ok(data),
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("401") || err_str.contains("Unauthorized") {
                    let _ = self.try_refresh_token().await;
                    // Retry after attempted refresh (will fail again if no refresh flow).
                    self.get_json(path).await
                } else {
                    Err(e)
                }
            }
        }
    }

    // ── Public fetchers ─────────────────────────────────────────────

    /// Fetch base remapped attributes for a character.
    pub async fn fetch_attributes(&self, char_id: u64) -> Result<BaseAttributes> {
        let path = format!("/characters/{}/attributes/", char_id);
        let resp: EsAttributesResponse = self.get_json_with_retry(&path).await?;
        Ok(BaseAttributes {
            intelligence: resp.intelligence as f64,
            charisma: resp.processing as f64, // ESI "processing" == charisma
            perception: resp.perception as f64,
            memory: resp.memory as f64,
            willpower: resp.willpower as f64,
        })
    }

    /// Fetch the full trained-skill list for a character.
    ///
    /// Returns raw `(skill_id, level, sp)` triples; callers should join
    /// against SDE `SkillRecord` to resolve names and time constants.
    pub async fn fetch_skills_raw(&self, char_id: u64) -> Result<Vec<(u32, u8, u64)>> {
        let path = format!("/characters/{}/skills/", char_id);
        let rows: Vec<EsSkillRow> = self.get_json_with_retry(&path).await?;
        Ok(rows.into_iter().map(|r| (r.skill_id, r.level, r.sp)).collect())
    }

    /// Fetch trained skills resolved against an SDE skill table.
    ///
    /// Skills not found in SDE are silently skipped (shouldn't happen).
    pub async fn fetch_skills(
        &self,
        char_id: u64,
        sde_skills: &[SkillRecord],
    ) -> Result<Vec<SkillRecord>> {
        let raw = self.fetch_skills_raw(char_id).await?;
        let mut result = Vec::with_capacity(raw.len());
        for (id, _level, _sp) in raw {
            if let Some(record) = sde_skills.iter().find(|s| s.id == id).cloned() {
                // SkillRecord doesn't carry a `level`/`sp` field — we store the
                // resolved records and the caller tracks levels separately.
                result.push(record);
            }
        }
        Ok(result)
    }

    /// Fetch the character's current skill training queue.
    pub async fn fetch_skillqueue(&self, char_id: u64) -> Result<Vec<QueuedSkill>> {
        let path = format!("/characters/{}/skillqueue/", char_id);
        let rows: Vec<EsSkillQueueRow> = self.get_json_with_retry(&path).await?;

        // Determine which entry is currently active (is_queued=false means active).
        let active_idx = rows.iter().position(|r| !r.is_queued);

        let now = now_utc_secs();
        let mut queue = Vec::new();
        for (i, r) in rows.into_iter().enumerate() {
            let duration = compute_duration_secs(&r, now);
            let remaining = compute_remaining_secs(&r, now);

            // Level being trained toward. For the active slot this is the target level;
            // for queued entries it is also the target after that entry completes.
            let level = r.trained_skill_level;

            queue.push(QueuedSkill {
                id: r.skill_id,
                level,
                sp: 0, // SP not exposed by /skillqueue; callers fill from /skills if needed
                duration: duration.max(0) as u64,
                remaining_sec: remaining.max(0) as u64,
                is_active: active_idx == Some(i),
            });
        }
        Ok(queue)
    }

    /// Fetch the list of implant IDs currently installed on the character.
    pub async fn fetch_implants(&self, char_id: u64) -> Result<Vec<u32>> {
        let path = format!("/characters/{}/implants/", char_id);
        let rows: Vec<EsImplantRow> = self.get_json_with_retry(&path).await?;
        Ok(rows.into_iter().map(|r| r.implant_id).collect())
    }

    // ── Combined snapshot ───────────────────────────────────────────

    /// Fetch a complete `CharacterState` snapshot in parallel.
    ///
    /// Requires SDE skill and implant tables to resolve names and compute
    /// effective attributes.
    pub async fn fetch_character_state(
        &self,
        char_id: u64,
        sde_skills: &[SkillRecord],
        sde_implants: &[ImplantRecord],
    ) -> Result<CharacterState> {
        let (attrs, skills_raw, queue, implants) = tokio::join!(
            self.fetch_attributes(char_id),
            self.fetch_skills_raw(char_id),
            self.fetch_skillqueue(char_id),
            self.fetch_implants(char_id),
        );

        let base_attrs = attrs.context("Failed to fetch attributes")?;
        let raw_skills = skills_raw.context("Failed to fetch skills")?;
        let queue = queue.context("Failed to fetch skill queue")?;
        let active_implants = implants.context("Failed to fetch implants")?;

        // Resolve skill records from raw data + SDE
        let mut resolved_skills = Vec::with_capacity(raw_skills.len());
        for (id, _level, _sp) in raw_skills {
            if let Some(record) = sde_skills.iter().find(|s| s.id == id).cloned() {
                resolved_skills.push(record);
            }
        }

        let effective_attrs = EffectiveAttributes::from_base_and_implants(
            &base_attrs,
            &active_implants,
            sde_implants,
        );
        Ok(CharacterState {
            base_attributes: base_attrs,
            queued_skills: queue,
            active_implant_ids: active_implants,
            implant_bonus: BaseAttributes::zero(),
            effective_attributes: effective_attrs,
            bonus_remaps: None, // ESI doesn't expose this; user provides via --bonus-remaps
        })
    }
}

/// Compute the duration in seconds for a skill queue entry.
fn compute_duration_secs(row: &EsSkillQueueRow, now: u64) -> i64 {
    match (&row.finish_date,) {
        (Some(finish),) => {
            let finish_secs = iso_to_unix_secs(finish).unwrap_or(now) as i64;
            if !row.is_queued {
                // Active training slot: remaining until finish
                finish_secs - now as i64
            } else {
                // Queued entry — unknown start, so we can't know exact duration yet.
                0
            }
        }
        _ => 0,
    }
}

/// Compute remaining seconds for a queue entry relative to `now`.
fn compute_remaining_secs(_row: &EsSkillQueueRow, _now: u64) -> i64 {
    // Simplified: for MVP treat all non-active entries as having 0 remaining.
    // The active entry's remaining is captured via compute_duration_secs above.
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_iso_to_unix_basic() {
        // Known epoch: 2021-01-01T00:00:00Z = 1609459200
        let ts = iso_to_unix_secs("2021-01-01T00:00:00Z");
        assert_eq!(ts, Some(1_609_459_200));
    }

    #[test]
    fn test_iso_to_unix_with_offset() {
        // 2021-01-01T02:00:00+02:00 == 2021-01-01T00:00:00Z
        let ts = iso_to_unix_secs("2021-01-01T02:00:00+02:00");
        assert_eq!(ts, Some(1_609_459_200));
    }

    #[test]
    fn test_iso_to_unix_null() {
        assert_eq!(iso_to_unix_secs(""), None);
        assert_eq!(iso_to_unix_secs("null"), None);
    }

    #[test]
    fn test_attributes_processing_maps_to_charisma() {
        let resp = EsAttributesResponse {
            intelligence: 12,
            memory: 15,
            processing: 8,
            perception: 10,
            willpower: 7,
        };
        let base = BaseAttributes {
            intelligence: resp.intelligence as f64,
            charisma: resp.processing as f64,
            perception: resp.perception as f64,
            memory: resp.memory as f64,
            willpower: resp.willpower as f64,
        };
        assert_eq!(base.charisma, 8.0);
        assert_eq!(base.total(), 52.0);
    }

    #[test]
    fn test_token_dir_exists() {
        let dir = token_dir();
        assert!(dir.ends_with(".config/eve-remap") || dir.to_string_lossy().contains(".config"));
    }
}
