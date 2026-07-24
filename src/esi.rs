use anyhow::{Context, Result};
use serde::Deserialize;

use crate::auth::StoredAccount;
use crate::data::models::BaseAttributes;

// ── Response Types ────────────────────────────────────────────────────────

/// GET /characters/{character_id}/attributes/
#[allow(dead_code)] // last_remap_date kept for schema completeness
#[derive(Debug, Deserialize)]
pub struct EsCharacterAttributes {
    pub perception: i64,
    pub memory: i64,
    pub willpower: i64,
    pub intelligence: i64,
    pub charisma: i64,
    #[serde(default)]
    pub accrued_remap_cooldown_date: Option<String>,
    #[serde(default)]
    pub bonus_remaps: Option<i64>,
    #[serde(default)]
    pub last_remap_date: Option<String>,
}

/// Single entry from GET /characters/{character_id}/skillqueue/
#[allow(dead_code)] // start_date kept for schema completeness
#[derive(Debug, Deserialize, Clone)]
pub struct EsSkillQueueEntry {
    pub skill_id: i32,
    pub finished_level: i32,
    pub queue_position: i32,
    /// Cumulative SP at the start of this level transition (blank→level_start_sp).
    pub level_start_sp: f64,
    /// Cumulative SP where training actually started for this entry.
    /// Equals level_start_sp if no progress; higher if partially trained.
    pub training_start_sp: f64,
    /// Cumulative SP at the end of this level transition (= level_start_sp + level SP).
    pub level_end_sp: f64,
    #[allow(dead_code)] // kept for schema completeness / date-based fallbacks
    pub start_date: Option<String>,
    pub finish_date: Option<String>,
}

/// Response from GET /characters/{character_id}/skills/ — we only need trained_skill_level map.
#[derive(Debug, Deserialize)]
struct EsSkillsResponse {
    skills: Vec<EsTrainedSkill>,
}

#[derive(Debug, Deserialize)]
struct EsTrainedSkill {
    pub skill_id: i32,
    pub trained_skill_level: i32,
}

// ── ESI Client Construction ───────────────────────────────────────────────

/// Build an rfesi client pre-loaded with stored tokens.
fn build_esi_client(account: &StoredAccount) -> Result<rfesi::prelude::Esi> {
    use rfesi::prelude::EsiBuilder;

    // access_expiration is in milliseconds (Unix epoch); our expires_at is seconds.
    let exp_ms = Some((account.expires_at * 1000.0) as i64);

    Ok(EsiBuilder::new()
        .user_agent("eve-remap")
        .client_id(&account.client_id)
        .callback_url("http://localhost/callback")
        .enable_application_authentication(true)
        .access_token(Some(&account.access_token))
        .refresh_token(Some(&account.refresh_token))
        .access_expiration(exp_ms)
        .build()
        .context("failed to build ESI client")?)
}

// ── Token Refresh ─────────────────────────────────────────────────────────

/// Attempt to refresh the token if expired. Returns updated StoredAccount on success.
pub async fn ensure_fresh_token(account: &StoredAccount) -> Result<StoredAccount> {
    let mut esi = build_esi_client(account)?;

    // Check expiration: access_expiration is millis, current time from rfesi helper.
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    if esi.access_expiration.map(|e| e > now_ms).unwrap_or(false) {
        return Ok(account.clone()); // still fresh
    }

    esi.refresh_access_token(None)
        .await
        .context("token refresh failed — try logging in again")?;

    let new_account = StoredAccount {
        character_id: account.character_id,
        character_name: account.character_name.clone(),
        access_token: esi
            .access_token
            .clone()
            .ok_or_else(|| anyhow::anyhow!("refresh succeeded but no access token returned"))?,
        refresh_token: esi
            .refresh_token
            .clone()
            .ok_or_else(|| anyhow::anyhow!("refresh succeeded but no refresh token returned"))?,
        expires_at: esi
            .access_expiration
            .map(|ms| ms as f64 / 1000.0)
            .unwrap_or(0.0),
        client_id: account.client_id.clone(),
    };
    Ok(new_account)
}

// ── Fetch Functions ───────────────────────────────────────────────────────

/// Fetch current attributes from ESI. Uses the provided rfesi client (must be authenticated).
pub async fn fetch_attributes(
    esi: &mut rfesi::prelude::Esi,
    character_id: u64,
) -> Result<EsCharacterAttributes> {
    use rfesi::prelude::RequestType;

    let endpoint = format!("latest/characters/{}/attributes/", character_id);

    esi.query::<EsCharacterAttributes>("GET", RequestType::Authenticated, &endpoint, None, None)
        .await
        .context("failed to fetch character attributes")
}

