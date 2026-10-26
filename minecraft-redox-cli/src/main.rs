use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use minecraft_redox_gtnh::{GtnhSummary, lower_recipe_with_project, recover_with_project};
use minecraft_redox_java::{
    JavaProjectIndex, analyze_source, inventory_source,
    jimple::{
        AbstractValue, JimpleMethodSignature, JimpleProgram, ProgramExecution, parse_methods,
    },
};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
struct JimpleCallRequest {
    signature: JimpleMethodSignature,
    this_value: Option<AbstractValue>,
    #[serde(default)]
    arguments: Vec<AbstractValue>,
}

#[derive(Serialize)]
struct JimpleCallResult {
    return_value: Option<AbstractValue>,
    execution: ProgramExecution,
}

#[derive(Serialize)]
struct FileRecovery {
    path: String,
    recovery: minecraft_redox_gtnh::GtnhRecovery,
}

#[derive(Clone, Debug, Default, Serialize)]
struct BlockerDetailCensus {
    kind: String,
    detail: String,
    recipes: usize,
    sole_blocker_recipes: usize,
    examples: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
struct BlockerCensus {
    failed_recipes: usize,
    details: Vec<BlockerDetailCensus>,
}

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    match args.as_slice() {
        [_, path] => print_inventory(path),
        [_, command, path] if command == "inventory" => print_inventory(path),
        [_, command, path] if command == "chains" => print_chains(path),
        [_, command, path] if command == "gtnh" => print_gtnh(path, false),
        [_, command, flag, path] if command == "gtnh" && flag == "--summary" => {
            print_gtnh(path, true)
        }
        [_, command, flag, path] if command == "gtnh" && flag == "--blockers" => {
            print_gtnh_blockers(path)
        }
        [_, command, path, method, sink] if command == "jimple-slice" => {
            print_jimple_slice(path, method, sink)
        }
        [_, command, path, method, owner, field] if command == "jimple-execute-array" => {
            print_jimple_execute_array(path, method, owner, field)
        }
        [_, command, path, sequence] if command == "jimple-program" => {
            print_jimple_program(path, sequence)
        }
        [_, command, path, sequence, request] if command == "jimple-program-call" => {
            print_jimple_program_call(path, sequence, request)
        }
        [_, command, path] if command == "jimple-temporal" => print_jimple_temporal(path),
        [_, command, path, request] if command == "jimple-call" => print_jimple_call(path, request),
        _ => bail!(
            "usage: minecraft-redox [inventory|chains] <java-file> | gtnh [--summary|--blockers] <file-or-dir> | jimple-slice <json> <method-substring> <sink-substring> | jimple-execute-array <json> <method-substring> <field-owner> <field-name> | jimple-program <json> <owner::method,...> | jimple-program-call <json> <owner::method,...> <request-json> | jimple-call <json> <request-json> | jimple-temporal <json>"
        ),
    }
}

fn print_jimple_temporal(path: &str) -> Result<()> {
    let json = fs::read_to_string(path).with_context(|| format!("failed to read {path}"))?;
    let methods = parse_methods(&json)?;
    let summaries: Vec<_> = methods
        .iter()
        .map(minecraft_redox_java::jimple::JimpleMethod::temporal_summary)
        .filter(|summary| {
            !summary.counter_updates.is_empty()
                || !summary.constant_writes.is_empty()
                || !summary.comparisons.is_empty()
        })
        .collect();
    println!("{}", serde_json::to_string_pretty(&summaries)?);
    Ok(())
}

fn print_jimple_call(path: &str, request_path: &str) -> Result<()> {
    let json = fs::read_to_string(path).with_context(|| format!("failed to read {path}"))?;
    let methods = parse_methods(&json)?;
    let request_json = fs::read_to_string(request_path)
        .with_context(|| format!("failed to read {request_path}"))?;
    let request: JimpleCallRequest = serde_json::from_str(&request_json)
        .with_context(|| format!("failed to parse {request_path}"))?;
    let program = JimpleProgram::new(methods);
    let (return_value, execution) = program.execute_call(
        &request.signature,
        request.this_value,
        request.arguments,
        100_000,
    )?;
    println!(
        "{}",
        serde_json::to_string_pretty(&JimpleCallResult {
            return_value,
            execution,
        })?
    );
    Ok(())
}

fn print_jimple_program(path: &str, sequence: &str) -> Result<()> {
    let json = fs::read_to_string(path).with_context(|| format!("failed to read {path}"))?;
    let methods = parse_methods(&json)?;
    let program = JimpleProgram::new(methods);
    let parsed = parse_jimple_sequence(sequence)?;
    let execution = program.execute_named_sequence(&parsed, 100_000)?;
    println!("{}", serde_json::to_string_pretty(&execution)?);
    Ok(())
}

