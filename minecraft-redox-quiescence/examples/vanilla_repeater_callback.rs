use std::{
    collections::BTreeMap,
    env, fs, io,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use minecraft_redox_core::transaction::{
    MemoryTransactionHostCall, MemoryTransactionHostQuery, MemoryTransactionObject,
    TransactionDatum, TransactionProgram,
};
use minecraft_redox_quiescence::{
    ReactionArgument, ReactionHostPreparation, SnapshotInterner, TransactionReactionAdapter,
};
use quiescence_core::{SimDuration, SimTime};
use quiescence_runtime::{EvalError, Graph, ReactionOutput, RuntimeConfig, StateWrite};

const WORLD: u64 = 1;
const REPEATER: u64 = 10;
const X: i64 = 10;
const Y: i64 = 64;
const Z: i64 = 20;
const META: i64 = 4;
const UNPOWERED_STATE: u64 = 1;
const POWERED_STATE: u64 = 2;

fn integer(value: i64) -> TransactionDatum {
    TransactionDatum::Integer(value)
}

fn query(
    operation: &str,
    receiver: Option<TransactionDatum>,
    arguments: Vec<TransactionDatum>,
    result: TransactionDatum,
) -> MemoryTransactionHostQuery {
    MemoryTransactionHostQuery {
        operation: operation.to_owned(),
        receiver,
        arguments,
        result,
        consume: false,
    }
}

fn world_query(
    operation: &str,
    arguments: Vec<TransactionDatum>,
    result: TransactionDatum,
) -> MemoryTransactionHostQuery {
    query(
        operation,
        Some(TransactionDatum::Handle(WORLD)),
        arguments,
        result,
    )
}

fn block_write(block: &str) -> MemoryTransactionHostCall {
    MemoryTransactionHostCall {
        operation: String::from("world.set_block"),
        receiver: Some(TransactionDatum::Handle(WORLD)),
        arguments: vec![
            integer(X),
            integer(Y),
            integer(Z),
            TransactionDatum::String(block.to_owned()),
            integer(META),
            integer(2),
        ],
        result: TransactionDatum::Boolean(true),
        consume: false,
    }
}

fn preparation() -> ReactionHostPreparation {
    let air = TransactionDatum::String(String::from("minecraft:air"));
    ReactionHostPreparation {
        objects: BTreeMap::from([
            (
                WORLD,
                MemoryTransactionObject {
                    components: BTreeMap::new(),
                    semantic_tag: Some(String::from("world")),
                },
            ),
            (
                REPEATER,
                MemoryTransactionObject {
                    components: BTreeMap::from([(
                        String::from("is_powered"),
                        TransactionDatum::Boolean(false),
                    )]),
                    semantic_tag: Some(String::from("repeater")),
                },
            ),
        ]),
        host_queries: vec![
            world_query(
                "world.block_metadata_at",
                vec![integer(X), integer(Y), integer(Z)],
                integer(META),
            ),
            world_query(
                "world.block_at",
                vec![integer(X - 1), integer(Y), integer(Z)],
                air.clone(),
            ),
            query(
                "block.is_redstone_diode",
                None,
                vec![air.clone()],
                TransactionDatum::Boolean(false),
            ),
            world_query(
                "world.block_at",
                vec![integer(X + 1), integer(Y), integer(Z)],
                air.clone(),
            ),
            world_query(
                "world.indirect_power_at",
                vec![integer(X), integer(Y), integer(Z + 1), integer(3)],
                integer(0),
            ),
            world_query(
                "world.block_at",
                vec![integer(X), integer(Y), integer(Z + 1)],
                air.clone(),
            ),
            world_query(
                "world.block_at",
                vec![integer(X), integer(Y), integer(Z - 1)],
                air,
            ),
        ],
        host_calls: vec![
            block_write("minecraft:powered_repeater"),
            block_write("minecraft:unpowered_repeater"),
        ],
        ..ReactionHostPreparation::default()
    }
}

fn block_state_write(
    node: quiescence_core::NodeId,
    call: &minecraft_redox_core::transaction::MemoryTransactionPerformedHostCall,
) -> Result<Vec<StateWrite>, EvalError> {
    if call.operation != "world.set_block"
        || call.receiver != Some(TransactionDatum::Handle(WORLD))
        || call.arguments.len() != 6
        || call.arguments[0] != integer(X)
        || call.arguments[1] != integer(Y)
        || call.arguments[2] != integer(Z)
        || call.arguments[4] != integer(META)
        || call.arguments[5] != integer(2)
        || call.result != TransactionDatum::Boolean(true)
    {
        return Err(EvalError::Message(String::from(
            "unexpected vanilla set-block call",
        )));
    }
    let TransactionDatum::String(block) = &call.arguments[3] else {
        return Err(EvalError::Message(String::from(
            "set-block target is not a block ID",
        )));
    };
    let state = match block.as_str() {
        "minecraft:unpowered_repeater" => UNPOWERED_STATE,
        "minecraft:powered_repeater" => POWERED_STATE,
        _ => {
            return Err(EvalError::Message(format!(
                "unexpected repeater block target {block:?}"
            )));
        }
    };
    Ok(vec![StateWrite::atom(node, state)])
}

fn run(program: TransactionProgram) -> Result<(), Box<dyn std::error::Error>> {
    let mut graph = Graph::new(RuntimeConfig::default());
    let block_state = graph.add_atom_input("repeater block state", UNPOWERED_STATE)?;
    let callback_count = Arc::new(AtomicUsize::new(0));
    let observed_callback = Arc::clone(&callback_count);
    let callback = graph.add_reaction("scheduled repeater updateTick", move |context| {
        if context.now() != SimTime::ZERO {
            observed_callback.fetch_add(1, Ordering::Relaxed);
        }
        Ok(ReactionOutput::idle())
    })?;

    let adapter = TransactionReactionAdapter::new(
        Arc::new(program),
        Arc::new(Mutex::new(SnapshotInterner::new())),
        Vec::new(),
        vec![
            ReactionArgument::Datum(TransactionDatum::Handle(WORLD)),
            ReactionArgument::Datum(integer(X)),
            ReactionArgument::Datum(integer(Y)),
            ReactionArgument::Datum(integer(Z)),
            ReactionArgument::Datum(TransactionDatum::Null),
        ],
        Some(ReactionArgument::Datum(TransactionDatum::Handle(REPEATER))),
        "quantity",
    )
    .with_host_preparer(|_| Ok(preparation()))
    .with_host_call_publisher(move |call| block_state_write(block_state, call))
    .with_schedule_unit(SimDuration::from_millis(50));

    let observed = Arc::new(Mutex::new(None));
    let captured = Arc::clone(&observed);
    graph.add_reaction("recovered repeater callback body", move |context| {
        if context.now() != SimTime::ZERO {
            return Ok(ReactionOutput::idle());
        }
        let result = adapter.execute(context)?;
        *captured
            .lock()
            .map_err(|_| EvalError::Message(String::from("capture lock poisoned")))? = Some((
            result.execution.steps,
            result.performed_host_calls.clone(),
            result.schedule_requests.clone(),
        ));
        Ok(result.into_reaction_output_for_callback(callback))
    })?;

    graph.settle()?;
    let state_after_commit = graph.value_atom(block_state)?;
    let callbacks_after_commit = callback_count.load(Ordering::Relaxed);
    graph.advance_to(SimTime::from_millis(199))?;
    let callbacks_at_199 = callback_count.load(Ordering::Relaxed);
    graph.advance_to(SimTime::from_millis(200))?;
    let callbacks_at_200 = callback_count.load(Ordering::Relaxed);

    let observed = observed
        .lock()
        .map_err(|_| io::Error::other("capture lock poisoned"))?;
    let Some((steps, calls, schedules)) = observed.as_ref() else {
        return Err(io::Error::other("recovered callback did not execute").into());
    };
    println!("executed_statements={steps}");
    println!("committed_host_calls={}", calls.len());
    println!("schedule_requests={}", schedules.len());
    println!("block_state_after_commit={state_after_commit}");
    println!("callback_count_after_commit={callbacks_after_commit}");
    println!("callback_count_at_199ms={callbacks_at_199}");
    println!("callback_count_at_200ms={callbacks_at_200}");
    println!(
        "write_and_schedule_atomic={}",
        *steps == 130
            && calls.len() == 1
            && schedules.len() == 1
            && state_after_commit == POWERED_STATE
            && callbacks_after_commit == 0
            && callbacks_at_199 == 0
            && callbacks_at_200 == 1
    );
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = env::args()
        .nth(1)
        .ok_or_else(|| io::Error::other("usage: vanilla_repeater_callback <transaction.json>"))?;
    let json = fs::read_to_string(path)?;
    run(serde_json::from_str(&json)?)
}
