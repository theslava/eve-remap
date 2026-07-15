use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;

use crate::data::models::{Attribute, ImplantRecord, SkillRecord};

/// Dogma attribute IDs that map to character attributes for skills' primary/secondary.
const DOGMA_ATTR_MAP: [(u64, Attribute); 5] = [
    (164, Attribute::Charisma),
    (165, Attribute::Intelligence),
    (166, Attribute::Memory),
    (167, Attribute::Perception),
    (168, Attribute::Willpower),
];

/// Implant bonus dogma attribute IDs → character attribute mapping.
const IMPLANT_BONUS_IDS: [(u64, Attribute); 5] = [
    (175, Attribute::Charisma),
    (176, Attribute::Intelligence),
    (177, Attribute::Memory),
    (178, Attribute::Perception),
    (179, Attribute::Willpower),
];

pub struct SdeParser {
    types: HashMap<u64, Value>,
    groups: HashMap<u64, Value>,
    char_attrs: HashMap<u64, Value>,
    dogma_attrs: HashMap<u64, Value>,
    type_dogma: HashMap<u64, Value>,
}

impl SdeParser {
    /// Parse all JSONL files from an extracted SDE directory.
    pub fn from_dir(dir: &std::path::Path) -> Result<Self> {
        let types = load_jsonl_into_map(dir.join("types.jsonl"))?;
        let groups = load_jsonl_into_map(dir.join("groups.jsonl"))?;
        let char_attrs = load_jsonl_into_map(dir.join("characterAttributes.jsonl"))?;
        let dogma_attrs = load_jsonl_into_map(dir.join("dogmaAttributes.jsonl"))?;
        let type_dogma = load_jsonl_into_map(dir.join("typeDogma.jsonl"))?;

        Ok(SdeParser {
            types,
            groups,
            char_attrs,
            dogma_attrs,
            type_dogma,
        })
    }

    /// Find the skill category ID (category whose name is "Skill").
    fn find_skill_category_id(&self) -> Option<u64> {
        // Categories are stored in groups via categoryID; we need to scan for
        // groups with category 16 (known value), but let's discover it properly.
        // The Skill category has _key=16, and groups have categoryID pointing to their parent category.
        // We know skills have categoryID == 16 from SDE schema.
        Some(16)
    }

    /// Extract all published skills into SkillRecord structs.
    pub fn extract_skills(&self) -> Result<Vec<SkillRecord>> {
        let skill_cat_id = self.find_skill_category_id()
            .context("Could not determine skill category ID")?;

        // Collect group IDs belonging to the skill category.
        let skill_group_ids: std::collections::HashSet<u64> = self.groups.iter()
            .filter(|(_, g)| {
                g.get("categoryID")
                    .and_then(|v| v.as_u64())
                    .map_or(false, |cid| cid == skill_cat_id)
            })
            .map(|(id, _)| *id)
            .collect();

        let mut skills = Vec::new();
        for (tid, t) in &self.types {
            let group_id = match t.get("groupID").and_then(|v| v.as_u64()) {
                Some(gid) => gid,
                None => continue,
            };
            if !skill_group_ids.contains(&group_id) {
                continue;
            }
            if !t.get("published").map_or(true, |v| v.as_bool().unwrap_or(false)) {
                continue;
            }

            let name = t.get("name")
                .and_then(|n| n.get("en"))
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string();

            let td = match self.type_dogma.get(tid.as_ref()) {
                Some(v) => v,
                None => continue,
            };

            // Extract dogma attribute values.
            let mut dogma_map: HashMap<u64, f64> = HashMap::new();
            if let Some(attrs) = td.get("dogmaAttributes").and_then(|a| a.as_array()) {
                for attr in attrs {
                    if let (Some(aid), Some(val)) = (
                        attr.get("attributeID").and_then(|v| v.as_u64()),
                        attr.get("value").and_then(|v| v.as_f64()),
                    ) {
                        dogma_map.insert(aid, val);
                    }
                }
            }

            let primary_attr_id = *dogma_map.get(&180).context("Missing primaryAttribute (dogma 180)")?;
            let secondary_attr_id = *dogma_map.get(&181).context("Missing secondaryAttribute (dogma 181)")?;
            let time_constant = *dogma_map.get(&275).context("Missing skillTimeConstant (dogma 275)")?;

            let primary = match DOGMA_ATTR_MAP.iter().find(|(did, _)| **did as f64 == primary_attr_id) {
                Some((_, attr)) => *attr,
                None => anyhow::bail!("Unknown primary attribute dogma ID value: {}", primary_attr_id),
            };
            let secondary = match DOGMA_ATTR_MAP.iter().find(|(did, _)| **did as f64 == secondary_attr_id) {
                Some((_, attr)) => *attr,
                None => anyhow::bail!("Unknown secondary attribute dogma ID value: {}", secondary_attr_id),
            };

            skills.push(SkillRecord {
                id: *tid as u32,
                name,
                primary_attribute: primary,
                secondary_attribute: secondary,
                skill_time_constant: time_constant,
            });
        }

        skills.sort_by_key(|s| s.id);
        Ok(skills)
    }

