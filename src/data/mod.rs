pub mod models;
pub mod esi;

use std::path::PathBuf;
use anyhow::{Context, Result};
use serde_json;

use crate::data::models::{SkillRecord, ImplantRecord};

/// Load pre-parsed skill records from the assets directory.
pub fn load_skills() -> Result<Vec<SkillRecord>> {
    let path = asset_path("skills.json");
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read skills file at {}", path.display()))?;
    let skills: Vec<SkillRecord> = serde_json::from_str(&content)
        .context("Failed to parse skills JSON")?;
    Ok(skills)
}

/// Load pre-parsed implant records from the assets directory.
pub fn load_implants() -> Result<Vec<ImplantRecord>> {
    let path = asset_path("implants.json");
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read implants file at {}", path.display()))?;
    let implants: Vec<ImplantRecord> = serde_json::from_str(&content)
        .context("Failed to parse implants JSON")?;
    Ok(implants)
}

/// Resolve a path relative to the binary's `assets/` directory.
fn asset_path(filename: &str) -> PathBuf {
    // Try next to the binary first, then fall back to repo root.
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(PathBuf::from));
    if let Some(base) = exe_dir {
        let candidate = base.join("assets").join(filename);
        if candidate.exists() {
            return candidate;
        }
    }
    // Fallback: use CWD or manifest dir
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets").join(filename)
}
