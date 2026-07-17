use anyhow::{Context, Result};
pub use sso::run_pkce_flow;

mod sso;

// ── JWT Claims ───────────────────────────────────────────────────────────

/// Parsed claims from an EVE access token JWT payload.
#[derive(Debug, Clone)]
pub struct JwtClaims {
    pub owner_character_id: u64,
    pub character_name: String,
    pub scopes: Vec<String>,
    /// Expiration timestamp (seconds since epoch) from the JWT.
    pub expires_at: u64,
}

/// Decode the payload of a JWT access token without verifying the signature.
///
/// This is safe for introspection since we just need to read what the server told us.
/// The token will be validated on next API call anyway.
pub fn decode_jwt_token(token: &str) -> Option<JwtClaims> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() < 2 {
        return None;
    }

    // EVE uses base64url without padding for the payload.
    let raw = base64::Engine::decode(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
        parts[1],
    ).ok()?;

    let text = std::str::from_utf8(&raw).ok()?;

    #[derive(serde::Deserialize)]
    struct RawClaims {
        /// e.g. "CHARACTER:EVE:1092366687"
        sub: String,
        /// Character display name, e.g. "Test Pilot"
        name: Option<String>,
        /// Scopes as array of strings, e.g. ["esi-skills.read_skills.v1", ...]
        scp: Vec<String>,
        /// Expiration timestamp (seconds since epoch)
        exp: u64,
        /// Issued-at timestamp
        #[allow(dead_code)] iat: u64,
    }

    let claims: RawClaims = serde_json::from_str(text).ok()?;

    // Extract character ID from sub: "CHARACTER:EVE:<id>"
    let char_id = claims.sub.split(':')
        .last()
        .and_then(|s| s.parse::<u64>().ok())?;

    Some(JwtClaims {
        owner_character_id: char_id,
        character_name: claims.name.unwrap_or_else(|| format!("Character {}", char_id)),
        scopes: claims.scp,
        expires_at: claims.exp,
    })
}

// ── Token Store ──────────────────────────────────────────────────────────

/// A single stored account entry.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StoredAccountEntry {
    pub character_id: u64,
    /// Character name resolved from ESI (may be empty until first API call).
    pub character_name: String,
    pub access_token: String,
    pub refresh_token: String,
    /// Seconds since epoch when the access token expires.
    pub expires_at: u64,
    pub scopes: Vec<String>,
    /// When this entry was created/last updated.
    pub created_at: u64,
}

fn accounts_dir() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    std::path::PathBuf::from(home).join(".config").join("eve-remap")
}

fn accounts_path() -> std::path::PathBuf {
    accounts_dir().join("accounts.json")
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Load all stored account entries. Returns empty vec if file doesn't exist.
pub fn load_accounts() -> Result<Vec<StoredAccountEntry>> {
    let path = accounts_path();
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let accounts: Vec<StoredAccountEntry> = serde_json::from_str(&content)
        .context("Invalid accounts JSON format")?;
    Ok(accounts)
}

fn save_accounts_inner(accounts: &[StoredAccountEntry]) -> Result<()> {
    let dir = accounts_dir();
    std::fs::create_dir_all(&dir).context("Failed to create config directory")?;
    let content = serde_json::to_string_pretty(accounts)
        .context("Failed to serialize accounts")?;
    std::fs::write(accounts_path(), content)
        .context("Failed to write accounts file")?;
    Ok(())
}
// ── Path-aware helpers (for testing) ────────────────────────────────────

#[cfg(test)]
fn load_accounts_at(path: &std::path::Path) -> Result<Vec<StoredAccountEntry>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?;
    let accounts: Vec<StoredAccountEntry> = serde_json::from_str(&content).context("Invalid accounts JSON")?;
    Ok(accounts)
}

#[cfg(test)]
fn save_accounts_at(accounts: &[StoredAccountEntry], dir: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(dir).context("Failed to create config directory")?;
    let content = serde_json::to_string_pretty(accounts).context("Failed to serialize accounts")?;
    std::fs::write(dir.join("accounts.json"), content).context("Failed to write accounts file")?;
    Ok(())
}

/// Save or update an account entry (replaces any existing entry with the same character ID).
pub fn save_account(entry: StoredAccountEntry) -> Result<()> {
    let mut accounts = load_accounts()?;
    // Remove existing entry for this character if present.
    accounts.retain(|a| a.character_id != entry.character_id);
    accounts.push(entry);
    save_accounts_inner(&accounts)
}

