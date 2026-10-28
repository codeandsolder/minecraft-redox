use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
};

use anyhow::{Context, Result, bail};
use num_bigint::BigInt;
use serde::{Deserialize, Serialize};

use super::{
    AbstractArrayEntry, AbstractArrayIdentity, AbstractArraySnapshot, AbstractFieldValue,
    AbstractInvocation, AbstractObjectSnapshot, AbstractValue, ExecutionState, JimpleDirectEffect,
    JimpleFieldComparison, JimpleGuard, JimpleInvocation, JimpleMethod, JimpleMethodSignature,
    JimpleOperation, JimpleStatement, JimpleValue, JimpleValueKind, MutableArray, MutableObject,
    SymbolicPiecewiseArm, SymbolicValue, abstract_array_identity, abstract_binary, abstract_cast,
    abstract_default_value, abstract_truth, abstract_unary, substitute_symbolic,
};

#[derive(Clone, Debug, Default)]
pub struct JimpleProgram {
    methods: Vec<JimpleMethod>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct JimpleEffectCallFrame {
    pub caller: JimpleMethodSignature,
    pub statement_id: usize,
    pub invoke_kind: String,
    pub callee: JimpleMethodSignature,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct JimpleEffectGuard {
    pub method: JimpleMethodSignature,
    pub branch_statement_id: usize,
    pub expected: bool,
    pub condition: SymbolicValue,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct JimpleProgramFieldWrite {
    pub call_path: Vec<JimpleEffectCallFrame>,
    pub guards: Vec<JimpleEffectGuard>,
    pub statement_id: usize,
    pub field: super::JimpleFieldSignature,
    pub receiver: Option<SymbolicValue>,
    pub value: SymbolicValue,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JimpleUnresolvedEffectReason {
    MissingMethod,
    DynamicDispatch,
    RecursiveCycle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JimpleProgramCallResolution {
    Expanded,
    MissingMethod,
    DynamicDispatch,
    RecursiveCycle,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct JimpleProgramCall {
    pub call_path: Vec<JimpleEffectCallFrame>,
    pub guards: Vec<JimpleEffectGuard>,
    pub receiver: Option<SymbolicValue>,
    pub arguments: Vec<SymbolicValue>,
    pub resolution: JimpleProgramCallResolution,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JimpleCallOperandBinding {
    Receiver,
    Argument { index: usize },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JimpleIndexedStoreOperationKind {
    Read,
    Take,
    Write,
    Touch,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct JimpleIndexedStoreBinding {
    pub callee: JimpleMethodSignature,
    pub operation: JimpleIndexedStoreOperationKind,
    pub store: JimpleCallOperandBinding,
    #[serde(default)]
    pub key: Vec<JimpleCallOperandBinding>,
    pub value: Option<JimpleCallOperandBinding>,
    pub amount: Option<JimpleCallOperandBinding>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct JimpleIndexedStoreOperation {
    pub call_path: Vec<JimpleEffectCallFrame>,
    pub guards: Vec<JimpleEffectGuard>,
    pub resolution: JimpleProgramCallResolution,
    pub operation: JimpleIndexedStoreOperationKind,
    pub store: SymbolicValue,
    pub key: Vec<SymbolicValue>,
    pub value: Option<SymbolicValue>,
    pub amount: Option<SymbolicValue>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JimpleIndexedStoreQueryKind {
    Extent,
    Keys,
    Empty,
    Full,
    AllowsRead,
    AllowsWrite,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct JimpleIndexedStoreQueryBinding {
    pub callee: JimpleMethodSignature,
    pub operation: JimpleIndexedStoreQueryKind,
    pub store: JimpleCallOperandBinding,
    #[serde(default)]
    pub key: Vec<JimpleCallOperandBinding>,
    pub value: Option<JimpleCallOperandBinding>,
    #[serde(default)]
    pub context: Vec<JimpleCallOperandBinding>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct JimpleIndexedStoreQueryCandidate {
    pub call_path: Vec<JimpleEffectCallFrame>,
    pub guards: Vec<JimpleEffectGuard>,
    pub resolution: JimpleProgramCallResolution,
    pub operation: JimpleIndexedStoreQueryKind,
    pub store: SymbolicValue,
    pub key: Vec<SymbolicValue>,
    pub value: Option<SymbolicValue>,
    pub context: Vec<SymbolicValue>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct JimpleObjectStateWriteBinding {
    pub callee: JimpleMethodSignature,
    pub target: JimpleCallOperandBinding,
    pub state: String,
    pub value: JimpleCallOperandBinding,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct JimpleObjectStateWriteCandidate {
    pub call_path: Vec<JimpleEffectCallFrame>,
    pub guards: Vec<JimpleEffectGuard>,
    pub resolution: JimpleProgramCallResolution,
    pub target: SymbolicValue,
    pub state: String,
    pub value: SymbolicValue,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct JimpleCapabilityBinding {
    pub source_type: String,
    pub capability: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct JimpleHostQueryBinding {
    pub callee: JimpleMethodSignature,
    pub operation: String,
    pub receiver: Option<JimpleCallOperandBinding>,
    #[serde(default)]
    pub arguments: Vec<JimpleCallOperandBinding>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct JimpleHostQueryCandidate {
    pub call_path: Vec<JimpleEffectCallFrame>,
    pub guards: Vec<JimpleEffectGuard>,
    pub resolution: JimpleProgramCallResolution,
    pub operation: String,
    pub receiver: Option<SymbolicValue>,
    pub arguments: Vec<SymbolicValue>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JimpleStaticConstantValue {
    Integer { value: i64 },
    Boolean { value: bool },
    String { value: String },
    Null,
    Sequence { values: Vec<Self> },
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct JimpleStaticConstantBinding {
    pub field: super::JimpleFieldSignature,
    pub value: JimpleStaticConstantValue,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct JimpleValueComponentBinding {
    pub field: super::JimpleFieldSignature,
    pub component: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct JimpleIndexedStoreValueOrigin {
    pub operation: JimpleIndexedStoreOperationKind,
    pub store: SymbolicValue,
    pub key: Vec<SymbolicValue>,
    pub amount: Option<SymbolicValue>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct JimpleValueComponentUpdateCandidate {
    pub call_path: Vec<JimpleEffectCallFrame>,
    pub guards: Vec<JimpleEffectGuard>,
    pub statement_id: usize,
    pub component: String,
    pub indexed_store_origin: Option<JimpleIndexedStoreValueOrigin>,
    pub value: SymbolicValue,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct JimpleTransactionBindings {
    #[serde(default)]
    pub indexed_stores: Vec<JimpleIndexedStoreBinding>,
    #[serde(default)]
    pub snapshot_clones: Vec<JimpleSnapshotCloneBinding>,
    #[serde(default)]
    pub value_components: Vec<JimpleValueComponentBinding>,
    #[serde(default)]
    pub pure_values: Vec<JimplePureValueBinding>,
    #[serde(default)]
    pub store_queries: Vec<JimpleIndexedStoreQueryBinding>,
    #[serde(default)]
    pub state_writes: Vec<JimpleObjectStateWriteBinding>,
    #[serde(default)]
    pub capabilities: Vec<JimpleCapabilityBinding>,
    #[serde(default)]
    pub host_queries: Vec<JimpleHostQueryBinding>,
    #[serde(default)]
    pub schedule_once: Vec<JimpleScheduleOnceBinding>,
    #[serde(default)]
    pub static_constants: Vec<JimpleStaticConstantBinding>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct JimpleIndexedStoreTransactionCoverage {
    pub operations: Vec<JimpleIndexedStoreOperation>,
    pub rollbacks: Vec<JimpleSpeculativeTakeRollbackCandidate>,
    pub component_updates: Vec<JimpleValueComponentUpdateCandidate>,
    pub value_operations: Vec<JimplePureValueCandidate>,
    pub store_queries: Vec<JimpleIndexedStoreQueryCandidate>,
    pub state_writes: Vec<JimpleObjectStateWriteCandidate>,
    pub host_queries: Vec<JimpleHostQueryCandidate>,
    pub schedule_once: Vec<JimpleScheduleOnceCandidate>,
    pub loops: Vec<JimpleTransactionLoopCandidate>,
    pub unaccounted_field_writes: Vec<JimpleProgramFieldWrite>,
    pub unaccounted_calls: Vec<JimpleProgramCall>,
    pub complete: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct JimpleSnapshotCloneBinding {
    pub callee: JimpleMethodSignature,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct JimpleSpeculativeTakeRollbackCandidate {
    pub store: SymbolicValue,
    pub key: Vec<SymbolicValue>,
    pub amount: SymbolicValue,
    pub snapshot: SymbolicValue,
    pub take_call_path: Vec<JimpleEffectCallFrame>,
    pub restore_call_path: Vec<JimpleEffectCallFrame>,
    pub take_guards: Vec<JimpleEffectGuard>,
    pub restore_guards: Vec<JimpleEffectGuard>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JimplePureValueOperation {
    Component { component: String },
    Equal,
    Min,
    Max,
    Sequence,
    Length,
    Index,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct JimplePureValueBinding {
    pub callee: JimpleMethodSignature,
    pub operation: JimplePureValueOperation,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct JimplePureValueCandidate {
    pub call_path: Vec<JimpleEffectCallFrame>,
    pub guards: Vec<JimpleEffectGuard>,
    pub resolution: JimpleProgramCallResolution,
    pub operation: JimplePureValueOperation,
    pub receiver: Option<SymbolicValue>,
    pub arguments: Vec<SymbolicValue>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct JimpleUniformIntBinding {
    pub callee: JimpleMethodSignature,
    pub bound_argument: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct JimpleUniformIntCandidate {
    pub call_path: Vec<JimpleEffectCallFrame>,
    pub guards: Vec<JimpleEffectGuard>,
    pub resolution: JimpleProgramCallResolution,
    pub stream: Option<SymbolicValue>,
    pub bound_exclusive: SymbolicValue,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct JimpleScheduleOnceBinding {
    pub callee: JimpleMethodSignature,
    pub key_arguments: Vec<usize>,
    pub delay_argument: usize,
    pub priority_argument: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct JimpleScheduleOnceCandidate {
    pub call_path: Vec<JimpleEffectCallFrame>,
    pub guards: Vec<JimpleEffectGuard>,
    pub resolution: JimpleProgramCallResolution,
    pub key: Vec<SymbolicValue>,
    pub delay: SymbolicValue,
    pub priority: Option<SymbolicValue>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct JimpleUnresolvedEffectCall {
    pub call_path: Vec<JimpleEffectCallFrame>,
    pub guards: Vec<JimpleEffectGuard>,
    pub receiver: Option<SymbolicValue>,
    pub arguments: Vec<SymbolicValue>,
    pub reason: JimpleUnresolvedEffectReason,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct JimpleProgramInductionLoop {
    pub call_path: Vec<JimpleEffectCallFrame>,
    pub guards: Vec<JimpleEffectGuard>,
    pub method: JimpleMethodSignature,
    pub header_statement_id: usize,
    pub condition_statement_id: usize,
    pub induction_local: String,
    pub initial_value: i64,
    pub step: i64,
    pub comparison_operator: String,
    pub bound: SymbolicValue,
    pub continue_when_condition: bool,
    pub iterations: Option<u64>,
    pub statement_ids: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JimpleTransactionLoopBound {
    StoreExtent {
        store: SymbolicValue,
    },
    StoreKeysLength {
        store: SymbolicValue,
        context: Vec<SymbolicValue>,
    },
    Symbolic {
        value: SymbolicValue,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct JimpleTransactionLoopCandidate {
    pub call_path: Vec<JimpleEffectCallFrame>,
    pub guards: Vec<JimpleEffectGuard>,
    pub method: JimpleMethodSignature,
    pub header_statement_id: usize,
    pub condition_statement_id: usize,
    pub induction_local: String,
    pub initial_value: i64,
    pub step: i64,
    pub comparison_operator: String,
    pub bound: JimpleTransactionLoopBound,
    pub continue_when_condition: bool,
    pub iterations: Option<u64>,
    pub statement_ids: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct JimpleProgramEffectSummary {
    pub entry: JimpleMethodSignature,
    pub possible_field_writes: Vec<JimpleProgramFieldWrite>,
    pub calls: Vec<JimpleProgramCall>,
    pub induction_loops: Vec<JimpleProgramInductionLoop>,
    pub unresolved_calls: Vec<JimpleUnresolvedEffectCall>,
}

impl JimpleProgramEffectSummary {
    /// Bind source calls to generic indexed-store operations.
    ///
    /// Bindings describe only operand roles and operation class. They do not assign domain types
    /// to stored values, so inventories, registries, slot maps, or other indexed resources can use
    /// the same semantic surface.
    ///
    /// # Errors
    ///
    /// Returns an error when a matching call is missing a configured receiver/argument operand.
    pub fn bind_indexed_store(
        &self,
        bindings: &[JimpleIndexedStoreBinding],
    ) -> Result<Vec<JimpleIndexedStoreOperation>> {
        let mut operations = Vec::new();
        for call in &self.calls {
            let Some(callee) = call.call_path.last().map(|frame| &frame.callee) else {
                continue;
            };
            for binding in bindings.iter().filter(|binding| &binding.callee == callee) {
                let store = select_call_operand(call, &binding.store, "store", &binding.callee)?;
                let key = binding
                    .key
                    .iter()
                    .map(|selector| select_call_operand(call, selector, "key", &binding.callee))
                    .collect::<Result<Vec<_>>>()?;
                let value = binding
                    .value
                    .as_ref()
                    .map(|selector| select_call_operand(call, selector, "value", &binding.callee))
                    .transpose()?;
                let amount = binding
                    .amount
                    .as_ref()
                    .map(|selector| select_call_operand(call, selector, "amount", &binding.callee))
                    .transpose()?;
                operations.push(JimpleIndexedStoreOperation {
                    call_path: call.call_path.clone(),
                    guards: call.guards.clone(),
                    resolution: call.resolution,
                    operation: binding.operation,
                    store,
                    key,
                    value,
                    amount,
                });
            }
        }
        Ok(operations)
    }

    /// Bind source calls to indexed-store policy/topology queries.
    ///
    /// Operand selectors define the store, key, optional value, and contextual dimensions such
    /// as a side or capability view. The source method name itself carries no store semantics.
    ///
    /// # Errors
    ///
    /// Returns an error when a matching call is missing a configured operand.
    pub fn bind_indexed_store_queries(
        &self,
        bindings: &[JimpleIndexedStoreQueryBinding],
    ) -> Result<Vec<JimpleIndexedStoreQueryCandidate>> {
        let mut candidates = Vec::new();
        for call in &self.calls {
            let Some(callee) = call.call_path.last().map(|frame| &frame.callee) else {
                continue;
            };
            let mut matching = bindings.iter().filter(|binding| &binding.callee == callee);
            let Some(binding) = matching.next() else {
                continue;
            };
            if matching.next().is_some() {
                continue;
            }
            let store = select_call_operand(call, &binding.store, "store query store", callee)?;
            let key = binding
                .key
                .iter()
                .map(|selector| select_call_operand(call, selector, "store query key", callee))
                .collect::<Result<Vec<_>>>()?;
            let value = binding
                .value
                .as_ref()
                .map(|selector| select_call_operand(call, selector, "store query value", callee))
                .transpose()?;
            let context = binding
                .context
                .iter()
                .map(|selector| select_call_operand(call, selector, "store query context", callee))
                .collect::<Result<Vec<_>>>()?;
            candidates.push(JimpleIndexedStoreQueryCandidate {
                call_path: call.call_path.clone(),
                guards: call.guards.clone(),
                resolution: call.resolution,
                operation: binding.operation,
                store,
                key,
                value,
                context,
            });
        }
        Ok(candidates)
    }

    /// Bind source setter-style calls to generic object-state writes.
    ///
    /// # Errors
    ///
    /// Returns an error when a matching call is missing its configured target or value operand.
    pub fn bind_object_state_writes(
        &self,
        bindings: &[JimpleObjectStateWriteBinding],
    ) -> Result<Vec<JimpleObjectStateWriteCandidate>> {
        let mut candidates = Vec::new();
        for call in &self.calls {
            let Some(callee) = call.call_path.last().map(|frame| &frame.callee) else {
                continue;
            };
            let mut matching = bindings.iter().filter(|binding| &binding.callee == callee);
            let Some(binding) = matching.next() else {
                continue;
            };
            if matching.next().is_some() {
                continue;
            }
            let target = select_call_operand(call, &binding.target, "state target", callee)?;
            let value = select_call_operand(call, &binding.value, "state value", callee)?;
            candidates.push(JimpleObjectStateWriteCandidate {
                call_path: call.call_path.clone(),
                guards: call.guards.clone(),
                resolution: call.resolution,
                target,
                state: binding.state.clone(),
                value,
            });
        }
        Ok(candidates)
    }

    /// Bind configured mutable source fields into transaction-local immutable component updates.
    ///
    /// Indexed-store provenance is retained when it can be proven, but it is not required: a
    /// value may also come from another bound semantic source such as an entity payload. The
    /// component binding itself defines the field semantics; executable lowering remains
    /// fail-closed if the receiver cannot be represented in the transaction IR.
    #[must_use]
    pub fn bind_value_component_updates(
        &self,
        store_bindings: &[JimpleIndexedStoreBinding],
        component_bindings: &[JimpleValueComponentBinding],
    ) -> Vec<JimpleValueComponentUpdateCandidate> {
        self.possible_field_writes
            .iter()
            .filter_map(|write| {
                let component = unique_value_component_name(&write.field, component_bindings)?;
                let indexed_store_origin = write
                    .receiver
                    .as_ref()
                    .and_then(|receiver| indexed_store_value_origin(receiver, store_bindings));
                Some(JimpleValueComponentUpdateCandidate {
                    call_path: write.call_path.clone(),
                    guards: write.guards.clone(),
                    statement_id: write.statement_id,
                    component,
                    indexed_store_origin,
                    value: write.value.clone(),
                })
            })
            .collect()
    }

    /// Recover speculative indexed-store takes that are explicitly rolled back from a snapshot.
    ///
    /// The detector is intentionally conservative. A take and restore must target the same
    /// symbolic store/key in the same source-method scope, the restore must occur later and carry
    /// every guard of the take, and its value must be a configured snapshot clone of a configured
    /// read of that exact store/key.
    ///
    /// # Errors
    ///
    /// Returns an error when indexed-store bindings reference missing call operands.
    pub fn bind_speculative_take_rollbacks(
        &self,
        store_bindings: &[JimpleIndexedStoreBinding],
        clone_bindings: &[JimpleSnapshotCloneBinding],
    ) -> Result<Vec<JimpleSpeculativeTakeRollbackCandidate>> {
        let operations = self.bind_indexed_store(store_bindings)?;
        let reads: Vec<_> = store_bindings
            .iter()
            .filter(|binding| binding.operation == JimpleIndexedStoreOperationKind::Read)
            .collect();
        let mut candidates = Vec::new();
        for take in operations
            .iter()
            .filter(|operation| operation.operation == JimpleIndexedStoreOperationKind::Take)
        {
            let Some(amount) = take.amount.clone() else {
                continue;
            };
            for restore in operations
                .iter()
                .filter(|operation| operation.operation == JimpleIndexedStoreOperationKind::Write)
            {
                if take.store != restore.store
                    || take.key != restore.key
                    || !same_scope_later(take, restore)
                    || !take
                        .guards
                        .iter()
                        .all(|guard| restore.guards.contains(guard))
                {
                    continue;
                }
                let Some(value) = restore.value.as_ref() else {
                    continue;
                };
                let Some(snapshot) =
                    cloned_read_origin(value, clone_bindings, &reads, &take.store, &take.key)
                else {
                    continue;
                };
                candidates.push(JimpleSpeculativeTakeRollbackCandidate {
                    store: take.store.clone(),
                    key: take.key.clone(),
                    amount: amount.clone(),
                    snapshot,
                    take_call_path: take.call_path.clone(),
                    restore_call_path: restore.call_path.clone(),
                    take_guards: take.guards.clone(),
                    restore_guards: restore.guards.clone(),
                });
            }
        }
        Ok(candidates)
    }

    /// Report how much of an indexed-store transaction surface is semantically accounted for.
    ///
    /// Bound store operations and proven snapshot rollbacks are separated from field writes and
    /// unresolved calls that still require value/domain semantics. `complete` is deliberately
    /// fail-closed: any unaccounted externally visible write or unresolved call keeps it false.
    ///
    /// # Errors
    ///
    /// Returns an error when indexed-store bindings reference missing call operands.
    pub fn indexed_store_transaction_coverage(
        &self,
        bindings: &JimpleTransactionBindings,
    ) -> Result<JimpleIndexedStoreTransactionCoverage> {
        let operations = self.bind_indexed_store(&bindings.indexed_stores)?;
        let rollbacks = self
            .bind_speculative_take_rollbacks(&bindings.indexed_stores, &bindings.snapshot_clones)?;
        let component_updates =
            self.bind_value_component_updates(&bindings.indexed_stores, &bindings.value_components);
        let value_operations = self.bind_pure_value_operations(&bindings.pure_values);
        let store_queries = self.bind_indexed_store_queries(&bindings.store_queries)?;
        let state_writes = self.bind_object_state_writes(&bindings.state_writes)?;
        let host_queries = self.bind_host_queries(&bindings.host_queries)?;
        let schedule_once = bindings
            .schedule_once
            .iter()
            .map(|binding| self.bind_schedule_once(binding))
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        let loops = self.bind_transaction_loops(bindings)?;
        let unaccounted_field_writes: Vec<_> = self
            .possible_field_writes
            .iter()
            .filter(|write| {
                !call_path_crosses_transaction_boundary(&write.call_path, bindings)
                    && !component_updates.iter().any(|update| {
                        update.statement_id == write.statement_id
                            && update.call_path == write.call_path
                            && bindings
                                .value_components
                                .iter()
                                .any(|binding| binding.field == write.field)
                    })
            })
            .cloned()
            .collect();
        let unaccounted_calls: Vec<_> = self
            .calls
            .iter()
            .filter(|call| {
                call.resolution != JimpleProgramCallResolution::Expanded
                    && !call_path_crosses_transaction_boundary(&call.call_path, bindings)
                    && !value_operations
                        .iter()
                        .any(|candidate| candidate.call_path == call.call_path)
                    && !store_queries
                        .iter()
                        .any(|candidate| candidate.call_path == call.call_path)
                    && !state_writes
                        .iter()
                        .any(|candidate| candidate.call_path == call.call_path)
                    && !host_queries
                        .iter()
                        .any(|candidate| candidate.call_path == call.call_path)
                    && !schedule_once
                        .iter()
                        .any(|candidate| candidate.call_path == call.call_path)
            })
            .cloned()
            .collect();
        let complete = unaccounted_field_writes.is_empty() && unaccounted_calls.is_empty();
        Ok(JimpleIndexedStoreTransactionCoverage {
            operations,
            rollbacks,
            component_updates,
            value_operations,
            store_queries,
            state_writes,
            host_queries,
            schedule_once,
            loops,
            unaccounted_field_writes,
            unaccounted_calls,
            complete,
        })
    }

    /// Bind source calls to source-neutral host queries.
    ///
    /// # Errors
    ///
    /// Returns an error when a binding is ambiguous or references a missing operand.
    pub fn bind_host_queries(
        &self,
        bindings: &[JimpleHostQueryBinding],
    ) -> Result<Vec<JimpleHostQueryCandidate>> {
        let mut candidates = Vec::new();
        for call in &self.calls {
            let Some(callee) = call.call_path.last().map(|frame| &frame.callee) else {
                continue;
            };
            let mut matching = bindings.iter().filter(|binding| binding.callee == *callee);
            let Some(binding) = matching.next() else {
                continue;
            };
            if matching.next().is_some() {
                bail!("ambiguous host-query binding for {callee:?}");
            }
            let receiver = binding
                .receiver
                .as_ref()
                .map(|selector| select_call_operand(call, selector, "host query receiver", callee))
                .transpose()?;
            let arguments = binding
                .arguments
                .iter()
                .map(|selector| select_call_operand(call, selector, "host query argument", callee))
                .collect::<Result<Vec<_>>>()?;
            candidates.push(JimpleHostQueryCandidate {
                call_path: call.call_path.clone(),
                guards: call.guards.clone(),
                resolution: call.resolution,
                operation: binding.operation.clone(),
                receiver,
                arguments,
            });
        }
        Ok(candidates)
    }

    /// Bind source calls to typed pure value operations.
    ///
    /// Bindings are semantic rather than permissive: receiver/argument shape must match the
    /// configured operation, and ambiguous duplicate bindings fail closed for that call.
    #[must_use]
    pub fn bind_pure_value_operations(
        &self,
        bindings: &[JimplePureValueBinding],
    ) -> Vec<JimplePureValueCandidate> {
        self.calls
            .iter()
            .filter_map(|call| {
                let callee = &call.call_path.last()?.callee;
                let binding = unique_pure_value_binding(callee, bindings)?;
                if !pure_value_shape_matches(&binding.operation, call) {
                    return None;
                }
                Some(JimplePureValueCandidate {
                    call_path: call.call_path.clone(),
                    guards: call.guards.clone(),
                    resolution: call.resolution,
                    operation: binding.operation,
                    receiver: call.receiver.clone(),
                    arguments: call.arguments.clone(),
                })
            })
            .collect()
    }

    /// Bind interprocedurally recovered induction loops to transaction-level loop bounds.
    ///
    /// Store extent and key-list length are recognized through the same declarative store-query
    /// bindings as ordinary calls. Other bounds remain explicit symbolic expressions.
    ///
    /// # Errors
    ///
    /// Returns an error if a matching store-query binding references a missing operand.
    pub fn bind_transaction_loops(
        &self,
        bindings: &JimpleTransactionBindings,
    ) -> Result<Vec<JimpleTransactionLoopCandidate>> {
        self.induction_loops
            .iter()
            .map(|item| {
                Ok(JimpleTransactionLoopCandidate {
                    call_path: item.call_path.clone(),
                    guards: item.guards.clone(),
                    method: item.method.clone(),
                    header_statement_id: item.header_statement_id,
                    condition_statement_id: item.condition_statement_id,
                    induction_local: item.induction_local.clone(),
                    initial_value: item.initial_value,
                    step: item.step,
                    comparison_operator: item.comparison_operator.clone(),
                    bound: transaction_loop_bound(&item.bound, &bindings.store_queries)?,
                    continue_when_condition: item.continue_when_condition,
                    iterations: item.iterations,
                    statement_ids: item.statement_ids.clone(),
                })
            })
            .collect()
    }

    /// Bind calls to a source RNG API into uniform integer draws.
    ///
    /// The receiver remains explicit so distinct source RNG streams do not collapse together.
    ///
    /// # Errors
    ///
    /// Returns an error if a matching call does not contain the configured bound argument.
    pub fn bind_uniform_int(
        &self,
        binding: &JimpleUniformIntBinding,
    ) -> Result<Vec<JimpleUniformIntCandidate>> {
        self.calls
            .iter()
            .filter(|call| {
                call.call_path
                    .last()
                    .is_some_and(|frame| frame.callee == binding.callee)
            })
            .map(|call| {
                let bound_exclusive = call
                    .arguments
                    .get(binding.bound_argument)
                    .cloned()
                    .with_context(|| {
                        format!(
                            "uniform-int bound argument {} missing from call to {:?}",
                            binding.bound_argument, binding.callee
                        )
                    })?;
                Ok(JimpleUniformIntCandidate {
                    call_path: call.call_path.clone(),
                    guards: call.guards.clone(),
                    resolution: call.resolution,
                    stream: call.receiver.clone(),
                    bound_exclusive,
                })
            })
            .collect()
    }

    /// Bind calls to a source scheduler API into generic schedule-once candidates.
    ///
    /// The binding is declarative: argument positions define the canonical deduplication key,
    /// delay, and optional priority. The source method name itself carries no semantics here.
    ///
    /// # Errors
    ///
    /// Returns an error if a matching call does not contain an argument required by the binding.
    pub fn bind_schedule_once(
        &self,
        binding: &JimpleScheduleOnceBinding,
    ) -> Result<Vec<JimpleScheduleOnceCandidate>> {
        self.calls
            .iter()
            .filter(|call| {
                call.call_path
                    .last()
                    .is_some_and(|frame| frame.callee == binding.callee)
            })
            .map(|call| {
                let key = binding
                    .key_arguments
                    .iter()
                    .map(|index| {
                        call.arguments.get(*index).cloned().with_context(|| {
                            format!(
                                "schedule binding key argument {index} missing from call to {:?}",
                                binding.callee
                            )
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                let delay = call
                    .arguments
                    .get(binding.delay_argument)
                    .cloned()
                    .with_context(|| {
                        format!(
                            "schedule binding delay argument {} missing from call to {:?}",
                            binding.delay_argument, binding.callee
                        )
                    })?;
                let priority = binding
                    .priority_argument
                    .map(|index| {
                        call.arguments.get(index).cloned().with_context(|| {
                            format!(
                                "schedule binding priority argument {index} missing from call to {:?}",
                                binding.callee
                            )
                        })
                    })
                    .transpose()?;
                Ok(JimpleScheduleOnceCandidate {
                    call_path: call.call_path.clone(),
                    guards: call.guards.clone(),
                    resolution: call.resolution,
                    key,
                    delay,
                    priority,
                })
            })
            .collect()
    }
}

fn transaction_loop_bound(
    value: &SymbolicValue,
    bindings: &[JimpleIndexedStoreQueryBinding],
) -> Result<JimpleTransactionLoopBound> {
    if let Some((store, _context)) =
        semantic_store_query_bound(value, bindings, JimpleIndexedStoreQueryKind::Extent)?
    {
        return Ok(JimpleTransactionLoopBound::StoreExtent { store });
    }
    if let SymbolicValue::ArrayLength { value } = value
        && let Some((store, context)) =
            semantic_store_query_bound(value, bindings, JimpleIndexedStoreQueryKind::Keys)?
    {
        return Ok(JimpleTransactionLoopBound::StoreKeysLength { store, context });
    }
    Ok(JimpleTransactionLoopBound::Symbolic {
        value: value.clone(),
    })
}

fn semantic_store_query_bound(
    value: &SymbolicValue,
    bindings: &[JimpleIndexedStoreQueryBinding],
    expected: JimpleIndexedStoreQueryKind,
) -> Result<Option<(SymbolicValue, Vec<SymbolicValue>)>> {
    let SymbolicValue::Invoke {
        method,
        base,
        arguments,
    } = value
    else {
        return Ok(None);
    };
    let mut matching = bindings
        .iter()
        .filter(|binding| binding.callee == *method && binding.operation == expected);
    let Some(binding) = matching.next() else {
        return Ok(None);
    };
    if matching.next().is_some() {
        return Ok(None);
    }
    let store =
        select_symbolic_operand(base.as_deref(), arguments, &binding.store).with_context(|| {
            format!(
                "transaction loop store operand missing for {:?}",
                binding.callee
            )
        })?;
    let context = binding
        .context
        .iter()
        .map(|selector| {
            select_symbolic_operand(base.as_deref(), arguments, selector).with_context(|| {
                format!(
                    "transaction loop context operand missing for {:?}",
                    binding.callee
                )
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Some((store, context)))
}

fn select_call_operand(
    call: &JimpleProgramCall,
    selector: &JimpleCallOperandBinding,
    role: &str,
    callee: &JimpleMethodSignature,
) -> Result<SymbolicValue> {
    match selector {
        JimpleCallOperandBinding::Receiver => call
            .receiver
            .clone()
            .with_context(|| format!("semantic {role} receiver missing from call to {callee:?}")),
        JimpleCallOperandBinding::Argument { index } => {
            call.arguments.get(*index).cloned().with_context(|| {
                format!("semantic {role} argument {index} missing from call to {callee:?}")
            })
        }
    }
}

fn call_path_crosses_transaction_boundary(
    call_path: &[JimpleEffectCallFrame],
    bindings: &JimpleTransactionBindings,
) -> bool {
    call_path
        .iter()
        .any(|frame| transaction_callee_is_bound(&frame.callee, bindings))
}

fn transaction_callee_is_bound(
    callee: &JimpleMethodSignature,
    bindings: &JimpleTransactionBindings,
) -> bool {
    bindings
        .indexed_stores
        .iter()
        .any(|binding| binding.callee == *callee)
        || bindings
            .snapshot_clones
            .iter()
            .any(|binding| binding.callee == *callee)
        || bindings
            .pure_values
            .iter()
            .any(|binding| binding.callee == *callee)
        || bindings
            .store_queries
            .iter()
            .any(|binding| binding.callee == *callee)
        || bindings
            .state_writes
            .iter()
            .any(|binding| binding.callee == *callee)
        || bindings
            .host_queries
            .iter()
            .any(|binding| binding.callee == *callee)
        || bindings
            .schedule_once
            .iter()
            .any(|binding| binding.callee == *callee)
}

fn unique_pure_value_binding(
    callee: &JimpleMethodSignature,
    bindings: &[JimplePureValueBinding],
) -> Option<JimplePureValueBinding> {
    let mut matching = bindings.iter().filter(|binding| binding.callee == *callee);
    let binding = matching.next()?.clone();
    if matching.next().is_some() {
        return None;
    }
    Some(binding)
}

fn pure_value_shape_matches(
    operation: &JimplePureValueOperation,
    call: &JimpleProgramCall,
) -> bool {
    match operation {
        JimplePureValueOperation::Component { .. } => {
            call.receiver.is_some() && call.arguments.is_empty()
        }
        JimplePureValueOperation::Equal
        | JimplePureValueOperation::Min
        | JimplePureValueOperation::Max => call.receiver.is_none() && call.arguments.len() == 2,
        JimplePureValueOperation::Sequence => call.receiver.is_none(),
        JimplePureValueOperation::Length => call.receiver.is_some() && call.arguments.is_empty(),
        JimplePureValueOperation::Index => call.receiver.is_some() && call.arguments.len() == 1,
    }
}

fn unique_value_component_name(
    field: &super::JimpleFieldSignature,
    bindings: &[JimpleValueComponentBinding],
) -> Option<String> {
    let mut matching = bindings.iter().filter(|binding| binding.field == *field);
    let binding = matching.next()?;
    if matching.next().is_some() {
        return None;
    }
    Some(binding.component.clone())
}

fn indexed_store_value_origin(
    value: &SymbolicValue,
    bindings: &[JimpleIndexedStoreBinding],
) -> Option<JimpleIndexedStoreValueOrigin> {
    let SymbolicValue::Invoke {
        method,
        base,
        arguments,
    } = value
    else {
        return None;
    };
    let mut matching = bindings.iter().filter(|binding| {
        binding.callee == *method
            && matches!(
                binding.operation,
                JimpleIndexedStoreOperationKind::Read | JimpleIndexedStoreOperationKind::Take
            )
    });
    let binding = matching.next()?;
    if matching.next().is_some() {
        return None;
    }
    let store = select_symbolic_operand(base.as_deref(), arguments, &binding.store)?;
    let key = binding
        .key
        .iter()
        .map(|selector| select_symbolic_operand(base.as_deref(), arguments, selector))
        .collect::<Option<Vec<_>>>()?;
    let amount = binding
        .amount
        .as_ref()
        .and_then(|selector| select_symbolic_operand(base.as_deref(), arguments, selector));
    Some(JimpleIndexedStoreValueOrigin {
        operation: binding.operation,
        store,
        key,
        amount,
    })
}

fn select_symbolic_operand(
    receiver: Option<&SymbolicValue>,
    arguments: &[SymbolicValue],
    selector: &JimpleCallOperandBinding,
) -> Option<SymbolicValue> {
    match selector {
        JimpleCallOperandBinding::Receiver => receiver.cloned(),
        JimpleCallOperandBinding::Argument { index } => arguments.get(*index).cloned(),
    }
}

fn same_scope_later(
    take: &JimpleIndexedStoreOperation,
    restore: &JimpleIndexedStoreOperation,
) -> bool {
    let (Some(take_frame), Some(restore_frame)) = (take.call_path.last(), restore.call_path.last())
    else {
        return false;
    };
    take_frame.caller == restore_frame.caller
        && take_frame.statement_id < restore_frame.statement_id
}

fn cloned_read_origin(
    value: &SymbolicValue,
    clone_bindings: &[JimpleSnapshotCloneBinding],
    read_bindings: &[&JimpleIndexedStoreBinding],
    expected_store: &SymbolicValue,
    expected_key: &[SymbolicValue],
) -> Option<SymbolicValue> {
    let SymbolicValue::Invoke {
        method,
        base: Some(snapshot),
        arguments,
    } = value
    else {
        return None;
    };
    if !arguments.is_empty()
        || !clone_bindings
            .iter()
            .any(|binding| binding.callee == *method)
    {
        return None;
    }
    let (store, key) = read_origin(snapshot, read_bindings)?;
    if &store != expected_store || key != expected_key {
        return None;
    }
    Some((**snapshot).clone())
}

fn read_origin(
    value: &SymbolicValue,
    read_bindings: &[&JimpleIndexedStoreBinding],
) -> Option<(SymbolicValue, Vec<SymbolicValue>)> {
    let SymbolicValue::Invoke {
        method,
        base,
        arguments,
    } = value
    else {
        return None;
    };
    let mut matching = read_bindings
        .iter()
        .copied()
        .filter(|binding| binding.callee == *method);
    let binding = matching.next()?;
    if matching.next().is_some() {
        return None;
    }
    let store = select_symbolic_invoke_operand(base.as_deref(), arguments, &binding.store)?;
    let key = binding
        .key
        .iter()
        .map(|selector| select_symbolic_invoke_operand(base.as_deref(), arguments, selector))
        .collect::<Option<Vec<_>>>()?;
    Some((store, key))
}

fn select_symbolic_invoke_operand(
    receiver: Option<&SymbolicValue>,
    arguments: &[SymbolicValue],
    selector: &JimpleCallOperandBinding,
) -> Option<SymbolicValue> {
    match selector {
        JimpleCallOperandBinding::Receiver => receiver.cloned(),
        JimpleCallOperandBinding::Argument { index } => arguments.get(*index).cloned(),
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct JimpleClockedStateCandidate {
    pub update_statement_id: usize,
    pub field: super::JimpleFieldSignature,
    pub receiver: Option<SymbolicValue>,
    pub receiver_is_entry_this: bool,
    pub delta_per_activation: i64,
    pub activation_guards: Vec<JimpleEffectGuard>,
    pub guarded_writes: Vec<JimpleProgramFieldWrite>,
    pub direct_comparisons: Vec<JimpleFieldComparison>,
    pub observed_guards: Vec<JimpleEffectGuard>,
}

#[derive(Clone, Copy)]
struct StructuralEffectContext<'a> {
    this_value: Option<&'a SymbolicValue>,
    arguments: &'a [SymbolicValue],
    call_path: &'a [JimpleEffectCallFrame],
    guards: &'a [JimpleEffectGuard],
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AbstractStaticFieldValue {
    pub field: super::JimpleFieldSignature,
    pub value: AbstractValue,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AbstractMapEntry {
    pub key: AbstractValue,
    pub value: AbstractValue,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AbstractMapSnapshot {
    pub object_id: u64,
    pub entries: Vec<AbstractMapEntry>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AbstractListSnapshot {
    pub object_id: u64,
    pub values: Vec<AbstractValue>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AbstractSetSnapshot {
    pub object_id: u64,
    pub values: Vec<AbstractValue>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AbstractNamedRegistryEntry {
    pub name: AbstractValue,
    pub values: Vec<AbstractValue>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AbstractOpaqueCallShape {
    pub method: JimpleMethodSignature,
    pub base: Option<AbstractValue>,
    pub arguments: Vec<AbstractValue>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AbstractArrayWrite {
    pub base: AbstractValue,
    pub index: AbstractValue,
    pub value: AbstractValue,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ProgramExecution {
    pub steps: usize,
    pub initialized_classes: Vec<String>,
    pub static_fields: Vec<AbstractStaticFieldValue>,
    pub arrays: Vec<AbstractArraySnapshot>,
    pub objects: Vec<AbstractObjectSnapshot>,
    pub maps: Vec<AbstractMapSnapshot>,
    pub lists: Vec<AbstractListSnapshot>,
    pub sets: Vec<AbstractSetSnapshot>,
    pub named_registry: Vec<AbstractNamedRegistryEntry>,
    pub opaque_call_shapes: Vec<AbstractOpaqueCallShape>,
    pub unresolved_array_writes: Vec<AbstractArrayWrite>,
    pub exceptional_paths: Vec<AbstractValue>,
}

impl JimpleProgram {
    #[must_use]
    pub fn new(methods: Vec<JimpleMethod>) -> Self {
        Self { methods }
    }

    pub fn extend(&mut self, methods: impl IntoIterator<Item = JimpleMethod>) {
        self.methods.extend(methods);
    }

    #[must_use]
    pub fn method(&self, signature: &JimpleMethodSignature) -> Option<&JimpleMethod> {
        self.methods
            .iter()
            .find(|method| method.signature == *signature)
    }

    /// Expand possible field writes through structurally exact in-program calls.
    ///
    /// This is a structural effect summary, not path execution: writes retain their call path but
    /// are not claimed to occur unconditionally. Static/special calls are exact. Virtual calls are
    /// expanded only when the receiver is structurally known to be exactly the declared owner
    /// (`this` of that concrete entry class or a directly allocated instance). Other dynamic calls
    /// remain explicit blockers instead of guessing dispatch. Recursive call-graph cycles are cut
    /// explicitly after preserving effects reached before the cycle.
    ///
    /// # Errors
    ///
    /// Returns an error when `entry` is absent from the program.
    pub fn structural_effects(
        &self,
        entry: &JimpleMethodSignature,
    ) -> Result<JimpleProgramEffectSummary> {
        self.structural_effects_with_runtime_this(entry, None)
    }

    /// Expand structural effects with an exact runtime class for the entry receiver.
    ///
    /// The runtime class must be the entry owner itself or a subtype proven by the exported
    /// class hierarchy. Virtual/interface calls are then dispatched by walking that exact class
    /// hierarchy, while the concrete receiver identity is preserved through inherited methods.
    ///
    /// # Errors
    ///
    /// Returns an error when the entry is absent or the supplied runtime class is not known to be
    /// compatible with the entry owner.
    pub fn structural_effects_for_runtime_this(
        &self,
        entry: &JimpleMethodSignature,
        runtime_type: &str,
    ) -> Result<JimpleProgramEffectSummary> {
        if !self.is_known_subtype(runtime_type, &entry.owner) {
            bail!(
                "runtime type {runtime_type:?} is not a known subtype of entry owner {:?}",
                entry.owner
            );
        }
        self.structural_effects_with_runtime_this(entry, Some(runtime_type))
    }

    fn structural_effects_with_runtime_this(
        &self,
        entry: &JimpleMethodSignature,
        runtime_type: Option<&str>,
    ) -> Result<JimpleProgramEffectSummary> {
        let method = self
            .method(entry)
            .with_context(|| format!("Jimple method {entry:?} does not exist"))?;
        let mut summary = JimpleProgramEffectSummary {
            entry: entry.clone(),
            possible_field_writes: Vec::new(),
            calls: Vec::new(),
            induction_loops: Vec::new(),
            unresolved_calls: Vec::new(),
        };
        let runtime_this = runtime_type.map(|type_name| SymbolicValue::This {
            type_name: type_name.to_owned(),
        });
        let mut active = vec![entry.clone()];
        self.expand_structural_effects(
            method,
            StructuralEffectContext {
                this_value: runtime_this.as_ref(),
                arguments: &[],
                call_path: &[],
                guards: &[],
            },
            &mut active,
            &mut summary,
        );
        Ok(summary)
    }

    /// Recover fields that advance by a fixed amount on each entry activation.
    ///
    /// This deliberately stops short of assigning wall-clock meaning to one activation. A caller
    /// that knows the entry cadence can lower a stable monotonic candidate to a deadline/epoch,
    /// while irregular entrypoints can keep the same per-activation representation.
    ///
    /// # Errors
    ///
    /// Returns an error when `entry` is absent from the program.
    pub fn clocked_state_candidates(
        &self,
        entry: &JimpleMethodSignature,
    ) -> Result<Vec<JimpleClockedStateCandidate>> {
        let method = self
            .method(entry)
            .with_context(|| format!("Jimple method {entry:?} does not exist"))?;
        let temporal = method.temporal_summary();
        let effects = self.structural_effects(entry)?;
        let mut candidates = Vec::new();
        for update in temporal.counter_updates {
            let Some(update_effect) = effects.possible_field_writes.iter().find(|write| {
                write.call_path.is_empty()
                    && write.statement_id == update.statement_id
                    && write.field == update.field
                    && write.receiver == update.receiver
            }) else {
                continue;
            };
            let guarded_writes: Vec<_> = effects
                .possible_field_writes
                .iter()
                .filter(|write| {
                    !(write.call_path.is_empty() && write.statement_id == update.statement_id)
                        && write.field == update.field
                        && write.receiver == update.receiver
                })
                .cloned()
                .collect();
            let direct_comparisons: Vec<_> = temporal
                .comparisons
                .iter()
                .filter(|comparison| {
                    comparison.field == update.field && comparison.receiver == update.receiver
                })
                .cloned()
                .collect();
            let mut observed_guards: Vec<_> = effects
                .possible_field_writes
                .iter()
                .flat_map(|write| write.guards.iter())
                .filter(|guard| {
                    symbolic_references_field(
                        &guard.condition,
                        &update.field,
                        update.receiver.as_ref(),
                    )
                })
                .cloned()
                .collect();
            dedupe_effect_guards(&mut observed_guards);
            let receiver_is_entry_this = matches!(
                &update.receiver,
                Some(SymbolicValue::This { type_name }) if type_name == &entry.owner
            );
            candidates.push(JimpleClockedStateCandidate {
                update_statement_id: update.statement_id,
                field: update.field,
                receiver: update.receiver,
                receiver_is_entry_this,
                delta_per_activation: update.delta,
                activation_guards: update_effect.guards.clone(),
                guarded_writes,
                direct_comparisons,
                observed_guards,
            });
        }
        Ok(candidates)
    }

    fn expand_structural_effects(
        &self,
        method: &JimpleMethod,
        context: StructuralEffectContext<'_>,
        active: &mut Vec<JimpleMethodSignature>,
        summary: &mut JimpleProgramEffectSummary,
    ) {
        for induction in method.induction_loops() {
            let mut guards = context.guards.to_vec();
            guards.extend(self.substitute_guards(
                &method.signature,
                &method.control_guards(induction.header_statement_id),
                context.this_value,
                context.arguments,
            ));
            dedupe_guards(&mut guards);
            summary.induction_loops.push(JimpleProgramInductionLoop {
                call_path: context.call_path.to_vec(),
                guards,
                method: method.signature.clone(),
                header_statement_id: induction.header_statement_id,
                condition_statement_id: induction.condition_statement_id,
                induction_local: induction.induction_local,
                initial_value: induction.initial_value,
                step: induction.step,
                comparison_operator: induction.comparison_operator,
                bound: self.simplify_symbolic_value(
                    substitute_symbolic(&induction.bound, context.this_value, context.arguments),
                    0,
                ),
                continue_when_condition: induction.continue_when_condition,
                iterations: induction.iterations,
                statement_ids: induction.statement_ids,
            });
        }
        for effect in method.effect_summary().effects {
            match effect {
                JimpleDirectEffect::FieldWrite(write) => {
                    let mut guards = context.guards.to_vec();
                    guards.extend(self.substitute_guards(
                        &method.signature,
                        &write.guards,
                        context.this_value,
                        context.arguments,
                    ));
                    dedupe_guards(&mut guards);
                    summary.possible_field_writes.push(JimpleProgramFieldWrite {
                        call_path: context.call_path.to_vec(),
                        guards,
                        statement_id: write.statement_id,
                        field: write.field,
                        receiver: write.receiver.as_ref().map(|receiver| {
                            substitute_symbolic(receiver, context.this_value, context.arguments)
                        }),
                        value: self.simplify_symbolic_value(
                            substitute_symbolic(
                                &write.value,
                                context.this_value,
                                context.arguments,
                            ),
                            0,
                        ),
                    });
                }
                JimpleDirectEffect::Invocation(invocation) => {
                    self.expand_structural_invocation(
                        method,
                        &invocation,
                        &context,
                        active,
                        summary,
                    );
                }
            }
        }
    }

    fn record_program_call(
        summary: &mut JimpleProgramEffectSummary,
        call_path: &[JimpleEffectCallFrame],
        guards: &[JimpleEffectGuard],
        receiver: Option<&SymbolicValue>,
        arguments: &[SymbolicValue],
        resolution: JimpleProgramCallResolution,
    ) {
        summary.calls.push(JimpleProgramCall {
            call_path: call_path.to_vec(),
            guards: guards.to_vec(),
            receiver: receiver.cloned(),
            arguments: arguments.to_vec(),
            resolution,
        });
    }

    fn record_unresolved_call(
        summary: &mut JimpleProgramEffectSummary,
        call_path: Vec<JimpleEffectCallFrame>,
        guards: Vec<JimpleEffectGuard>,
        receiver: Option<SymbolicValue>,
        arguments: Vec<SymbolicValue>,
        reason: JimpleUnresolvedEffectReason,
    ) {
        let resolution = match reason {
            JimpleUnresolvedEffectReason::MissingMethod => {
                JimpleProgramCallResolution::MissingMethod
            }
            JimpleUnresolvedEffectReason::DynamicDispatch => {
                JimpleProgramCallResolution::DynamicDispatch
            }
            JimpleUnresolvedEffectReason::RecursiveCycle => {
                JimpleProgramCallResolution::RecursiveCycle
            }
        };
        Self::record_program_call(
            summary,
            &call_path,
            &guards,
            receiver.as_ref(),
            &arguments,
            resolution,
        );
        summary.unresolved_calls.push(JimpleUnresolvedEffectCall {
            call_path,
            guards,
            receiver,
            arguments,
            reason,
        });
    }

    fn expand_structural_invocation(
        &self,
        current_method: &JimpleMethod,
        invocation: &JimpleInvocation,
        context: &StructuralEffectContext<'_>,
        active: &mut Vec<JimpleMethodSignature>,
        summary: &mut JimpleProgramEffectSummary,
    ) {
        let receiver = invocation.receiver.as_ref().map(|receiver| {
            self.simplify_symbolic_value(
                substitute_symbolic(receiver, context.this_value, context.arguments),
                0,
            )
        });
        let call_arguments: Vec<_> = invocation
            .arguments
            .iter()
            .map(|argument| {
                self.simplify_symbolic_value(
                    substitute_symbolic(argument, context.this_value, context.arguments),
                    0,
                )
            })
            .collect();
        let mut invocation_guards = context.guards.to_vec();
        invocation_guards.extend(self.substitute_guards(
            &current_method.signature,
            &invocation.guards,
            context.this_value,
            context.arguments,
        ));
        dedupe_guards(&mut invocation_guards);
        let frame = JimpleEffectCallFrame {
            caller: current_method.signature.clone(),
            statement_id: invocation.statement_id,
            invoke_kind: invocation.invoke_kind.clone(),
            callee: invocation.method.clone(),
        };
        let mut next_path = context.call_path.to_vec();
        next_path.push(frame);

        let target_method = match self.structural_dispatch_target(
            &invocation.invoke_kind,
            receiver.as_ref(),
            &invocation.method,
        ) {
            Ok(target) => target,
            Err(reason) => {
                Self::record_unresolved_call(
                    summary,
                    next_path,
                    invocation_guards,
                    receiver,
                    call_arguments,
                    reason,
                );
                return;
            }
        };

        if active.contains(&target_method.signature) {
            Self::record_unresolved_call(
                summary,
                next_path,
                invocation_guards,
                receiver,
                call_arguments,
                JimpleUnresolvedEffectReason::RecursiveCycle,
            );
            return;
        }

        Self::record_program_call(
            summary,
            &next_path,
            &invocation_guards,
            receiver.as_ref(),
            &call_arguments,
            JimpleProgramCallResolution::Expanded,
        );
        active.push(target_method.signature.clone());
        self.expand_structural_effects(
            target_method,
            StructuralEffectContext {
                this_value: receiver.as_ref(),
                arguments: &call_arguments,
                call_path: &next_path,
                guards: &invocation_guards,
            },
            active,
            summary,
        );
        active.pop();
    }

    fn structural_dispatch_target(
        &self,
        invoke_kind: &str,
        receiver: Option<&SymbolicValue>,
        declared: &JimpleMethodSignature,
    ) -> std::result::Result<&JimpleMethod, JimpleUnresolvedEffectReason> {
        match invoke_kind {
            "JStaticInvokeExpr" => self
                .method(declared)
                .or_else(|| self.dispatched_method_in_hierarchy(&declared.owner, declared))
                .ok_or(JimpleUnresolvedEffectReason::MissingMethod),
            "JSpecialInvokeExpr" => self
                .method(declared)
                .ok_or(JimpleUnresolvedEffectReason::MissingMethod),
            "JVirtualInvokeExpr" | "JInterfaceInvokeExpr" => {
                let runtime_owner = exact_symbolic_runtime_type(receiver)
                    .ok_or(JimpleUnresolvedEffectReason::DynamicDispatch)?;
                if !self.is_known_subtype(runtime_owner, &declared.owner) {
                    return Err(JimpleUnresolvedEffectReason::DynamicDispatch);
                }
                self.dispatched_method_in_hierarchy(runtime_owner, declared)
                    .ok_or(JimpleUnresolvedEffectReason::MissingMethod)
            }
            _ => Err(JimpleUnresolvedEffectReason::DynamicDispatch),
        }
    }

    pub(super) fn dispatched_method_in_hierarchy(
        &self,
        runtime_owner: &str,
        declared: &JimpleMethodSignature,
    ) -> Option<&JimpleMethod> {
        let mut owner = Some(runtime_owner);
        let mut visited = BTreeSet::new();
        while let Some(current) = owner {
            if !visited.insert(current.to_owned()) {
                return None;
            }
            if let Some(method) = self.dispatch_method_on_owner(current, declared) {
                return Some(method);
            }
            owner = self.superclass_of(current);
        }
        if self.is_known_subtype(runtime_owner, &declared.owner) {
            self.method(declared)
        } else {
            None
        }
    }

    fn dispatch_method_on_owner(
        &self,
        owner: &str,
        declared: &JimpleMethodSignature,
    ) -> Option<&JimpleMethod> {
        let mut matching = self.methods.iter().filter(|method| {
            method.signature.owner == owner
                && method.signature.name == declared.name
                && method.signature.parameter_types == declared.parameter_types
        });
        let first = matching.next()?;
        if first.signature.return_type == declared.return_type {
            return Some(first);
        }
        matching
            .find(|method| method.signature.return_type == declared.return_type)
            .or(Some(first))
    }

    fn superclass_of(&self, owner: &str) -> Option<&str> {
        self.methods
            .iter()
            .find(|method| method.signature.owner == owner)
            .and_then(|method| method.declaring_superclass.as_deref())
    }

    fn direct_interfaces_of(&self, owner: &str) -> &[String] {
        self.methods
            .iter()
            .find(|method| method.signature.owner == owner)
            .map_or(&[], |method| method.declaring_interfaces.as_slice())
    }

    pub(super) fn is_known_subtype(&self, candidate: &str, expected: &str) -> bool {
        if candidate == expected {
            return true;
        }
        let mut pending = vec![candidate.to_owned()];
        let mut visited = BTreeSet::new();
        while let Some(current) = pending.pop() {
            if !visited.insert(current.clone()) {
                continue;
            }
            if let Some(superclass) = self.superclass_of(&current) {
                if superclass == expected {
                    return true;
                }
                pending.push(superclass.to_owned());
            }
            for interface in self.direct_interfaces_of(&current) {
                if interface == expected {
                    return true;
                }
                pending.push(interface.clone());
            }
        }
        false
    }

    fn simplify_symbolic_value(&self, value: SymbolicValue, depth: usize) -> SymbolicValue {
        if depth >= 16 {
            return value;
        }
        match value {
            SymbolicValue::Invoke {
                method,
                base,
                arguments,
            } => self.simplify_symbolic_invoke(method, base, arguments, depth),
            SymbolicValue::InstanceField { base, field } => SymbolicValue::InstanceField {
                base: Box::new(self.simplify_symbolic_value(*base, depth + 1)),
                field,
            },
            SymbolicValue::ArrayElement { base, index } => SymbolicValue::ArrayElement {
                base: Box::new(self.simplify_symbolic_value(*base, depth + 1)),
                index: Box::new(self.simplify_symbolic_value(*index, depth + 1)),
            },
            SymbolicValue::Binary {
                operator,
                left,
                right,
            } => SymbolicValue::Binary {
                operator,
                left: Box::new(self.simplify_symbolic_value(*left, depth + 1)),
                right: Box::new(self.simplify_symbolic_value(*right, depth + 1)),
            },
            SymbolicValue::Cast { type_name, value } => SymbolicValue::Cast {
                type_name,
                value: Box::new(self.simplify_symbolic_value(*value, depth + 1)),
            },
            SymbolicValue::Unary { operator, value } => SymbolicValue::Unary {
                operator,
                value: Box::new(self.simplify_symbolic_value(*value, depth + 1)),
            },
            SymbolicValue::ArrayLength { value } => SymbolicValue::ArrayLength {
                value: Box::new(self.simplify_symbolic_value(*value, depth + 1)),
            },
            SymbolicValue::NewArray { element_type, size } => SymbolicValue::NewArray {
                element_type,
                size: Box::new(self.simplify_symbolic_value(*size, depth + 1)),
            },
            SymbolicValue::NewMultiArray { array_type, sizes } => SymbolicValue::NewMultiArray {
                array_type,
                sizes: sizes
                    .into_iter()
                    .map(|size| self.simplify_symbolic_value(size, depth + 1))
                    .collect(),
            },
            SymbolicValue::Phi { values } => SymbolicValue::Phi {
                values: values
                    .into_iter()
                    .map(|value| self.simplify_symbolic_value(value, depth + 1))
                    .collect(),
            },
            SymbolicValue::Piecewise { arms } => SymbolicValue::Piecewise {
                arms: arms
                    .into_iter()
                    .map(|arm| SymbolicPiecewiseArm {
                        guards: arm
                            .guards
                            .into_iter()
                            .map(|guard| JimpleGuard {
                                branch_statement_id: guard.branch_statement_id,
                                expected: guard.expected,
                                condition: self.simplify_symbolic_value(guard.condition, depth + 1),
                            })
                            .collect(),
                        value: self.simplify_symbolic_value(arm.value, depth + 1),
                    })
                    .collect(),
            },
            other => other,
        }
    }

    fn simplify_symbolic_invoke(
        &self,
        method: JimpleMethodSignature,
        base: Option<Box<SymbolicValue>>,
        arguments: Vec<SymbolicValue>,
        depth: usize,
    ) -> SymbolicValue {
        let base = base.map(|base| Box::new(self.simplify_symbolic_value(*base, depth + 1)));
        let arguments: Vec<_> = arguments
            .into_iter()
            .map(|argument| self.simplify_symbolic_value(argument, depth + 1))
            .collect();
        let invoke_kind = if base.is_some() {
            "JVirtualInvokeExpr"
        } else {
            "JStaticInvokeExpr"
        };
        if let Ok(target) = self.structural_dispatch_target(invoke_kind, base.as_deref(), &method) {
            let mut active = Vec::new();
            if let Some(returned) = self.pure_symbolic_return(
                target,
                base.as_deref(),
                &arguments,
                depth + 1,
                &mut active,
            ) {
                return self.simplify_symbolic_value(returned, depth + 1);
            }
        }
        SymbolicValue::Invoke {
            method,
            base,
            arguments,
        }
    }

    fn pure_symbolic_return(
        &self,
        method: &JimpleMethod,
        this_value: Option<&SymbolicValue>,
        arguments: &[SymbolicValue],
        depth: usize,
        active: &mut Vec<JimpleMethodSignature>,
    ) -> Option<SymbolicValue> {
        if depth >= 16
            || method.has_nonlocal_write_or_opaque_control()
            || active.contains(&method.signature)
        {
            return None;
        }
        active.push(method.signature.clone());
        let result = (|| {
            let returned = method.symbolic_return_value()?;
            for effect in method.effect_summary().effects {
                match effect {
                    JimpleDirectEffect::FieldWrite(_) => return None,
                    JimpleDirectEffect::Invocation(invocation) => {
                        let receiver = invocation
                            .receiver
                            .as_ref()
                            .map(|receiver| substitute_symbolic(receiver, this_value, arguments));
                        let call_arguments: Vec<_> = invocation
                            .arguments
                            .iter()
                            .map(|argument| substitute_symbolic(argument, this_value, arguments))
                            .collect();
                        let target = self
                            .structural_dispatch_target(
                                &invocation.invoke_kind,
                                receiver.as_ref(),
                                &invocation.method,
                            )
                            .ok()?;
                        self.pure_symbolic_return(
                            target,
                            receiver.as_ref(),
                            &call_arguments,
                            depth + 1,
                            active,
                        )?;
                    }
                }
            }
            Some(substitute_symbolic(&returned, this_value, arguments))
        })();
        active.pop();
        result
    }

    fn substitute_guards(
        &self,
        method: &JimpleMethodSignature,
        guards: &[JimpleGuard],
        this_value: Option<&SymbolicValue>,
        arguments: &[SymbolicValue],
    ) -> Vec<JimpleEffectGuard> {
        guards
            .iter()
            .map(|guard| {
                let condition = substitute_symbolic(&guard.condition, this_value, arguments);
                let condition = self.simplify_guard_value(condition, 0);
                let (expected, condition) = normalize_guard_polarity(guard.expected, condition);
                JimpleEffectGuard {
                    method: method.clone(),
                    branch_statement_id: guard.branch_statement_id,
                    expected,
                    condition,
                }
            })
            .collect()
    }

    fn simplify_guard_value(&self, value: SymbolicValue, depth: usize) -> SymbolicValue {
        if depth >= 16 {
            return value;
        }
        match value {
            SymbolicValue::Invoke {
                method,
                base,
                arguments,
            } => self.simplify_guard_invoke(method, base, arguments, depth),
            SymbolicValue::Binary {
                operator,
                left,
                right,
            } => {
                let left = self.simplify_guard_value(*left, depth + 1);
                let right = self.simplify_guard_value(*right, depth + 1);
                if let Some(expanded) = expand_piecewise_binary(&operator, &left, &right) {
                    return self.simplify_guard_value(expanded, depth + 1);
                }
                if matches!(operator.as_str(), "&&" | "||") {
                    return simplify_boolean_connective(operator, left, right);
                }
                simplify_boolean_comparison(operator, left, right)
            }
            SymbolicValue::Unary { operator, value } => {
                self.simplify_guard_unary(operator, *value, depth)
            }
            SymbolicValue::InstanceField { base, field } => SymbolicValue::InstanceField {
                base: Box::new(self.simplify_guard_value(*base, depth + 1)),
                field,
            },
            SymbolicValue::ArrayElement { base, index } => SymbolicValue::ArrayElement {
                base: Box::new(self.simplify_guard_value(*base, depth + 1)),
                index: Box::new(self.simplify_guard_value(*index, depth + 1)),
            },
            SymbolicValue::Cast { type_name, value } => SymbolicValue::Cast {
                type_name,
                value: Box::new(self.simplify_guard_value(*value, depth + 1)),
            },
            SymbolicValue::ArrayLength { value } => SymbolicValue::ArrayLength {
                value: Box::new(self.simplify_guard_value(*value, depth + 1)),
            },
            SymbolicValue::NewArray { element_type, size } => SymbolicValue::NewArray {
                element_type,
                size: Box::new(self.simplify_guard_value(*size, depth + 1)),
            },
            SymbolicValue::NewMultiArray { array_type, sizes } => SymbolicValue::NewMultiArray {
                array_type,
                sizes: sizes
                    .into_iter()
                    .map(|size| self.simplify_guard_value(size, depth + 1))
                    .collect(),
            },
            SymbolicValue::Phi { values } => SymbolicValue::Phi {
                values: values
                    .into_iter()
                    .map(|item| self.simplify_guard_value(item, depth + 1))
                    .collect(),
            },
            SymbolicValue::Piecewise { arms } => SymbolicValue::Piecewise {
                arms: arms
                    .into_iter()
                    .map(|arm| SymbolicPiecewiseArm {
                        guards: arm
                            .guards
                            .into_iter()
                            .map(|guard| JimpleGuard {
                                branch_statement_id: guard.branch_statement_id,
                                expected: guard.expected,
                                condition: self.simplify_guard_value(guard.condition, depth + 1),
                            })
                            .collect(),
                        value: self.simplify_guard_value(arm.value, depth + 1),
                    })
                    .collect(),
            },
            other => other,
        }
    }

    fn simplify_guard_invoke(
        &self,
        method: JimpleMethodSignature,
        base: Option<Box<SymbolicValue>>,
        arguments: Vec<SymbolicValue>,
        depth: usize,
    ) -> SymbolicValue {
        let base = base.map(|base| Box::new(self.simplify_guard_value(*base, depth + 1)));
        let arguments: Vec<_> = arguments
            .into_iter()
            .map(|argument| self.simplify_guard_value(argument, depth + 1))
            .collect();
        let invoke_kind = if base.is_some() {
            "JVirtualInvokeExpr"
        } else {
            "JStaticInvokeExpr"
        };
        if let Ok(target) = self.structural_dispatch_target(invoke_kind, base.as_deref(), &method)
            && (base.is_some() || !method_has_this(target))
        {
            let mut active = Vec::new();
            if let Some(pure_return) = self.pure_symbolic_return(
                target,
                base.as_deref(),
                &arguments,
                depth + 1,
                &mut active,
            ) {
                let returned = target
                    .boolean_materialized_return()
                    .map_or(pure_return, |boolean| {
                        substitute_symbolic(&boolean, base.as_deref(), &arguments)
                    });
                return self.simplify_guard_value(returned, depth + 1);
            }
        }
        SymbolicValue::Invoke {
            method,
            base,
            arguments,
        }
    }

    fn simplify_guard_unary(
        &self,
        operator: String,
        value: SymbolicValue,
        depth: usize,
    ) -> SymbolicValue {
        let value = self.simplify_guard_value(value, depth + 1);
        if operator == "!" {
            if let SymbolicValue::Unary {
                operator: inner_operator,
                value: inner,
            } = &value
                && inner_operator == "!"
            {
                return (**inner).clone();
            }
            if let SymbolicValue::Integer {
                value: integer,
                type_name,
            } = &value
                && matches!(integer, 0 | 1)
            {
                return SymbolicValue::Integer {
                    value: i64::from(*integer == 0),
                    type_name: type_name.clone(),
                };
            }
        }
        SymbolicValue::Unary {
            operator,
            value: Box::new(value),
        }
    }

    pub(super) fn class_initializer(&self, owner: &str) -> Option<&JimpleMethod> {
        self.methods.iter().find(|method| {
            method.signature.owner == owner
                && method.signature.name == "<clinit>"
                && method.signature.parameter_types.is_empty()
        })
    }

    fn dispatched_method(
        &self,
        runtime_owner: &str,
        declared: &JimpleMethodSignature,
    ) -> Option<&JimpleMethod> {
        self.methods.iter().find(|method| {
            method.signature.owner == runtime_owner
                && method.signature.name == declared.name
                && method.signature.parameter_types == declared.parameter_types
        })
    }

    /// Execute a sequence of unique no-argument methods in one shared abstract JVM state.
    ///
    /// This is useful for explicit initialization flows such as class initializers followed
    /// by registration methods. Ordinary calls made by those methods are resolved
    /// interprocedurally when their exact bytecode signature exists in the program.
    ///
    /// # Errors
    ///
    /// Returns an error when a requested entrypoint is ambiguous or absent, or when finite
    /// execution encounters an unresolved branch, malformed CFG, opaque reached statement,
    /// excessive recursion, or the global step budget is exhausted.
    pub fn execute_named_sequence(
        &self,
        sequence: &[(&str, &str)],
        max_steps: usize,
    ) -> Result<ProgramExecution> {
        let mut vm = ProgramVm::new(self, max_steps);
        self.execute_named_sequence_on_vm(&mut vm, sequence)?;
        Ok(vm.finish())
    }

    /// Execute initialization/registration entrypoints, then one exact method in the same JVM state.
    ///
    /// This is the bridge between startup-state recovery and helper evaluation: registry mutations
    /// performed by the named sequence remain visible to the final call.
    ///
    /// # Errors
    ///
    /// Returns an error when any entrypoint or the exact call cannot be executed finitely.
    pub fn execute_named_sequence_then_call(
        &self,
        sequence: &[(&str, &str)],
        signature: &JimpleMethodSignature,
        this_value: Option<AbstractValue>,
        arguments: Vec<AbstractValue>,
        max_steps: usize,
    ) -> Result<(Option<AbstractValue>, ProgramExecution)> {
        let method = self
            .method(signature)
            .with_context(|| format!("Jimple method {signature:?} not found"))?;
        let mut vm = ProgramVm::new(self, max_steps);
        self.execute_named_sequence_on_vm(&mut vm, sequence)?;
        vm.ensure_class_initialized(&signature.owner, 0)?;
        let return_value = vm.execute_method(method, this_value, arguments, 0)?;
        Ok((return_value, vm.finish()))
    }

    fn execute_named_sequence_on_vm(
        &self,
        vm: &mut ProgramVm<'_>,
        sequence: &[(&str, &str)],
    ) -> Result<()> {
        for (owner, name) in sequence {
            if *name == "<clinit>" {
                vm.ensure_class_initialized(owner, 0)?;
            } else {
                let method = self.unique_method(owner, name)?;
                vm.ensure_class_initialized(owner, 0)?;
                vm.execute_method(method, None, Vec::new(), 0)?;
            }
        }
        Ok(())
    }

    /// Execute one exact method with caller-supplied abstract JVM values.
    ///
    /// # Errors
    ///
    /// Returns an error when the signature is absent or finite abstract execution fails.
    pub fn execute_call(
        &self,
        signature: &JimpleMethodSignature,
        this_value: Option<AbstractValue>,
        arguments: Vec<AbstractValue>,
        max_steps: usize,
    ) -> Result<(Option<AbstractValue>, ProgramExecution)> {
        let method = self
            .method(signature)
            .with_context(|| format!("Jimple method {signature:?} not found"))?;
        let mut vm = ProgramVm::new(self, max_steps);
        if signature.name == "<clinit>" {
            vm.ensure_class_initialized(&signature.owner, 0)?;
            return Ok((None, vm.finish()));
        }
        vm.ensure_class_initialized(&signature.owner, 0)?;
        let return_value = vm.execute_method(method, this_value, arguments, 0)?;
        Ok((return_value, vm.finish()))
    }

    fn unique_method(&self, owner: &str, name: &str) -> Result<&JimpleMethod> {
        let mut matching = self
            .methods
            .iter()
            .filter(|method| method.signature.owner == owner && method.signature.name == name);
        let method = matching
            .next()
            .with_context(|| format!("Jimple method {owner}.{name} not found"))?;
        if matching.next().is_some() {
            bail!("Jimple method {owner}.{name} is overloaded; exact signature required");
        }
        Ok(method)
    }
}

impl ProgramExecution {
    #[must_use]
    pub fn static_map(&self, owner: &str, name: &str) -> Option<&AbstractMapSnapshot> {
        let value = self.static_fields.iter().find_map(|entry| {
            (entry.field.owner == owner && entry.field.name == name).then_some(&entry.value)
        })?;
        let AbstractValue::ObjectRef { id, .. } = value else {
            return None;
        };
        self.maps.iter().find(|map| map.object_id == *id)
    }

    #[must_use]
    pub fn object(&self, id: u64) -> Option<&AbstractObjectSnapshot> {
        self.objects.iter().find(|object| object.id == id)
    }

    #[must_use]
    pub fn static_object_fields(&self, object_id: u64) -> Vec<&super::JimpleFieldSignature> {
        self.static_fields
            .iter()
            .filter_map(|entry| match &entry.value {
                AbstractValue::ObjectRef { id, .. } if *id == object_id => Some(&entry.field),
                _ => None,
            })
            .collect()
    }

    #[must_use]
    pub fn static_field_value(&self, owner: &str, name: &str) -> Option<&AbstractValue> {
        self.static_fields
            .iter()
            .find(|entry| entry.field.owner == owner && entry.field.name == name)
            .map(|entry| &entry.value)
    }

    #[must_use]
    pub fn static_map_lookup(
        &self,
        map_owner: &str,
        map_name: &str,
        key_owner: &str,
        key_name: &str,
    ) -> Option<&AbstractValue> {
        let key = self.static_field_value(key_owner, key_name)?;
        self.static_map(map_owner, map_name)?
            .entries
            .iter()
            .find(|entry| entry.key == *key)
            .map(|entry| &entry.value)
    }
}

#[derive(Clone, Debug, Default)]
struct Frame {
    locals: BTreeMap<String, AbstractValue>,
    this_value: Option<AbstractValue>,
    arguments: Vec<AbstractValue>,
}

#[derive(Clone)]
struct ProgramVm<'a> {
    program: &'a JimpleProgram,
    state: ExecutionState,
    maps: BTreeMap<u64, Vec<(AbstractValue, AbstractValue)>>,
    lists: BTreeMap<u64, Vec<AbstractValue>>,
    sets: BTreeMap<u64, Vec<AbstractValue>>,
    iterators: BTreeMap<u64, AbstractIterator>,
    named_registry: Vec<(AbstractValue, Vec<AbstractValue>)>,
    opaque_call_shapes: Vec<AbstractOpaqueCallShape>,
    unresolved_array_writes: Vec<AbstractArrayWrite>,
    exceptional_paths: Vec<AbstractValue>,
    initialized_classes: BTreeSet<String>,
    initializing_classes: BTreeSet<String>,
    steps: usize,
    max_steps: usize,
    path_splits: usize,
    max_path_splits: usize,
    branch_depth: usize,
    max_branch_depth: usize,
}

#[derive(Clone, Debug, PartialEq)]
struct AbstractIterator {
    values: Vec<AbstractValue>,
    index: usize,
}

#[derive(Clone, Debug)]
struct PathThrow(AbstractValue);

impl fmt::Display for PathThrow {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "abstract JVM path threw {:?}", self.0)
    }
}

impl Error for PathThrow {}

#[derive(Clone, Debug)]
enum ExecutionHalt {
    Reached(Frame),
    Returned(Option<AbstractValue>),
}

#[derive(Clone, Debug)]
enum PathOutcome {
    Reached(Frame),
    Returned(Option<AbstractValue>),
    Thrown(AbstractValue),
}

#[derive(Clone, Debug)]
enum BranchResolution {
    Continue { frame: Frame, statement_id: usize },
    Returned(Option<AbstractValue>),
}

struct BranchExploration<'a> {
    merge: Option<usize>,
    original_unresolved: Vec<AbstractArrayWrite>,
    false_vm: ProgramVm<'a>,
    false_result: PathOutcome,
    true_vm: ProgramVm<'a>,
    true_result: PathOutcome,
}

fn capture_path_outcome(result: Result<ExecutionHalt>) -> Result<PathOutcome> {
    match result {
        Ok(ExecutionHalt::Reached(frame)) => Ok(PathOutcome::Reached(frame)),
        Ok(ExecutionHalt::Returned(value)) => Ok(PathOutcome::Returned(value)),
        Err(error) => match error.downcast_ref::<PathThrow>() {
            Some(thrown) => Ok(PathOutcome::Thrown(thrown.0.clone())),
            None => Err(error),
        },
    }
}

impl<'a> ProgramVm<'a> {
    fn new(program: &'a JimpleProgram, max_steps: usize) -> Self {
        Self {
            program,
            state: ExecutionState::default(),
            maps: BTreeMap::new(),
            lists: BTreeMap::new(),
            sets: BTreeMap::new(),
            iterators: BTreeMap::new(),
            named_registry: Vec::new(),
            opaque_call_shapes: Vec::new(),
            unresolved_array_writes: Vec::new(),
            exceptional_paths: Vec::new(),
            initialized_classes: BTreeSet::new(),
            initializing_classes: BTreeSet::new(),
            steps: 0,
            max_steps,
            path_splits: 0,
            max_path_splits: max_steps.min(4_096),
            branch_depth: 0,
            max_branch_depth: 16,
        }
    }

    fn ensure_class_initialized(&mut self, owner: &str, depth: usize) -> Result<()> {
        if self.initialized_classes.contains(owner) || self.initializing_classes.contains(owner) {
            return Ok(());
        }
        let Some(initializer) = self.program.class_initializer(owner) else {
            self.initialized_classes.insert(owner.to_owned());
            return Ok(());
        };

        self.initializing_classes.insert(owner.to_owned());
        let result = self.execute_method(initializer, None, Vec::new(), depth + 1);
        self.initializing_classes.remove(owner);
        result?;
        self.initialized_classes.insert(owner.to_owned());
        Ok(())
    }

    fn execute_method(
        &mut self,
        method: &JimpleMethod,
        this_value: Option<AbstractValue>,
        arguments: Vec<AbstractValue>,
        depth: usize,
    ) -> Result<Option<AbstractValue>> {
        if depth > 64 {
            bail!("Jimple interprocedural execution exceeded 64 call frames");
        }
        let first = method
            .statements
            .first()
            .with_context(|| format!("cannot execute empty method {}", method.display_signature))?;
        match self.execute_until(
            method,
            Frame {
                locals: BTreeMap::new(),
                this_value,
                arguments,
            },
            Some(first.id),
            depth,
            None,
        )? {
            ExecutionHalt::Returned(value) => Ok(value),
            ExecutionHalt::Reached(_) => {
                bail!("method execution unexpectedly stopped at an internal CFG boundary")
            }
        }
    }

    fn execute_until(
        &mut self,
        method: &JimpleMethod,
        mut frame: Frame,
        mut pc: Option<usize>,
        depth: usize,
        stop_before: Option<usize>,
    ) -> Result<ExecutionHalt> {
        while let Some(statement_id) = pc {
            if stop_before == Some(statement_id) {
                return Ok(ExecutionHalt::Reached(frame));
            }
            if self.steps >= self.max_steps {
                bail!(
                    "interprocedural Jimple execution exceeded {} steps",
                    self.max_steps
                );
            }
            let statement = method
                .statements
                .iter()
                .find(|statement| statement.id == statement_id)
                .with_context(|| {
                    format!(
                        "Jimple statement {statement_id} missing from {}",
                        method.display_signature
                    )
                })?;
            self.steps += 1;
            match &statement.operation {
                JimpleOperation::Assign { left, right }
                | JimpleOperation::Identity { left, right } => {
                    let value = self.evaluate(&mut frame, right, depth)?;
                    self.assign(&mut frame, left, value, depth)?;
                    pc = Self::fallthrough(statement)?;
                }
                JimpleOperation::Invoke { expression } => {
                    if let Some(expression) = expression {
                        self.evaluate(&mut frame, expression, depth)?;
                    }
                    pc = Self::fallthrough(statement)?;
                }
                JimpleOperation::If { condition } => {
                    let condition = self.evaluate(&mut frame, condition, depth)?;
                    let [when_false, when_true] = statement.successors.as_slice() else {
                        bail!("if statement {} does not have two targets", statement.id);
                    };
                    if let Some(truth) = abstract_truth(&condition) {
                        pc = Some(if truth { *when_true } else { *when_false });
                    } else {
                        match self
                            .execute_unknown_branch(method, &frame, statement, &condition, depth)?
                        {
                            BranchResolution::Continue {
                                frame: joined_frame,
                                statement_id,
                            } => {
                                frame = joined_frame;
                                pc = Some(statement_id);
                            }
                            BranchResolution::Returned(value) => {
                                return Ok(ExecutionHalt::Returned(value));
                            }
                        }
                    }
                }
                JimpleOperation::Goto => pc = Self::jump(statement)?,
                JimpleOperation::Return { value } => {
                    return self
                        .evaluate(&mut frame, value, depth)
                        .map(|value| ExecutionHalt::Returned(Some(value)));
                }
                JimpleOperation::ReturnVoid => return Ok(ExecutionHalt::Returned(None)),
                JimpleOperation::Throw { value } => {
                    let value = self.evaluate(&mut frame, value, depth)?;
                    return Err(PathThrow(value).into());
                }
                JimpleOperation::Switch {
                    key,
                    cases,
                    default_target,
                } => {
                    let key = self.evaluate(&mut frame, key, depth)?;
                    let AbstractValue::Integer { value } = key else {
                        bail!(
                            "interprocedural Jimple switch key is not concrete in {} statement {}: {key:?}",
                            method.display_signature,
                            statement.id
                        );
                    };
                    pc = Some(
                        cases
                            .iter()
                            .find(|case| case.value == value)
                            .map_or(*default_target, |case| case.target),
                    );
                }
                JimpleOperation::Opaque => {
                    bail!(
                        "cannot execute opaque Jimple statement {} in {}",
                        statement.id,
                        method.display_signature
                    );
                }
            }
        }
        Ok(ExecutionHalt::Returned(None))
    }

    fn execute_unknown_branch(
        &mut self,
        method: &JimpleMethod,
        frame: &Frame,
        statement: &JimpleStatement,
        condition: &AbstractValue,
        depth: usize,
    ) -> Result<BranchResolution> {
        let [when_false, when_true] = statement.successors.as_slice() else {
            bail!("if statement {} does not have two targets", statement.id);
        };
        let statement_id = statement.id;
        if self.path_splits >= self.max_path_splits {
            bail!(
                "interprocedural Jimple execution exceeded {} unknown branch splits at {} statement {}: {condition:?}",
                self.max_path_splits,
                method.display_signature,
                statement_id
            );
        }
        if self.branch_depth >= self.max_branch_depth {
            bail!(
                "interprocedural Jimple execution exceeded {} nested unknown branches at {} statement {}: {condition:?}",
                self.max_branch_depth,
                method.display_signature,
                statement_id
            );
        }

        let merge = immediate_postdominator(method, statement_id);
        let base_steps = self.steps;
        let base_splits = self.path_splits;
        let original_unresolved = self.unresolved_array_writes.clone();
        let (false_vm, false_result) =
            self.explore_branch(method, frame, *when_false, depth, self.state.next_id, merge)?;
        // Keep allocations made after the fork distinct across paths. Objects and arrays
        // that existed before the fork retain their shared IDs. Shared post-dominator work
        // is deliberately not executed here; it runs once after the branch states are joined.
        let (true_vm, true_result) = self.explore_branch(
            method,
            frame,
            *when_true,
            depth,
            false_vm.state.next_id,
            merge,
        )?;
        self.charge_branch_work(
            base_steps,
            base_splits,
            &false_vm,
            &true_vm,
            method,
            statement_id,
        )?;

        self.resolve_branch_outcomes(
            method,
            statement_id,
            BranchExploration {
                merge,
                original_unresolved,
                false_vm,
                false_result,
                true_vm,
                true_result,
            },
        )
    }

    fn resolve_branch_outcomes(
        &mut self,
        method: &JimpleMethod,
        statement_id: usize,
        exploration: BranchExploration<'a>,
    ) -> Result<BranchResolution> {
        let BranchExploration {
            merge,
            original_unresolved,
            false_vm,
            false_result,
            true_vm,
            true_result,
        } = exploration;
        match (merge, false_result, true_result) {
            (Some(merge), PathOutcome::Reached(false_frame), PathOutcome::Reached(true_frame)) => {
                self.adopt_joined_branches(&false_vm, &true_vm, &original_unresolved);
                Ok(BranchResolution::Continue {
                    frame: join_frames(&false_frame, &true_frame),
                    statement_id: merge,
                })
            }
            (Some(merge), PathOutcome::Reached(frame), PathOutcome::Thrown(thrown)) => {
                self.adopt_normal_branch(false_vm, original_unresolved);
                extend_unique(&mut self.exceptional_paths, true_vm.exceptional_paths);
                extend_unique(&mut self.exceptional_paths, vec![thrown]);
                Ok(BranchResolution::Continue {
                    frame,
                    statement_id: merge,
                })
            }
            (Some(merge), PathOutcome::Thrown(thrown), PathOutcome::Reached(frame)) => {
                self.adopt_normal_branch(true_vm, original_unresolved);
                extend_unique(&mut self.exceptional_paths, false_vm.exceptional_paths);
                extend_unique(&mut self.exceptional_paths, vec![thrown]);
                Ok(BranchResolution::Continue {
                    frame,
                    statement_id: merge,
                })
            }
            (_, PathOutcome::Returned(false_return), PathOutcome::Returned(true_return)) => {
                self.adopt_joined_branches(&false_vm, &true_vm, &original_unresolved);
                match (false_return, true_return) {
                    (None, None) => Ok(BranchResolution::Returned(None)),
                    (Some(left), Some(right)) => Ok(BranchResolution::Returned(Some(join_value(
                        &left,
                        &right,
                        &method.signature.return_type,
                    )))),
                    _ => bail!(
                        "unknown branch at {} statement {} has incompatible normal return shapes",
                        method.display_signature,
                        statement_id
                    ),
                }
            }
            (_, PathOutcome::Returned(value), PathOutcome::Thrown(thrown)) => {
                self.adopt_normal_branch(false_vm, original_unresolved);
                extend_unique(&mut self.exceptional_paths, true_vm.exceptional_paths);
                extend_unique(&mut self.exceptional_paths, vec![thrown]);
                Ok(BranchResolution::Returned(value))
            }
            (_, PathOutcome::Thrown(thrown), PathOutcome::Returned(value)) => {
                self.adopt_normal_branch(true_vm, original_unresolved);
                extend_unique(&mut self.exceptional_paths, false_vm.exceptional_paths);
                extend_unique(&mut self.exceptional_paths, vec![thrown]);
                Ok(BranchResolution::Returned(value))
            }
            (_, PathOutcome::Thrown(left), PathOutcome::Thrown(right)) => {
                let thrown = join_value(&left, &right, "java.lang.Throwable");
                Err(PathThrow(thrown).into())
            }
            (Some(_), PathOutcome::Reached(_), PathOutcome::Returned(_))
            | (Some(_), PathOutcome::Returned(_), PathOutcome::Reached(_)) => bail!(
                "post-dominator analysis disagreed with return flow at {} statement {}",
                method.display_signature,
                statement_id
            ),
            (None, PathOutcome::Reached(_), _) | (None, _, PathOutcome::Reached(_)) => {
                bail!("branch execution reached a stop without a post-dominator")
            }
        }
    }

    fn explore_branch(
        &self,
        method: &JimpleMethod,
        frame: &Frame,
        start: usize,
        depth: usize,
        allocation_floor: u64,
        stop_before: Option<usize>,
    ) -> Result<(ProgramVm<'a>, PathOutcome)> {
        let mut branch = self.clone();
        branch.state.next_id = branch.state.next_id.max(allocation_floor);
        branch.path_splits = branch.path_splits.saturating_add(1);
        branch.branch_depth = branch.branch_depth.saturating_add(1);
        let outcome = capture_path_outcome(branch.execute_until(
            method,
            frame.clone(),
            Some(start),
            depth,
            stop_before,
        ))?;
        Ok((branch, outcome))
    }

    fn adopt_joined_branches(
        &mut self,
        false_vm: &ProgramVm<'a>,
        true_vm: &ProgramVm<'a>,
        original_unresolved: &[AbstractArrayWrite],
    ) {
        self.state = join_execution_states(&false_vm.state, &true_vm.state);
        self.maps = join_maps(&false_vm.maps, &true_vm.maps);
        self.lists = join_lists(&false_vm.lists, &true_vm.lists);
        self.sets = join_sets(&false_vm.sets, &true_vm.sets);
        self.iterators = join_iterators(&false_vm.iterators, &true_vm.iterators);
        self.named_registry =
            join_named_registry(&false_vm.named_registry, &true_vm.named_registry);
        self.opaque_call_shapes
            .clone_from(&false_vm.opaque_call_shapes);
        extend_unique(
            &mut self.opaque_call_shapes,
            true_vm.opaque_call_shapes.clone(),
        );
        self.unresolved_array_writes = original_unresolved.to_vec();
        extend_unique(
            &mut self.unresolved_array_writes,
            false_vm.unresolved_array_writes.clone(),
        );
        extend_unique(
            &mut self.unresolved_array_writes,
            true_vm.unresolved_array_writes.clone(),
        );
        self.initialized_classes
            .clone_from(&false_vm.initialized_classes);
        self.initialized_classes
            .extend(true_vm.initialized_classes.iter().cloned());
        self.initializing_classes
            .clone_from(&false_vm.initializing_classes);
        self.initializing_classes
            .extend(true_vm.initializing_classes.iter().cloned());
        self.exceptional_paths
            .clone_from(&false_vm.exceptional_paths);
        extend_unique(
            &mut self.exceptional_paths,
            true_vm.exceptional_paths.clone(),
        );
    }

    fn charge_branch_work(
        &mut self,
        base_steps: usize,
        base_splits: usize,
        false_vm: &ProgramVm<'a>,
        true_vm: &ProgramVm<'a>,
        method: &JimpleMethod,
        statement_id: usize,
    ) -> Result<()> {
        let joined_steps = base_steps
            .saturating_add(false_vm.steps.saturating_sub(base_steps))
            .saturating_add(true_vm.steps.saturating_sub(base_steps));
        if joined_steps > self.max_steps {
            bail!(
                "interprocedural Jimple execution exceeded {} steps while exploring unknown branch at {} statement {}",
                self.max_steps,
                method.display_signature,
                statement_id
            );
        }
        let nested_split_baseline = base_splits.saturating_add(1);
        let joined_splits = nested_split_baseline
            .saturating_add(false_vm.path_splits.saturating_sub(nested_split_baseline))
            .saturating_add(true_vm.path_splits.saturating_sub(nested_split_baseline));
        if joined_splits > self.max_path_splits {
            bail!(
                "interprocedural Jimple execution exceeded {} unknown branch splits while exploring {} statement {}",
                self.max_path_splits,
                method.display_signature,
                statement_id
            );
        }
        self.steps = joined_steps;
        self.path_splits = joined_splits;
        Ok(())
    }

    fn adopt_normal_branch(
        &mut self,
        branch: ProgramVm<'a>,
        original_unresolved: Vec<AbstractArrayWrite>,
    ) {
        self.state = branch.state;
        self.maps = branch.maps;
        self.lists = branch.lists;
        self.sets = branch.sets;
        self.iterators = branch.iterators;
        self.named_registry = branch.named_registry;
        self.opaque_call_shapes = branch.opaque_call_shapes;
        self.initialized_classes = branch.initialized_classes;
        self.initializing_classes = branch.initializing_classes;
        self.unresolved_array_writes = original_unresolved;
        extend_unique(
            &mut self.unresolved_array_writes,
            branch.unresolved_array_writes,
        );
        self.exceptional_paths = branch.exceptional_paths;
    }

    fn fallthrough(statement: &JimpleStatement) -> Result<Option<usize>> {
        match statement.successors.as_slice() {
            [] => Ok(None),
            [next] => Ok(Some(*next)),
            successors => bail!(
                "statement {} has {} fallthrough successors",
                statement.id,
                successors.len()
            ),
        }
    }

    fn jump(statement: &JimpleStatement) -> Result<Option<usize>> {
        let [target] = statement.successors.as_slice() else {
            bail!("goto statement {} does not have one target", statement.id);
        };
        Ok(Some(*target))
    }

    fn assign(
        &mut self,
        frame: &mut Frame,
        target: &JimpleValue,
        value: AbstractValue,
        depth: usize,
    ) -> Result<()> {
        match &target.kind {
            JimpleValueKind::Local { name } => {
                frame.locals.insert(name.clone(), value);
            }
            JimpleValueKind::StaticField { field } => {
                self.ensure_class_initialized(&field.owner, depth)?;
                self.state.static_fields.insert(field.clone(), value);
            }
            JimpleValueKind::InstanceField { base, field } => {
                let base = self.evaluate(frame, base, depth)?;
                if let AbstractValue::ObjectRef { id, .. } = base
                    && let Some(object) = self.state.objects.get_mut(&id)
                {
                    object.fields.insert(field.clone(), value);
                }
            }
            JimpleValueKind::ArrayRef { base, index } => {
                let base = self.evaluate(frame, base, depth)?;
                let index = self.evaluate(frame, index, depth)?;
                let AbstractValue::Integer { value: index } = index else {
                    bail!("array write index is not concrete: {index:?}");
                };
                if let Some(identity) = abstract_array_identity(&base) {
                    self.state
                        .arrays
                        .entry(identity)
                        .or_default()
                        .entries
                        .insert(index, value);
                } else {
                    self.unresolved_array_writes.push(AbstractArrayWrite {
                        base,
                        index: AbstractValue::Integer { value: index },
                        value,
                    });
                }
            }
            _ => bail!("unsupported Jimple assignment target: {}", target.text),
        }
        Ok(())
    }

    fn evaluate(
        &mut self,
        frame: &mut Frame,
        value: &JimpleValue,
        depth: usize,
    ) -> Result<AbstractValue> {
        let result = match &value.kind {
            JimpleValueKind::InstanceField { base, field } => {
                self.evaluate_instance_field(frame, base, field, depth)?
            }
            JimpleValueKind::ArrayRef { base, index } => {
                self.evaluate_array_ref(frame, base, index, depth)?
            }
            JimpleValueKind::Binary {
                operator,
                left,
                right,
            } => self.evaluate_binary(frame, operator, left, right, &value.type_name, depth)?,
            JimpleValueKind::Cast { cast_type, value } => {
                abstract_cast(cast_type, self.evaluate(frame, value, depth)?)
            }
            JimpleValueKind::Unary {
                operator,
                value: operand,
            } => abstract_unary(
                operator,
                self.evaluate(frame, operand, depth)?,
                &value.type_name,
            ),
            JimpleValueKind::ArrayLength { value } => {
                self.evaluate_array_length(frame, value, depth)?
            }
            JimpleValueKind::New { class_type } => {
                self.ensure_class_initialized(class_type, depth)?;
                self.allocate_object(class_type)
            }
            JimpleValueKind::NewArray { element_type, size } => {
                let size = self.evaluate(frame, size, depth)?;
                self.allocate_array(element_type, &size)
            }
            JimpleValueKind::NewMultiArray { .. } | JimpleValueKind::Phi { .. } => {
                AbstractValue::Unknown {
                    type_name: value.type_name.clone(),
                    text: value.text.clone(),
                }
            }
            JimpleValueKind::Invoke { .. } => self.evaluate_invoke_value(frame, value, depth)?,
            JimpleValueKind::StaticField { field } => {
                self.ensure_class_initialized(&field.owner, depth)?;
                self.state
                    .static_fields
                    .get(field)
                    .cloned()
                    .unwrap_or_else(|| AbstractValue::StaticField {
                        field: field.clone(),
                    })
            }
            _ => Self::evaluate_simple(frame, value).with_context(|| {
                format!("unsupported simple Jimple value kind in {}", value.text)
            })?,
        };
        Ok(result)
    }

    fn evaluate_simple(frame: &Frame, value: &JimpleValue) -> Option<AbstractValue> {
        match &value.kind {
            JimpleValueKind::Local { name } => Some(
                frame
                    .locals
                    .get(name)
                    .cloned()
                    .unwrap_or_else(|| AbstractValue::Unknown {
                        type_name: value.type_name.clone(),
                        text: value.text.clone(),
                    }),
            ),
            JimpleValueKind::IntegerConstant { value } => {
                Some(AbstractValue::Integer { value: *value })
            }
            JimpleValueKind::FloatConstant { value } => {
                Some(AbstractValue::Float { value: *value })
            }
            JimpleValueKind::StringConstant { value } => Some(AbstractValue::String {
                value: value.clone(),
            }),
            JimpleValueKind::ClassConstant { class_type } => Some(AbstractValue::Class {
                class_type: class_type.clone(),
            }),
            JimpleValueKind::MethodHandle {
                handle_kind,
                method,
            } => Some(AbstractValue::MethodHandle {
                handle_kind: handle_kind.clone(),
                method: method.clone(),
            }),
            JimpleValueKind::MethodType { .. } | JimpleValueKind::Opaque => {
                Some(AbstractValue::Unknown {
                    type_name: value.type_name.clone(),
                    text: value.text.clone(),
                })
            }
            JimpleValueKind::NullConstant => Some(AbstractValue::Null),
            JimpleValueKind::Parameter { index } => {
                Some(frame.arguments.get(*index).cloned().unwrap_or_else(|| {
                    AbstractValue::Parameter {
                        index: *index,
                        type_name: value.type_name.clone(),
                    }
                }))
            }
            JimpleValueKind::This => {
                Some(
                    frame
                        .this_value
                        .clone()
                        .unwrap_or_else(|| AbstractValue::This {
                            type_name: value.type_name.clone(),
                        }),
                )
            }
            _ => None,
        }
    }

    fn evaluate_instance_field(
        &mut self,
        frame: &mut Frame,
        base: &JimpleValue,
        field: &super::JimpleFieldSignature,
        depth: usize,
    ) -> Result<AbstractValue> {
        let base = self.evaluate(frame, base, depth)?;
        if let AbstractValue::ObjectRef { id, .. } = &base
            && let Some(object) = self.state.objects.get(id)
        {
            if let Some(value) = object.fields.get(field) {
                return Ok(value.clone());
            }
            if !object.fields_may_be_opaque {
                return Ok(abstract_default_value(&field.type_name));
            }
        }
        Ok(AbstractValue::InstanceField {
            base: Box::new(base),
            field: field.clone(),
        })
    }

    fn evaluate_array_ref(
        &mut self,
        frame: &mut Frame,
        base: &JimpleValue,
        index: &JimpleValue,
        depth: usize,
    ) -> Result<AbstractValue> {
        let base = self.evaluate(frame, base, depth)?;
        let index = self.evaluate(frame, index, depth)?;
        if let AbstractValue::Integer { value: integer } = index
            && let Some(identity) = abstract_array_identity(&base)
            && let Some(array) = self.state.arrays.get(&identity)
        {
            if let Some(value) = array.entries.get(&integer) {
                return Ok(value.clone());
            }
            if let (
                AbstractArrayIdentity::Allocated { .. },
                AbstractValue::ArrayRef { element_type, .. },
                Some(length),
                Ok(index),
            ) = (&identity, &base, array.length, usize::try_from(integer))
                && index < length
            {
                return Ok(abstract_default_value(element_type));
            }
        }
        Ok(AbstractValue::ArrayElement {
            base: Box::new(base),
            index: Box::new(index),
        })
    }

    fn evaluate_binary(
        &mut self,
        frame: &mut Frame,
        operator: &str,
        left: &JimpleValue,
        right: &JimpleValue,
        type_name: &str,
        depth: usize,
    ) -> Result<AbstractValue> {
        let left = self.evaluate(frame, left, depth)?;
        let right = self.evaluate(frame, right, depth)?;
        Ok(object_binary(operator.trim(), left, right, type_name))
    }

    fn evaluate_array_length(
        &mut self,
        frame: &mut Frame,
        value: &JimpleValue,
        depth: usize,
    ) -> Result<AbstractValue> {
        let value = self.evaluate(frame, value, depth)?;
        if let Some(identity) = abstract_array_identity(&value)
            && let Some(length) = self
                .state
                .arrays
                .get(&identity)
                .and_then(|array| array.length)
            && let Ok(length) = i64::try_from(length)
        {
            return Ok(AbstractValue::Integer { value: length });
        }
        Ok(AbstractValue::ArrayLength {
            value: Box::new(value),
        })
    }

    fn evaluate_invoke_value(
        &mut self,
        frame: &mut Frame,
        value: &JimpleValue,
        depth: usize,
    ) -> Result<AbstractValue> {
        let JimpleValueKind::Invoke {
            invoke_kind,
            method,
            base,
            arguments,
            bootstrap_method,
            bootstrap_arguments,
        } = &value.kind
        else {
            bail!(
                "evaluate_invoke_value received non-invoke value {}",
                value.text
            );
        };
        let base = match base.as_deref() {
            Some(base) => Some(self.evaluate(frame, base, depth)?),
            None => None,
        };
        let arguments = arguments
            .iter()
            .map(|argument| self.evaluate(frame, argument, depth))
            .collect::<Result<Vec<_>>>()?;
        if invoke_kind == "JDynamicInvokeExpr"
            && bootstrap_method.as_ref().is_some_and(|bootstrap| {
                bootstrap.owner == "java.lang.runtime.SwitchBootstraps"
                    && bootstrap.name == "typeSwitch"
            })
        {
            let bootstrap_arguments = bootstrap_arguments
                .iter()
                .map(|argument| self.evaluate(frame, argument, depth))
                .collect::<Result<Vec<_>>>()?;
            if let Some(result) = evaluate_type_switch(&arguments, &bootstrap_arguments) {
                return Ok(result);
            }
        }

        if invoke_kind == "JDynamicInvokeExpr"
            && bootstrap_method
                .as_ref()
                .is_some_and(|bootstrap| bootstrap.owner == "java.lang.invoke.LambdaMetafactory")
        {
            let bootstrap_arguments = bootstrap_arguments
                .iter()
                .map(|argument| self.evaluate(frame, argument, depth))
                .collect::<Result<Vec<_>>>()?;
            if let Some(AbstractValue::MethodHandle {
                handle_kind,
                method: target,
            }) = bootstrap_arguments
                .iter()
                .find(|argument| matches!(argument, AbstractValue::MethodHandle { .. }))
            {
                return Ok(AbstractValue::Lambda {
                    handle_kind: handle_kind.clone(),
                    target: target.clone(),
                    captures: arguments,
                });
            }
        }
        self.invoke(invoke_kind, method, base, arguments, depth)
    }

    fn invoke(
        &mut self,
        invoke_kind: &str,
        method: &JimpleMethodSignature,
        base: Option<AbstractValue>,
        arguments: Vec<AbstractValue>,
        depth: usize,
    ) -> Result<AbstractValue> {
        if invoke_kind == "JStaticInvokeExpr" {
            self.ensure_class_initialized(&method.owner, depth)?;
        }
        if let Some(value) = self.invoke_jdk(method, base.as_ref(), &arguments, depth)? {
            return Ok(value);
        }
        if let Some(value) = self.invoke_runtime_library(method, &arguments) {
            return Ok(value);
        }

        if matches!(invoke_kind, "JVirtualInvokeExpr" | "JInterfaceInvokeExpr")
            && let Some(AbstractValue::ObjectRef { class_type, .. }) = base.as_ref()
            && let Some(callee) = self.program.dispatched_method(class_type, method)
        {
            return Ok(self
                .execute_method(callee, base.clone(), arguments.clone(), depth + 1)?
                .unwrap_or(AbstractValue::Null));
        }

        if method.name == "<init>" && invoke_kind == "JSpecialInvokeExpr" {
            if let Some(AbstractValue::ObjectRef { id, .. }) = &base
                && let Some(object) = self.state.objects.get_mut(id)
                && object.constructor.is_none()
            {
                object.constructor = Some(AbstractInvocation {
                    method: method.clone(),
                    arguments: arguments.clone(),
                });
            }
            if let Some(callee) = self.program.method(method) {
                self.execute_method(callee, base.clone(), arguments, depth + 1)?;
            } else if method.owner != "java.lang.Object"
                && let Some(AbstractValue::ObjectRef { id, .. }) = &base
                && let Some(object) = self.state.objects.get_mut(id)
            {
                object.fields_may_be_opaque = true;
            }
            return Ok(base.unwrap_or(AbstractValue::Null));
        }

        if let Some(callee) = self.program.method(method) {
            return Ok(self
                .execute_method(callee, base.clone(), arguments.clone(), depth + 1)?
                .unwrap_or(AbstractValue::Null));
        }

        let shape = AbstractOpaqueCallShape {
            method: method.clone(),
            base: base.clone(),
            arguments: arguments.clone(),
        };
        if !self.opaque_call_shapes.contains(&shape) {
            self.opaque_call_shapes.push(shape);
        }
        Ok(AbstractValue::Invoke {
            method: method.clone(),
            base: base.map(Box::new),
            arguments,
        })
    }

    fn invoke_runtime_library(
        &mut self,
        method: &JimpleMethodSignature,
        arguments: &[AbstractValue],
    ) -> Option<AbstractValue> {
        if method.owner != "net.minecraftforge.oredict.OreDictionary" {
            return None;
        }
        match (method.name.as_str(), arguments) {
            ("registerOre", [name, stack])
                if method.parameter_types
                    == ["java.lang.String", "net.minecraft.item.ItemStack"] =>
            {
                self.named_registry_insert(name.clone(), stack.clone());
                Some(AbstractValue::Null)
            }
            ("registerOre", [AbstractValue::Integer { value: id }, stack])
                if method.parameter_types == ["int", "net.minecraft.item.ItemStack"] =>
            {
                if let Some(name) = self.named_registry_name(*id) {
                    self.named_registry_insert(name, stack.clone());
                }
                Some(AbstractValue::Null)
            }
            ("getOreID", [name]) if method.parameter_types == ["java.lang.String"] => {
                let id = self.named_registry_id(name.clone());
                Some(AbstractValue::Integer { value: id })
            }
            ("getOreName", [AbstractValue::Integer { value: id }]) => Some(
                self.named_registry_name(*id)
                    .unwrap_or(AbstractValue::String {
                        value: String::new(),
                    }),
            ),
            ("doesOreNameExist", [name]) => Some(AbstractValue::Integer {
                value: i64::from(self.named_registry_entry(name).is_some()),
            }),
            ("getOres", [name]) if method.parameter_types == ["java.lang.String"] => {
                let values = self
                    .named_registry_entry(name)
                    .map_or_else(Vec::new, |(_, values)| values.clone());
                Some(self.allocate_list("java.util.ArrayList", values))
            }
            ("getOres", [name, _]) if method.parameter_types == ["java.lang.String", "boolean"] => {
                let values = self
                    .named_registry_entry(name)
                    .map_or_else(Vec::new, |(_, values)| values.clone());
                Some(self.allocate_list("java.util.ArrayList", values))
            }
            ("getOres", [AbstractValue::Integer { value: id }]) => {
                let values = self
                    .named_registry_name(*id)
                    .and_then(|name| self.named_registry_entry(&name))
                    .map_or_else(Vec::new, |(_, values)| values.clone());
                Some(self.allocate_list("java.util.ArrayList", values))
            }
            _ => None,
        }
    }

    fn named_registry_entry(
        &self,
        name: &AbstractValue,
    ) -> Option<&(AbstractValue, Vec<AbstractValue>)> {
        self.named_registry
            .iter()
            .find(|(existing, _)| existing == name)
    }

    fn named_registry_insert(&mut self, name: AbstractValue, value: AbstractValue) {
        if let Some((_, values)) = self
            .named_registry
            .iter_mut()
            .find(|(existing, _)| existing == &name)
        {
            if !values.contains(&value) {
                values.push(value);
            }
            return;
        }
        self.named_registry.push((name, vec![value]));
    }

    fn named_registry_id(&mut self, name: AbstractValue) -> i64 {
        if let Some(index) = self
            .named_registry
            .iter()
            .position(|(existing, _)| existing == &name)
        {
            return i64::try_from(index).unwrap_or(i64::MAX);
        }
        let id = i64::try_from(self.named_registry.len()).unwrap_or(i64::MAX);
        self.named_registry.push((name, Vec::new()));
        id
    }

    fn named_registry_name(&self, id: i64) -> Option<AbstractValue> {
        usize::try_from(id)
            .ok()
            .and_then(|index| self.named_registry.get(index))
            .map(|(name, _)| name.clone())
    }

    fn invoke_jdk(
        &mut self,
        method: &JimpleMethodSignature,
        base: Option<&AbstractValue>,
        arguments: &[AbstractValue],
        depth: usize,
    ) -> Result<Option<AbstractValue>> {
        if let Some(value) = invoke_big_integer(method, base, arguments) {
            return Ok(Some(value));
        }
        if let Some(value) = invoke_boxed_primitive(method, base, arguments) {
            return Ok(Some(value));
        }
        if let Some(value) = self.invoke_collection_factory(method, arguments) {
            return Ok(Some(value));
        }
        if let Some(value) = self.invoke_list(method, base, arguments)? {
            return Ok(Some(value));
        }
        if let Some(value) = self.invoke_set(method, base, arguments) {
            return Ok(Some(value));
        }
        if let Some(value) = self.invoke_iterator(method, base, arguments) {
            return Ok(Some(value));
        }
        if let Some(value) = self.invoke_stream(method, base, arguments, depth)? {
            return Ok(Some(value));
        }
        if let Some(value) = self.invoke_array_clone(method, base) {
            return Ok(Some(value));
        }
        if let Some(value) = invoke_string(method, base, arguments) {
            return Ok(Some(value));
        }
        if matches!(method.name.as_str(), "put" | "get" | "getOrDefault")
            && let Some(AbstractValue::ObjectRef { id, class_type }) = base
            && is_modeled_map(class_type)
        {
            return self.invoke_map(*id, &method.name, arguments).map(Some);
        }
        Ok(None)
    }

    fn invoke_collection_factory(
        &mut self,
        method: &JimpleMethodSignature,
        arguments: &[AbstractValue],
    ) -> Option<AbstractValue> {
        if method.owner == "com.google.common.collect.Lists" && method.name == "newArrayList" {
            return Some(self.allocate_list("java.util.ArrayList", Vec::new()));
        }
        if method.owner == "com.google.common.collect.Maps"
            && matches!(
                method.name.as_str(),
                "newHashMap" | "newHashMapWithExpectedSize"
            )
        {
            return Some(self.allocate_object("java.util.HashMap"));
        }
        if method.owner == "com.google.common.collect.Sets" && method.name == "newHashSet" {
            return Some(self.allocate_object("java.util.HashSet"));
        }
        if method.owner == "java.util.Collections" && method.name == "emptyList" {
            return Some(self.allocate_list("java.util.Collections$EmptyList", Vec::new()));
        }
        if method.owner == "java.util.Collections"
            && method.name == "unmodifiableList"
            && let [list] = arguments
        {
            return Some(list.clone());
        }
        None
    }

    fn invoke_list(
        &mut self,
        method: &JimpleMethodSignature,
        base: Option<&AbstractValue>,
        arguments: &[AbstractValue],
    ) -> Result<Option<AbstractValue>> {
        let Some(AbstractValue::ObjectRef { id, .. }) = base else {
            return Ok(None);
        };
        if !matches!(
            method.owner.as_str(),
            "java.util.List" | "java.util.ArrayList"
        ) || !self.lists.contains_key(id)
        {
            return Ok(None);
        }
        match (method.name.as_str(), arguments) {
            ("add", [value]) => {
                self.lists.entry(*id).or_default().push(value.clone());
                Ok(Some(AbstractValue::Integer { value: 1 }))
            }
            ("get", [AbstractValue::Integer { value: index }]) => {
                let index = usize::try_from(*index).context("negative list index")?;
                let value = self
                    .lists
                    .get(id)
                    .and_then(|values| values.get(index))
                    .cloned()
                    .unwrap_or(AbstractValue::Null);
                Ok(Some(value))
            }
            ("size", []) => {
                let size = self.lists.get(id).map_or(0, Vec::len);
                let size = i64::try_from(size).context("list size does not fit i64")?;
                Ok(Some(AbstractValue::Integer { value: size }))
            }
            ("contains", [needle]) => Ok(Some(AbstractValue::Integer {
                value: i64::from(
                    self.lists
                        .get(id)
                        .is_some_and(|values| values.contains(needle)),
                ),
            })),
            ("iterator", []) => {
                let values = self.lists.get(id).cloned().unwrap_or_default();
                Ok(Some(self.allocate_iterator(values)))
            }
            _ => Ok(None),
        }
    }

    fn invoke_set(
        &mut self,
        method: &JimpleMethodSignature,
        base: Option<&AbstractValue>,
        arguments: &[AbstractValue],
    ) -> Option<AbstractValue> {
        let Some(AbstractValue::ObjectRef { id, .. }) = base else {
            return None;
        };
        let values = self.sets.get_mut(id)?;
        match (method.name.as_str(), arguments) {
            ("add", [value]) => {
                let inserted = !values.contains(value);
                if inserted {
                    values.push(value.clone());
                }
                Some(AbstractValue::Integer {
                    value: i64::from(inserted),
                })
            }
            ("remove", [value]) => {
                let before = values.len();
                values.retain(|existing| existing != value);
                Some(AbstractValue::Integer {
                    value: i64::from(values.len() != before),
                })
            }
            ("contains", [value]) => Some(AbstractValue::Integer {
                value: i64::from(values.contains(value)),
            }),
            ("isEmpty", []) => Some(AbstractValue::Integer {
                value: i64::from(values.is_empty()),
            }),
            ("size", []) => Some(AbstractValue::Integer {
                value: i64::try_from(values.len()).unwrap_or(i64::MAX),
            }),
            ("clear", []) => {
                values.clear();
                Some(AbstractValue::Null)
            }
            ("iterator", []) => {
                let snapshot = values.clone();
                Some(self.allocate_iterator(snapshot))
            }
            _ => None,
        }
    }

    fn invoke_iterator(
        &mut self,
        method: &JimpleMethodSignature,
        base: Option<&AbstractValue>,
        arguments: &[AbstractValue],
    ) -> Option<AbstractValue> {
        if method.owner != "java.util.Iterator" || !arguments.is_empty() {
            return None;
        }
        let Some(AbstractValue::ObjectRef { id, .. }) = base else {
            return None;
        };
        let iterator = self.iterators.get_mut(id)?;
        match method.name.as_str() {
            "hasNext" => Some(AbstractValue::Integer {
                value: i64::from(iterator.index < iterator.values.len()),
            }),
            "next" => {
                let value = iterator
                    .values
                    .get(iterator.index)
                    .cloned()
                    .unwrap_or(AbstractValue::Null);
                iterator.index = iterator.index.saturating_add(1);
                Some(value)
            }
            _ => None,
        }
    }

    fn allocate_list(&mut self, class_type: &str, values: Vec<AbstractValue>) -> AbstractValue {
        let value = self.allocate_object(class_type);
        if let AbstractValue::ObjectRef { id, .. } = value {
            self.lists.insert(id, values);
            AbstractValue::ObjectRef {
                id,
                class_type: class_type.to_owned(),
            }
        } else {
            value
        }
    }

    fn allocate_iterator(&mut self, values: Vec<AbstractValue>) -> AbstractValue {
        let id = self.state.next_id;
        self.state.next_id = self.state.next_id.saturating_add(1);
        self.state.objects.insert(
            id,
            MutableObject {
                class_type: String::from("java.util.Iterator"),
                constructor: None,
                fields_may_be_opaque: false,
                fields: BTreeMap::new(),
            },
        );
        self.iterators
            .insert(id, AbstractIterator { values, index: 0 });
        AbstractValue::ObjectRef {
            id,
            class_type: String::from("java.util.Iterator"),
        }
    }

    fn invoke_stream(
        &mut self,
        method: &JimpleMethodSignature,
        base: Option<&AbstractValue>,
        arguments: &[AbstractValue],
        depth: usize,
    ) -> Result<Option<AbstractValue>> {
        if method.owner == "java.util.Arrays"
            && method.name == "stream"
            && let [array @ AbstractValue::ArrayRef { element_type, .. }] = arguments
            && let Some(values) = self.concrete_array_values(array)
        {
            return Ok(Some(AbstractValue::Sequence {
                element_type: element_type.clone(),
                values,
            }));
        }
        if method.owner == "java.util.stream.LongStream"
            && method.name == "map"
            && let (
                Some(AbstractValue::Sequence {
                    element_type,
                    values,
                }),
                [lambda @ AbstractValue::Lambda { .. }],
            ) = (base, arguments)
        {
            let mut mapped = Vec::with_capacity(values.len());
            for value in values {
                mapped.push(self.apply_lambda(lambda, value.clone(), depth + 1)?);
            }
            return Ok(Some(AbstractValue::Sequence {
                element_type: element_type.clone(),
                values: mapped,
            }));
        }
        if method.owner == "java.util.stream.LongStream"
            && method.name == "toArray"
            && arguments.is_empty()
            && let Some(AbstractValue::Sequence {
                element_type,
                values,
            }) = base
        {
            return Ok(Some(self.allocate_array_values(element_type, values)));
        }
        Ok(None)
    }

    fn invoke_array_clone(
        &mut self,
        method: &JimpleMethodSignature,
        base: Option<&AbstractValue>,
    ) -> Option<AbstractValue> {
        if method.owner != "java.lang.Object" || method.name != "clone" {
            return None;
        }
        let base = base?;
        let identity = abstract_array_identity(base)?;
        let array = self.state.arrays.get(&identity)?.clone();
        let id = self.state.next_id;
        self.state.next_id = self.state.next_id.saturating_add(1);
        self.state
            .arrays
            .insert(AbstractArrayIdentity::Allocated { id }, array);
        let element_type = match base {
            AbstractValue::ArrayRef { element_type, .. } => element_type.clone(),
            _ => String::from("java.lang.Object"),
        };
        Some(AbstractValue::ArrayRef { id, element_type })
    }

    fn concrete_array_values(&self, value: &AbstractValue) -> Option<Vec<AbstractValue>> {
        let identity = abstract_array_identity(value)?;
        let array = self.state.arrays.get(&identity)?;
        let length = array.length?;
        let AbstractValue::ArrayRef { element_type, .. } = value else {
            return None;
        };
        (0..length)
            .map(|index| {
                let index = i64::try_from(index).ok()?;
                Some(
                    array
                        .entries
                        .get(&index)
                        .cloned()
                        .unwrap_or_else(|| abstract_default_value(element_type)),
                )
            })
            .collect()
    }

    fn apply_lambda(
        &mut self,
        lambda: &AbstractValue,
        argument: AbstractValue,
        depth: usize,
    ) -> Result<AbstractValue> {
        let AbstractValue::Lambda {
            handle_kind,
            target,
            captures,
        } = lambda
        else {
            return Ok(AbstractValue::Unknown {
                type_name: String::from("java.lang.Object"),
                text: String::from("non-lambda passed to lambda application"),
            });
        };
        let Some(callee) = self.program.method(target) else {
            return Ok(AbstractValue::Invoke {
                method: target.clone(),
                base: None,
                arguments: captures.iter().cloned().chain([argument]).collect(),
            });
        };
        if handle_kind.contains("INVOKE_STATIC") {
            let arguments = captures.iter().cloned().chain([argument]).collect();
            return Ok(self
                .execute_method(callee, None, arguments, depth)?
                .unwrap_or(AbstractValue::Null));
        }
        Ok(AbstractValue::Unknown {
            type_name: target.return_type.clone(),
            text: format!("unsupported lambda handle kind {handle_kind}"),
        })
    }

    fn allocate_array_values(
        &mut self,
        element_type: &str,
        values: &[AbstractValue],
    ) -> AbstractValue {
        let id = self.state.next_id;
        self.state.next_id = self.state.next_id.saturating_add(1);
        let entries = values
            .iter()
            .enumerate()
            .filter_map(|(index, value)| {
                i64::try_from(index)
                    .ok()
                    .map(|index| (index, value.clone()))
            })
            .collect();
        self.state.arrays.insert(
            AbstractArrayIdentity::Allocated { id },
            MutableArray {
                length: Some(values.len()),
                entries,
            },
        );
        AbstractValue::ArrayRef {
            id,
            element_type: element_type.to_owned(),
        }
    }

    fn invoke_map(
        &mut self,
        object_id: u64,
        method: &str,
        arguments: &[AbstractValue],
    ) -> Result<AbstractValue> {
        let entries = self.maps.entry(object_id).or_default();
        match (method, arguments) {
            ("put", [key, value]) => {
                if let Some((_, old)) = entries.iter_mut().find(|(existing, _)| existing == key) {
                    return Ok(std::mem::replace(old, value.clone()));
                }
                entries.push((key.clone(), value.clone()));
                Ok(AbstractValue::Null)
            }
            ("get", [key]) => Ok(entries
                .iter()
                .find(|(existing, _)| existing == key)
                .map_or(AbstractValue::Null, |(_, value)| value.clone())),
            ("getOrDefault", [key, default]) => Ok(entries
                .iter()
                .find(|(existing, _)| existing == key)
                .map_or_else(|| default.clone(), |(_, value)| value.clone())),
            _ => bail!("unsupported modeled map invocation {method}"),
        }
    }

    fn allocate_object(&mut self, class_type: &str) -> AbstractValue {
        let id = self.state.next_id;
        self.state.next_id = self.state.next_id.saturating_add(1);
        self.state.objects.insert(
            id,
            MutableObject {
                class_type: class_type.to_owned(),
                constructor: None,
                fields_may_be_opaque: false,
                fields: BTreeMap::new(),
            },
        );
        if is_modeled_map(class_type) {
            self.maps.insert(id, Vec::new());
        }
        if is_modeled_set(class_type) {
            self.sets.insert(id, Vec::new());
        }
        if is_jdk_list(class_type) {
            self.lists.insert(id, Vec::new());
        }
        AbstractValue::ObjectRef {
            id,
            class_type: class_type.to_owned(),
        }
    }

    fn allocate_array(&mut self, element_type: &str, size: &AbstractValue) -> AbstractValue {
        let length = match size {
            AbstractValue::Integer { value } => usize::try_from(*value).ok(),
            _ => None,
        };
        let id = self.state.next_id;
        self.state.next_id = self.state.next_id.saturating_add(1);
        self.state.arrays.insert(
            AbstractArrayIdentity::Allocated { id },
            MutableArray {
                length,
                entries: BTreeMap::new(),
            },
        );
        AbstractValue::ArrayRef {
            id,
            element_type: element_type.to_owned(),
        }
    }

    fn finish(self) -> ProgramExecution {
        ProgramExecution {
            steps: self.steps,
            initialized_classes: self.initialized_classes.into_iter().collect(),
            static_fields: self
                .state
                .static_fields
                .into_iter()
                .map(|(field, value)| AbstractStaticFieldValue { field, value })
                .collect(),
            arrays: self
                .state
                .arrays
                .into_iter()
                .map(|(identity, array)| AbstractArraySnapshot {
                    identity,
                    length: array.length,
                    entries: array
                        .entries
                        .into_iter()
                        .map(|(index, value)| AbstractArrayEntry { index, value })
                        .collect(),
                })
                .collect(),
            objects: self
                .state
                .objects
                .into_iter()
                .map(|(id, object)| AbstractObjectSnapshot {
                    id,
                    class_type: object.class_type,
                    constructor: object.constructor,
                    fields_may_be_opaque: object.fields_may_be_opaque,
                    fields: object
                        .fields
                        .into_iter()
                        .map(|(field, value)| AbstractFieldValue { field, value })
                        .collect(),
                })
                .collect(),
            maps: self
                .maps
                .into_iter()
                .map(|(object_id, entries)| AbstractMapSnapshot {
                    object_id,
                    entries: entries
                        .into_iter()
                        .map(|(key, value)| AbstractMapEntry { key, value })
                        .collect(),
                })
                .collect(),
            lists: self
                .lists
                .into_iter()
                .map(|(object_id, values)| AbstractListSnapshot { object_id, values })
                .collect(),
            sets: self
                .sets
                .into_iter()
                .map(|(object_id, values)| AbstractSetSnapshot { object_id, values })
                .collect(),
            named_registry: self
                .named_registry
                .into_iter()
                .map(|(name, values)| AbstractNamedRegistryEntry { name, values })
                .collect(),
            opaque_call_shapes: self.opaque_call_shapes,
            unresolved_array_writes: self.unresolved_array_writes,
            exceptional_paths: self.exceptional_paths,
        }
    }
}

fn dedupe_effect_guards(guards: &mut Vec<JimpleEffectGuard>) {
    let mut index = 0;
    while index < guards.len() {
        if guards[..index].contains(&guards[index]) {
            guards.remove(index);
        } else {
            index += 1;
        }
    }
}

fn symbolic_references_field(
    value: &SymbolicValue,
    field: &super::JimpleFieldSignature,
    receiver: Option<&SymbolicValue>,
) -> bool {
    match value {
        SymbolicValue::StaticField { field: candidate } => candidate == field && receiver.is_none(),
        SymbolicValue::InstanceField {
            base,
            field: candidate,
        } => candidate == field && receiver == Some(base.as_ref()),
        SymbolicValue::ArrayElement { base, index }
        | SymbolicValue::Binary {
            left: base,
            right: index,
            ..
        } => {
            symbolic_references_field(base, field, receiver)
                || symbolic_references_field(index, field, receiver)
        }
        SymbolicValue::Cast { value, .. }
        | SymbolicValue::InstanceOf { value, .. }
        | SymbolicValue::Unary { value, .. }
        | SymbolicValue::ArrayLength { value }
        | SymbolicValue::NewArray { size: value, .. } => {
            symbolic_references_field(value, field, receiver)
        }
        SymbolicValue::NewMultiArray { sizes, .. } | SymbolicValue::Phi { values: sizes } => sizes
            .iter()
            .any(|item| symbolic_references_field(item, field, receiver)),
        SymbolicValue::Piecewise { arms } => arms.iter().any(|arm| {
            symbolic_references_field(&arm.value, field, receiver)
                || arm
                    .guards
                    .iter()
                    .any(|guard| symbolic_references_field(&guard.condition, field, receiver))
        }),
        SymbolicValue::Invoke {
            base, arguments, ..
        } => {
            base.as_deref()
                .is_some_and(|base| symbolic_references_field(base, field, receiver))
                || arguments
                    .iter()
                    .any(|argument| symbolic_references_field(argument, field, receiver))
        }
        SymbolicValue::Integer { .. }
        | SymbolicValue::Float { .. }
        | SymbolicValue::String { .. }
        | SymbolicValue::Null
        | SymbolicValue::Parameter { .. }
        | SymbolicValue::Local { .. }
        | SymbolicValue::This { .. }
        | SymbolicValue::New { .. }
        | SymbolicValue::Unknown { .. } => false,
    }
}

fn method_has_this(method: &JimpleMethod) -> bool {
    method.statements.iter().any(|statement| {
        matches!(
            &statement.operation,
            JimpleOperation::Identity {
                right: JimpleValue {
                    kind: JimpleValueKind::This,
                    ..
                },
                ..
            }
        )
    })
}

fn symbolic_boolean(value: &SymbolicValue) -> bool {
    match value {
        SymbolicValue::Integer { value, .. } => matches!(value, 0 | 1),
        SymbolicValue::InstanceField { field, .. } | SymbolicValue::StaticField { field } => {
            field.type_name == "boolean"
        }
        SymbolicValue::Binary { operator, .. } => {
            matches!(
                operator.as_str(),
                "==" | "!=" | "<" | "<=" | ">" | ">=" | "&&" | "||"
            )
        }
        SymbolicValue::Unary { operator, .. } => operator == "!",
        SymbolicValue::Invoke { method, .. } => method.return_type == "boolean",
        SymbolicValue::Cast { type_name, value } => {
            type_name == "boolean" || symbolic_boolean(value)
        }
        _ => false,
    }
}

fn expand_piecewise_binary(
    operator: &str,
    left: &SymbolicValue,
    right: &SymbolicValue,
) -> Option<SymbolicValue> {
    match (left, right) {
        (SymbolicValue::Piecewise { arms }, other) => {
            piecewise_binary_formula(operator, arms, other, true)
        }
        (other, SymbolicValue::Piecewise { arms }) => {
            piecewise_binary_formula(operator, arms, other, false)
        }
        _ => None,
    }
}

fn piecewise_binary_formula(
    operator: &str,
    arms: &[SymbolicPiecewiseArm],
    other: &SymbolicValue,
    piecewise_on_left: bool,
) -> Option<SymbolicValue> {
    let common_guards = common_piecewise_guards(arms);
    let mut alternatives = arms.iter().map(|arm| {
        let comparison = if piecewise_on_left {
            SymbolicValue::Binary {
                operator: operator.to_owned(),
                left: Box::new(arm.value.clone()),
                right: Box::new(other.clone()),
            }
        } else {
            SymbolicValue::Binary {
                operator: operator.to_owned(),
                left: Box::new(other.clone()),
                right: Box::new(arm.value.clone()),
            }
        };
        arm.guards
            .iter()
            .filter(|guard| !common_guards.contains(guard))
            .map(guard_as_condition)
            .chain(std::iter::once(comparison))
            .reduce(and_symbolic)
    });
    let first = alternatives.next()??;
    let alternatives =
        alternatives.try_fold(first, |left, right| Some(or_symbolic(left, right?)))?;
    common_guards
        .iter()
        .map(guard_as_condition)
        .chain(std::iter::once(alternatives))
        .reduce(and_symbolic)
}

fn common_piecewise_guards(arms: &[SymbolicPiecewiseArm]) -> Vec<JimpleGuard> {
    let Some(first) = arms.first() else {
        return Vec::new();
    };
    first
        .guards
        .iter()
        .filter(|guard| arms.iter().all(|arm| arm.guards.contains(guard)))
        .cloned()
        .collect()
}

fn and_symbolic(left: SymbolicValue, right: SymbolicValue) -> SymbolicValue {
    SymbolicValue::Binary {
        operator: String::from("&&"),
        left: Box::new(left),
        right: Box::new(right),
    }
}

fn or_symbolic(left: SymbolicValue, right: SymbolicValue) -> SymbolicValue {
    SymbolicValue::Binary {
        operator: String::from("||"),
        left: Box::new(left),
        right: Box::new(right),
    }
}

fn guard_as_condition(guard: &JimpleGuard) -> SymbolicValue {
    if guard.expected {
        guard.condition.clone()
    } else {
        SymbolicValue::Unary {
            operator: String::from("!"),
            value: Box::new(guard.condition.clone()),
        }
    }
}

fn simplify_boolean_connective(
    operator: String,
    left: SymbolicValue,
    right: SymbolicValue,
) -> SymbolicValue {
    if left == right {
        return left;
    }
    let left_constant = symbolic_boolean_constant(&left);
    let right_constant = symbolic_boolean_constant(&right);
    match (operator.as_str(), left_constant, right_constant) {
        ("&&", Some(false), _) | ("&&", _, Some(false)) => boolean_constant(false),
        ("&&", Some(true), _) => right,
        ("&&", _, Some(true)) | ("||", _, Some(false)) => left,
        ("||", Some(true), _) | ("||", _, Some(true)) => boolean_constant(true),
        ("||", Some(false), _) => right,
        _ => SymbolicValue::Binary {
            operator,
            left: Box::new(left),
            right: Box::new(right),
        },
    }
}

fn symbolic_boolean_constant(value: &SymbolicValue) -> Option<bool> {
    let SymbolicValue::Integer { value, .. } = value else {
        return None;
    };
    match value {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    }
}

fn boolean_constant(value: bool) -> SymbolicValue {
    SymbolicValue::Integer {
        value: i64::from(value),
        type_name: String::from("boolean"),
    }
}

fn reverse_comparison_operator(operator: &str) -> Option<&'static str> {
    match operator {
        "<" => Some(">"),
        "<=" => Some(">="),
        ">" => Some("<"),
        ">=" => Some("<="),
        "==" => Some("=="),
        "!=" => Some("!="),
        _ => None,
    }
}

fn simplify_cmp_zero(
    operator: &str,
    left: &SymbolicValue,
    right: &SymbolicValue,
) -> Option<SymbolicValue> {
    let build = |outer: &str, cmp: &SymbolicValue| {
        let SymbolicValue::Binary {
            operator: cmp_operator,
            left,
            right,
        } = cmp
        else {
            return None;
        };
        if cmp_operator != "cmp" {
            return None;
        }
        Some(SymbolicValue::Binary {
            operator: outer.to_owned(),
            left: left.clone(),
            right: right.clone(),
        })
    };
    if matches!(right, SymbolicValue::Integer { value: 0, .. }) {
        return build(operator, left);
    }
    if matches!(left, SymbolicValue::Integer { value: 0, .. }) {
        return build(reverse_comparison_operator(operator)?, right);
    }
    None
}

fn simplify_boolean_comparison(
    operator: String,
    left: SymbolicValue,
    right: SymbolicValue,
) -> SymbolicValue {
    if let Some(simplified) = simplify_cmp_zero(&operator, &left, &right) {
        return simplified;
    }
    if let (
        SymbolicValue::Integer {
            value: left_value, ..
        },
        SymbolicValue::Integer {
            value: right_value, ..
        },
    ) = (&left, &right)
    {
        let result = match operator.as_str() {
            "==" => Some(left_value == right_value),
            "!=" => Some(left_value != right_value),
            "<" => Some(left_value < right_value),
            "<=" => Some(left_value <= right_value),
            ">" => Some(left_value > right_value),
            ">=" => Some(left_value >= right_value),
            _ => None,
        };
        if let Some(result) = result {
            return boolean_constant(result);
        }
    }
    let simplify = |boolean: SymbolicValue, integer: i64| {
        if !symbolic_boolean(&boolean) || !matches!(integer, 0 | 1) {
            return None;
        }
        let equivalent = match (operator.as_str(), integer) {
            ("!=", 0) | ("==", 1) => true,
            ("==", 0) | ("!=", 1) => false,
            _ => return None,
        };
        Some(if equivalent {
            boolean
        } else {
            SymbolicValue::Unary {
                operator: String::from("!"),
                value: Box::new(boolean),
            }
        })
    };
    if let SymbolicValue::Integer { value, .. } = &right
        && let Some(value) = simplify(left.clone(), *value)
    {
        return value;
    }
    if let SymbolicValue::Integer { value, .. } = &left
        && let Some(value) = simplify(right.clone(), *value)
    {
        return value;
    }
    SymbolicValue::Binary {
        operator,
        left: Box::new(left),
        right: Box::new(right),
    }
}

fn normalize_guard_polarity(
    mut expected: bool,
    mut condition: SymbolicValue,
) -> (bool, SymbolicValue) {
    loop {
        let SymbolicValue::Unary { operator, value } = condition else {
            return (expected, condition);
        };
        if operator != "!" {
            return (expected, SymbolicValue::Unary { operator, value });
        }
        expected = !expected;
        condition = *value;
    }
}

fn dedupe_guards(guards: &mut Vec<JimpleEffectGuard>) {
    let mut index = 0;
    while index < guards.len() {
        let duplicate = guards[..index].iter().any(|existing| {
            existing.expected == guards[index].expected
                && existing.condition == guards[index].condition
        });
        if duplicate {
            guards.remove(index);
        } else {
            index += 1;
        }
    }
}

fn exact_symbolic_runtime_type(value: Option<&SymbolicValue>) -> Option<&str> {
    match value? {
        SymbolicValue::This { type_name } => Some(type_name),
        SymbolicValue::New { class_type } => Some(class_type),
        _ => None,
    }
}

fn immediate_postdominator(method: &JimpleMethod, statement_id: usize) -> Option<usize> {
    const EXIT: usize = usize::MAX;

    let ids: BTreeSet<_> = method
        .statements
        .iter()
        .map(|statement| statement.id)
        .collect();
    if !ids.contains(&statement_id) {
        return None;
    }
    let mut universe = ids.clone();
    universe.insert(EXIT);
    let mut postdominators: BTreeMap<usize, BTreeSet<usize>> = ids
        .iter()
        .copied()
        .map(|id| (id, universe.clone()))
        .collect();
    postdominators.insert(EXIT, BTreeSet::from([EXIT]));

    loop {
        let mut changed = false;
        for statement in method.statements.iter().rev() {
            let successors: Vec<_> = if statement.successors.is_empty() {
                vec![EXIT]
            } else {
                statement.successors.clone()
            };
            let mut intersection = universe.clone();
            for successor in successors {
                let successors_postdominators = postdominators.get(&successor)?;
                intersection.retain(|candidate| successors_postdominators.contains(candidate));
            }
            intersection.insert(statement.id);
            let previous = postdominators.get(&statement.id)?;
            if previous != &intersection {
                postdominators.insert(statement.id, intersection);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    let strict: BTreeSet<_> = postdominators
        .get(&statement_id)?
        .iter()
        .copied()
        .filter(|candidate| *candidate != statement_id && *candidate != EXIT)
        .collect();
    strict.iter().copied().find(|candidate| {
        strict.iter().all(|other| {
            other == candidate
                || !postdominators
                    .get(other)
                    .is_some_and(|other_set| other_set.contains(candidate))
        })
    })
}

fn join_frames(left: &Frame, right: &Frame) -> Frame {
    let local_names: BTreeSet<_> = left
        .locals
        .keys()
        .chain(right.locals.keys())
        .cloned()
        .collect();
    let locals = local_names
        .into_iter()
        .map(|name| {
            let value = join_optional_branch_value(
                left.locals.get(&name),
                right.locals.get(&name),
                &format!("local {name}"),
            )
            .unwrap_or(AbstractValue::Unknown {
                type_name: String::from("java.lang.Object"),
                text: format!("branch-dependent local {name}"),
            });
            (name, value)
        })
        .collect();

    let argument_count = left.arguments.len().max(right.arguments.len());
    let arguments = (0..argument_count)
        .map(|index| {
            join_optional_branch_value(
                left.arguments.get(index),
                right.arguments.get(index),
                &format!("argument {index}"),
            )
            .unwrap_or(AbstractValue::Unknown {
                type_name: String::from("java.lang.Object"),
                text: format!("branch-dependent argument {index}"),
            })
        })
        .collect();

    Frame {
        locals,
        this_value: join_optional_branch_value(
            left.this_value.as_ref(),
            right.this_value.as_ref(),
            "this",
        ),
        arguments,
    }
}

fn join_optional_branch_value(
    left: Option<&AbstractValue>,
    right: Option<&AbstractValue>,
    description: &str,
) -> Option<AbstractValue> {
    match (left, right) {
        (Some(left), Some(right)) => Some(join_value(left, right, &value_type_hint(left))),
        (Some(value), None) | (None, Some(value)) => Some(AbstractValue::Unknown {
            type_name: value_type_hint(value),
            text: format!("{description} exists on only one branch"),
        }),
        (None, None) => None,
    }
}

fn join_execution_states(left: &ExecutionState, right: &ExecutionState) -> ExecutionState {
    let next_id = left.next_id.max(right.next_id);
    ExecutionState {
        locals: BTreeMap::new(),
        static_fields: join_field_values(&left.static_fields, &right.static_fields),
        arrays: join_arrays(&left.arrays, &right.arrays),
        objects: join_objects(&left.objects, &right.objects),
        next_id,
    }
}

fn join_field_values(
    left: &BTreeMap<super::JimpleFieldSignature, AbstractValue>,
    right: &BTreeMap<super::JimpleFieldSignature, AbstractValue>,
) -> BTreeMap<super::JimpleFieldSignature, AbstractValue> {
    let keys: BTreeSet<_> = left.keys().chain(right.keys()).cloned().collect();
    keys.into_iter()
        .map(|field| {
            let value = match (left.get(&field), right.get(&field)) {
                (Some(left), Some(right)) => join_value(left, right, &field.type_name),
                (Some(value), None) | (None, Some(value)) => AbstractValue::Unknown {
                    type_name: field.type_name.clone(),
                    text: format!("branch-dependent field; one path was {value:?}"),
                },
                (None, None) => unreachable!("field key came from neither branch"),
            };
            (field, value)
        })
        .collect()
}

fn join_arrays(
    left: &BTreeMap<AbstractArrayIdentity, MutableArray>,
    right: &BTreeMap<AbstractArrayIdentity, MutableArray>,
) -> BTreeMap<AbstractArrayIdentity, MutableArray> {
    let identities: BTreeSet<_> = left.keys().chain(right.keys()).cloned().collect();
    identities
        .into_iter()
        .filter_map(
            |identity| match (left.get(&identity), right.get(&identity)) {
                (Some(left), Some(right)) => {
                    let indices: BTreeSet<_> = left
                        .entries
                        .keys()
                        .chain(right.entries.keys())
                        .copied()
                        .collect();
                    let entries = indices
                        .into_iter()
                        .map(|index| {
                            let value = match (left.entries.get(&index), right.entries.get(&index))
                            {
                                (Some(left), Some(right)) => {
                                    join_value(left, right, &value_type_hint(left))
                                }
                                (Some(value), None) | (None, Some(value)) => {
                                    AbstractValue::Unknown {
                                        type_name: value_type_hint(value),
                                        text: format!(
                                            "branch-dependent array element; one path was {value:?}"
                                        ),
                                    }
                                }
                                (None, None) => {
                                    unreachable!("array index came from neither branch")
                                }
                            };
                            (index, value)
                        })
                        .collect();
                    Some((
                        identity,
                        MutableArray {
                            length: (left.length == right.length)
                                .then_some(left.length)
                                .flatten(),
                            entries,
                        },
                    ))
                }
                // These identities were allocated after the fork on exactly one path. Their
                // references cannot compare equal to an allocation on the other path because the
                // true path starts allocating above the false path's final ID.
                (Some(array), None) | (None, Some(array)) => Some((identity, array.clone())),
                (None, None) => None,
            },
        )
        .collect()
}

fn join_objects(
    left: &BTreeMap<u64, MutableObject>,
    right: &BTreeMap<u64, MutableObject>,
) -> BTreeMap<u64, MutableObject> {
    let ids: BTreeSet<_> = left.keys().chain(right.keys()).copied().collect();
    ids.into_iter()
        .filter_map(|id| match (left.get(&id), right.get(&id)) {
            (Some(left), Some(right)) => {
                let fields: BTreeSet<_> = left
                    .fields
                    .keys()
                    .chain(right.fields.keys())
                    .cloned()
                    .collect();
                let joined_fields = fields
                    .into_iter()
                    .map(|field| {
                        let left_value = object_field_value(left, &field);
                        let right_value = object_field_value(right, &field);
                        (
                            field.clone(),
                            join_value(&left_value, &right_value, &field.type_name),
                        )
                    })
                    .collect();
                let constructor_agrees = left.constructor == right.constructor;
                Some((
                    id,
                    MutableObject {
                        class_type: if left.class_type == right.class_type {
                            left.class_type.clone()
                        } else {
                            String::from("java.lang.Object")
                        },
                        constructor: constructor_agrees
                            .then(|| left.constructor.clone())
                            .flatten(),
                        fields_may_be_opaque: left.fields_may_be_opaque
                            || right.fields_may_be_opaque
                            || !constructor_agrees
                            || left.class_type != right.class_type,
                        fields: joined_fields,
                    },
                ))
            }
            (Some(object), None) | (None, Some(object)) => Some((id, object.clone())),
            (None, None) => None,
        })
        .collect()
}

fn object_field_value(
    object: &MutableObject,
    field: &super::JimpleFieldSignature,
) -> AbstractValue {
    object.fields.get(field).cloned().unwrap_or_else(|| {
        if object.fields_may_be_opaque {
            AbstractValue::Unknown {
                type_name: field.type_name.clone(),
                text: String::from("field may have been changed by opaque constructor"),
            }
        } else {
            abstract_default_value(&field.type_name)
        }
    })
}

fn join_named_registry(
    left: &[(AbstractValue, Vec<AbstractValue>)],
    right: &[(AbstractValue, Vec<AbstractValue>)],
) -> Vec<(AbstractValue, Vec<AbstractValue>)> {
    let mut joined = left.to_vec();
    for (name, right_values) in right {
        if let Some((_, values)) = joined.iter_mut().find(|(existing, _)| existing == name) {
            for value in right_values {
                if !values.contains(value) {
                    values.push(value.clone());
                }
            }
        } else {
            joined.push((name.clone(), right_values.clone()));
        }
    }
    joined
}

fn join_sets(
    left: &BTreeMap<u64, Vec<AbstractValue>>,
    right: &BTreeMap<u64, Vec<AbstractValue>>,
) -> BTreeMap<u64, Vec<AbstractValue>> {
    let ids: BTreeSet<_> = left.keys().chain(right.keys()).copied().collect();
    ids.into_iter()
        .filter_map(|id| match (left.get(&id), right.get(&id)) {
            (Some(left), Some(right)) if left == right => Some((id, left.clone())),
            (Some(values), None) | (None, Some(values)) => Some((id, values.clone())),
            _ => None,
        })
        .collect()
}

fn join_lists(
    left: &BTreeMap<u64, Vec<AbstractValue>>,
    right: &BTreeMap<u64, Vec<AbstractValue>>,
) -> BTreeMap<u64, Vec<AbstractValue>> {
    let ids: BTreeSet<_> = left.keys().chain(right.keys()).copied().collect();
    ids.into_iter()
        .filter_map(|id| match (left.get(&id), right.get(&id)) {
            (Some(left), Some(right)) if left.len() == right.len() => Some((
                id,
                left.iter()
                    .zip(right)
                    .map(|(left, right)| join_value(left, right, &value_type_hint(left)))
                    .collect(),
            )),
            (Some(values), None) | (None, Some(values)) => Some((id, values.clone())),
            (Some(_), Some(_)) | (None, None) => None,
        })
        .collect()
}

fn join_iterators(
    left: &BTreeMap<u64, AbstractIterator>,
    right: &BTreeMap<u64, AbstractIterator>,
) -> BTreeMap<u64, AbstractIterator> {
    let ids: BTreeSet<_> = left.keys().chain(right.keys()).copied().collect();
    ids.into_iter()
        .filter_map(|id| match (left.get(&id), right.get(&id)) {
            (Some(left), Some(right)) if left == right => Some((id, left.clone())),
            (Some(iterator), None) | (None, Some(iterator)) => Some((id, iterator.clone())),
            _ => None,
        })
        .collect()
}

fn join_maps(
    left: &BTreeMap<u64, Vec<(AbstractValue, AbstractValue)>>,
    right: &BTreeMap<u64, Vec<(AbstractValue, AbstractValue)>>,
) -> BTreeMap<u64, Vec<(AbstractValue, AbstractValue)>> {
    let ids: BTreeSet<_> = left.keys().chain(right.keys()).copied().collect();
    ids.into_iter()
        .filter_map(|id| match (left.get(&id), right.get(&id)) {
            (Some(left), Some(right)) => Some((id, join_map_entries(left, right))),
            (Some(entries), None) | (None, Some(entries)) => Some((id, entries.clone())),
            (None, None) => None,
        })
        .collect()
}

fn join_map_entries(
    left: &[(AbstractValue, AbstractValue)],
    right: &[(AbstractValue, AbstractValue)],
) -> Vec<(AbstractValue, AbstractValue)> {
    let mut joined = Vec::new();
    let mut matched_right = vec![false; right.len()];
    for (key, left_value) in left {
        if let Some((index, (_, right_value))) = right
            .iter()
            .enumerate()
            .find(|(index, (right_key, _))| !matched_right[*index] && right_key == key)
        {
            matched_right[index] = true;
            joined.push((
                key.clone(),
                join_value(left_value, right_value, &value_type_hint(left_value)),
            ));
        } else {
            joined.push((
                key.clone(),
                AbstractValue::Unknown {
                    type_name: value_type_hint(left_value),
                    text: format!("branch-dependent map entry; one path contained {left_value:?}"),
                },
            ));
        }
    }
    for (index, (key, value)) in right.iter().enumerate() {
        if !matched_right[index] {
            joined.push((
                key.clone(),
                AbstractValue::Unknown {
                    type_name: value_type_hint(value),
                    text: format!("branch-dependent map entry; one path contained {value:?}"),
                },
            ));
        }
    }
    joined
}

fn join_value(left: &AbstractValue, right: &AbstractValue, type_name: &str) -> AbstractValue {
    if left == right {
        left.clone()
    } else {
        AbstractValue::Unknown {
            type_name: type_name.to_owned(),
            text: format!("branch join: {left:?} | {right:?}"),
        }
    }
}

fn value_type_hint(value: &AbstractValue) -> String {
    match value {
        AbstractValue::Integer { .. } => String::from("long"),
        AbstractValue::Float { .. } => String::from("double"),
        AbstractValue::String { .. } => String::from("java.lang.String"),
        AbstractValue::Class { .. } => String::from("java.lang.Class"),
        AbstractValue::BigInteger { .. } => String::from("java.math.BigInteger"),
        AbstractValue::MethodHandle { .. } => String::from("java.lang.invoke.MethodHandle"),
        AbstractValue::Lambda { .. } | AbstractValue::Sequence { .. } => {
            String::from("java.lang.Object")
        }
        AbstractValue::Parameter { type_name, .. }
        | AbstractValue::This { type_name }
        | AbstractValue::Cast { type_name, .. }
        | AbstractValue::Unknown { type_name, .. } => type_name.clone(),
        AbstractValue::StaticField { field } | AbstractValue::InstanceField { field, .. } => {
            field.type_name.clone()
        }
        AbstractValue::Null
        | AbstractValue::ArrayElement { .. }
        | AbstractValue::Binary { .. }
        | AbstractValue::Unary { .. }
        | AbstractValue::ArrayLength { .. } => String::from("java.lang.Object"),
        AbstractValue::ObjectRef { class_type, .. } => class_type.clone(),
        AbstractValue::ArrayRef { element_type, .. } => format!("{element_type}[]"),
        AbstractValue::Invoke { method, .. } => method.return_type.clone(),
    }
}

fn extend_unique<T: PartialEq>(destination: &mut Vec<T>, values: Vec<T>) {
    for value in values {
        if !destination.contains(&value) {
            destination.push(value);
        }
    }
}

fn evaluate_type_switch(
    arguments: &[AbstractValue],
    bootstrap_arguments: &[AbstractValue],
) -> Option<AbstractValue> {
    let [input, AbstractValue::Integer { value: restart }] = arguments else {
        return None;
    };
    if matches!(input, AbstractValue::Null) {
        return Some(AbstractValue::Integer { value: -1 });
    }
    let restart = usize::try_from(*restart).ok()?;
    let input_type = value_type_hint(input);
    if input_type == "java.lang.Object" {
        return None;
    }
    for (index, candidate) in bootstrap_arguments.iter().enumerate().skip(restart) {
        let AbstractValue::Class { class_type } = candidate else {
            return None;
        };
        if class_type == &input_type {
            return i64::try_from(index)
                .ok()
                .map(|value| AbstractValue::Integer { value });
        }
    }
    None
}

fn invoke_big_integer(
    method: &JimpleMethodSignature,
    base: Option<&AbstractValue>,
    arguments: &[AbstractValue],
) -> Option<AbstractValue> {
    if method.owner != "java.math.BigInteger" {
        return None;
    }
    if method.name == "valueOf"
        && let [AbstractValue::Integer { value }] = arguments
    {
        return Some(AbstractValue::BigInteger {
            decimal: BigInt::from(*value).to_string(),
        });
    }
    if matches!(method.name.as_str(), "multiply" | "divide")
        && let (
            Some(AbstractValue::BigInteger { decimal: left }),
            [AbstractValue::BigInteger { decimal: right }],
        ) = (base, arguments)
        && let (Ok(left), Ok(right)) = (left.parse::<BigInt>(), right.parse::<BigInt>())
    {
        if method.name == "divide" && right == BigInt::from(0) {
            return None;
        }
        let value = if method.name == "multiply" {
            left * right
        } else {
            left / right
        };
        return Some(AbstractValue::BigInteger {
            decimal: value.to_string(),
        });
    }
    if method.name == "longValueExact"
        && arguments.is_empty()
        && let Some(AbstractValue::BigInteger { decimal }) = base
        && let Ok(value) = decimal.parse::<i64>()
    {
        return Some(AbstractValue::Integer { value });
    }
    None
}

fn invoke_string(
    method: &JimpleMethodSignature,
    base: Option<&AbstractValue>,
    arguments: &[AbstractValue],
) -> Option<AbstractValue> {
    if method.owner == "java.lang.String"
        && method.name == "contains"
        && let (
            Some(AbstractValue::String { value: haystack }),
            [AbstractValue::String { value: needle }],
        ) = (base, arguments)
    {
        return Some(AbstractValue::Integer {
            value: i64::from(haystack.contains(needle)),
        });
    }
    if method.owner == "java.lang.String"
        && method.name == "toUpperCase"
        && let Some(AbstractValue::String { value }) = base
    {
        return Some(AbstractValue::String {
            value: value.to_uppercase(),
        });
    }
    None
}

fn invoke_boxed_primitive(
    method: &JimpleMethodSignature,
    base: Option<&AbstractValue>,
    arguments: &[AbstractValue],
) -> Option<AbstractValue> {
    let primitive_owner = matches!(
        method.owner.as_str(),
        "java.lang.Integer"
            | "java.lang.Long"
            | "java.lang.Short"
            | "java.lang.Byte"
            | "java.lang.Boolean"
    );
    if !primitive_owner {
        return None;
    }
    if method.name == "valueOf"
        && let [value @ AbstractValue::Integer { .. }] = arguments
    {
        return Some(value.clone());
    }
    if matches!(
        method.name.as_str(),
        "intValue" | "longValue" | "shortValue" | "byteValue" | "booleanValue"
    ) && arguments.is_empty()
        && let Some(value @ AbstractValue::Integer { .. }) = base
    {
        return Some(value.clone());
    }
    None
}

fn is_jdk_list(class_type: &str) -> bool {
    matches!(
        class_type,
        "java.util.ArrayList" | "java.util.LinkedList" | "java.util.Collections$EmptyList"
    )
}

fn is_modeled_map(class_type: &str) -> bool {
    matches!(
        class_type,
        "java.util.HashMap" | "java.util.LinkedHashMap" | "java.util.EnumMap"
    ) || (class_type.starts_with("it.unimi.dsi.fastutil.") && class_type.ends_with("Map"))
}

fn is_modeled_set(class_type: &str) -> bool {
    matches!(
        class_type,
        "java.util.HashSet" | "java.util.LinkedHashSet" | "java.util.Collections$EmptySet"
    ) || (class_type.starts_with("it.unimi.dsi.fastutil.") && class_type.ends_with("Set"))
}

fn object_binary(
    operator: &str,
    left: AbstractValue,
    right: AbstractValue,
    type_name: &str,
) -> AbstractValue {
    if matches!(operator, "==" | "!=") && reference_comparable(&left, &right) {
        let equal = left == right;
        return AbstractValue::Integer {
            value: i64::from(if operator == "==" { equal } else { !equal }),
        };
    }
    abstract_binary(operator, left, right, type_name)
}

fn reference_comparable(left: &AbstractValue, right: &AbstractValue) -> bool {
    concrete_reference(left) && concrete_reference(right)
}

fn concrete_reference(value: &AbstractValue) -> bool {
    matches!(
        value,
        AbstractValue::ObjectRef { .. } | AbstractValue::ArrayRef { .. } | AbstractValue::Null
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(owner: &str, name: &str, type_name: &str) -> super::super::JimpleFieldSignature {
        super::super::JimpleFieldSignature {
            owner: owner.to_owned(),
            name: name.to_owned(),
            type_name: type_name.to_owned(),
        }
    }

    fn value(kind: JimpleValueKind, type_name: &str, text: &str) -> JimpleValue {
        JimpleValue {
            jimple_kind: String::from("test"),
            type_name: type_name.to_owned(),
            text: text.to_owned(),
            kind,
        }
    }

    fn local(name: &str, type_name: &str) -> JimpleValue {
        value(
            JimpleValueKind::Local {
                name: name.to_owned(),
            },
            type_name,
            name,
        )
    }

    fn static_field(field: &super::super::JimpleFieldSignature) -> JimpleValue {
        value(
            JimpleValueKind::StaticField {
                field: field.clone(),
            },
            &field.type_name,
            &format!("{}.{}", field.owner, field.name),
        )
    }

    fn new_object(class_type: &str) -> JimpleValue {
        value(
            JimpleValueKind::New {
                class_type: class_type.to_owned(),
            },
            class_type,
            &format!("new {class_type}"),
        )
    }

    fn invoke(
        base: JimpleValue,
        owner: &str,
        name: &str,
        parameter_types: Vec<&str>,
        arguments: Vec<JimpleValue>,
    ) -> JimpleValue {
        value(
            JimpleValueKind::Invoke {
                invoke_kind: String::from("JVirtualInvokeExpr"),
                method: JimpleMethodSignature {
                    owner: owner.to_owned(),
                    name: name.to_owned(),
                    return_type: String::from("java.lang.Object"),
                    parameter_types: parameter_types.into_iter().map(str::to_owned).collect(),
                },
                base: Some(Box::new(base)),
                arguments,
                bootstrap_method: None,
                bootstrap_arguments: Vec::new(),
            },
            "java.lang.Object",
            "invoke",
        )
    }

    fn statement(id: usize, successors: Vec<usize>, operation: JimpleOperation) -> JimpleStatement {
        JimpleStatement {
            id,
            kind: String::from("test"),
            text: format!("statement {id}"),
            def: None,
            uses: Vec::new(),
            successors,
            operation,
        }
    }

    fn method(owner: &str, name: &str, statements: Vec<JimpleStatement>) -> JimpleMethod {
        JimpleMethod {
            signature: JimpleMethodSignature {
                owner: owner.to_owned(),
                name: name.to_owned(),
                return_type: String::from("void"),
                parameter_types: Vec::new(),
            },
            display_signature: format!("<{owner}: void {name}()>"),
            declaring_superclass: None,
            declaring_interfaces: Vec::new(),
            statements,
        }
    }

    fn integer(value_: i64) -> JimpleValue {
        value(
            JimpleValueKind::IntegerConstant { value: value_ },
            "int",
            &value_.to_string(),
        )
    }

    fn binary(operator: &str, left: JimpleValue, right: JimpleValue) -> JimpleValue {
        value(
            JimpleValueKind::Binary {
                operator: operator.to_owned(),
                left: Box::new(left),
                right: Box::new(right),
            },
            "boolean",
            "binary",
        )
    }

    fn instance_field(
        base: JimpleValue,
        field: &super::super::JimpleFieldSignature,
    ) -> JimpleValue {
        value(
            JimpleValueKind::InstanceField {
                base: Box::new(base),
                field: field.clone(),
            },
            &field.type_name,
            &format!("instance.{}", field.name),
        )
    }

    fn branch_join_program(false_value: i64, true_value: i64) -> (JimpleProgram, u64) {
        let object_field = field("demo.Registry", "OBJECT", "demo.Box");
        let flag_field = field("demo.Config", "FLAG", "boolean");
        let value_field = field("demo.Box", "value", "int");
        let object = local("object", "demo.Box");
        let object_ref = static_field(&object_field);
        let branch_condition = binary("!=", static_field(&flag_field), integer(0));
        let method = method(
            "demo.Registry",
            "run",
            vec![
                statement(
                    0,
                    vec![1],
                    JimpleOperation::Assign {
                        left: object.clone(),
                        right: new_object("demo.Box"),
                    },
                ),
                statement(
                    1,
                    vec![2],
                    JimpleOperation::Assign {
                        left: static_field(&object_field),
                        right: object,
                    },
                ),
                statement(
                    2,
                    vec![3, 5],
                    JimpleOperation::If {
                        condition: branch_condition,
                    },
                ),
                statement(
                    3,
                    vec![4],
                    JimpleOperation::Assign {
                        left: instance_field(object_ref.clone(), &value_field),
                        right: integer(false_value),
                    },
                ),
                statement(4, vec![7], JimpleOperation::Goto),
                statement(
                    5,
                    vec![6],
                    JimpleOperation::Assign {
                        left: instance_field(object_ref, &value_field),
                        right: integer(true_value),
                    },
                ),
                statement(6, vec![7], JimpleOperation::Goto),
                statement(7, Vec::new(), JimpleOperation::ReturnVoid),
            ],
        );
        (JimpleProgram::new(vec![method]), 0)
    }

    fn joined_box_value(execution: &ProgramExecution) -> Result<&AbstractValue> {
        let object = execution
            .static_field_value("demo.Registry", "OBJECT")
            .context("joined object static missing")?;
        let AbstractValue::ObjectRef { id, .. } = object else {
            bail!("joined object identity was lost: {object:?}");
        };
        let object = execution.object(*id).context("joined object missing")?;
        object
            .fields
            .iter()
            .find(|field| field.field.owner == "demo.Box" && field.field.name == "value")
            .map(|field| &field.value)
            .context("joined object value missing")
    }

    #[test]
    fn recovers_clocked_state_candidate_with_guarded_reset() -> Result<()> {
        let counter_field = field("demo.Timer", "counter", "int");
        let loaded = local("loaded", "int");
        let decremented = local("decremented", "int");
        let counter = static_field(&counter_field);
        let tick = method(
            "demo.Timer",
            "tick",
            vec![
                statement(
                    0,
                    vec![1],
                    JimpleOperation::Assign {
                        left: loaded.clone(),
                        right: counter.clone(),
                    },
                ),
                statement(
                    1,
                    vec![2],
                    JimpleOperation::Assign {
                        left: decremented.clone(),
                        right: binary("-", loaded, integer(1)),
                    },
                ),
                statement(
                    2,
                    vec![3],
                    JimpleOperation::Assign {
                        left: counter.clone(),
                        right: decremented,
                    },
                ),
                statement(
                    3,
                    vec![5, 4],
                    JimpleOperation::If {
                        condition: binary("<=", counter.clone(), integer(0)),
                    },
                ),
                statement(
                    4,
                    vec![5],
                    JimpleOperation::Assign {
                        left: counter,
                        right: integer(0),
                    },
                ),
                statement(5, Vec::new(), JimpleOperation::ReturnVoid),
            ],
        );
        let signature = tick.signature.clone();
        let candidates = JimpleProgram::new(vec![tick]).clocked_state_candidates(&signature)?;
        let [candidate] = candidates.as_slice() else {
            return Err(anyhow::anyhow!(
                "expected one clocked-state candidate, got {}",
                candidates.len()
            ));
        };
        assert_eq!(candidate.field, counter_field);
        assert!(!candidate.receiver_is_entry_this);
        assert_eq!(candidate.delta_per_activation, -1);
        assert_eq!(candidate.guarded_writes.len(), 1);
        assert_eq!(candidate.direct_comparisons.len(), 1);
        assert_eq!(candidate.direct_comparisons[0].operator, "<=");
        assert_eq!(candidate.direct_comparisons[0].constant, 0);
        assert_eq!(candidate.observed_guards.len(), 1);
        assert!(candidate.observed_guards[0].expected);
        Ok(())
    }

    #[test]
    fn simplifies_piecewise_boolean_comparison_to_path_predicate() {
        let flag = SymbolicValue::StaticField {
            field: field("demo.Config", "FLAG", "boolean"),
        };
        let piecewise = SymbolicValue::Piecewise {
            arms: vec![
                SymbolicPiecewiseArm {
                    guards: vec![JimpleGuard {
                        branch_statement_id: 1,
                        expected: false,
                        condition: flag.clone(),
                    }],
                    value: SymbolicValue::Integer {
                        value: 1,
                        type_name: String::from("int"),
                    },
                },
                SymbolicPiecewiseArm {
                    guards: vec![JimpleGuard {
                        branch_statement_id: 1,
                        expected: true,
                        condition: flag.clone(),
                    }],
                    value: SymbolicValue::Integer {
                        value: 0,
                        type_name: String::from("int"),
                    },
                },
            ],
        };
        let expression = SymbolicValue::Binary {
            operator: String::from("=="),
            left: Box::new(piecewise),
            right: Box::new(SymbolicValue::Integer {
                value: 0,
                type_name: String::from("int"),
            }),
        };
        let simplified = JimpleProgram::default().simplify_guard_value(expression, 0);
        assert_eq!(simplified, flag);
    }

    #[test]
    fn unknown_branch_preserves_agreed_heap_effect() -> Result<()> {
        let (program, _) = branch_join_program(7, 7);
        let execution = program.execute_named_sequence(&[("demo.Registry", "run")], 100)?;
        assert_eq!(
            joined_box_value(&execution)?,
            &AbstractValue::Integer { value: 7 }
        );
        Ok(())
    }

    #[test]
    fn unknown_branch_reconverges_before_shared_allocation() -> Result<()> {
        let flag_field = field("demo.Config", "FLAG", "boolean");
        let array_field = field("demo.Registry", "ARRAY", "int[]");
        let array = local("array", "int[]");
        let branch_condition = binary("!=", static_field(&flag_field), integer(0));
        let run = method(
            "demo.Registry",
            "run",
            vec![
                statement(
                    0,
                    vec![1, 2],
                    JimpleOperation::If {
                        condition: branch_condition,
                    },
                ),
                statement(1, vec![2], JimpleOperation::Goto),
                statement(
                    2,
                    vec![3],
                    JimpleOperation::Assign {
                        left: array.clone(),
                        right: value(
                            JimpleValueKind::NewArray {
                                element_type: String::from("int"),
                                size: Box::new(integer(1)),
                            },
                            "int[]",
                            "new int[1]",
                        ),
                    },
                ),
                statement(
                    3,
                    vec![4],
                    JimpleOperation::Assign {
                        left: static_field(&array_field),
                        right: array,
                    },
                ),
                statement(4, Vec::new(), JimpleOperation::ReturnVoid),
            ],
        );

        let execution = JimpleProgram::new(vec![run])
            .execute_named_sequence(&[("demo.Registry", "run")], 100)?;
        assert!(matches!(
            execution.static_field_value("demo.Registry", "ARRAY"),
            Some(AbstractValue::ArrayRef { element_type, .. }) if element_type == "int"
        ));
        assert_eq!(execution.arrays.len(), 1);
        Ok(())
    }

    #[test]
    fn unknown_branch_retains_normal_state_and_records_throw() -> Result<()> {
        let flag_field = field("demo.Config", "FLAG", "boolean");
        let result_field = field("demo.Registry", "RESULT", "int");
        let thrown = local("thrown", "java.lang.IllegalArgumentException");
        let branch_condition = binary("!=", static_field(&flag_field), integer(0));
        let run = method(
            "demo.Registry",
            "run",
            vec![
                statement(
                    0,
                    vec![1, 3],
                    JimpleOperation::If {
                        condition: branch_condition,
                    },
                ),
                statement(
                    1,
                    vec![2],
                    JimpleOperation::Assign {
                        left: static_field(&result_field),
                        right: integer(7),
                    },
                ),
                statement(2, Vec::new(), JimpleOperation::ReturnVoid),
                statement(
                    3,
                    vec![4],
                    JimpleOperation::Assign {
                        left: thrown.clone(),
                        right: new_object("java.lang.IllegalArgumentException"),
                    },
                ),
                statement(4, Vec::new(), JimpleOperation::Throw { value: thrown }),
            ],
        );
        let execution = JimpleProgram::new(vec![run])
            .execute_named_sequence(&[("demo.Registry", "run")], 100)?;
        assert_eq!(
            execution.static_field_value("demo.Registry", "RESULT"),
            Some(&AbstractValue::Integer { value: 7 })
        );
        assert_eq!(execution.exceptional_paths.len(), 1);
        assert!(matches!(
            &execution.exceptional_paths[0],
            AbstractValue::ObjectRef { class_type, .. }
                if class_type == "java.lang.IllegalArgumentException"
        ));
        Ok(())
    }

    #[test]
    fn unknown_branch_degrades_disagreed_heap_effect() -> Result<()> {
        let (program, _) = branch_join_program(7, 9);
        let execution = program.execute_named_sequence(&[("demo.Registry", "run")], 100)?;
        assert!(matches!(
            joined_box_value(&execution)?,
            AbstractValue::Unknown { type_name, .. } if type_name == "int"
        ));
        Ok(())
    }

    #[test]
    fn initializes_static_owner_before_first_use() -> Result<()> {
        let answer_field = field("demo.AutoInit", "ANSWER", "int");
        let clinit = method(
            "demo.AutoInit",
            "<clinit>",
            vec![
                statement(
                    0,
                    vec![1],
                    JimpleOperation::Assign {
                        left: static_field(&answer_field),
                        right: integer(42),
                    },
                ),
                statement(1, Vec::new(), JimpleOperation::ReturnVoid),
            ],
        );
        let getter = JimpleMethod {
            signature: JimpleMethodSignature {
                owner: String::from("demo.AutoInit"),
                name: String::from("answer"),
                return_type: String::from("int"),
                parameter_types: Vec::new(),
            },
            display_signature: String::from("<demo.AutoInit: int answer()>"),
            declaring_superclass: None,
            declaring_interfaces: Vec::new(),
            statements: vec![statement(
                0,
                Vec::new(),
                JimpleOperation::Return {
                    value: static_field(&answer_field),
                },
            )],
        };
        let signature = getter.signature.clone();
        let (returned, execution) = JimpleProgram::new(vec![clinit, getter]).execute_call(
            &signature,
            None,
            Vec::new(),
            100,
        )?;

        assert_eq!(returned, Some(AbstractValue::Integer { value: 42 }));
        assert_eq!(
            execution.initialized_classes,
            vec![String::from("demo.AutoInit")]
        );
        assert_eq!(
            execution.static_field_value("demo.AutoInit", "ANSWER"),
            Some(&AbstractValue::Integer { value: 42 })
        );
        Ok(())
    }

    #[test]
    fn explicit_class_initializer_is_not_run_twice() -> Result<()> {
        let object_field = field("demo.Once", "OBJECT", "demo.Value");
        let object = local("object", "demo.Value");
        let clinit = method(
            "demo.Once",
            "<clinit>",
            vec![
                statement(
                    0,
                    vec![1],
                    JimpleOperation::Assign {
                        left: object.clone(),
                        right: new_object("demo.Value"),
                    },
                ),
                statement(
                    1,
                    vec![2],
                    JimpleOperation::Assign {
                        left: static_field(&object_field),
                        right: object,
                    },
                ),
                statement(2, Vec::new(), JimpleOperation::ReturnVoid),
            ],
        );
        let read = method(
            "demo.Once",
            "read",
            vec![
                statement(
                    0,
                    vec![1],
                    JimpleOperation::Assign {
                        left: local("copy", "demo.Value"),
                        right: static_field(&object_field),
                    },
                ),
                statement(1, Vec::new(), JimpleOperation::ReturnVoid),
            ],
        );
        let execution = JimpleProgram::new(vec![clinit, read])
            .execute_named_sequence(&[("demo.Once", "<clinit>"), ("demo.Once", "read")], 100)?;

        assert_eq!(execution.objects.len(), 1);
        assert_eq!(
            execution.initialized_classes,
            vec![String::from("demo.Once"), String::from("demo.Value")]
        );
        Ok(())
    }

    fn timer_setter(timer: &super::super::JimpleFieldSignature) -> JimpleMethod {
        let this_local = local("this", "demo.Timer");
        let parameter_local = local("value", "int");
        let mut setter = method(
            "demo.Timer",
            "set",
            vec![
                statement(
                    0,
                    vec![1],
                    JimpleOperation::Identity {
                        left: this_local.clone(),
                        right: value(JimpleValueKind::This, "demo.Timer", "@this"),
                    },
                ),
                statement(
                    1,
                    vec![2],
                    JimpleOperation::Identity {
                        left: parameter_local.clone(),
                        right: value(
                            JimpleValueKind::Parameter { index: 0 },
                            "int",
                            "@parameter0",
                        ),
                    },
                ),
                statement(
                    2,
                    vec![3],
                    JimpleOperation::Assign {
                        left: instance_field(this_local, timer),
                        right: parameter_local,
                    },
                ),
                statement(3, Vec::new(), JimpleOperation::ReturnVoid),
            ],
        );
        setter.signature.return_type = String::from("java.lang.Object");
        setter.signature.parameter_types = vec![String::from("int")];
        setter
    }

    fn timer_transfer() -> JimpleMethod {
        let this_local = local("this", "demo.Timer");
        let mut transfer = method(
            "demo.Timer",
            "transfer",
            vec![
                statement(
                    0,
                    vec![1],
                    JimpleOperation::Identity {
                        left: this_local.clone(),
                        right: value(JimpleValueKind::This, "demo.Timer", "@this"),
                    },
                ),
                statement(
                    1,
                    vec![2],
                    JimpleOperation::Invoke {
                        expression: Some(invoke(
                            this_local,
                            "demo.Timer",
                            "set",
                            vec!["int"],
                            vec![integer(8)],
                        )),
                    },
                ),
                statement(2, Vec::new(), JimpleOperation::ReturnVoid),
            ],
        );
        transfer.signature.return_type = String::from("java.lang.Object");
        transfer
    }

    fn timer_tick() -> JimpleMethod {
        let this_local = local("this", "demo.Timer");
        method(
            "demo.Timer",
            "tick",
            vec![
                statement(
                    0,
                    vec![1],
                    JimpleOperation::Identity {
                        left: this_local.clone(),
                        right: value(JimpleValueKind::This, "demo.Timer", "@this"),
                    },
                ),
                statement(
                    1,
                    vec![2],
                    JimpleOperation::Invoke {
                        expression: Some(invoke(
                            this_local.clone(),
                            "demo.Timer",
                            "set",
                            vec!["int"],
                            vec![integer(0)],
                        )),
                    },
                ),
                statement(
                    2,
                    vec![3],
                    JimpleOperation::Invoke {
                        expression: Some(invoke(
                            this_local,
                            "demo.Timer",
                            "transfer",
                            Vec::new(),
                            Vec::new(),
                        )),
                    },
                ),
                statement(3, Vec::new(), JimpleOperation::ReturnVoid),
            ],
        )
    }

    #[test]
    fn composes_parameterized_field_writes_through_exact_calls() -> Result<()> {
        let timer = field("demo.Timer", "timer", "int");
        let tick = timer_tick();
        let entry = tick.signature.clone();
        let program = JimpleProgram::new(vec![tick, timer_transfer(), timer_setter(&timer)]);
        let effects = program.structural_effects(&entry)?;

        let values: Vec<_> = effects
            .possible_field_writes
            .iter()
            .filter_map(|write| match write.value {
                SymbolicValue::Integer { value, .. } => Some((value, write.call_path.len())),
                _ => None,
            })
            .collect();
        assert_eq!(values, vec![(0, 1), (8, 2)]);
        assert_eq!(effects.unresolved_calls, Vec::new());
        assert_eq!(effects.calls.len(), 3);
        assert!(
            effects
                .calls
                .iter()
                .all(|call| call.resolution == JimpleProgramCallResolution::Expanded)
        );
        assert!(
            effects
                .possible_field_writes
                .iter()
                .all(|write| write.field == timer)
        );
        Ok(())
    }

    fn typed_invoke(
        invoke_kind: &str,
        base: Option<JimpleValue>,
        owner: &str,
        name: &str,
        return_type: &str,
        parameter_types: Vec<&str>,
        arguments: Vec<JimpleValue>,
    ) -> JimpleValue {
        value(
            JimpleValueKind::Invoke {
                invoke_kind: invoke_kind.to_owned(),
                method: JimpleMethodSignature {
                    owner: owner.to_owned(),
                    name: name.to_owned(),
                    return_type: return_type.to_owned(),
                    parameter_types: parameter_types.into_iter().map(str::to_owned).collect(),
                },
                base: base.map(Box::new),
                arguments,
                bootstrap_method: None,
                bootstrap_arguments: Vec::new(),
            },
            return_type,
            "typed invoke",
        )
    }

    fn runtime_binding_entry() -> JimpleMethod {
        let this = local("this", "demo.Base");
        let argument = local("argument", "int");
        let delay = typed_invoke(
            "JVirtualInvokeExpr",
            Some(this.clone()),
            "demo.Base",
            "delay",
            "int",
            vec!["int"],
            vec![argument.clone()],
        );
        let schedule = typed_invoke(
            "JStaticInvokeExpr",
            None,
            "demo.Engine",
            "schedule",
            "void",
            vec!["int"],
            vec![delay],
        );
        JimpleMethod {
            signature: JimpleMethodSignature {
                owner: String::from("demo.Base"),
                name: String::from("run"),
                return_type: String::from("void"),
                parameter_types: vec![String::from("int")],
            },
            display_signature: String::from("<demo.Base: void run(int)>"),
            declaring_superclass: Some(String::from("java.lang.Object")),
            declaring_interfaces: Vec::new(),
            statements: vec![
                statement(
                    0,
                    vec![1],
                    JimpleOperation::Identity {
                        left: this,
                        right: value(JimpleValueKind::This, "demo.Base", "@this"),
                    },
                ),
                statement(
                    1,
                    vec![2],
                    JimpleOperation::Identity {
                        left: argument,
                        right: value(
                            JimpleValueKind::Parameter { index: 0 },
                            "int",
                            "@parameter0",
                        ),
                    },
                ),
                statement(
                    2,
                    vec![3],
                    JimpleOperation::Invoke {
                        expression: Some(schedule),
                    },
                ),
                statement(3, Vec::new(), JimpleOperation::ReturnVoid),
            ],
        }
    }

    fn runtime_binding_wrapper() -> JimpleMethod {
        let this = local("this", "demo.Base");
        let argument = local("argument", "int");
        let implementation = typed_invoke(
            "JVirtualInvokeExpr",
            Some(this.clone()),
            "demo.Base",
            "delayImpl",
            "int",
            vec!["int"],
            vec![argument.clone()],
        );
        JimpleMethod {
            signature: JimpleMethodSignature {
                owner: String::from("demo.Base"),
                name: String::from("delay"),
                return_type: String::from("int"),
                parameter_types: vec![String::from("int")],
            },
            display_signature: String::from("<demo.Base: int delay(int)>"),
            declaring_superclass: Some(String::from("java.lang.Object")),
            declaring_interfaces: Vec::new(),
            statements: vec![
                statement(
                    0,
                    vec![1],
                    JimpleOperation::Identity {
                        left: this,
                        right: value(JimpleValueKind::This, "demo.Base", "@this"),
                    },
                ),
                statement(
                    1,
                    vec![2],
                    JimpleOperation::Identity {
                        left: argument,
                        right: value(
                            JimpleValueKind::Parameter { index: 0 },
                            "int",
                            "@parameter0",
                        ),
                    },
                ),
                statement(
                    2,
                    Vec::new(),
                    JimpleOperation::Return {
                        value: implementation,
                    },
                ),
            ],
        }
    }

    fn runtime_binding_implementation() -> JimpleMethod {
        JimpleMethod {
            signature: JimpleMethodSignature {
                owner: String::from("demo.Impl"),
                name: String::from("delayImpl"),
                return_type: String::from("int"),
                parameter_types: vec![String::from("int")],
            },
            display_signature: String::from("<demo.Impl: int delayImpl(int)>"),
            declaring_superclass: Some(String::from("demo.Base")),
            declaring_interfaces: Vec::new(),
            statements: vec![statement(
                0,
                Vec::new(),
                JimpleOperation::Return { value: integer(2) },
            )],
        }
    }

    #[test]
    fn runtime_type_binding_dispatches_inherited_wrapper_to_concrete_override() -> Result<()> {
        let entry = runtime_binding_entry();
        let signature = entry.signature.clone();
        let summary = JimpleProgram::new(vec![
            entry,
            runtime_binding_wrapper(),
            runtime_binding_implementation(),
        ])
        .structural_effects_for_runtime_this(&signature, "demo.Impl")?;
        let engine = summary
            .calls
            .iter()
            .find(|call| {
                call.call_path
                    .last()
                    .is_some_and(|frame| frame.callee.owner == "demo.Engine")
            })
            .context("engine boundary missing")?;
        assert_eq!(
            engine.resolution,
            JimpleProgramCallResolution::MissingMethod
        );
        assert!(matches!(
            engine.arguments.as_slice(),
            [SymbolicValue::Integer { value: 2, .. }]
        ));
        let callee = engine
            .call_path
            .last()
            .context("engine call frame missing")?
            .callee
            .clone();
        let candidates = summary.bind_schedule_once(&JimpleScheduleOnceBinding {
            callee,
            key_arguments: Vec::new(),
            delay_argument: 0,
            priority_argument: None,
        })?;
        let [candidate] = candidates.as_slice() else {
            bail!("expected one bound schedule candidate");
        };
        assert!(matches!(
            candidate.delay,
            SymbolicValue::Integer { value: 2, .. }
        ));
        Ok(())
    }

    fn rollback_test_call(
        caller: &JimpleMethodSignature,
        statement_id: usize,
        target: JimpleMethodSignature,
        receiver: SymbolicValue,
        arguments: Vec<SymbolicValue>,
        guards: Vec<JimpleEffectGuard>,
    ) -> JimpleProgramCall {
        JimpleProgramCall {
            call_path: vec![JimpleEffectCallFrame {
                caller: caller.clone(),
                statement_id,
                invoke_kind: String::from("JVirtualInvokeExpr"),
                callee: target,
            }],
            guards,
            receiver: Some(receiver),
            arguments,
            resolution: JimpleProgramCallResolution::DynamicDispatch,
        }
    }

    fn rollback_test_bindings() -> (
        Vec<JimpleIndexedStoreBinding>,
        Vec<JimpleSnapshotCloneBinding>,
        JimpleMethodSignature,
        JimpleMethodSignature,
        JimpleMethodSignature,
    ) {
        let read = JimpleMethodSignature {
            owner: String::from("demo.Store"),
            name: String::from("read"),
            return_type: String::from("demo.Value"),
            parameter_types: vec![String::from("int")],
        };
        let take = JimpleMethodSignature {
            owner: String::from("demo.Store"),
            name: String::from("take"),
            return_type: String::from("demo.Value"),
            parameter_types: vec![String::from("int"), String::from("int")],
        };
        let write = JimpleMethodSignature {
            owner: String::from("demo.Store"),
            name: String::from("write"),
            return_type: String::from("void"),
            parameter_types: vec![String::from("int"), String::from("demo.Value")],
        };
        let clone = JimpleMethodSignature {
            owner: String::from("demo.Value"),
            name: String::from("snapshot"),
            return_type: String::from("demo.Value"),
            parameter_types: Vec::new(),
        };
        let receiver = JimpleCallOperandBinding::Receiver;
        let key = vec![JimpleCallOperandBinding::Argument { index: 0 }];
        let bindings = vec![
            JimpleIndexedStoreBinding {
                callee: read.clone(),
                operation: JimpleIndexedStoreOperationKind::Read,
                store: receiver.clone(),
                key: key.clone(),
                value: None,
                amount: None,
            },
            JimpleIndexedStoreBinding {
                callee: take.clone(),
                operation: JimpleIndexedStoreOperationKind::Take,
                store: receiver.clone(),
                key: key.clone(),
                value: None,
                amount: Some(JimpleCallOperandBinding::Argument { index: 1 }),
            },
            JimpleIndexedStoreBinding {
                callee: write.clone(),
                operation: JimpleIndexedStoreOperationKind::Write,
                store: receiver,
                key,
                value: Some(JimpleCallOperandBinding::Argument { index: 1 }),
                amount: None,
            },
        ];
        (
            bindings,
            vec![JimpleSnapshotCloneBinding { callee: clone }],
            read,
            take,
            write,
        )
    }

    fn symbolic_test_integer(value: i64) -> SymbolicValue {
        SymbolicValue::Integer {
            value,
            type_name: String::from("int"),
        }
    }

    fn rollback_test_context() -> (
        JimpleMethodSignature,
        SymbolicValue,
        SymbolicValue,
        JimpleEffectGuard,
    ) {
        let caller = JimpleMethodSignature {
            owner: String::from("demo.Transfer"),
            name: String::from("run"),
            return_type: String::from("void"),
            parameter_types: Vec::new(),
        };
        let store = SymbolicValue::Parameter {
            index: 0,
            type_name: String::from("demo.Store"),
        };
        let key = SymbolicValue::Parameter {
            index: 1,
            type_name: String::from("int"),
        };
        let guard = JimpleEffectGuard {
            method: caller.clone(),
            branch_statement_id: 5,
            expected: true,
            condition: SymbolicValue::Parameter {
                index: 2,
                type_name: String::from("boolean"),
            },
        };
        (caller, store, key, guard)
    }

    #[test]
    fn speculative_take_rollback_requires_exact_snapshot_origin_and_guard_superset() -> Result<()> {
        let (bindings, clones, read_sig, take_sig, write_sig) = rollback_test_bindings();
        let (caller, store, key, guard) = rollback_test_context();
        let read_value = SymbolicValue::Invoke {
            method: read_sig.clone(),
            base: Some(Box::new(store.clone())),
            arguments: vec![key.clone()],
        };
        let clone_value = SymbolicValue::Invoke {
            method: clones[0].callee.clone(),
            base: Some(Box::new(read_value)),
            arguments: Vec::new(),
        };
        let summary = JimpleProgramEffectSummary {
            entry: caller.clone(),
            possible_field_writes: Vec::new(),
            calls: vec![
                rollback_test_call(
                    &caller,
                    6,
                    read_sig,
                    store.clone(),
                    vec![key.clone()],
                    Vec::new(),
                ),
                rollback_test_call(
                    &caller,
                    7,
                    take_sig,
                    store.clone(),
                    vec![key.clone(), symbolic_test_integer(1)],
                    vec![guard.clone()],
                ),
                rollback_test_call(
                    &caller,
                    10,
                    write_sig.clone(),
                    store.clone(),
                    vec![key.clone(), clone_value.clone()],
                    vec![guard.clone()],
                ),
                rollback_test_call(
                    &caller,
                    11,
                    write_sig.clone(),
                    store.clone(),
                    vec![symbolic_test_integer(99), clone_value.clone()],
                    vec![guard.clone()],
                ),
                rollback_test_call(
                    &caller,
                    12,
                    write_sig,
                    store,
                    vec![key.clone(), clone_value],
                    Vec::new(),
                ),
            ],
            induction_loops: Vec::new(),
            unresolved_calls: Vec::new(),
        };
        let coverage = summary.indexed_store_transaction_coverage(&JimpleTransactionBindings {
            indexed_stores: bindings.clone(),
            snapshot_clones: clones.clone(),
            ..JimpleTransactionBindings::default()
        })?;
        assert!(coverage.complete);
        assert_eq!(coverage.rollbacks.len(), 1);
        let candidates = summary.bind_speculative_take_rollbacks(&bindings, &clones)?;
        let [candidate] = candidates.as_slice() else {
            bail!("expected exactly one rollback candidate");
        };
        assert_eq!(candidate.key, vec![key]);
        assert_eq!(candidate.take_guards, vec![guard.clone()]);
        assert_eq!(candidate.restore_guards, vec![guard]);
        assert!(matches!(
            candidate.amount,
            SymbolicValue::Integer { value: 1, .. }
        ));
        Ok(())
    }

    fn semantic_surface_signatures() -> (
        JimpleMethodSignature,
        JimpleMethodSignature,
        JimpleMethodSignature,
        JimpleMethodSignature,
        JimpleMethodSignature,
    ) {
        (
            JimpleMethodSignature {
                owner: String::from("demo.Transfer"),
                name: String::from("run"),
                return_type: String::from("void"),
                parameter_types: Vec::new(),
            },
            JimpleMethodSignature {
                owner: String::from("demo.Store"),
                name: String::from("read"),
                return_type: String::from("demo.Value"),
                parameter_types: vec![String::from("int")],
            },
            JimpleMethodSignature {
                owner: String::from("demo.Value"),
                name: String::from("capacity"),
                return_type: String::from("int"),
                parameter_types: Vec::new(),
            },
            JimpleMethodSignature {
                owner: String::from("demo.Store"),
                name: String::from("extent"),
                return_type: String::from("int"),
                parameter_types: Vec::new(),
            },
            JimpleMethodSignature {
                owner: String::from("demo.Store"),
                name: String::from("set_ready"),
                return_type: String::from("void"),
                parameter_types: vec![String::from("int")],
            },
        )
    }

    fn semantic_surface_summary(
        signatures: &(
            JimpleMethodSignature,
            JimpleMethodSignature,
            JimpleMethodSignature,
            JimpleMethodSignature,
            JimpleMethodSignature,
        ),
    ) -> (
        JimpleProgramEffectSummary,
        super::super::JimpleFieldSignature,
    ) {
        let (caller, read_sig, capacity_sig, extent_sig, state_sig) = signatures;
        let store = SymbolicValue::Parameter {
            index: 0,
            type_name: String::from("demo.Store"),
        };
        let key = symbolic_test_integer(0);
        let read_value = SymbolicValue::Invoke {
            method: read_sig.clone(),
            base: Some(Box::new(store.clone())),
            arguments: vec![key.clone()],
        };
        let quantity = field("demo.Value", "quantity", "int");
        let write = JimpleProgramFieldWrite {
            call_path: Vec::new(),
            guards: Vec::new(),
            statement_id: 10,
            field: quantity.clone(),
            receiver: Some(read_value.clone()),
            value: SymbolicValue::Binary {
                operator: String::from("+"),
                left: Box::new(SymbolicValue::InstanceField {
                    base: Box::new(read_value.clone()),
                    field: quantity.clone(),
                }),
                right: Box::new(symbolic_test_integer(1)),
            },
        };
        let calls = vec![
            rollback_test_call(
                caller,
                1,
                read_sig.clone(),
                store.clone(),
                vec![key],
                Vec::new(),
            ),
            rollback_test_call(
                caller,
                2,
                capacity_sig.clone(),
                read_value,
                Vec::new(),
                Vec::new(),
            ),
            rollback_test_call(
                caller,
                3,
                extent_sig.clone(),
                store.clone(),
                Vec::new(),
                Vec::new(),
            ),
            rollback_test_call(
                caller,
                4,
                state_sig.clone(),
                store,
                vec![symbolic_test_integer(1)],
                Vec::new(),
            ),
        ];
        (
            JimpleProgramEffectSummary {
                entry: caller.clone(),
                possible_field_writes: vec![write],
                calls,
                induction_loops: Vec::new(),
                unresolved_calls: Vec::new(),
            },
            quantity,
        )
    }

    fn semantic_surface_bindings(
        signatures: &(
            JimpleMethodSignature,
            JimpleMethodSignature,
            JimpleMethodSignature,
            JimpleMethodSignature,
            JimpleMethodSignature,
        ),
        quantity: super::super::JimpleFieldSignature,
    ) -> JimpleTransactionBindings {
        let (_, read_sig, capacity_sig, extent_sig, state_sig) = signatures;
        JimpleTransactionBindings {
            indexed_stores: vec![JimpleIndexedStoreBinding {
                callee: read_sig.clone(),
                operation: JimpleIndexedStoreOperationKind::Read,
                store: JimpleCallOperandBinding::Receiver,
                key: vec![JimpleCallOperandBinding::Argument { index: 0 }],
                value: None,
                amount: None,
            }],
            value_components: vec![JimpleValueComponentBinding {
                field: quantity,
                component: String::from("quantity"),
            }],
            pure_values: vec![JimplePureValueBinding {
                callee: capacity_sig.clone(),
                operation: JimplePureValueOperation::Component {
                    component: String::from("capacity"),
                },
            }],
            store_queries: vec![JimpleIndexedStoreQueryBinding {
                callee: extent_sig.clone(),
                operation: JimpleIndexedStoreQueryKind::Extent,
                store: JimpleCallOperandBinding::Receiver,
                key: Vec::new(),
                value: None,
                context: Vec::new(),
            }],
            state_writes: vec![JimpleObjectStateWriteBinding {
                callee: state_sig.clone(),
                target: JimpleCallOperandBinding::Receiver,
                state: String::from("ready"),
                value: JimpleCallOperandBinding::Argument { index: 0 },
            }],
            ..JimpleTransactionBindings::default()
        }
    }

    #[test]
    fn transaction_coverage_accounts_bound_semantic_surface() -> Result<()> {
        let signatures = semantic_surface_signatures();
        let (summary, quantity) = semantic_surface_summary(&signatures);
        let bindings = semantic_surface_bindings(&signatures, quantity);
        let coverage = summary.indexed_store_transaction_coverage(&bindings)?;
        assert!(coverage.complete);
        assert_eq!(coverage.operations.len(), 1);
        assert_eq!(coverage.component_updates.len(), 1);
        assert_eq!(coverage.value_operations.len(), 1);
        assert_eq!(coverage.store_queries.len(), 1);
        assert_eq!(coverage.state_writes.len(), 1);
        assert_eq!(coverage.unaccounted_field_writes.len(), 0);
        assert_eq!(coverage.unaccounted_calls.len(), 0);
        Ok(())
    }

    #[test]
    fn uniform_int_binding_preserves_stream_and_bound() -> Result<()> {
        let callee = JimpleMethodSignature {
            owner: String::from("demo.Random"),
            name: String::from("draw"),
            return_type: String::from("int"),
            parameter_types: vec![String::from("int")],
        };
        let stream = SymbolicValue::Parameter {
            index: 4,
            type_name: String::from("demo.Random"),
        };
        let summary = JimpleProgramEffectSummary {
            entry: JimpleMethodSignature {
                owner: String::from("demo.Handler"),
                name: String::from("run"),
                return_type: String::from("void"),
                parameter_types: Vec::new(),
            },
            possible_field_writes: Vec::new(),
            calls: vec![JimpleProgramCall {
                call_path: vec![JimpleEffectCallFrame {
                    caller: JimpleMethodSignature {
                        owner: String::from("demo.Handler"),
                        name: String::from("run"),
                        return_type: String::from("void"),
                        parameter_types: Vec::new(),
                    },
                    statement_id: 7,
                    invoke_kind: String::from("JVirtualInvokeExpr"),
                    callee: callee.clone(),
                }],
                guards: Vec::new(),
                receiver: Some(stream.clone()),
                arguments: vec![SymbolicValue::Integer {
                    value: 11,
                    type_name: String::from("int"),
                }],
                resolution: JimpleProgramCallResolution::DynamicDispatch,
            }],
            induction_loops: Vec::new(),
            unresolved_calls: Vec::new(),
        };
        let candidates = summary.bind_uniform_int(&JimpleUniformIntBinding {
            callee,
            bound_argument: 0,
        })?;
        let [candidate] = candidates.as_slice() else {
            bail!("expected one uniform draw candidate");
        };
        assert_eq!(candidate.stream, Some(stream));
        assert!(matches!(
            candidate.bound_exclusive,
            SymbolicValue::Integer { value: 11, .. }
        ));
        Ok(())
    }

    #[test]
    fn leaves_ambiguous_virtual_dispatch_explicit() -> Result<()> {
        let this_local = local("this", "demo.Caller");
        let target_local = local("target", "demo.Base");
        let target_field = field("demo.Caller", "target", "demo.Base");
        let caller = method(
            "demo.Caller",
            "run",
            vec![
                statement(
                    0,
                    vec![1],
                    JimpleOperation::Identity {
                        left: this_local.clone(),
                        right: value(JimpleValueKind::This, "demo.Caller", "@this"),
                    },
                ),
                statement(
                    1,
                    vec![2],
                    JimpleOperation::Assign {
                        left: target_local.clone(),
                        right: instance_field(this_local, &target_field),
                    },
                ),
                statement(
                    2,
                    vec![3],
                    JimpleOperation::Invoke {
                        expression: Some(invoke(
                            target_local,
                            "demo.Base",
                            "mutate",
                            Vec::new(),
                            Vec::new(),
                        )),
                    },
                ),
                statement(3, Vec::new(), JimpleOperation::ReturnVoid),
            ],
        );
        let mut base_method = method(
            "demo.Base",
            "mutate",
            vec![statement(0, Vec::new(), JimpleOperation::ReturnVoid)],
        );
        base_method.signature.return_type = String::from("java.lang.Object");
        let entry = caller.signature.clone();
        let effects = JimpleProgram::new(vec![caller, base_method]).structural_effects(&entry)?;
        assert_eq!(effects.unresolved_calls.len(), 1);
        assert_eq!(effects.calls.len(), 1);
        assert_eq!(
            effects.calls[0].resolution,
            JimpleProgramCallResolution::DynamicDispatch
        );
        assert_eq!(
            effects.unresolved_calls[0].reason,
            JimpleUnresolvedEffectReason::DynamicDispatch
        );
        Ok(())
    }

    #[test]
    fn executes_exact_method_with_abstract_arguments() -> Result<()> {
        let argument = local("argument", "int");
        let signature = JimpleMethodSignature {
            owner: String::from("demo.Identity"),
            name: String::from("apply"),
            return_type: String::from("int"),
            parameter_types: vec![String::from("int")],
        };
        let method = JimpleMethod {
            signature: signature.clone(),
            display_signature: String::from("<demo.Identity: int apply(int)>"),
            declaring_superclass: None,
            declaring_interfaces: Vec::new(),
            statements: vec![
                statement(
                    0,
                    vec![1],
                    JimpleOperation::Identity {
                        left: argument.clone(),
                        right: value(
                            JimpleValueKind::Parameter { index: 0 },
                            "int",
                            "@parameter0",
                        ),
                    },
                ),
                statement(1, Vec::new(), JimpleOperation::Return { value: argument }),
            ],
        };
        let program = JimpleProgram::new(vec![method]);
        let (returned, execution) = program.execute_call(
            &signature,
            None,
            vec![AbstractValue::Integer { value: 7 }],
            100,
        )?;
        assert_eq!(returned, Some(AbstractValue::Integer { value: 7 }));
        assert_eq!(execution.steps, 2);
        Ok(())
    }

    #[test]
    fn dispatches_interface_call_to_runtime_class() -> Result<()> {
        let service_field = field("demo.Registry", "SERVICE", "demo.Api");
        let result_field = field("demo.Registry", "RESULT", "demo.Value");
        let service = local("service", "demo.Impl");
        let result = local("result", "demo.Value");

        let clinit = method(
            "demo.Registry",
            "<clinit>",
            vec![
                statement(
                    0,
                    vec![1],
                    JimpleOperation::Assign {
                        left: service.clone(),
                        right: new_object("demo.Impl"),
                    },
                ),
                statement(
                    1,
                    vec![2],
                    JimpleOperation::Assign {
                        left: static_field(&service_field),
                        right: service,
                    },
                ),
                statement(2, Vec::new(), JimpleOperation::ReturnVoid),
            ],
        );

        let impl_method = JimpleMethod {
            signature: JimpleMethodSignature {
                owner: String::from("demo.Impl"),
                name: String::from("make"),
                return_type: String::from("demo.Value"),
                parameter_types: Vec::new(),
            },
            display_signature: String::from("<demo.Impl: demo.Value make()>"),
            declaring_superclass: None,
            declaring_interfaces: Vec::new(),
            statements: vec![
                statement(
                    0,
                    vec![1],
                    JimpleOperation::Assign {
                        left: result.clone(),
                        right: new_object("demo.Value"),
                    },
                ),
                statement(1, Vec::new(), JimpleOperation::Return { value: result }),
            ],
        };

        let interface_call = value(
            JimpleValueKind::Invoke {
                invoke_kind: String::from("JInterfaceInvokeExpr"),
                method: JimpleMethodSignature {
                    owner: String::from("demo.Api"),
                    name: String::from("make"),
                    return_type: String::from("demo.Value"),
                    parameter_types: Vec::new(),
                },
                base: Some(Box::new(static_field(&service_field))),
                arguments: Vec::new(),
                bootstrap_method: None,
                bootstrap_arguments: Vec::new(),
            },
            "demo.Value",
            "interfaceinvoke service.make()",
        );
        let caller = method(
            "demo.Registry",
            "run",
            vec![
                statement(
                    0,
                    vec![1],
                    JimpleOperation::Assign {
                        left: static_field(&result_field),
                        right: interface_call,
                    },
                ),
                statement(1, Vec::new(), JimpleOperation::ReturnVoid),
            ],
        );

        let execution = JimpleProgram::new(vec![clinit, impl_method, caller])
            .execute_named_sequence(
                &[("demo.Registry", "<clinit>"), ("demo.Registry", "run")],
                100,
            )?;
        let result = execution
            .static_field_value("demo.Registry", "RESULT")
            .context("runtime-dispatched result missing")?;
        assert!(matches!(
            result,
            AbstractValue::ObjectRef { class_type, .. } if class_type == "demo.Value"
        ));
        Ok(())
    }

    #[test]
    fn models_finite_list_iteration() -> Result<()> {
        let program = JimpleProgram::default();
        let mut vm = ProgramVm::new(&program, 100);
        let list = vm.allocate_list(
            "java.util.ArrayList",
            vec![
                AbstractValue::Integer { value: 3 },
                AbstractValue::Integer { value: 5 },
            ],
        );
        let iterator_method = JimpleMethodSignature {
            owner: String::from("java.util.List"),
            name: String::from("iterator"),
            return_type: String::from("java.util.Iterator"),
            parameter_types: Vec::new(),
        };
        let iterator = vm
            .invoke_list(&iterator_method, Some(&list), &[])?
            .context("iterator missing")?;
        let has_next = JimpleMethodSignature {
            owner: String::from("java.util.Iterator"),
            name: String::from("hasNext"),
            return_type: String::from("boolean"),
            parameter_types: Vec::new(),
        };
        let next = JimpleMethodSignature {
            owner: String::from("java.util.Iterator"),
            name: String::from("next"),
            return_type: String::from("java.lang.Object"),
            parameter_types: Vec::new(),
        };

        assert_eq!(
            vm.invoke_iterator(&has_next, Some(&iterator), &[]),
            Some(AbstractValue::Integer { value: 1 })
        );
        assert_eq!(
            vm.invoke_iterator(&next, Some(&iterator), &[]),
            Some(AbstractValue::Integer { value: 3 })
        );
        assert_eq!(
            vm.invoke_iterator(&next, Some(&iterator), &[]),
            Some(AbstractValue::Integer { value: 5 })
        );
        assert_eq!(
            vm.invoke_iterator(&has_next, Some(&iterator), &[]),
            Some(AbstractValue::Integer { value: 0 })
        );
        Ok(())
    }

    #[test]
    fn models_finite_set_membership_and_iteration() -> Result<()> {
        let program = JimpleProgram::default();
        let mut vm = ProgramVm::new(&program, 100);
        let set = vm.allocate_object("it.unimi.dsi.fastutil.objects.ObjectOpenHashSet");
        let add = JimpleMethodSignature {
            owner: String::from("java.util.Set"),
            name: String::from("add"),
            return_type: String::from("boolean"),
            parameter_types: vec![String::from("java.lang.Object")],
        };
        let contains = JimpleMethodSignature {
            owner: String::from("java.util.Set"),
            name: String::from("contains"),
            return_type: String::from("boolean"),
            parameter_types: vec![String::from("java.lang.Object")],
        };
        let iterator = JimpleMethodSignature {
            owner: String::from("java.util.Set"),
            name: String::from("iterator"),
            return_type: String::from("java.util.Iterator"),
            parameter_types: Vec::new(),
        };

        assert_eq!(
            vm.invoke_set(&add, Some(&set), &[AbstractValue::Integer { value: 7 }]),
            Some(AbstractValue::Integer { value: 1 })
        );
        assert_eq!(
            vm.invoke_set(
                &contains,
                Some(&set),
                &[AbstractValue::Integer { value: 7 }]
            ),
            Some(AbstractValue::Integer { value: 1 })
        );
        let iterator = vm
            .invoke_set(&iterator, Some(&set), &[])
            .context("set iterator missing")?;
        let next = JimpleMethodSignature {
            owner: String::from("java.util.Iterator"),
            name: String::from("next"),
            return_type: String::from("java.lang.Object"),
            parameter_types: Vec::new(),
        };
        assert_eq!(
            vm.invoke_iterator(&next, Some(&iterator), &[]),
            Some(AbstractValue::Integer { value: 7 })
        );
        Ok(())
    }

    #[test]
    fn models_guava_collection_factories_without_project_rules() -> Result<()> {
        let program = JimpleProgram::default();
        let mut vm = ProgramVm::new(&program, 100);
        let list_factory = JimpleMethodSignature {
            owner: String::from("com.google.common.collect.Lists"),
            name: String::from("newArrayList"),
            return_type: String::from("java.util.ArrayList"),
            parameter_types: Vec::new(),
        };
        let map_factory = JimpleMethodSignature {
            owner: String::from("com.google.common.collect.Maps"),
            name: String::from("newHashMapWithExpectedSize"),
            return_type: String::from("java.util.HashMap"),
            parameter_types: vec![String::from("int")],
        };

        let list = vm
            .invoke_collection_factory(&list_factory, &[])
            .context("list factory missing")?;
        let map = vm
            .invoke_collection_factory(&map_factory, &[AbstractValue::Integer { value: 96 }])
            .context("map factory missing")?;
        let AbstractValue::ObjectRef { id: list_id, .. } = list else {
            bail!("list factory did not allocate an object");
        };
        let AbstractValue::ObjectRef { id: map_id, .. } = map else {
            bail!("map factory did not allocate an object");
        };
        assert_eq!(vm.lists.get(&list_id), Some(&Vec::new()));
        assert_eq!(vm.maps.get(&map_id), Some(&Vec::new()));
        Ok(())
    }

    #[test]
    fn records_unresolved_call_shapes_with_receiver_provenance() -> Result<()> {
        let target_field = field("demo.Registry", "TARGET", "demo.Api");
        let call = invoke(
            static_field(&target_field),
            "demo.Api",
            "submit",
            vec!["int"],
            vec![integer(7)],
        );
        let run = method(
            "demo.Registry",
            "run",
            vec![
                statement(
                    0,
                    vec![1],
                    JimpleOperation::Invoke {
                        expression: Some(call),
                    },
                ),
                statement(1, Vec::new(), JimpleOperation::ReturnVoid),
            ],
        );

        let execution = JimpleProgram::new(vec![run])
            .execute_named_sequence(&[("demo.Registry", "run")], 100)?;
        let [shape] = execution.opaque_call_shapes.as_slice() else {
            bail!("expected exactly one opaque call shape")
        };
        assert_eq!(shape.method.owner, "demo.Api");
        assert_eq!(shape.method.name, "submit");
        assert_eq!(
            shape.base,
            Some(AbstractValue::StaticField {
                field: target_field
            })
        );
        assert_eq!(shape.arguments, vec![AbstractValue::Integer { value: 7 }]);
        Ok(())
    }

    fn registry_lookup_method(
        map_field: &super::super::JimpleFieldSignature,
    ) -> (JimpleMethodSignature, JimpleMethod) {
        let key = local("key", "int");
        let result = local("result", "java.lang.Object");
        let signature = JimpleMethodSignature {
            owner: String::from("demo.Registry"),
            name: String::from("lookup"),
            return_type: String::from("java.lang.Object"),
            parameter_types: vec![String::from("int")],
        };
        let method = JimpleMethod {
            signature: signature.clone(),
            display_signature: String::from("<demo.Registry: java.lang.Object lookup(int)>"),
            declaring_superclass: None,
            declaring_interfaces: Vec::new(),
            statements: vec![
                statement(
                    0,
                    vec![1],
                    JimpleOperation::Identity {
                        left: key.clone(),
                        right: value(
                            JimpleValueKind::Parameter { index: 0 },
                            "int",
                            "@parameter0",
                        ),
                    },
                ),
                statement(
                    1,
                    vec![2],
                    JimpleOperation::Assign {
                        left: result.clone(),
                        right: invoke(
                            static_field(map_field),
                            "java.util.HashMap",
                            "get",
                            vec!["java.lang.Object"],
                            vec![key],
                        ),
                    },
                ),
                statement(2, Vec::new(), JimpleOperation::Return { value: result }),
            ],
        };
        (signature, method)
    }

    #[test]
    fn startup_registry_state_flows_into_followup_helper_call() -> Result<()> {
        let map_field = field("demo.Registry", "MAP", "java.util.HashMap");
        let map = local("map", "java.util.HashMap");
        let clinit = method(
            "demo.Registry",
            "<clinit>",
            vec![
                statement(
                    0,
                    vec![1],
                    JimpleOperation::Assign {
                        left: map.clone(),
                        right: new_object("java.util.HashMap"),
                    },
                ),
                statement(
                    1,
                    vec![2],
                    JimpleOperation::Assign {
                        left: static_field(&map_field),
                        right: map,
                    },
                ),
                statement(2, Vec::new(), JimpleOperation::ReturnVoid),
            ],
        );
        let register = method(
            "demo.Registry",
            "register",
            vec![
                statement(
                    0,
                    vec![1],
                    JimpleOperation::Invoke {
                        expression: Some(invoke(
                            static_field(&map_field),
                            "java.util.HashMap",
                            "put",
                            vec!["java.lang.Object", "java.lang.Object"],
                            vec![integer(7), integer(9)],
                        )),
                    },
                ),
                statement(1, Vec::new(), JimpleOperation::ReturnVoid),
            ],
        );
        let (signature, lookup) = registry_lookup_method(&map_field);

        let program = JimpleProgram::new(vec![clinit, register, lookup]);
        let (returned, execution) = program.execute_named_sequence_then_call(
            &[("demo.Registry", "register")],
            &signature,
            None,
            vec![AbstractValue::Integer { value: 7 }],
            100,
        )?;

        assert_eq!(returned, Some(AbstractValue::Integer { value: 9 }));
        assert_eq!(execution.maps.len(), 1);
        assert_eq!(
            execution.initialized_classes,
            vec![
                String::from("demo.Registry"),
                String::from("java.util.HashMap")
            ]
        );
        Ok(())
    }

    #[test]
    fn materializes_static_map_with_static_object_key_provenance() -> Result<()> {
        let map_field = field("demo.Registry", "MAP", "java.util.HashMap");
        let key_field = field("demo.Keys", "ALPHA", "demo.Key");
        let map = local("map", "java.util.HashMap");
        let key = local("key", "demo.Key");
        let item = local("item", "demo.Item");

        let clinit = method(
            "demo.Registry",
            "<clinit>",
            vec![
                statement(
                    0,
                    vec![1],
                    JimpleOperation::Assign {
                        left: map.clone(),
                        right: new_object("java.util.HashMap"),
                    },
                ),
                statement(
                    1,
                    vec![2],
                    JimpleOperation::Assign {
                        left: static_field(&map_field),
                        right: map,
                    },
                ),
                statement(
                    2,
                    vec![3],
                    JimpleOperation::Assign {
                        left: key.clone(),
                        right: new_object("demo.Key"),
                    },
                ),
                statement(
                    3,
                    vec![4],
                    JimpleOperation::Assign {
                        left: static_field(&key_field),
                        right: key,
                    },
                ),
                statement(4, Vec::new(), JimpleOperation::ReturnVoid),
            ],
        );

        let register = method(
            "demo.Registry",
            "register",
            vec![
                statement(
                    0,
                    vec![1],
                    JimpleOperation::Assign {
                        left: item.clone(),
                        right: new_object("demo.Item"),
                    },
                ),
                statement(
                    1,
                    vec![2],
                    JimpleOperation::Invoke {
                        expression: Some(invoke(
                            static_field(&map_field),
                            "java.util.HashMap",
                            "put",
                            vec!["java.lang.Object", "java.lang.Object"],
                            vec![static_field(&key_field), item],
                        )),
                    },
                ),
                statement(2, Vec::new(), JimpleOperation::ReturnVoid),
            ],
        );

        let execution = JimpleProgram::new(vec![clinit, register]).execute_named_sequence(
            &[("demo.Registry", "<clinit>"), ("demo.Registry", "register")],
            100,
        )?;
        let stored = execution
            .static_map_lookup("demo.Registry", "MAP", "demo.Keys", "ALPHA")
            .context("static map entry missing")?;
        let AbstractValue::ObjectRef { id, class_type } = stored else {
            bail!("expected mapped object, got {stored:?}");
        };
        assert_eq!(class_type, "demo.Item");
        assert_eq!(execution.static_object_fields(*id).len(), 0);

        let key = execution
            .static_field_value("demo.Keys", "ALPHA")
            .context("key static field missing")?;
        let AbstractValue::ObjectRef { id: key_id, .. } = key else {
            bail!("expected key object, got {key:?}");
        };
        assert_eq!(execution.static_object_fields(*key_id), vec![&key_field]);
        Ok(())
    }
}
