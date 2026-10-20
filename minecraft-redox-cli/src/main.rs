use std::{env, fs};

use anyhow::{Context, Result, bail};
use minecraft_redox_java::inventory_source;

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        bail!("usage: minecraft-redox <path-to-java-source>");
    }
    let path = &args[1];
    let source = fs::read_to_string(path).with_context(|| format!("failed to read {path}"))?;
    let inventory = inventory_source(path, "unknown", None, &source)?;
    println!("{}", serde_json::to_string_pretty(&inventory)?);
    Ok(())
}
