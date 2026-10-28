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
    MemoryTransactionPerformedHostCall, TransactionDatum, TransactionProgram,
};
use minecraft_redox_quiescence::{
    ReactionArgument, ReactionHostPreparation, SnapshotInterner, TransactionReactionAdapter,
    TransactionScheduleRequest,
};
use quiescence_core::{NodeId, ReactionId, SimDuration, SimTime};
use quiescence_runtime::{EvalError, Graph, ReactionOutput, RuntimeConfig, StateWrite};

const WORLD: u64 = 1;
const REPEATER: u64 = 10;
const X: i64 = 10;
const Y: i64 = 64;
const Z: i64 = 20;
const META: i64 = 4;
const UNPOWERED_STATE: u64 = 1;
const POWERED_STATE: u64 = 2;
const STONE_STATE: u64 = 3;
const CHUNK_UNAVAILABLE: u64 = 0;
const CHUNK_AVAILABLE: u64 = 1;

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

fn preparation(powered: bool) -> ReactionHostPreparation {
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
                        TransactionDatum::Boolean(powered),
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

fn block_id_for_state(state: u64) -> Result<&'static str, EvalError> {
    match state {
        UNPOWERED_STATE => Ok("minecraft:unpowered_repeater"),
        POWERED_STATE => Ok("minecraft:powered_repeater"),
        STONE_STATE => Ok("minecraft:stone"),
        _ => Err(EvalError::Message(format!(
            "unknown block-state atom {state}"
        ))),
    }
}

fn associated_block(current: &str, expected: &str) -> bool {
    let repeater = |block: &str| {
        matches!(
            block,
            "minecraft:unpowered_repeater" | "minecraft:powered_repeater"
        )
    };
    current == expected || repeater(current) && repeater(expected)
}

fn callback_matches_schedule(
    adapter: &TransactionReactionAdapter,
    context: &quiescence_runtime::EvalContext<'_>,
    block_state: quiescence_core::NodeId,
) -> Result<bool, EvalError> {
    let key = context
        .scheduled_key()
        .ok_or_else(|| EvalError::Message(String::from("callback was not keyed")))?;
    let identity = adapter.schedule_identity(key)?;
    let [
        TransactionDatum::Integer(x),
        TransactionDatum::Integer(y),
        TransactionDatum::Integer(z),
        TransactionDatum::String(expected),
    ] = identity.as_slice()
    else {
        return Err(EvalError::Message(format!(
            "unexpected repeater schedule identity {identity:?}"
        )));
    };
    if (*x, *y, *z) != (X, Y, Z) {
        return Err(EvalError::Message(format!(
            "callback identity targets unexpected position ({x},{y},{z})"
        )));
    }
    let current = block_id_for_state(context.read_atom(block_state)?)?;
    Ok(associated_block(current, expected))
}

type ExecutionRecord = (
    usize,
    Vec<MemoryTransactionPerformedHostCall>,
    Vec<TransactionScheduleRequest>,
);

struct Scenario {
    graph: Graph,
    block_state: NodeId,
    chunk_available: NodeId,
    callback: ReactionId,
    accepted_callbacks: Arc<AtomicUsize>,
    rejected_callbacks: Arc<AtomicUsize>,
    unavailable_callbacks: Arc<AtomicUsize>,
    observed: Arc<Mutex<Vec<ExecutionRecord>>>,
}

struct ScenarioResult {
    state_after_initial: u64,
    next_after_initial: Option<SimTime>,
    executions_at_199: usize,
    state_after_associated_callback: u64,
    accepted_at_200: usize,
    rejected_at_200: usize,
    executions_at_399: usize,
    accepted_at_400: usize,
    rejected_at_400: usize,
    next_after_rejection: Option<SimTime>,
    executions_after_reseed: usize,
    next_after_reseed: Option<SimTime>,
    unavailable_at_700: usize,
    unavailable_at_701: usize,
    executions_after_unavailable: usize,
    next_after_unavailable: Option<SimTime>,
}