fn print_jimple_program_call(path: &str, sequence: &str, request_path: &str) -> Result<()> {
    let json = fs::read_to_string(path).with_context(|| format!("failed to read {path}"))?;
    let methods = parse_methods(&json)?;
    let request_json = fs::read_to_string(request_path)
        .with_context(|| format!("failed to read {request_path}"))?;
    let request: JimpleCallRequest = serde_json::from_str(&request_json)
        .with_context(|| format!("failed to parse {request_path}"))?;
    let program = JimpleProgram::new(methods);
    let parsed = parse_jimple_sequence(sequence)?;
    let (return_value, execution) = program.execute_named_sequence_then_call(
        &parsed,
        &request.signature,
        request.this_value,
        request.arguments,
        100_000,
    )?;
    println!(
        "{}",
        serde_json::to_string_pretty(&JimpleCallResult {
            return_value,
            execution,
        })?
    );
    Ok(())
}

fn parse_jimple_sequence(sequence: &str) -> Result<Vec<(&str, &str)>> {
    sequence
        .split(',')
        .map(|entry| {
            entry
                .rsplit_once("::")
                .with_context(|| format!("invalid method entry {entry:?}; expected owner::method"))
        })
        .collect()
}

fn print_jimple_execute_array(
    path: &str,
    method_filter: &str,
    field_owner: &str,
    field_name: &str,
) -> Result<()> {
    let json = fs::read_to_string(path).with_context(|| format!("failed to read {path}"))?;
    let methods = parse_methods(&json)?;
    let matching: Vec<_> = methods
        .iter()
        .filter(|method| {
            method.signature.name.contains(method_filter)
                || method.display_signature.contains(method_filter)
        })
        .collect();
    let [method] = matching.as_slice() else {
        bail!(
            "expected one Jimple method matching {method_filter:?}, found {}",
            matching.len()
        );
    };
    let execution = method.execute_finite(10_000)?;
    let array = execution
        .static_array(field_owner, field_name)
        .with_context(|| {
            format!("no writes observed for static array {field_owner}.{field_name}")
        })?;
    let report = serde_json::json!({
        "method": method.signature,
        "steps": execution.steps,
        "array": array,
        "objects": execution.objects,
    });
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn print_jimple_slice(path: &str, method_filter: &str, sink_filter: &str) -> Result<()> {
    let json = fs::read_to_string(path).with_context(|| format!("failed to read {path}"))?;
    let methods = parse_methods(&json)?;
    let matching: Vec<_> = methods
        .iter()
        .filter(|method| {
            method.signature.name.contains(method_filter)
                || method.display_signature.contains(method_filter)
        })
        .collect();
    let [method] = matching.as_slice() else {
        bail!(
            "expected one Jimple method matching {method_filter:?}, found {}",
            matching.len()
        );
    };
    let sink = method
        .statement_containing(sink_filter)
        .with_context(|| format!("no statement contains {sink_filter:?}"))?;
    let slice_ids = method.backward_slice(sink.id);
    let slice: Vec<_> = slice_ids
        .iter()
        .filter_map(|id| {
            method
                .statements
                .iter()
                .find(|statement| statement.id == *id)
        })
        .collect();
    let symbolic = method.symbolic_assignment(sink.id).ok();
    let report = serde_json::json!({
        "method": method.signature,
        "sink_id": sink.id,
        "slice": slice,
        "symbolic_assignment": symbolic,
    });
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
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

fn print_gtnh_blockers(path: &str) -> Result<()> {
    let paths = java_paths(Path::new(path))?;
    let project = build_project_index(&paths)?;
    let census = blocker_census(&paths, &project)?;
    println!("{}", serde_json::to_string_pretty(&census)?);
    Ok(())
}

fn blocker_census(paths: &[PathBuf], project: &JavaProjectIndex) -> Result<BlockerCensus> {
    let mut details: BTreeMap<(String, String), BlockerDetailCensus> = BTreeMap::new();
    let mut failed_recipes = 0usize;
    for source_path in paths {
        let display = source_path.to_string_lossy().into_owned();
        let source = read_source(source_path)?;
        let analysis = analyze_source(&display, "GTNH", None, &source)?;
        let recovery = recover_with_project(&analysis, project);
        for recipe in &recovery.recipes {
            let Err(failure) = lower_recipe_with_project(recipe, &analysis.bindings, project)
            else {
                continue;
            };
            failed_recipes += 1;
            let sole = failure.blockers.len() == 1;
            for blocker in &failure.blockers {
                let kind = blocker.kind.as_str().to_owned();
                let detail = blocker.detail.clone();
                let entry = details
                    .entry((kind.clone(), detail.clone()))
                    .or_insert_with(|| BlockerDetailCensus {
                        kind,
                        detail,
                        ..BlockerDetailCensus::default()
                    });
                entry.recipes += 1;
                entry.sole_blocker_recipes += usize::from(sole);
                if entry.examples.len() < 3 && !entry.examples.contains(&display) {
                    entry.examples.push(display.clone());
                }
            }
        }
    }
    let mut details: Vec<_> = details.into_values().collect();
    details.sort_by(|left, right| {
        right
            .sole_blocker_recipes
            .cmp(&left.sole_blocker_recipes)
            .then_with(|| right.recipes.cmp(&left.recipes))
            .then_with(|| left.kind.cmp(&right.kind))
            .then_with(|| left.detail.cmp(&right.detail))
    });
    Ok(BlockerCensus {
        failed_recipes,
        details,
    })
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