/// Remove an account by character ID. Returns true if something was removed.
pub fn remove_account(char_id: u64) -> Result<bool> {
    let mut accounts = load_accounts()?;
    let len_before = accounts.len();
    accounts.retain(|a| a.character_id != char_id);
    if accounts.len() < len_before {
        save_accounts_inner(&accounts)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// List all characters as (id, name) pairs.
pub fn list_characters() -> Result<Vec<(u64, String)>> {
    let accounts = load_accounts()?;
    Ok(accounts.iter().map(|a| (a.character_id, a.character_name.clone())).collect())
}

/// Find the first non-expired account and return its access token + character ID.
pub fn find_valid_token() -> Result<Option<(String, u64)>> {
    let now = now_secs();
    for account in load_accounts()? {
        if account.expires_at > now {
            return Ok(Some((account.access_token, account.character_id)));
        }
    }
    // Also check expired tokens — they might still work until ESI rejects them.
    if let Some(account) = load_accounts()?.into_iter().next() {
        return Ok(Some((account.access_token, account.character_id)));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_jwt_token_minimal() {
        let payload = serde_json::json!({
            "sub": "CHARACTER:EVE:90123456",
            "name": "Test Pilot",
            "scp": ["esi-skills.read_skills.v1"],
            "exp": 999999999u64,
            "iat": 1700000000
        });
        let encoded_payload = base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            payload.to_string(),
        );
        let token = format!("{}.{}", "header_b64", encoded_payload);

        let claims = decode_jwt_token(&token).expect("Should parse valid JWT");
        assert_eq!(claims.owner_character_id, 90123456);
        assert_eq!(claims.character_name, "Test Pilot");
        assert!(claims.scopes.contains(&"esi-skills.read_skills.v1".to_string()));
    }

    #[test]
    fn test_decode_jwt_token_malformed_returns_none() {
        assert!(decode_jwt_token("").is_none());
        assert!(decode_jwt_token("not.a.jwt.token").is_none());
        assert!(decode_jwt_token("a.b.c").is_none()); // b is not valid base64 JSON
    }

    #[test]
    fn test_accounts_roundtrip() -> Result<()> {
        let tmp_dir = tempfile::tempdir()?;
        let config_dir = tmp_dir.path().join(".config").join("eve-remap");

        let entry = StoredAccountEntry {
            character_id: 12345,
            character_name: "Test Pilot".to_string(),
            access_token: "fake_token_abc".to_string(),
            refresh_token: "fake_refresh_xyz".to_string(),
            expires_at: now_secs() + 3600,
            scopes: vec!["esi-skills.read_skills.v1".to_string()],
            created_at: now_secs(),
        };

        save_accounts_at(&[entry.clone()], &config_dir)?;
        let accounts = load_accounts_at(&config_dir.join("accounts.json"))?;
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].character_id, 12345);
        assert_eq!(accounts[0].character_name, "Test Pilot");
        assert_eq!(accounts[0].access_token, "fake_token_abc");

        // Verify removal logic.
        let mut filtered = accounts;
        filtered.retain(|a| a.character_id != 12345);
        assert_eq!(filtered.len(), 0);

        Ok(())
    }

    #[test]
    fn test_find_valid_token_with_expired_and_fresh() -> Result<()> {
        let tmp_dir = tempfile::tempdir()?;
        let config_dir = tmp_dir.path().join(".config").join("eve-remap");

        let expired = StoredAccountEntry {
            character_id: 999,
            character_name: "Expired".to_string(),
            access_token: "old_token".to_string(),
            refresh_token: "".to_string(),
            expires_at: now_secs() - 100,
            scopes: vec![],
            created_at: now_secs(),
        };
        let fresh = StoredAccountEntry {
            character_id: 888,
            character_name: "Fresh".to_string(),
            access_token: "new_token".to_string(),
            refresh_token: "refresh_new".to_string(),
            expires_at: now_secs() + 3600,
            scopes: vec!["esi-skills.read_skills.v1".to_string()],
            created_at: now_secs(),
        };

        save_accounts_at(&[expired.clone(), fresh.clone()], &config_dir)?;
        let accounts = load_accounts_at(&config_dir.join("accounts.json"))?;

        // Simulate find_valid_token logic.
        let now = now_secs();
        let valid: Vec<_> = accounts.iter().filter(|a| a.expires_at > now).collect();
        assert_eq!(valid.len(), 1);
        assert_eq!(valid[0].character_id, 888);
        assert_eq!(valid[0].access_token, "new_token");

        Ok(())
    }
}
