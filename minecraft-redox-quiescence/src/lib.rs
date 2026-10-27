//! Adapter between source-neutral Redox transactions and Quiescence atom-state reactions.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

use minecraft_redox_core::transaction::{
    MemoryTransactionHost, MemoryTransactionObject, MemoryTransactionState, MemoryTransactionStore,
    TransactionDatum, TransactionExecution, TransactionExecutionLimits, TransactionProgram,
    execute_transaction,
};
use quiescence_core::NodeId;
use quiescence_runtime::{EvalContext, EvalError, StateWrite};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CanonicalValueSnapshot {
    pub components: BTreeMap<String, TransactionDatum>,
    #[serde(default)]
    pub semantic_tag: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CanonicalStoreSnapshot {
    pub slots: Vec<Option<CanonicalValueSnapshot>>,
    #[serde(default)]
    pub capabilities: BTreeSet<String>,
    #[serde(default = "default_true")]
    pub allows_read: bool,
    #[serde(default = "default_true")]
    pub allows_write: bool,
}

const fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum CanonicalDatumKey {
    Integer(i64),
    Float(u64),
    Boolean(bool),
    String(String),
    Null,
    Sequence(Vec<Self>),
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct CanonicalValueKey {
    components: Vec<(String, CanonicalDatumKey)>,
    semantic_tag: Option<String>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct CanonicalStoreKey {
    slots: Vec<Option<CanonicalValueKey>>,
    capabilities: Vec<String>,
    allows_read: bool,
    allows_write: bool,
}

#[derive(Clone, Debug, Default)]
pub struct SnapshotInterner {
    snapshots: Vec<CanonicalStoreSnapshot>,
    ids: BTreeMap<CanonicalStoreKey, u64>,
}

impl SnapshotInterner {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Intern one immutable store snapshot and return its exact atom identity.
    ///
    /// Canonical identity is structural. Floating-point components use their exact IEEE bit
    /// pattern, so signed zero and different NaN payloads do not collapse accidentally.
    ///
    /// # Errors
    ///
    /// Returns an error if the snapshot contains an ephemeral transaction handle or if the
    /// snapshot count cannot be represented as `u64`.
    pub fn intern(&mut self, snapshot: CanonicalStoreSnapshot) -> Result<u64, String> {
        let key = canonical_store_key(&snapshot)?;
        if let Some(id) = self.ids.get(&key) {
            return Ok(*id);
        }
        let next = self
            .snapshots
            .len()
            .checked_add(1)
            .ok_or_else(|| String::from("snapshot count overflow"))?;
        let id = u64::try_from(next).map_err(|_| String::from("snapshot ID exceeds u64"))?;
        self.snapshots.push(snapshot);
        self.ids.insert(key, id);
        Ok(id)
    }

    #[must_use]
    pub fn get(&self, id: u64) -> Option<&CanonicalStoreSnapshot> {
        let index = usize::try_from(id).ok()?.checked_sub(1)?;
        self.snapshots.get(index)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.snapshots.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.snapshots.is_empty()
    }
}

fn canonical_store_key(snapshot: &CanonicalStoreSnapshot) -> Result<CanonicalStoreKey, String> {
    let slots = snapshot
        .slots
        .iter()
        .map(|slot| slot.as_ref().map(canonical_value_key).transpose())
        .collect::<Result<Vec<_>, _>>()?;
    Ok(CanonicalStoreKey {
        slots,
        capabilities: snapshot.capabilities.iter().cloned().collect(),
        allows_read: snapshot.allows_read,
        allows_write: snapshot.allows_write,
    })
}

fn canonical_value_key(value: &CanonicalValueSnapshot) -> Result<CanonicalValueKey, String> {
    let components = value
        .components
        .iter()
        .map(|(name, value)| Ok((name.clone(), canonical_datum_key(value)?)))
        .collect::<Result<Vec<_>, String>>()?;
    Ok(CanonicalValueKey {
        components,
        semantic_tag: value.semantic_tag.clone(),
    })
}

fn canonical_datum_key(value: &TransactionDatum) -> Result<CanonicalDatumKey, String> {
    match value {
        TransactionDatum::Integer(value) => Ok(CanonicalDatumKey::Integer(*value)),
        TransactionDatum::Float(value) => Ok(CanonicalDatumKey::Float(value.to_bits())),
        TransactionDatum::Boolean(value) => Ok(CanonicalDatumKey::Boolean(*value)),
        TransactionDatum::String(value) => Ok(CanonicalDatumKey::String(value.clone())),
        TransactionDatum::Null => Ok(CanonicalDatumKey::Null),
        TransactionDatum::Sequence(values) => values
            .iter()
            .map(canonical_datum_key)
            .collect::<Result<Vec<_>, _>>()
            .map(CanonicalDatumKey::Sequence),
        TransactionDatum::Handle(handle) => Err(format!(
            "canonical value component contains ephemeral handle {handle}"
        )),
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ReactionStoreBinding {
    pub node: NodeId,
    pub local_handle: u64,
}

#[derive(Clone, Debug)]
pub enum ReactionArgument {
    Store { binding_index: usize },
    Datum(TransactionDatum),
}

#[derive(Clone)]
pub struct TransactionReactionAdapter {
    program: Arc<TransactionProgram>,
    interner: Arc<Mutex<SnapshotInterner>>,
    stores: Vec<ReactionStoreBinding>,
    arguments: Vec<ReactionArgument>,
    receiver: Option<ReactionArgument>,
    quantity_component: String,
}

#[derive(Clone, Debug)]
pub struct TransactionReactionResult {
    pub execution: TransactionExecution,
    pub writes: Vec<StateWrite>,
    pub state_values: BTreeMap<u64, BTreeMap<String, TransactionDatum>>,
}

impl TransactionReactionAdapter {
    #[must_use]
    pub fn new(
        program: Arc<TransactionProgram>,
        interner: Arc<Mutex<SnapshotInterner>>,
        stores: Vec<ReactionStoreBinding>,
        arguments: Vec<ReactionArgument>,
        receiver: Option<ReactionArgument>,
        quantity_component: impl Into<String>,
    ) -> Self {
        Self {
            program,
            interner,
            stores,
            arguments,
            receiver,
            quantity_component: quantity_component.into(),
        }
    }

    #[must_use]
    pub fn interner(&self) -> &Arc<Mutex<SnapshotInterner>> {
        &self.interner
    }

    /// Execute the transaction using atom nodes as canonical store snapshots.
    ///
    /// Reading store atoms happens only when this method is called, so a surrounding reaction can
    /// preserve temporal phase-dependent dependencies by checking deadlines before invoking it.
    ///
    /// # Errors
    ///
    /// Returns `EvalError::Message` for invalid snapshot IDs, poisoned snapshot storage, malformed
    /// transaction state, or transaction execution failures. Ordinary graph read errors propagate.
    pub fn execute(
        &self,
        context: &EvalContext<'_>,
    ) -> Result<TransactionReactionResult, EvalError> {
        let snapshot_ids = self
            .stores
            .iter()
            .map(|binding| context.read_atom(binding.node))
            .collect::<Result<Vec<_>, _>>()?;
        let mut interner = self
            .interner
            .lock()
            .map_err(|_| EvalError::Message(String::from("snapshot interner lock poisoned")))?;
        let mut host = build_host(
            &interner,
            &self.stores,
            &snapshot_ids,
            &self.quantity_component,
        )
        .map_err(EvalError::Message)?;
        let receiver = self
            .receiver
            .as_ref()
            .map(|argument| lower_argument(argument, &self.stores))
            .transpose()
            .map_err(EvalError::Message)?;
        let arguments = self
            .arguments
            .iter()
            .map(|argument| lower_argument(argument, &self.stores))
            .collect::<Result<Vec<_>, _>>()
            .map_err(EvalError::Message)?;
        let execution = execute_transaction(
            &self.program,
            &mut host,
            receiver,
            arguments,
            TransactionExecutionLimits::default(),
        )
        .map_err(|error| EvalError::Message(error.to_string()))?;
        let state_values = host.state.state_values.clone();
        let writes = publish_stores(&mut interner, &self.stores, &snapshot_ids, &host.state)
            .map_err(EvalError::Message)?;
        Ok(TransactionReactionResult {
            execution,
            writes,
            state_values,
        })
    }
}

fn lower_argument(
    argument: &ReactionArgument,
    stores: &[ReactionStoreBinding],
) -> Result<TransactionDatum, String> {
    match argument {
        ReactionArgument::Datum(value) => Ok(value.clone()),
        ReactionArgument::Store { binding_index } => stores
            .get(*binding_index)
            .map(|binding| TransactionDatum::Handle(binding.local_handle))
            .ok_or_else(|| format!("store binding index {binding_index} is out of bounds")),
    }
}

fn build_host(
    interner: &SnapshotInterner,
    bindings: &[ReactionStoreBinding],
    snapshot_ids: &[u64],
    quantity_component: &str,
) -> Result<MemoryTransactionHost, String> {
    if bindings.len() != snapshot_ids.len() {
        return Err(String::from("store binding/snapshot count mismatch"));
    }
    let max_store_handle = bindings
        .iter()
        .map(|binding| binding.local_handle)
        .max()
        .unwrap_or(0);
    let mut state = MemoryTransactionState {
        next_handle: max_store_handle.saturating_add(1),
        ..MemoryTransactionState::default()
    };
    for (binding, snapshot_id) in bindings.iter().zip(snapshot_ids) {
        let snapshot = interner
            .get(*snapshot_id)
            .ok_or_else(|| format!("canonical snapshot {snapshot_id} does not exist"))?;
        materialize_store(&mut state, binding.local_handle, snapshot)?;
    }
    Ok(MemoryTransactionHost {
        state,
        quantity_component: quantity_component.to_owned(),
    })
}

fn materialize_store(
    state: &mut MemoryTransactionState,
    store_handle: u64,
    snapshot: &CanonicalStoreSnapshot,
) -> Result<(), String> {
    let mut slots = Vec::with_capacity(snapshot.slots.len());
    for slot in &snapshot.slots {
        let handle = match slot {
            None => None,
            Some(value) => {
                validate_components(&value.components)?;
                let handle = state.next_handle;
                state.next_handle = state.next_handle.saturating_add(1);
                state.objects.insert(
                    handle,
                    MemoryTransactionObject {
                        components: value.components.clone(),
                        semantic_tag: value.semantic_tag.clone(),
                    },
                );
                Some(handle)
            }
        };
        slots.push(handle);
    }
    state.stores.insert(
        store_handle,
        MemoryTransactionStore {
            slots,
            allows_read: snapshot.allows_read,
            allows_write: snapshot.allows_write,
        },
    );
    if !snapshot.capabilities.is_empty() {
        state
            .capabilities
            .insert(store_handle, snapshot.capabilities.clone());
    }
    Ok(())
}

fn validate_components(components: &BTreeMap<String, TransactionDatum>) -> Result<(), String> {
    for value in components.values() {
        validate_canonical_datum(value)?;
    }
    Ok(())
}

fn validate_canonical_datum(value: &TransactionDatum) -> Result<(), String> {
    match value {
        TransactionDatum::Handle(handle) => Err(format!(
            "canonical value component contains ephemeral handle {handle}"
        )),
        TransactionDatum::Sequence(values) => {
            for value in values {
                validate_canonical_datum(value)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn publish_stores(
    interner: &mut SnapshotInterner,
    bindings: &[ReactionStoreBinding],
    old_snapshot_ids: &[u64],
    state: &MemoryTransactionState,
) -> Result<Vec<StateWrite>, String> {
    let mut writes = Vec::new();
    for (index, binding) in bindings.iter().enumerate() {
        let snapshot = extract_store(state, binding.local_handle)?;
        let snapshot_id = interner.intern(snapshot)?;
        if old_snapshot_ids.get(index).copied() != Some(snapshot_id) {
            writes.push(StateWrite::atom(binding.node, snapshot_id));
        }
    }
    Ok(writes)
}

fn extract_store(
    state: &MemoryTransactionState,
    store_handle: u64,
) -> Result<CanonicalStoreSnapshot, String> {
    let store = state
        .stores
        .get(&store_handle)
        .ok_or_else(|| format!("transaction store {store_handle} is missing after execution"))?;
    let slots = store
        .slots
        .iter()
        .map(|slot| {
            slot.map(|handle| {
                let object = state
                    .objects
                    .get(&handle)
                    .ok_or_else(|| format!("published object {handle} is missing"))?;
                validate_components(&object.components)?;
                Ok(CanonicalValueSnapshot {
                    components: object.components.clone(),
                    semantic_tag: object.semantic_tag.clone(),
                })
            })
            .transpose()
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(CanonicalStoreSnapshot {
        slots,
        capabilities: state
            .capabilities
            .get(&store_handle)
            .cloned()
            .unwrap_or_default(),
        allows_read: store.allows_read,
        allows_write: store.allows_write,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    use minecraft_redox_core::transaction::{
        TransactionEffect, TransactionFunction, TransactionFunctionId, TransactionLocalId,
        TransactionOperation, TransactionStatement, TransactionStatementId, TransactionValue,
    };
    use quiescence_core::{SimDuration, SimTime};
    use quiescence_runtime::{Graph, ReactionOutput, RuntimeConfig};

    use super::*;

    fn statement(id: u32, operation: TransactionOperation) -> TransactionStatement {
        TransactionStatement {
            id: TransactionStatementId(id),
            operation,
        }
    }

    fn program(statements: Vec<TransactionStatement>, locals: u32) -> TransactionProgram {
        TransactionProgram {
            entry: TransactionFunctionId(0),
            functions: vec![TransactionFunction {
                id: TransactionFunctionId(0),
                parameter_count: 1,
                local_count: locals,
                entry_statement: TransactionStatementId(0),
                statements,
            }],
        }
    }

    fn stack(quantity: i64) -> CanonicalValueSnapshot {
        CanonicalValueSnapshot {
            components: [(
                String::from("quantity"),
                TransactionDatum::Integer(quantity),
            )]
            .into_iter()
            .collect(),
            semantic_tag: Some(String::from("test_item")),
        }
    }

    fn snapshot(quantity: i64) -> CanonicalStoreSnapshot {
        CanonicalStoreSnapshot {
            slots: vec![Some(stack(quantity))],
            capabilities: BTreeSet::default(),
            allows_read: true,
            allows_write: true,
        }
    }

    fn adapter(
        program: TransactionProgram,
        interner: Arc<Mutex<SnapshotInterner>>,
        store: NodeId,
    ) -> TransactionReactionAdapter {
        TransactionReactionAdapter::new(
            Arc::new(program),
            interner,
            vec![ReactionStoreBinding {
                node: store,
                local_handle: 1,
            }],
            vec![ReactionArgument::Store { binding_index: 0 }],
            None,
            "quantity",
        )
    }

    fn take_restore_program() -> TransactionProgram {
        let store = TransactionValue::Parameter { index: 0 };
        let key = vec![TransactionValue::Integer { value: 0 }];
        program(
            vec![
                statement(
                    0,
                    TransactionOperation::Assign {
                        target: TransactionLocalId(0),
                        value: TransactionValue::CloneValue {
                            value: Box::new(TransactionValue::StoreRead {
                                store: Box::new(store.clone()),
                                key: key.clone(),
                            }),
                        },
                        next: TransactionStatementId(1),
                    },
                ),
                statement(
                    1,
                    TransactionOperation::Assign {
                        target: TransactionLocalId(1),
                        value: TransactionValue::StoreTake {
                            store: Box::new(store.clone()),
                            key: key.clone(),
                            amount: Box::new(TransactionValue::Integer { value: 1 }),
                        },
                        next: TransactionStatementId(2),
                    },
                ),
                statement(
                    2,
                    TransactionOperation::Effect {
                        effect: TransactionEffect::StoreWrite {
                            store,
                            key,
                            value: TransactionValue::Local {
                                local: TransactionLocalId(0),
                            },
                        },
                        next: TransactionStatementId(3),
                    },
                ),
                statement(3, TransactionOperation::Return { value: None }),
            ],
            2,
        )
    }

    fn idle_program() -> TransactionProgram {
        program(
            vec![statement(0, TransactionOperation::Return { value: None })],
            0,
        )
    }

    #[test]
    fn interner_validates_handles_and_uses_exact_float_identity() -> Result<(), String> {
        let mut plus_zero = snapshot(5);
        plus_zero.slots[0]
            .as_mut()
            .ok_or_else(|| String::from("test slot missing"))?
            .components
            .insert(String::from("phase"), TransactionDatum::Float(0.0));
        let mut minus_zero = plus_zero.clone();
        minus_zero.slots[0]
            .as_mut()
            .ok_or_else(|| String::from("test slot missing"))?
            .components
            .insert(String::from("phase"), TransactionDatum::Float(-0.0));
        let mut interner = SnapshotInterner::new();
        let plus_id = interner.intern(plus_zero.clone())?;
        let minus_id = interner.intern(minus_zero)?;
        assert_ne!(plus_id, minus_id);
        assert_eq!(interner.intern(plus_zero)?, plus_id);

        let mut invalid = snapshot(5);
        invalid.slots[0]
            .as_mut()
            .ok_or_else(|| String::from("test slot missing"))?
            .components
            .insert(String::from("bad"), TransactionDatum::Handle(99));
        let error = interner
            .intern(invalid)
            .err()
            .ok_or_else(|| String::from("ephemeral handle was accepted"))?;
        assert!(error.contains("ephemeral handle"));
        Ok(())
    }

    #[test]
    fn unchanged_canonical_store_emits_no_atom_write() -> Result<(), Box<dyn std::error::Error>> {
        let mut interner = SnapshotInterner::new();
        let initial = interner.intern(snapshot(5))?;
        let interner = Arc::new(Mutex::new(interner));
        let mut graph = Graph::new(RuntimeConfig::default());
        let store = graph.add_atom_input("store", initial)?;
        let adapter = adapter(take_restore_program(), Arc::clone(&interner), store);
        let write_count = Arc::new(AtomicUsize::new(usize::MAX));
        let observed = Arc::clone(&write_count);
        graph.add_reaction("take restore", move |context| {
            let result = adapter.execute(context)?;
            observed.store(result.writes.len(), Ordering::Relaxed);
            Ok(ReactionOutput::immediate(result.writes))
        })?;

        graph.settle()?;
        assert_eq!(graph.value_atom(store)?, initial);
        assert_eq!(write_count.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn store_dependency_appears_only_after_deadline() -> Result<(), Box<dyn std::error::Error>> {
        let mut interner = SnapshotInterner::new();
        let first = interner.intern(snapshot(5))?;
        let second = interner.intern(snapshot(4))?;
        let interner = Arc::new(Mutex::new(interner));
        let mut graph = Graph::new(RuntimeConfig::default());
        let deadline = graph.add_atom_input("deadline_ms", 100)?;
        let store = graph.add_atom_input("store", first)?;
        let adapter = adapter(idle_program(), interner, store);
        let evaluations = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&evaluations);
        graph.add_reaction("deadline gated transaction", move |context| {
            observed.fetch_add(1, Ordering::Relaxed);
            let now_ms = u64::try_from(context.now().as_nanos() / 1_000_000)
                .map_err(|_| EvalError::Message(String::from("negative time")))?;
            let ready_at = context.read_atom(deadline)?;
            if now_ms < ready_at {
                let delay = i64::try_from(ready_at - now_ms)
                    .map_err(|_| EvalError::Message(String::from("deadline delta overflow")))?;
                return Ok(ReactionOutput::reevaluate_after(SimDuration::from_millis(
                    delay,
                )));
            }
            let result = adapter.execute(context)?;
            Ok(ReactionOutput::immediate(result.writes))
        })?;

        graph.settle()?;
        assert_eq!(evaluations.load(Ordering::Relaxed), 1);
        graph.schedule_atom(store, SimTime::from_millis(50), second)?;
        graph.advance_to(SimTime::from_millis(50))?;
        assert_eq!(evaluations.load(Ordering::Relaxed), 1);
        graph.advance_to(SimTime::from_millis(100))?;
        assert_eq!(evaluations.load(Ordering::Relaxed), 2);
        graph.schedule_atom(store, SimTime::from_millis(110), first)?;
        graph.advance_to(SimTime::from_millis(110))?;
        assert_eq!(evaluations.load(Ordering::Relaxed), 3);
        Ok(())
    }
}
