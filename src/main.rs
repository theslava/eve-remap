mod calculator;
mod data;
mod auth;

use anyhow::Result;

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
    // TODO: implement optimizer pipeline
    println!("Optimizer coming soon. Run 'eve-remap verify' first.");
    Ok(())
}
