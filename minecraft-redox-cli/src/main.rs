use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use minecraft_redox_gtnh::{
    GtnhSummary, TargetContentRecovery, lower_target_content, recover_with_project,
};
use minecraft_redox_java::{JavaProjectIndex, analyze_source, inventory_source};
use rayon::prelude::*;
use serde::Serialize;

#[derive(Serialize)]
struct FileRecovery {
    path: String,
    recovery: minecraft_redox_gtnh::GtnhRecovery,
}

#[derive(Serialize)]
struct TargetFileRecovery {
    path: String,
    target: TargetContentRecovery,
}

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    match args.as_slice() {
        [_, path] => print_inventory(path),
        [_, command, path] if command == "inventory" => print_inventory(path),
        [_, command, path] if command == "chains" => print_chains(path),
        [_, command, path] if command == "gtnh" => print_gtnh(path, false),
        [_, command, path] if command == "gtnh-target" => print_gtnh_target(path),
        [_, command, flag, path] if command == "gtnh" && flag == "--summary" => {
            print_gtnh(path, true)
        }
        _ => bail!(
            "usage: minecraft-redox [inventory|chains] <java-file> | gtnh [--summary] <file-or-dir> | gtnh-target <file-or-dir>"
        ),
    }
}

fn print_inventory(path: &str) -> Result<()> {
    let source = read_source(Path::new(path))?;
    let inventory = inventory_source(path, "unknown", None, &source)?;
    println!("{}", serde_json::to_string_pretty(&inventory)?);
    Ok(())
}

fn print_chains(path: &str) -> Result<()> {
    let source = read_source(Path::new(path))?;
    let analysis = analyze_source(path, "unknown", None, &source)?;
    println!("{}", serde_json::to_string_pretty(&analysis.call_chains)?);
    Ok(())
}

fn print_gtnh(path: &str, summary_only: bool) -> Result<()> {
    let paths = java_paths(Path::new(path))?;
    let project = build_project_index(&paths)?;
    if summary_only {
        let summary = summarize_gtnh(&paths, &project)?;
        println!("{}", serde_json::to_string_pretty(&summary)?);
        return Ok(());
    }

    let mut files = Vec::new();
    for source_path in paths {
        let display = source_path.to_string_lossy().into_owned();
        let source = read_source(&source_path)?;
        let analysis = analyze_source(&display, "GTNH", None, &source)?;
        let recovery = recover_with_project(&analysis, &project);
        if !recovery.recipes.is_empty() || !recovery.materials.is_empty() {
            files.push(FileRecovery {
                path: display,
                recovery,
            });
        }
    }
    println!("{}", serde_json::to_string_pretty(&files)?);
    Ok(())
}

fn print_gtnh_target(path: &str) -> Result<()> {
    let paths = java_paths(Path::new(path))?;
    let project = build_project_index(&paths)?;
    let mut files = Vec::new();
    for source_path in paths {
        let display = source_path.to_string_lossy().into_owned();
        let source = read_source(&source_path)?;
        let analysis = analyze_source(&display, "GTNH", None, &source)?;
        let recovery = recover_with_project(&analysis, &project);
        let target = lower_target_content(&analysis, &recovery, &project);
        if target.materials.is_empty()
            && target.material_failures.is_empty()
            && target.recipes.is_empty()
            && target.recipe_failures.is_empty()
        {
            continue;
        }
        files.push(TargetFileRecovery {
            path: display,
            target,
        });
    }
    println!("{}", serde_json::to_string_pretty(&files)?);
    Ok(())
}

fn build_project_index(paths: &[PathBuf]) -> Result<JavaProjectIndex> {
    paths
        .par_iter()
        .map(|source_path| {
            let display = source_path.to_string_lossy().into_owned();
            let source = read_source(source_path)?;
            let analysis = analyze_source(&display, "GTNH", None, &source)?;
            let mut index = JavaProjectIndex::default();
            index.add_analysis(&analysis);
            Ok(index)
        })
        .try_reduce(JavaProjectIndex::default, |left, right| {
            Ok(left.merged(right))
        })
}

fn summarize_gtnh(paths: &[PathBuf], project: &JavaProjectIndex) -> Result<GtnhSummary> {
    paths
        .par_iter()
        .map(|source_path| {
            let display = source_path.to_string_lossy().into_owned();
            let source = read_source(source_path)?;
            let analysis = analyze_source(&display, "GTNH", None, &source)?;
            let recovery = recover_with_project(&analysis, project);
            let mut summary = GtnhSummary::default();
            summary.absorb_with_project(&analysis, &recovery, project);
            Ok(summary)
        })
        .try_reduce(GtnhSummary::default, |left, right| Ok(left.merged(right)))
}

fn read_source(path: &Path) -> Result<String> {
    fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))
}

fn java_paths(path: &Path) -> Result<Vec<PathBuf>> {
    if path.is_file() {
        return Ok(vec![path.to_path_buf()]);
    }
    if !path.is_dir() {
        bail!("{} is neither a file nor directory", path.display());
    }
    let mut files = Vec::new();
    collect_java(path, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_java(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_java(&path, files)?;
        } else if path
            .extension()
            .is_some_and(|extension| extension == "java")
        {
            files.push(path);
        }
    }
    Ok(())
}