fn build_adapter(program: TransactionProgram, block_state: NodeId) -> TransactionReactionAdapter {
    TransactionReactionAdapter::new(
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
    .with_host_preparer(move |context| {
        let state = context.read_atom(block_state)?;
        let powered = match state {
            UNPOWERED_STATE => false,
            POWERED_STATE => true,
            _ => {
                return Err(EvalError::Message(format!(
                    "cannot execute repeater callback for block-state atom {state}"
                )));
            }
        };
        Ok(preparation(powered))
    })
    .with_host_call_publisher(move |call| block_state_write(block_state, call))
    .with_schedule_unit(SimDuration::from_millis(50))
}

fn build_scenario(program: TransactionProgram) -> Result<Scenario, Box<dyn std::error::Error>> {
    let mut graph = Graph::new(RuntimeConfig::default());
    let block_state = graph.add_atom_input("repeater block state", UNPOWERED_STATE)?;
    let chunk_available = graph.add_atom_input("callback chunk available", CHUNK_AVAILABLE)?;
    let adapter = build_adapter(program, block_state);

    let callback_id = Arc::new(Mutex::new(None));
    let callback_id_for_body = Arc::clone(&callback_id);
    let accepted_callbacks = Arc::new(AtomicUsize::new(0));
    let rejected_callbacks = Arc::new(AtomicUsize::new(0));
    let unavailable_callbacks = Arc::new(AtomicUsize::new(0));
    let accepted = Arc::clone(&accepted_callbacks);
    let rejected = Arc::clone(&rejected_callbacks);
    let unavailable = Arc::clone(&unavailable_callbacks);
    let observed = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&observed);
    let callback_adapter = adapter.clone();

    let callback = graph.add_event_callback("recovered repeater updateTick", move |context| {
        if context.scheduled_key().is_some()
            && context.read_atom(chunk_available)? == CHUNK_UNAVAILABLE
        {
            unavailable.fetch_add(1, Ordering::Relaxed);
            return Ok(ReactionOutput::idle());
        }
        if context.scheduled_key().is_some()
            && !callback_matches_schedule(&callback_adapter, context, block_state)?
        {
            rejected.fetch_add(1, Ordering::Relaxed);
            return Ok(ReactionOutput::idle());
        }
        if context.scheduled_key().is_some() {
            accepted.fetch_add(1, Ordering::Relaxed);
        }
        let result = callback_adapter.execute(context)?;
        captured
            .lock()
            .map_err(|_| EvalError::Message(String::from("capture lock poisoned")))?
            .push((
                result.execution.steps,
                result.performed_host_calls.clone(),
                result.schedule_requests.clone(),
            ));
        let target = callback_id_for_body
            .lock()
            .map_err(|_| EvalError::Message(String::from("callback-ID lock poisoned")))?
            .ok_or_else(|| EvalError::Message(String::from("callback ID is not initialized")))?;
        Ok(result.into_reaction_output_for_callback(target))
    })?;
    *callback_id
        .lock()
        .map_err(|_| io::Error::other("callback-ID lock poisoned"))? = Some(callback);

    Ok(Scenario {
        graph,
        block_state,
        chunk_available,
        callback,
        accepted_callbacks,
        rejected_callbacks,
        unavailable_callbacks,
        observed,
    })
}

fn execution_count(scenario: &Scenario) -> Result<usize, io::Error> {
    scenario
        .observed
        .lock()
        .map(|observed| observed.len())
        .map_err(|_| io::Error::other("capture lock poisoned"))
}

fn run_timeline(scenario: &mut Scenario) -> Result<ScenarioResult, Box<dyn std::error::Error>> {
    scenario
        .graph
        .schedule_reaction_event(scenario.callback, SimTime::ZERO, 0)?;
    scenario.graph.advance_to(SimTime::ZERO)?;
    let state_after_initial = scenario.graph.value_atom(scenario.block_state)?;
    let next_after_initial = scenario.graph.next_event_time();

    scenario.graph.schedule_atom(
        scenario.block_state,
        SimTime::from_millis(100),
        UNPOWERED_STATE,
    )?;
    scenario.graph.advance_to(SimTime::from_millis(199))?;
    let executions_at_199 = execution_count(scenario)?;
    scenario.graph.advance_to(SimTime::from_millis(200))?;
    let state_after_associated_callback = scenario.graph.value_atom(scenario.block_state)?;
    let accepted_at_200 = scenario.accepted_callbacks.load(Ordering::Relaxed);
    let rejected_at_200 = scenario.rejected_callbacks.load(Ordering::Relaxed);

    scenario
        .graph
        .schedule_atom(scenario.block_state, SimTime::from_millis(300), STONE_STATE)?;
    scenario.graph.advance_to(SimTime::from_millis(399))?;
    let executions_at_399 = execution_count(scenario)?;
    scenario.graph.advance_to(SimTime::from_millis(400))?;
    let accepted_at_400 = scenario.accepted_callbacks.load(Ordering::Relaxed);
    let rejected_at_400 = scenario.rejected_callbacks.load(Ordering::Relaxed);
    let next_after_rejection = scenario.graph.next_event_time();

    scenario.graph.schedule_atom(
        scenario.block_state,
        SimTime::from_millis(500),
        UNPOWERED_STATE,
    )?;
    scenario.graph.schedule_atom(
        scenario.chunk_available,
        SimTime::from_millis(500),
        CHUNK_AVAILABLE,
    )?;
    scenario
        .graph
        .schedule_reaction_event(scenario.callback, SimTime::from_millis(501), 0)?;
    scenario.graph.advance_to(SimTime::from_millis(501))?;
    let executions_after_reseed = execution_count(scenario)?;
    let next_after_reseed = scenario.graph.next_event_time();
    scenario.graph.schedule_atom(
        scenario.chunk_available,
        SimTime::from_millis(600),
        CHUNK_UNAVAILABLE,
    )?;
    scenario.graph.advance_to(SimTime::from_millis(700))?;
    let unavailable_at_700 = scenario.unavailable_callbacks.load(Ordering::Relaxed);
    scenario.graph.advance_to(SimTime::from_millis(701))?;
    let unavailable_at_701 = scenario.unavailable_callbacks.load(Ordering::Relaxed);
    let executions_after_unavailable = execution_count(scenario)?;
    let next_after_unavailable = scenario.graph.next_event_time();

    Ok(ScenarioResult {
        state_after_initial,
        next_after_initial,
        executions_at_199,
        state_after_associated_callback,
        accepted_at_200,
        rejected_at_200,
        executions_at_399,
        accepted_at_400,
        rejected_at_400,
        next_after_rejection,
        executions_after_reseed,
        next_after_reseed,
        unavailable_at_700,
        unavailable_at_701,
        executions_after_unavailable,
        next_after_unavailable,
    })
}

