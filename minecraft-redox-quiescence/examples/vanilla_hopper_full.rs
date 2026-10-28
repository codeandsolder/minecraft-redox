use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs, io,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use minecraft_redox_core::transaction::{
    MemoryTransactionHostQuery, MemoryTransactionObject, TransactionDatum, TransactionProgram,
};
use minecraft_redox_quiescence::{
    CanonicalStoreSnapshot, CanonicalValueSnapshot, ReactionArgument, ReactionHostPreparation,
    ReactionStoreBinding, SnapshotInterner, TransactionReactionAdapter, TransactionReactionResult,
};
use quiescence_core::{NodeId, SimDuration, SimTime};
use quiescence_runtime::{EvalError, Graph, ReactionOutput, RuntimeConfig, StateWrite};

const TICK_MS: u64 = 50;

fn integer(value: i64) -> TransactionDatum {
    TransactionDatum::Integer(value)
}

fn item(quantity: i64, identity: &str) -> CanonicalValueSnapshot {
    CanonicalValueSnapshot {
        components: BTreeMap::from([
            (String::from("quantity"), integer(quantity)),
            (String::from("max_quantity"), integer(64)),
            (
                String::from("item_identity"),
                TransactionDatum::String(identity.to_owned()),
            ),
            (String::from("metadata"), integer(0)),
        ]),
        semantic_tag: Some(identity.to_owned()),
    }
}

fn store(slots: Vec<Option<CanonicalValueSnapshot>>, hopper: bool) -> CanonicalStoreSnapshot {
    let mut capabilities = BTreeSet::from([String::from("indexed_store")]);
    if hopper {
        capabilities.insert(String::from("transfer_cooldown"));
    }
    CanonicalStoreSnapshot {
        slots,
        capabilities,
        allows_read: true,
        allows_write: true,
    }
}

fn object_query(coordinates: [i64; 3], handle: u64) -> MemoryTransactionHostQuery {
    MemoryTransactionHostQuery {
        operation: String::from("topology.object_at"),
        receiver: Some(TransactionDatum::Handle(10)),
        arguments: coordinates.into_iter().map(integer).collect(),
        result: TransactionDatum::Handle(handle),
        consume: false,
    }
}

fn host_preparation(destination: NodeId, source: NodeId) -> ReactionHostPreparation {
    ReactionHostPreparation {
        stores: vec![
            ReactionStoreBinding {
                node: destination,
                local_handle: 2,
            },
            ReactionStoreBinding {
                node: source,
                local_handle: 3,
            },
        ],
        objects: BTreeMap::from([
            (
                1,
                MemoryTransactionObject {
                    components: BTreeMap::from([
                        (String::from("world"), TransactionDatum::Handle(10)),
                        (String::from("block_x"), integer(10)),
                        (String::from("block_y"), integer(20)),
                        (String::from("block_z"), integer(30)),
                        (String::from("block_metadata"), integer(-1)),
                        (String::from("cooldown"), integer(0)),
                        (String::from("position_x"), TransactionDatum::Float(10.2)),
                        (String::from("position_y"), TransactionDatum::Float(20.3)),
                        (String::from("position_z"), TransactionDatum::Float(30.4)),
                    ]),
                    semantic_tag: Some(String::from("hopper")),
                },
            ),
            (
                10,
                MemoryTransactionObject {
                    components: BTreeMap::from([(
                        String::from("is_remote"),
                        TransactionDatum::Boolean(false),
                    )]),
                    semantic_tag: Some(String::from("world")),
                },
            ),
        ]),
        host_queries: vec![
            MemoryTransactionHostQuery {
                operation: String::from("topology.block_metadata_at"),
                receiver: Some(TransactionDatum::Handle(10)),
                arguments: [10, 20, 30].into_iter().map(integer).collect(),
                result: integer(5),
                consume: false,
            },
            object_query([11, 20, 30], 2),
            object_query([10, 21, 30], 3),
        ],
        ..ReactionHostPreparation::default()
    }
}

fn cooldown_ticks(result: &TransactionReactionResult) -> Result<Option<u64>, EvalError> {
    let Some(value) = result
        .state_values
        .get(&1)
        .and_then(|states| states.get("cooldown"))
    else {
        return Ok(None);
    };
    let TransactionDatum::Integer(value) = value else {
        return Err(EvalError::Message(String::from("cooldown is not integer")));
    };
    u64::try_from(*value)
        .map(Some)
        .map_err(|_| EvalError::Message(String::from("negative cooldown")))
}

fn slot_quantity(
    snapshots: &Arc<Mutex<SnapshotInterner>>,
    id: u64,
) -> Result<Option<i64>, Box<dyn std::error::Error>> {
    let snapshots = snapshots
        .lock()
        .map_err(|_| io::Error::other("snapshot interner lock poisoned"))?;
    let snapshot = snapshots
        .get(id)
        .ok_or_else(|| io::Error::other("snapshot missing"))?;
    let Some(item) = snapshot.slots.first().and_then(Option::as_ref) else {
        return Ok(None);
    };
    let value = item
        .components
        .get("quantity")
        .ok_or_else(|| io::Error::other("quantity missing"))?;
    let TransactionDatum::Integer(value) = value else {
        return Err(io::Error::other("quantity is not integer").into());
    };
    Ok(Some(*value))
}

