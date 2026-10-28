use std::{
    env, fs, io,
    sync::{Arc, Mutex},
};

use minecraft_redox_quiescence::{AffineBitSelector, AffineBitSelectorSpec};
use quiescence_core::SimTime;
use quiescence_runtime::{EvalError, Graph, ReactionOutput, RuntimeConfig};
use serde::Deserialize;

const SELECTOR_SEED: i32 = 81_709;
const CALLBACK_TIME_MS: i64 = 50;

#[derive(Clone, Debug, Deserialize)]
struct SelectorSliceEvidence {
    argument_index: usize,
    shift: u32,
    mask: i64,
}

#[derive(Clone, Debug, Deserialize)]
struct SelectorEvidence {
    loop_iterations: u64,
    multiplier: i64,
    addend: i64,
    slices: Vec<SelectorSliceEvidence>,
}

fn pack_position([x, y, z]: [i32; 3]) -> Result<u64, io::Error> {
    for component in [x, y, z] {
        if !(0..=15).contains(&component) {
            return Err(io::Error::other(format!(
                "selector component {component} is outside one section"
            )));
        }
    }
    let x = u64::try_from(x).map_err(io::Error::other)?;
    let y = u64::try_from(y).map_err(io::Error::other)?;
    let z = u64::try_from(z).map_err(io::Error::other)?;
    Ok(x | (y << 4) | (z << 8))
}

fn unpack_position(token: u64) -> [u64; 3] {
    [token & 15, (token >> 4) & 15, (token >> 8) & 15]
}

fn accepts_random_tick(position: [i32; 3]) -> bool {
    // A tiny mock section: only this selected block has getTickRandomly() == true.
    // The real adapter owns the same block lookup/property decision outside Quiescence.
    position == [4, 1, 1]
}

fn run(evidence: &SelectorEvidence) -> Result<(), Box<dyn std::error::Error>> {
    let slices = evidence
        .slices
        .iter()
        .map(|slice| (slice.argument_index, slice.shift, slice.mask))
        .collect::<Vec<_>>();
    let spec =
        AffineBitSelectorSpec::from_absolute_slices(evidence.multiplier, evidence.addend, &slices)?;
    let mut selector = AffineBitSelector::new(SELECTOR_SEED, spec)?;

    let mut graph = Graph::new(RuntimeConfig::default());
    let observed = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&observed);
    let callback = graph.add_event_callback("random-tick updateTick", move |context| {
        let token = context.external_token().ok_or_else(|| {
            EvalError::Message(String::from(
                "random-tick callback arrived without source token",
            ))
        })?;
        captured
            .lock()
            .map_err(|_| EvalError::Message(String::from("token capture lock poisoned")))?
            .push(token);
        Ok(ReactionOutput::idle())
    })?;

    let mut selected = Vec::new();
    let mut emitted = Vec::new();
    for _ in 0..evidence.loop_iterations {
        let position = selector.next_components();
        selected.push(position);
        if accepts_random_tick(position) {
            let token = pack_position(position)?;
            emitted.push(token);
            graph.schedule_reaction_event_with_token(
                callback,
                SimTime::from_millis(CALLBACK_TIME_MS),
                0,
                token,
            )?;
        }
    }

    graph.advance_to(SimTime::from_millis(CALLBACK_TIME_MS - 1))?;
    let callbacks_before = observed
        .lock()
        .map_err(|_| io::Error::other("token capture lock poisoned"))?
        .len();
    graph.advance_to(SimTime::from_millis(CALLBACK_TIME_MS))?;
    let callbacks = observed
        .lock()
        .map_err(|_| io::Error::other("token capture lock poisoned"))?
        .clone();

    println!("selector_iterations={}", evidence.loop_iterations);
    println!("selected_positions={selected:?}");
    println!("emitted_tokens={emitted:?}");
    println!(
        "callback_positions={:?}",
        callbacks
            .iter()
            .copied()
            .map(unpack_position)
            .collect::<Vec<_>>()
    );
    println!("callbacks_before_50ms={callbacks_before}");
    println!("callbacks_at_50ms={}", callbacks.len());
    println!("selector_final_state={}", selector.state());
    println!(
        "dense_random_tick_boundary_correct={}",
        evidence.loop_iterations == 3
            && selected == vec![[9, 12, 12], [4, 1, 1], [4, 1, 1]]
            && emitted.len() == 2
            && emitted[0] == emitted[1]
            && callbacks_before == 0
            && callbacks == emitted
    );
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = env::args()
        .nth(1)
        .ok_or_else(|| io::Error::other("usage: vanilla_random_tick_selector <selector.json>"))?;
    let json = fs::read_to_string(path)?;
    let candidates: Vec<SelectorEvidence> = serde_json::from_str(&json)?;
    let [evidence] = candidates.as_slice() else {
        return Err(io::Error::other(format!(
            "expected exactly one selector candidate, got {}",
            candidates.len()
        ))
        .into());
    };
    run(evidence)
}