/// Fetch skill queue from ESI. Returns entries sorted by position.
pub async fn fetch_skillqueue(
    esi: &mut rfesi::prelude::Esi,
    character_id: u64,
) -> Result<Vec<EsSkillQueueEntry>> {
    use rfesi::prelude::RequestType;

    let endpoint = format!("latest/characters/{}/skillqueue/", character_id);

    let entries: Vec<EsSkillQueueEntry> = esi
        .query("GET", RequestType::Authenticated, &endpoint, None, None)
        .await
        .context("failed to fetch skill queue")?;

    Ok(entries)
}

/// Fetch trained skills and return a map of skill_id -> trained_skill_level.
pub async fn fetch_trained_skills_map(
    esi: &mut rfesi::prelude::Esi,
    character_id: u64,
) -> Result<std::collections::HashMap<u32, u8>> {
    use rfesi::prelude::RequestType;

    let endpoint = format!("latest/characters/{}/skills/", character_id);

    let resp: EsSkillsResponse = esi
        .query("GET", RequestType::Authenticated, &endpoint, None, None)
        .await
        .context("failed to fetch trained skills")?;

    Ok(resp
        .skills
        .into_iter()
        .filter(|s| s.trained_skill_level >= 1)
        .map(|s| (s.skill_id as u32, s.trained_skill_level as u8))
        .collect())
}

// ── High-Level Fetcher ────────────────────────────────────────────────────

/// All ESI data needed by the optimizer for a single character.
#[derive(Debug)]
pub struct CharacterData {
    /// Base attributes from neural interface (not including implants).
    pub base_attributes: Option<BaseAttributes>,
    /// Bonus remaps remaining.
    pub bonus_remaps: Option<u32>,
    /// Cooldown date string if available.
    pub accrued_remap_cooldown_date: Option<String>,
    /// Skill queue entries (sorted by position).
    pub skill_queue: Vec<EsSkillQueueEntry>,
    /// Active implant type IDs on current clone.
    pub active_implant_ids: Vec<u32>,
    #[allow(dead_code)] // kept for future use; queue SP fields are authoritative now
    pub trained_skills: std::collections::HashMap<u32, u8>,
}

/// Fetch all relevant character data from ESI in parallel where possible.
///
/// Returns a `CharacterData` with optional fields — None means the endpoint
/// returned no value or failed. Callers merge these with CLI overrides.
pub async fn fetch_character_data(account: &StoredAccount) -> Result<CharacterData> {
    let account = ensure_fresh_token(account).await?;
    let mut esi = build_esi_client(&account)?;
    let cid = account.character_id;

    // Sequential calls — each is a single authenticated GET; total latency ~1-2s.
    // Mutable borrows prevent tokio::join!, and rfesi tracks shared error-limit state.

    let attrs = fetch_attributes(&mut esi, cid).await.ok();
    let skill_queue = fetch_skillqueue(&mut esi, cid).await.unwrap_or_default();
    let trained_skills = fetch_trained_skills_map(&mut esi, cid)
        .await
        .unwrap_or_default();

    // Implants via raw query (rfesi group accessor not yet available)
    use rfesi::prelude::RequestType;
    let impl_endpoint = format!("latest/characters/{}/implants/", cid);
    let active_implant_ids: Vec<u32> = esi
        .query(
            "GET",
            RequestType::Authenticated,
            &impl_endpoint,
            None,
            None,
        )
        .await
        .unwrap_or_default();

    let base_attributes = attrs.as_ref().map(|a| BaseAttributes {
        perception: a.perception as u32,
        memory: a.memory as u32,
        willpower: a.willpower as u32,
        intelligence: a.intelligence as u32,
        charisma: a.charisma as u32,
    });

    let bonus_remaps = attrs
        .as_ref()
        .and_then(|a| a.bonus_remaps.map(|v| v.max(0) as u32));

    let accrued_remap_cooldown_date = attrs.and_then(|a| a.accrued_remap_cooldown_date);

    Ok(CharacterData {
        base_attributes,
        bonus_remaps,
        accrued_remap_cooldown_date,
        skill_queue,
        active_implant_ids,
        trained_skills,
    })
}