    /// Extract all implants with attribute bonuses.
    pub fn extract_implants(&self) -> Result<Vec<ImplantRecord>> {
        // Find implant groups by name heuristic.
        let implant_group_ids: std::collections::HashSet<u64> = self.groups.iter()
            .filter(|(_, g)| {
                g.get("name")
                    .and_then(|n| n.get("en"))
                    .and_then(|s| s.as_str())
                    .map_or(false, |name| name.to_lowercase().contains("implant"))
            })
            .map(|(id, _)| *id)
            .collect();

        let mut implants = Vec::new();
        for (tid, t) in &self.types {
            let group_id = match t.get("groupID").and_then(|v| v.as_u64()) {
                Some(gid) => gid,
                None => continue,
            };
            if !implant_group_ids.contains(&group_id) {
                continue;
            }
            if !t.get("published").map_or(true, |v| v.as_bool().unwrap_or(false)) {
                continue;
            }

            let td = match self.type_dogma.get(tid.as_ref()) {
                Some(v) => v,
                None => continue,
            };

            // Extract dogma attribute values.
            let mut dogma_map: HashMap<u64, f64> = HashMap::new();
            if let Some(attrs) = td.get("dogmaAttributes").and_then(|a| a.as_array()) {
                for attr in attrs {
                    if let (Some(aid), Some(val)) = (
                        attr.get("attributeID").and_then(|v| v.as_u64()),
                        attr.get("value").and_then(|v| v.as_f64()),
                    ) {
                        dogma_map.insert(aid, val);
                    }
                }
            }

            let name = t.get("name")
                .and_then(|n| n.get("en"))
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string();

            // Map bonus IDs to attributes.
            let mut bonuses = std::collections::HashMap::new();
            for (bonus_id, attr) in &IMPLANT_BONUS_IDS {
                if let Some(&val) = dogma_map.get(bonus_id) {
                    if val != 0.0 {
                        bonuses.insert(*attr, val as i32);
                    }
                }
            }

            if !bonuses.is_empty() {
                implants.push(ImplantRecord {
                    type_id: *tid as u32,
                    name,
                    bonuses,
                });
            }
        }

        implants.sort_by_key(|i| i.type_id);
        Ok(implants)
    }
}

/// Download the latest SDE JSONL zip from CCP and extract it to a directory.
pub async fn download_sde(output_dir: &PathBuf) -> Result<()> {
    use reqwest::Client;
    use std::io::Write;

    output_dir.parent().map(|p| std::fs::create_dir_all(p));

    let url = "https://developers.eveonline.com/static-data/eve-online-static-data-latest-jsonl.zip";
    let client = Client::new();
    let resp = client.get(url).send().await
        .context("Failed to fetch SDE zip")?;
    let bytes = resp.bytes().await
        .context("Failed to read SDE zip body")?;

    let zip_path = output_dir.join("sde.zip");
    let mut file = std::fs::File::create(&zip_path)?;
    file.write_all(&bytes)?;

    // Extract needed files using flate2/gzip or unzip.
    // For now, just store the zip — extraction is done offline via Python script.
    println!("Downloaded SDE zip to {}", zip_path.display());
    Ok(())
}

fn load_jsonl_into_map(path: PathBuf) -> Result<HashMap<u64, Value>> {
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let mut map = HashMap::new();
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let val: Value = serde_json::from_str(line)
            .with_context(|| format!("Failed to parse JSONL line in {}: {}", path.display(), &line[..line.len().min(100)]))?;
        if let Some(key) = val.get("_key").and_then(|k| k.as_u64()) {
            map.insert(key, val);
        }
    }
    Ok(map)
}
