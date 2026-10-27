use std::{
    collections::BTreeSet,
    env, fs, io,
    sync::{Arc, Mutex},
};

use minecraft_redox_core::transaction::{TransactionDatum, TransactionProgram};
use minecraft_redox_quiescence::{
    CanonicalStoreSnapshot, CanonicalValueSnapshot, ReactionArgument, ReactionStoreBinding,
    SnapshotInterner, TransactionReactionAdapter,
};
use quiescence_core::{SimDuration, SimTime};
use quiescence_runtime::{EvalError, Graph, ReactionOutput, RuntimeConfig, StateWrite};

const TICK_MS: i64 = 50;

fn datum_integer(value: i64) -> TransactionDatum {
    TransactionDatum::Integer(value)
}

fn stack(item: &str, quantity: i64) -> CanonicalValueSnapshot {
    CanonicalValueSnapshot {
        components: [
            (String::from("quantity"), datum_integer(quantity)),
            (
                String::from("item_identity"),
                TransactionDatum::String(item.to_owned()),
            ),
            (String::from("metadata"), datum_integer(0)),
            (String::from("max_quantity"), datum_integer(64)),
        ]
        .into_iter()
        .collect(),
        semantic_tag: None,
    }
}

fn empty_destination() -> CanonicalStoreSnapshot {
    CanonicalStoreSnapshot {
        slots: vec![None; 5],
        capabilities: [String::from("transfer_cooldown")].into_iter().collect(),
        allows_read: true,
        allows_write: true,
    }
}

fn source_inventory() -> CanonicalStoreSnapshot {
    CanonicalStoreSnapshot {
        slots: vec![Some(stack("iron_ingot", 5)), None, None],
        capabilities: BTreeSet::default(),
        allows_read: true,
        allows_write: true,
    }
}

fn cooldown_ticks(
    result: &minecraft_redox_quiescence::TransactionReactionResult,
) -> Result<Option<i64>, EvalError> {
    let Some(value) = result
        .state_values
        .get(&1)
        .and_then(|states| states.get("cooldown"))
    else {
        return Ok(None);
    };
    match value {
        TransactionDatum::Integer(value) => Ok(Some(*value)),
        value => Err(EvalError::Message(format!(
            "cooldown state is not an integer: {value:?}"
        ))),
    }
}

fn quantity(snapshot: &CanonicalStoreSnapshot, slot: usize) -> Option<i64> {
    let value = snapshot.slots.get(slot)?.as_ref()?;
    match value.components.get("quantity")? {
        TransactionDatum::Integer(value) => Some(*value),
        _ => None,
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let program_path = env::args().nth(1).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: vanilla_hopper_recovered <transaction-program.json>",
        )
    })?;
    let program: TransactionProgram = serde_json::from_str(&fs::read_to_string(&program_path)?)?;
    run(program)
}

fn run(program: TransactionProgram) -> Result<(), Box<dyn std::error::Error>> {
    let mut snapshots = SnapshotInterner::new();
    let destination_snapshot = snapshots
        .intern(empty_destination())
        .map_err(io::Error::other)?;
    let source_snapshot = snapshots
        .intern(source_inventory())
        .map_err(io::Error::other)?;
    let snapshots = Arc::new(Mutex::new(snapshots));

    let mut graph = Graph::new(RuntimeConfig::default());
    let ready_at_ms = graph.add_atom_input("hopper.ready_at_ms", 400)?;
    let source_node = graph.add_atom_input("hopper.source_inventory", source_snapshot)?;
    let destination_node =
        graph.add_atom_input("hopper.destination_inventory", destination_snapshot)?;

    let adapter = TransactionReactionAdapter::new(
        Arc::new(program),
        Arc::clone(&snapshots),
        vec![
            ReactionStoreBinding {
                node: destination_node,
                local_handle: 1,
            },
            ReactionStoreBinding {
                node: source_node,
                local_handle: 2,
            },
        ],
        vec![
            ReactionArgument::Store { binding_index: 0 },
            ReactionArgument::Store { binding_index: 1 },
            ReactionArgument::Datum(datum_integer(0)),
            ReactionArgument::Datum(datum_integer(-1)),
        ],
        None,
        "quantity",
    );

    let _hopper = graph.add_reaction("recovered hopper transaction", move |context| {
        let now_ms = u64::try_from(context.now().as_nanos() / 1_000_000)
            .map_err(|_| EvalError::Message(String::from("negative simulation time")))?;
        let ready_at = context.read_atom(ready_at_ms)?;
        if now_ms < ready_at {
            let delay_ms = i64::try_from(ready_at - now_ms)
                .map_err(|_| EvalError::Message(String::from("deadline delta exceeds i64")))?;
            return Ok(ReactionOutput::reevaluate_after(SimDuration::from_millis(
                delay_ms,
            )));
        }

        let result = adapter.execute(context)?;
        let cooldown = cooldown_ticks(&result)?;
        let mut writes = result.writes;
        if let Some(cooldown) = cooldown {
            let cooldown = u64::try_from(cooldown)
                .map_err(|_| EvalError::Message(String::from("negative cooldown")))?;
            let cooldown_ms = cooldown
                .checked_mul(
                    u64::try_from(TICK_MS)
                        .map_err(|_| EvalError::Message(String::from("negative tick duration")))?,
                )
                .ok_or_else(|| EvalError::Message(String::from("cooldown overflow")))?;
            let next_ready = now_ms
                .checked_add(cooldown_ms)
                .ok_or_else(|| EvalError::Message(String::from("deadline overflow")))?;
            writes.push(StateWrite::atom(ready_at_ms, next_ready));
        }
        if writes.is_empty() {
            Ok(ReactionOutput::idle())
        } else {
            Ok(ReactionOutput::immediate(writes))
        }
    })?;

    graph.settle()?;
    graph.advance_to(SimTime::from_millis(400))?;

    let source_after = graph.value_atom(source_node)?;
    let destination_after = graph.value_atom(destination_node)?;
    let interner = snapshots
        .lock()
        .map_err(|_| io::Error::other("snapshot interner lock poisoned"))?;
    let source = interner
        .get(source_after)
        .ok_or_else(|| io::Error::other("source snapshot missing"))?;
    let destination = interner
        .get(destination_after)
        .ok_or_else(|| io::Error::other("destination snapshot missing"))?;

    println!("source_snapshot={source_after}");
    println!("destination_snapshot={destination_after}");
    println!(
        "source_quantity={}",
        quantity(source, 0).unwrap_or_default()
    );
    println!(
        "destination_quantity={}",
        quantity(destination, 0).unwrap_or_default()
    );
    println!("next_ready_ms={}", graph.value_atom(ready_at_ms)?);
    println!("canonical_snapshots={}", interner.len());
    Ok(())
}
