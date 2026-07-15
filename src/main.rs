mod calculator;
mod data;
mod auth;
mod optimizer;
use anyhow::{Context, Result};

#[tokio::main]
async fn main() -> Result<()> {
    let args = std::env::args().collect::<Vec<_>>();
    
    if args.len() < 2 || args[1] == "--help" || args[1] == "-h" {
        print_usage();
        return Ok(());
    }

    match args[1].as_str() {
        "download" => cmd_download(&args[2..]).await?,
        "verify" => cmd_verify(),
        "optimize" => cmd_optimize(&args[2..]).await?,
        unknown => eprintln!("Unknown command: {}", unknown),
    }

    Ok(())
}

fn print_usage() {
    println!("eve-remap - EVE Online skill queue optimizer");
    println!();
    println!("Usage:");
    println!("  eve-remap download [--dir DIR]     Download SDE and parse assets");
    println!("  eve-remap verify                   Verify asset files are present");
    println!("  eve-remap optimize                 Optimize current character's skill queue");
}

async fn cmd_download(_subargs: &[String]) -> Result<()> {
    // TODO: implement SDE download and parsing
    println!("SDE download not yet implemented. Assets already in repo.");
    Ok(())
}

fn cmd_verify() {
    let skills = data::load_skills().expect("Failed to load skills");
    let implants = data::load_implants().expect("Failed to load implants");
    
    println!("Assets verified:");
    println!("  Skills: {} entries", skills.len());
    println!("  Implants: {} entries with attribute bonuses", implants.len());
    
    // Show sample skill
    if let Some(skill) = skills.first() {
        println!("  Sample: {} (primary={}, secondary={}, tc={})", 
            skill.name, skill.primary_attribute, skill.secondary_attribute, skill.skill_time_constant);
    }
}

async fn cmd_optimize(_subargs: &[String]) -> Result<()> {
    let skills_db = data::load_skills()?;
    let implants = data::load_implants()?;
    
    // For now, run with a sample character state for demonstration.
    // In production this would be populated from ESI /character and /skillqueue endpoints.
    let char_state = optimizer::CharacterState {
        base_attributes: crate::data::models::BaseAttributes {
            intelligence: 12.0,
            charisma: 3.0,
            perception: 4.0,
            memory: 4.0,
            willpower: 2.0,
        },
        active_implant_ids: vec![],
        queued_skills: vec![],
    };
    
    let result = optimizer::optimize(&char_state, &skills_db, &implants);
    
    println!("Optimization complete: {} epochs, {:.1} total days", 
        result.epochs.len(), result.total_days);
    
    for (i, epoch) in result.epochs.iter().enumerate() {
        println!("  Epoch {}: start={:.0}d attrs=({}, {}, {}, {}, {}) completed={} finish={:.0}d",
            i,
            epoch.start_offset_days,
            epoch.attributes.intelligence as u32,
            epoch.attributes.charisma as u32,
            epoch.attributes.perception as u32,
            epoch.attributes.memory as u32,
            epoch.attributes.willpower as u32,
            epoch.completed_skills.len(),
            epoch.projected_finish_days,
        );
    }
    
    Ok(())
}
