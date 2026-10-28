use std::{
    collections::BTreeMap,
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
    ReactionArgument, ReactionHostPreparation, SnapshotInterner, TransactionReactionAdapter,
    TransactionScheduleRequest,
};
use quiescence_core::{SimDuration, SimTime};
use quiescence_runtime::{EvalError, Graph, ReactionOutput, RuntimeConfig, interaction_channel};

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
        Some(TransactionDatum::Handle(1)),
        arguments,
        result,
    )
}

fn preparation() -> ReactionHostPreparation {
    let repeater = TransactionDatum::Handle(10);
    let air = TransactionDatum::String(String::from("minecraft:air"));
    ReactionHostPreparation {
        objects: BTreeMap::from([
            (
                1,
                MemoryTransactionObject {
                    components: BTreeMap::new(),
                    semantic_tag: Some(String::from("world")),
                },
            ),
            (
                10,
                MemoryTransactionObject {
                    components: BTreeMap::from([
                        (String::from("is_powered"), TransactionDatum::Boolean(false)),
                        (
                            String::from("semantic_id"),
                            TransactionDatum::String(String::from("minecraft:unpowered_repeater")),
                        ),
                    ]),
                    semantic_tag: Some(String::from("repeater")),
                },
            ),
        ]),
        host_queries: vec![
            world_query(
                "world.block_metadata_at",
                vec![integer(10), integer(64), integer(20)],
                integer(4),
            ),
            world_query(
                "world.block_at",
                vec![integer(9), integer(64), integer(20)],
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
                vec![integer(11), integer(64), integer(20)],
                air.clone(),
            ),
            world_query(
                "world.indirect_power_at",
                vec![integer(10), integer(64), integer(21), integer(3)],
                integer(15),
            ),
            world_query(
                "world.block_tick_pending_here",
                vec![integer(10), integer(64), integer(20), repeater],
                TransactionDatum::Boolean(false),
            ),
            world_query(
                "world.block_at",
                vec![integer(10), integer(64), integer(19)],
                air,
            ),
        ],
        ..ReactionHostPreparation::default()
    }
}

fn adapter(program: TransactionProgram) -> TransactionReactionAdapter {
    TransactionReactionAdapter::new(
        Arc::new(program),
        Arc::new(Mutex::new(SnapshotInterner::new())),
        Vec::new(),
        vec![
            ReactionArgument::Datum(TransactionDatum::Handle(1)),
            ReactionArgument::Datum(integer(10)),
            ReactionArgument::Datum(integer(64)),
            ReactionArgument::Datum(integer(20)),
            ReactionArgument::Datum(TransactionDatum::String(String::from("minecraft:stone"))),
        ],
        Some(ReactionArgument::Datum(TransactionDatum::Handle(10))),
        "quantity",
    )
    .with_host_preparer(|_| Ok(preparation()))
    .with_schedule_unit(SimDuration::from_millis(50))
    .with_schedule_identity_component("semantic_id")
}

fn run(program: TransactionProgram) -> Result<(), Box<dyn std::error::Error>> {
    let adapter = adapter(program);
    let producer_evaluations = Arc::new(AtomicUsize::new(0));
    let observed_producer = Arc::clone(&producer_evaluations);
    let callback_evaluations = Arc::new(AtomicUsize::new(0));
    let observed_callback = Arc::clone(&callback_evaluations);
    let requests = Arc::new(Mutex::new(Vec::<TransactionScheduleRequest>::new()));
    let observed_requests = Arc::clone(&requests);
    let mut graph = Graph::new(RuntimeConfig::default());
    let (interactions, inbox) = interaction_channel();
    let callback =
        graph.add_event_callback("recovered repeater updateTick callback", move |_| {
            observed_callback.fetch_add(1, Ordering::Relaxed);
            Ok(ReactionOutput::idle())
        })?;
    let producer = graph.add_reaction("recovered repeater neighbor handler", move |context| {
        let evaluation = observed_producer.fetch_add(1, Ordering::Relaxed);
        if evaluation >= 2 {
            return Ok(ReactionOutput::idle());
        }
        let result = adapter.execute(context)?;
        let [request] = result.schedule_requests.as_slice() else {
            return Err(EvalError::Message(String::from(
                "repeater transaction did not emit exactly one schedule request",
            )));
        };
        observed_requests
            .lock()
            .map_err(|_| EvalError::Message(String::from("request capture lock poisoned")))?
            .push(*request);
        Ok(result.into_reaction_output_for_callback(callback))
    })?;

    graph.quiesce()?;
    interactions.callback_at(producer, SimTime::from_millis(50), 0)?;
    graph.process_interactions(&inbox)?;
    graph.advance_to(SimTime::from_millis(50))?;
    let producers_at_50 = producer_evaluations.load(Ordering::Relaxed);
    let callbacks_at_50 = callback_evaluations.load(Ordering::Relaxed);
    graph.advance_to(SimTime::from_millis(199))?;
    let callbacks_at_199 = callback_evaluations.load(Ordering::Relaxed);
    graph.advance_to(SimTime::from_millis(200))?;
    let callbacks_at_200 = callback_evaluations.load(Ordering::Relaxed);

    let requests = requests
        .lock()
        .map_err(|_| io::Error::other("request capture lock poisoned"))?;
    let [first, duplicate] = requests.as_slice() else {
        return Err(io::Error::other("expected two source schedule requests").into());
    };
    println!("first_key={}", first.key);
    println!("duplicate_key={}", duplicate.key);
    println!("delay_ms={}", first.after.as_nanos() / 1_000_000);
    println!("priority={}", first.priority);
    println!("producer_evaluations_at_50ms={producers_at_50}");
    println!("callback_evaluations_at_50ms={callbacks_at_50}");
    println!("callback_evaluations_at_199ms={callbacks_at_199}");
    println!("callback_evaluations_at_200ms={callbacks_at_200}");
    println!(
        "duplicate_preserved_first={}",
        producers_at_50 == 2
            && callbacks_at_50 == 0
            && callbacks_at_199 == 0
            && callbacks_at_200 == 1
    );
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = env::args()
        .nth(1)
        .ok_or_else(|| io::Error::other("usage: vanilla_repeater_schedule <transaction.json>"))?;
    let json = fs::read_to_string(path)?;
    run(serde_json::from_str(&json)?)
}
