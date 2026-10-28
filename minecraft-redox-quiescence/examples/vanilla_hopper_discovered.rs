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
const COORDINATES: [i64; 3] = [10, 21, 30];

fn datum_integer(value: i64) -> TransactionDatum {
    TransactionDatum::Integer(value)
}

fn item(quantity: i64) -> CanonicalValueSnapshot {
    CanonicalValueSnapshot {
        components: BTreeMap::from([
            (String::from("quantity"), datum_integer(quantity)),
            (
                String::from("item_identity"),
                TransactionDatum::String(String::from("iron")),
            ),
            (String::from("metadata"), datum_integer(0)),
            (String::from("max_quantity"), datum_integer(64)),
        ]),
        semantic_tag: Some(String::from("iron")),
    }
}

fn hopper_snapshot() -> CanonicalStoreSnapshot {
    CanonicalStoreSnapshot {
        slots: vec![None; 5],
        capabilities: BTreeSet::from([
            String::from("indexed_store"),
            String::from("transfer_cooldown"),
        ]),
        allows_read: true,
        allows_write: true,
    }
}

fn source_snapshot(quantity: i64) -> CanonicalStoreSnapshot {
    CanonicalStoreSnapshot {
        slots: vec![Some(item(quantity))],
        capabilities: BTreeSet::from([String::from("indexed_store")]),
        allows_read: true,
        allows_write: true,
    }
}

fn host_preparation(source_node: NodeId) -> ReactionHostPreparation {
    let world = TransactionDatum::Handle(10);
    ReactionHostPreparation {
        stores: vec![ReactionStoreBinding {
            node: source_node,
            local_handle: 2,
        }],
        objects: BTreeMap::from([(
            1,
            MemoryTransactionObject {
                components: BTreeMap::from([
                    (String::from("world"), world.clone()),
                    (String::from("position_x"), TransactionDatum::Float(10.2)),
                    (String::from("position_y"), TransactionDatum::Float(20.3)),
                    (String::from("position_z"), TransactionDatum::Float(30.4)),
                ]),
                semantic_tag: Some(String::from("hopper")),
            },
        )]),
        host_queries: vec![MemoryTransactionHostQuery {
            operation: String::from("topology.object_at"),
            receiver: Some(world),
            arguments: COORDINATES.into_iter().map(datum_integer).collect(),
            result: TransactionDatum::Handle(2),
            consume: false,
        }],
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

fn snapshot_quantity(
    snapshots: &Arc<Mutex<SnapshotInterner>>,
    id: u64,
) -> Result<i64, Box<dyn std::error::Error>> {
    let snapshots = snapshots
        .lock()
        .map_err(|_| io::Error::other("snapshot interner lock poisoned"))?;
    let snapshot = snapshots
        .get(id)
        .ok_or_else(|| io::Error::other("snapshot missing"))?;
    let value = snapshot.slots[0]
        .as_ref()
        .and_then(|item| item.components.get("quantity"))
        .ok_or_else(|| io::Error::other("quantity missing"))?;
    let TransactionDatum::Integer(value) = value else {
        return Err(io::Error::other("quantity is not integer").into());
    };
    Ok(*value)
}

fn run(program: TransactionProgram) -> Result<(), Box<dyn std::error::Error>> {
    let mut interner = SnapshotInterner::new();
    let hopper_id = interner.intern(hopper_snapshot())?;
    let source_id = interner.intern(source_snapshot(5))?;
    let later_source_id = interner.intern(source_snapshot(7))?;
    let snapshots = Arc::new(Mutex::new(interner));
    let mut graph = Graph::new(RuntimeConfig::default());
    let deadline = graph.add_atom_input("hopper.ready_at_ms", 400)?;
    let hopper_node = graph.add_atom_input("hopper.inventory", hopper_id)?;
    let source_node = graph.add_atom_input("source.inventory", source_id)?;
    let adapter = TransactionReactionAdapter::new(
        Arc::new(program),
        Arc::clone(&snapshots),
        vec![ReactionStoreBinding {
            node: hopper_node,
            local_handle: 1,
        }],
        vec![ReactionArgument::Store { binding_index: 0 }],
        None,
        "quantity",
    )
    .with_host_preparer(move |_| Ok(host_preparation(source_node)));
    let evaluations = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&evaluations);
    graph.add_reaction("recovered outer hopper pull", move |context| {
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
        "first_hopper_quantity={}",
        snapshot_quantity(&snapshots, graph.value_atom(hopper_node)?)?
    );
    println!(
        "first_source_quantity={}",
        snapshot_quantity(&snapshots, graph.value_atom(source_node)?)?
    );
    println!("first_ready_ms={}", graph.value_atom(deadline)?);
    let after_first = evaluations.load(Ordering::Relaxed);
    graph.schedule_atom(source_node, SimTime::from_millis(500), later_source_id)?;
    graph.advance_to(SimTime::from_millis(500))?;
    println!(
        "source_change_woke_during_cooldown={}",
        evaluations.load(Ordering::Relaxed) != after_first
    );
    graph.advance_to(SimTime::from_millis(800))?;
    println!(
        "second_hopper_quantity={}",
        snapshot_quantity(&snapshots, graph.value_atom(hopper_node)?)?
    );
    println!(
        "second_source_quantity={}",
        snapshot_quantity(&snapshots, graph.value_atom(source_node)?)?
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
            "usage: vanilla_hopper_discovered <outer-pull-transaction.json>",
        )
    })?;
    let program: TransactionProgram = serde_json::from_str(&fs::read_to_string(path)?)?;
    run(program)
}