fn report_result(
    scenario: &Scenario,
    result: &ScenarioResult,
) -> Result<bool, Box<dyn std::error::Error>> {
    let observed = scenario
        .observed
        .lock()
        .map_err(|_| io::Error::other("capture lock poisoned"))?;
    println!("callback_body_executions={}", observed.len());
    println!("first_executed_statements={}", observed[0].0);
    println!("second_executed_statements={}", observed[1].0);
    println!("state_after_initial={}", result.state_after_initial);
    println!("next_after_initial={:?}", result.next_after_initial);
    println!("executions_at_199ms={}", result.executions_at_199);
    println!(
        "state_after_associated_callback={}",
        result.state_after_associated_callback
    );
    println!("accepted_at_200ms={}", result.accepted_at_200);
    println!("rejected_at_200ms={}", result.rejected_at_200);
    println!("executions_at_399ms={}", result.executions_at_399);
    println!("accepted_at_400ms={}", result.accepted_at_400);
    println!("rejected_at_400ms={}", result.rejected_at_400);
    println!("next_after_rejection={:?}", result.next_after_rejection);
    println!("executions_after_reseed={}", result.executions_after_reseed);
    println!("next_after_reseed={:?}", result.next_after_reseed);
    println!("unavailable_at_700ms={}", result.unavailable_at_700);
    println!("unavailable_at_701ms={}", result.unavailable_at_701);
    println!(
        "executions_after_unavailable={}",
        result.executions_after_unavailable
    );
    println!("next_after_unavailable={:?}", result.next_after_unavailable);
    Ok(observed.len() == 3
        && observed.iter().all(|(steps, calls, schedules)| {
            *steps == 130 && calls.len() == 1 && schedules.len() == 1
        })
        && result.state_after_initial == POWERED_STATE
        && result.next_after_initial == Some(SimTime::from_millis(200))
        && result.executions_at_199 == 1
        && result.state_after_associated_callback == POWERED_STATE
        && result.accepted_at_200 == 1
        && result.rejected_at_200 == 0
        && result.executions_at_399 == 2
        && result.accepted_at_400 == 1
        && result.rejected_at_400 == 1
        && result.next_after_rejection.is_none()
        && result.executions_after_reseed == 3
        && result.next_after_reseed == Some(SimTime::from_millis(701))
        && result.unavailable_at_700 == 0
        && result.unavailable_at_701 == 1
        && result.executions_after_unavailable == 3
        && result.next_after_unavailable.is_none())
}

fn run(program: TransactionProgram) -> Result<(), Box<dyn std::error::Error>> {
    let mut scenario = build_scenario(program)?;
    let result = run_timeline(&mut scenario)?;
    let correct = report_result(&scenario, &result)?;
    println!("callback_revalidation_correct={correct}");
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = env::args()
        .nth(1)
        .ok_or_else(|| io::Error::other("usage: vanilla_repeater_callback <transaction.json>"))?;
    let json = fs::read_to_string(path)?;
    run(serde_json::from_str(&json)?)
}