fn run(program: TransactionProgram) -> Result<(), Box<dyn std::error::Error>> {
    let mut interner = SnapshotInterner::new();
    let hopper_initial = interner.intern(store(
        vec![Some(item(5, "iron")), None, None, None, None],
        true,
    ))?;
    let destination_empty = interner.intern(store(vec![None], false))?;
    let destination_full = interner.intern(store(vec![Some(item(64, "cobble"))], false))?;
    let source_initial = interner.intern(store(vec![Some(item(5, "iron"))], false))?;
    let source_later = interner.intern(store(vec![Some(item(7, "iron"))], false))?;
    let snapshots = Arc::new(Mutex::new(interner));

    let mut graph = Graph::new(RuntimeConfig::default());
    let deadline = graph.add_atom_input("hopper.ready_at_ms", 400)?;
    let hopper = graph.add_atom_input("hopper.inventory", hopper_initial)?;
    let destination = graph.add_atom_input("destination.inventory", destination_empty)?;
    let source = graph.add_atom_input("source.inventory", source_initial)?;

    let adapter = TransactionReactionAdapter::new(
        Arc::new(program),
        Arc::clone(&snapshots),
        vec![ReactionStoreBinding {
            node: hopper,
            local_handle: 1,
        }],
        Vec::new(),
        Some(ReactionArgument::Store { binding_index: 0 }),
        "quantity",
    )
    .with_host_preparer(move |_| Ok(host_preparation(destination, source)));

    let evaluations = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&evaluations);
    graph.add_reaction("recovered full hopper attempt", move |context| {
        observed.fetch_add(1, Ordering::Relaxed);
        let now_ms = u64::try_from(context.now().as_nanos() / 1_000_000)
            .map_err(|_| EvalError::Message(String::from("negative simulation time")))?;
        let ready_at = context.read_atom(deadline)?;
        if now_ms < ready_at {
            let delay = i64::try_from(ready_at - now_ms)
                .map_err(|_| EvalError::Message(String::from("deadline delta overflow")))?;
            return Ok(ReactionOutput::reevaluate_after(SimDuration::from_millis(
                delay,
            )));
        }

        let result = adapter.execute(context)?;
        let cooldown = cooldown_ticks(&result)?;
        let mut writes = result.writes;
        if let Some(cooldown) = cooldown {
            let next = now_ms
                .checked_add(cooldown.saturating_mul(TICK_MS))
                .ok_or_else(|| EvalError::Message(String::from("deadline overflow")))?;
            writes.push(StateWrite::atom(deadline, next));
        }
        Ok(if writes.is_empty() {
            ReactionOutput::idle()
        } else {
            ReactionOutput::immediate(writes)
        })
    })?;

    graph.settle()?;
    graph.advance_to(SimTime::from_millis(400))?;
    println!(
        "first_hopper_quantity={:?}",
        slot_quantity(&snapshots, graph.value_atom(hopper)?)?
    );
    println!(
        "first_destination_quantity={:?}",
        slot_quantity(&snapshots, graph.value_atom(destination)?)?
    );
    println!(
        "first_source_quantity={:?}",
        slot_quantity(&snapshots, graph.value_atom(source)?)?
    );
    println!("first_ready_ms={}", graph.value_atom(deadline)?);

    let after_first = evaluations.load(Ordering::Relaxed);
    graph.schedule_atom(destination, SimTime::from_millis(500), destination_full)?;
    graph.schedule_atom(source, SimTime::from_millis(500), source_later)?;
    graph.advance_to(SimTime::from_millis(500))?;
    println!(
        "neighbor_changes_woke_during_cooldown={}",
        evaluations.load(Ordering::Relaxed) != after_first
    );

    graph.advance_to(SimTime::from_millis(800))?;
    println!(
        "second_hopper_quantity={:?}",
        slot_quantity(&snapshots, graph.value_atom(hopper)?)?
    );
    println!(
        "second_destination_quantity={:?}",
        slot_quantity(&snapshots, graph.value_atom(destination)?)?
    );
    println!(
        "second_source_quantity={:?}",
        slot_quantity(&snapshots, graph.value_atom(source)?)?
    );
    println!("second_ready_ms={}", graph.value_atom(deadline)?);
    println!(
        "reaction_evaluations={}",
        evaluations.load(Ordering::Relaxed)
    );
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = env::args().nth(1).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: vanilla_hopper_full <full-hopper-transaction.json>",
        )
    })?;
    let program: TransactionProgram = serde_json::from_str(&fs::read_to_string(path)?)?;
    run(program)
}
