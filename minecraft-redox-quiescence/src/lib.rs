//! Adapter between source-neutral Redox transactions and Quiescence atom-state reactions.

mod dense_selector;

pub use dense_selector::{AffineBitSelector, AffineBitSelectorError, AffineBitSelectorSpec};

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use minecraft_redox_core::transaction::{
    MemoryTransactionHost, MemoryTransactionHostCall, MemoryTransactionHostQuery,
    MemoryTransactionObject, MemoryTransactionPerformedHostCall, MemoryTransactionScheduleOnce,
    MemoryTransactionState, MemoryTransactionStore, TransactionDatum, TransactionDraft,
    TransactionExecution, TransactionExecutionError, TransactionExecutionLimits, TransactionHost,
    TransactionProgram, TransactionStoreQueryKind, execute_transaction,
};
use quiescence_core::{NodeId, ReactionId, SimDuration, SimTime};
use quiescence_runtime::{
    EvalContext, EvalError, Graph, InteractionIngressError, InteractionSender, ReactionOutput,
    RuntimeError, StateWrite,
};
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

#[derive(Clone, Debug, Default)]
struct ScheduleKeyInterner {
    ids: BTreeMap<Vec<CanonicalDatumKey>, u64>,
    identities: BTreeMap<u64, Vec<TransactionDatum>>,
}

impl ScheduleKeyInterner {
    fn intern(
        &mut self,
        key: Vec<CanonicalDatumKey>,
        identity: Vec<TransactionDatum>,
    ) -> Result<u64, String> {
        if let Some(id) = self.ids.get(&key) {
            return Ok(*id);
        }
        let next = self
            .ids
            .len()
            .checked_add(1)
            .ok_or_else(|| String::from("schedule key count overflow"))?;
        let id = u64::try_from(next).map_err(|_| String::from("schedule key ID exceeds u64"))?;
        self.ids.insert(key, id);
        self.identities.insert(id, identity);
        Ok(id)
    }

    fn identity(&self, key: u64) -> Option<&[TransactionDatum]> {
        self.identities.get(&key).map(Vec::as_slice)
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

#[derive(Clone, Debug, Default)]
pub struct TransactionInvocation {
    pub receiver: Option<ReactionArgument>,
    pub arguments: Vec<ReactionArgument>,
    pub preparation: ReactionHostPreparation,
}

impl TransactionInvocation {
    #[must_use]
    pub fn new(arguments: Vec<ReactionArgument>) -> Self {
        Self {
            arguments,
            ..Self::default()
        }
    }

    #[must_use]
    pub fn with_receiver(mut self, receiver: ReactionArgument) -> Self {
        self.receiver = Some(receiver);
        self
    }

    #[must_use]
    pub fn with_preparation(mut self, preparation: ReactionHostPreparation) -> Self {
        self.preparation = preparation;
        self
    }
}

#[derive(Clone, Debug, Default)]
pub struct ReactionHostPreparation {
    /// Graph-backed stores discoverable during this transaction evaluation.
    ///
    /// These bindings are demand-materialized: their atom nodes are read only when execution first
    /// semantically inspects or mutates the corresponding local handle. Untaken branches therefore
    /// do not become graph dependencies.
    pub stores: Vec<ReactionStoreBinding>,
    pub objects: BTreeMap<u64, MemoryTransactionObject>,
    pub capabilities: BTreeMap<u64, BTreeSet<String>>,
    pub host_queries: Vec<MemoryTransactionHostQuery>,
    pub host_calls: Vec<MemoryTransactionHostCall>,
}

#[derive(Debug)]
enum DemandHostError {
    NeedStore(u64),
    Inner(String),
}

impl fmt::Display for DemandHostError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NeedStore(handle) => {
                write!(formatter, "transaction requires lazy store {handle}")
            }
            Self::Inner(message) => formatter.write_str(message),
        }
    }
}

impl Error for DemandHostError {}

struct DemandTransactionHost {
    inner: MemoryTransactionHost,
    unresolved_stores: BTreeSet<u64>,
}

struct DemandTransactionDraft<'a> {
    inner: <MemoryTransactionHost as TransactionHost>::Draft<'a>,
    unresolved_stores: &'a BTreeSet<u64>,
}

impl TransactionHost for DemandTransactionHost {
    type Error = DemandHostError;
    type Draft<'a> = DemandTransactionDraft<'a>;

    fn begin(&mut self) -> Result<Self::Draft<'_>, Self::Error> {
        let unresolved_stores = &self.unresolved_stores;
        let inner = self.inner.begin().map_err(DemandHostError::Inner)?;
        Ok(DemandTransactionDraft {
            inner,
            unresolved_stores,
        })
    }
}

impl DemandTransactionDraft<'_> {
    fn require_resolved(&self, value: &TransactionDatum) -> Result<(), DemandHostError> {
        if let TransactionDatum::Handle(handle) = value
            && self.unresolved_stores.contains(handle)
        {
            return Err(DemandHostError::NeedStore(*handle));
        }
        Ok(())
    }
}

impl TransactionDraft for DemandTransactionDraft<'_> {
    type Error = DemandHostError;

    fn has_capability(
        &self,
        value: &TransactionDatum,
        capability: &str,
    ) -> Result<bool, Self::Error> {
        self.require_resolved(value)?;
        self.inner
            .has_capability(value, capability)
            .map_err(DemandHostError::Inner)
    }

    fn clone_value(&mut self, value: &TransactionDatum) -> Result<TransactionDatum, Self::Error> {
        self.inner
            .clone_value(value)
            .map_err(DemandHostError::Inner)
    }

    fn component(
        &self,
        value: &TransactionDatum,
        component: &str,
    ) -> Result<TransactionDatum, Self::Error> {
        self.inner
            .component(value, component)
            .map_err(DemandHostError::Inner)
    }

    fn set_component(
        &mut self,
        value: &TransactionDatum,
        component: &str,
        new_value: TransactionDatum,
    ) -> Result<(), Self::Error> {
        self.inner
            .set_component(value, component, new_value)
            .map_err(DemandHostError::Inner)
    }

    fn semantic_equal(
        &self,
        left: &TransactionDatum,
        right: &TransactionDatum,
    ) -> Result<bool, Self::Error> {
        self.inner
            .semantic_equal(left, right)
            .map_err(DemandHostError::Inner)
    }

    fn host_query(
        &mut self,
        operation: &str,
        receiver: Option<&TransactionDatum>,
        arguments: &[TransactionDatum],
    ) -> Result<TransactionDatum, Self::Error> {
        self.inner
            .host_query(operation, receiver, arguments)
            .map_err(DemandHostError::Inner)
    }

    fn host_call(
        &mut self,
        operation: &str,
        receiver: Option<&TransactionDatum>,
        arguments: &[TransactionDatum],
    ) -> Result<TransactionDatum, Self::Error> {
        self.inner
            .host_call(operation, receiver, arguments)
            .map_err(DemandHostError::Inner)
    }

    fn store_read(
        &self,
        store: &TransactionDatum,
        key: &[TransactionDatum],
    ) -> Result<TransactionDatum, Self::Error> {
        self.require_resolved(store)?;
        self.inner
            .store_read(store, key)
            .map_err(DemandHostError::Inner)
    }

    fn store_take(
        &mut self,
        store: &TransactionDatum,
        key: &[TransactionDatum],
        amount: &TransactionDatum,
    ) -> Result<TransactionDatum, Self::Error> {
        self.require_resolved(store)?;
        self.inner
            .store_take(store, key, amount)
            .map_err(DemandHostError::Inner)
    }

    fn store_write(
        &mut self,
        store: &TransactionDatum,
        key: &[TransactionDatum],
        value: TransactionDatum,
    ) -> Result<(), Self::Error> {
        self.require_resolved(store)?;
        self.inner
            .store_write(store, key, value)
            .map_err(DemandHostError::Inner)
    }

    fn store_touch(&mut self, store: &TransactionDatum) -> Result<(), Self::Error> {
        self.require_resolved(store)?;
        self.inner
            .store_touch(store)
            .map_err(DemandHostError::Inner)
    }

    fn store_query(
        &self,
        operation: TransactionStoreQueryKind,
        store: &TransactionDatum,
        key: &[TransactionDatum],
        value: Option<&TransactionDatum>,
        context: &[TransactionDatum],
    ) -> Result<TransactionDatum, Self::Error> {
        self.require_resolved(store)?;
        self.inner
            .store_query(operation, store, key, value, context)
            .map_err(DemandHostError::Inner)
    }

    fn schedule_once(
        &mut self,
        key: &[TransactionDatum],
        delay: &TransactionDatum,
        priority: &TransactionDatum,
    ) -> Result<(), Self::Error> {
        self.inner
            .schedule_once(key, delay, priority)
            .map_err(DemandHostError::Inner)
    }

    fn state_write(
        &mut self,
        target: &TransactionDatum,
        state: &str,
        value: TransactionDatum,
    ) -> Result<(), Self::Error> {
        self.inner
            .state_write(target, state, value)
            .map_err(DemandHostError::Inner)
    }

    fn commit(self) -> Result<(), Self::Error> {
        self.inner.commit().map_err(DemandHostError::Inner)
    }
}

type HostPreparer = dyn for<'a> Fn(&EvalContext<'a>) -> Result<ReactionHostPreparation, EvalError>
    + Send
    + Sync
    + 'static;
type HostCallPublisher = dyn Fn(&MemoryTransactionPerformedHostCall) -> Result<Vec<StateWrite>, EvalError>
    + Send
    + Sync
    + 'static;

#[derive(Debug)]
pub enum TransactionInteractionError {
    PayloadLockPoisoned,
    TokenExhausted,
    UnknownToken(u64),
    Ingress(InteractionIngressError),
}

impl fmt::Display for TransactionInteractionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PayloadLockPoisoned => {
                f.write_str("transaction interaction payload lock poisoned")
            }
            Self::TokenExhausted => f.write_str("transaction interaction token space exhausted"),
            Self::UnknownToken(token) => write!(f, "unknown transaction interaction token {token}"),
            Self::Ingress(source) => write!(f, "interaction ingress failed: {source}"),
        }
    }
}
impl Error for TransactionInteractionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Ingress(source) => Some(source),
            Self::PayloadLockPoisoned | Self::TokenExhausted | Self::UnknownToken(_) => None,
        }
    }
}
impl From<InteractionIngressError> for TransactionInteractionError {
    fn from(value: InteractionIngressError) -> Self {
        Self::Ingress(value)
    }
}

#[derive(Clone)]
pub struct TransactionInteractionSender {
    sender: InteractionSender,
    reaction: ReactionId,
    payloads: Arc<Mutex<BTreeMap<u64, TransactionInvocation>>>,
    next_token: Arc<AtomicU64>,
}

impl TransactionInteractionSender {
    fn reserve(
        &self,
        invocation: TransactionInvocation,
    ) -> Result<u64, TransactionInteractionError> {
        let token = self
            .next_token
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .map_err(|_| TransactionInteractionError::TokenExhausted)?;
        self.payloads
            .lock()
            .map_err(|_| TransactionInteractionError::PayloadLockPoisoned)?
            .insert(token, invocation);
        Ok(token)
    }

    fn rollback_reservation(&self, token: u64) {
        if let Ok(mut payloads) = self.payloads.lock() {
            payloads.remove(&token);
        }
    }

    /// Enqueue one transaction invocation at the graph owner's next quiescent boundary.
    ///
    /// # Errors
    /// Returns an error if payload storage is unavailable, the token space is exhausted, or the
    /// underlying Quiescence ingress queue rejects the submission.
    pub fn submit_now(
        &self,
        priority: i32,
        invocation: TransactionInvocation,
    ) -> Result<u64, TransactionInteractionError> {
        let token = self.reserve(invocation)?;
        if let Err(error) = self
            .sender
            .callback_now_with_token(self.reaction, priority, token)
        {
            self.rollback_reservation(token);
            return Err(error.into());
        }
        Ok(token)
    }

    /// Enqueue one transaction invocation at an explicit simulation timestamp.
    ///
    /// # Errors
    /// Returns an error if payload storage is unavailable, the token space is exhausted, or the
    /// underlying Quiescence ingress queue rejects the submission. A timestamp that has become
    /// causally stale is reported later by the graph owner when it imports the interaction.
    pub fn submit_at(
        &self,
        at: SimTime,
        priority: i32,
        invocation: TransactionInvocation,
    ) -> Result<u64, TransactionInteractionError> {
        let token = self.reserve(invocation)?;
        if let Err(error) = self
            .sender
            .callback_at_with_token(self.reaction, at, priority, token)
        {
            self.rollback_reservation(token);
            return Err(error.into());
        }
        Ok(token)
    }

    /// Re-enqueue a retained failed invocation at the next quiescent boundary.
    ///
    /// # Errors
    /// Returns an error for an unknown token, unavailable payload storage, or ingress failure.
    pub fn retry_now(&self, token: u64, priority: i32) -> Result<(), TransactionInteractionError> {
        if !self
            .payloads
            .lock()
            .map_err(|_| TransactionInteractionError::PayloadLockPoisoned)?
            .contains_key(&token)
        {
            return Err(TransactionInteractionError::UnknownToken(token));
        }
        self.sender
            .callback_now_with_token(self.reaction, priority, token)
            .map_err(Into::into)
    }

    /// Re-enqueue a retained failed invocation at an explicit simulation timestamp.
    ///
    /// # Errors
    /// Returns an error for an unknown token, unavailable payload storage, or ingress failure.
    pub fn retry_at(
        &self,
        token: u64,
        at: SimTime,
        priority: i32,
    ) -> Result<(), TransactionInteractionError> {
        if !self
            .payloads
            .lock()
            .map_err(|_| TransactionInteractionError::PayloadLockPoisoned)?
            .contains_key(&token)
        {
            return Err(TransactionInteractionError::UnknownToken(token));
        }
        self.sender
            .callback_at_with_token(self.reaction, at, priority, token)
            .map_err(Into::into)
    }

    /// Discard a retained invocation payload.
    ///
    /// # Errors
    /// Returns an error if the payload registry lock is poisoned.
    pub fn discard(&self, token: u64) -> Result<bool, TransactionInteractionError> {
        Ok(self
            .payloads
            .lock()
            .map_err(|_| TransactionInteractionError::PayloadLockPoisoned)?
            .remove(&token)
            .is_some())
    }

    /// Return the number of submitted invocations not yet committed successfully or discarded.
    ///
    /// # Errors
    /// Returns an error if the payload registry lock is poisoned.
    pub fn pending_payload_count(&self) -> Result<usize, TransactionInteractionError> {
        Ok(self
            .payloads
            .lock()
            .map_err(|_| TransactionInteractionError::PayloadLockPoisoned)?
            .len())
    }
}

#[derive(Clone)]
pub struct TransactionReactionAdapter {
    program: Arc<TransactionProgram>,
    interner: Arc<Mutex<SnapshotInterner>>,
    stores: Vec<ReactionStoreBinding>,
    arguments: Vec<ReactionArgument>,
    receiver: Option<ReactionArgument>,
    quantity_component: String,
    host_preparer: Option<Arc<HostPreparer>>,
    host_call_publisher: Option<Arc<HostCallPublisher>>,
    schedule_unit: Option<SimDuration>,
    schedule_identity_component: Option<String>,
    schedule_keys: Arc<Mutex<ScheduleKeyInterner>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TransactionScheduleRequest {
    pub key: u64,
    pub after: SimDuration,
    pub priority: i32,
}

#[derive(Clone, Debug)]
pub struct TransactionReactionResult {
    pub execution: TransactionExecution,
    pub writes: Vec<StateWrite>,
    pub state_values: BTreeMap<u64, BTreeMap<String, TransactionDatum>>,
    pub schedule_requests: Vec<TransactionScheduleRequest>,
    pub performed_host_calls: Vec<MemoryTransactionPerformedHostCall>,
}

impl TransactionReactionResult {
    /// Convert committed writes plus source schedule requests into one graph transition whose
    /// keyed callbacks target `callback_reaction` rather than re-running the producer reaction.
    #[must_use]
    pub fn into_reaction_output_for_callback(
        self,
        callback_reaction: ReactionId,
    ) -> ReactionOutput {
        let mut output = if self.writes.is_empty() {
            ReactionOutput::idle()
        } else {
            ReactionOutput::immediate(self.writes)
        };
        for request in self.schedule_requests {
            output = output.and_schedule_reaction_once_after(
                callback_reaction,
                request.after,
                request.key,
                request.priority,
            );
        }
        output
    }
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
            host_preparer: None,
            host_call_publisher: None,
            schedule_unit: None,
            schedule_identity_component: None,
            schedule_keys: Arc::new(Mutex::new(ScheduleKeyInterner::default())),
        }
    }

    #[must_use]
    pub fn with_host_preparer<F>(mut self, preparer: F) -> Self
    where
        F: for<'a> Fn(&EvalContext<'a>) -> Result<ReactionHostPreparation, EvalError>
            + Send
            + Sync
            + 'static,
    {
        self.host_preparer = Some(Arc::new(preparer));
        self
    }

    #[must_use]
    pub fn with_host_call_publisher<F>(mut self, publisher: F) -> Self
    where
        F: Fn(&MemoryTransactionPerformedHostCall) -> Result<Vec<StateWrite>, EvalError>
            + Send
            + Sync
            + 'static,
    {
        self.host_call_publisher = Some(Arc::new(publisher));
        self
    }

    #[must_use]
    pub fn with_schedule_unit(mut self, unit: SimDuration) -> Self {
        self.schedule_unit = Some(unit);
        self
    }

    #[must_use]
    pub fn with_schedule_identity_component(mut self, component: impl Into<String>) -> Self {
        self.schedule_identity_component = Some(component.into());
        self
    }

    /// Recover the stable source identity associated with an interned keyed callback.
    ///
    /// # Errors
    /// Returns an error for an unknown key or a poisoned key interner.
    pub fn schedule_identity(&self, key: u64) -> Result<Vec<TransactionDatum>, EvalError> {
        let keys = self
            .schedule_keys
            .lock()
            .map_err(|_| EvalError::Message(String::from("schedule-key interner lock poisoned")))?;
        keys.identity(key)
            .map(<[TransactionDatum]>::to_vec)
            .ok_or_else(|| EvalError::Message(format!("unknown schedule key {key}")))
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
        let preparation = self.host_preparer.as_ref().map_or_else(
            || Ok(ReactionHostPreparation::default()),
            |prepare| prepare(context),
        )?;
        execute_prepared_transaction(
            self,
            context,
            &preparation,
            InvocationBindings {
                receiver: self.receiver.as_ref(),
                arguments: &self.arguments,
            },
        )
    }

    /// Execute one event-local invocation using this adapter's graph/store configuration.
    ///
    /// # Errors
    /// Propagates graph reads, preparation conflicts, transaction execution failures, snapshot
    /// errors, schedule lowering errors, and host-publication errors as `EvalError`.
    pub fn execute_invocation(
        &self,
        context: &EvalContext<'_>,
        invocation: &TransactionInvocation,
    ) -> Result<TransactionReactionResult, EvalError> {
        let mut preparation = self.host_preparer.as_ref().map_or_else(
            || Ok(ReactionHostPreparation::default()),
            |prepare| prepare(context),
        )?;
        merge_preparation(&mut preparation, &invocation.preparation)?;
        execute_prepared_transaction(
            self,
            context,
            &preparation,
            InvocationBindings {
                receiver: invocation.receiver.as_ref(),
                arguments: &invocation.arguments,
            },
        )
    }
}

/// Install an event-only Quiescence callback that executes token-indexed transaction invocations.
///
/// # Errors
/// Returns a runtime error if the callback reaction cannot be installed in the graph.
pub fn install_transaction_interaction(
    graph: &mut Graph,
    sender: InteractionSender,
    name: impl Into<String>,
    adapter: TransactionReactionAdapter,
    schedule_target: Option<ReactionId>,
) -> Result<TransactionInteractionSender, RuntimeError> {
    let payloads = Arc::new(Mutex::new(BTreeMap::<u64, TransactionInvocation>::new()));
    let callback_payloads = Arc::clone(&payloads);
    let callback = graph.add_event_callback(name, move |context| {
        let token = context.external_token().ok_or_else(|| {
            EvalError::Message(String::from(
                "transaction interaction callback arrived without a payload token",
            ))
        })?;
        let invocation = callback_payloads
            .lock()
            .map_err(|_| {
                EvalError::Message(String::from(
                    "transaction interaction payload lock poisoned",
                ))
            })?
            .get(&token)
            .cloned()
            .ok_or_else(|| {
                EvalError::Message(format!("unknown transaction interaction token {token}"))
            })?;
        let result = adapter.execute_invocation(context, &invocation)?;
        let output = if result.schedule_requests.is_empty() {
            if result.writes.is_empty() {
                ReactionOutput::idle()
            } else {
                ReactionOutput::immediate(result.writes)
            }
        } else {
            let target = schedule_target.ok_or_else(|| {
                EvalError::Message(String::from(
                    "transaction interaction emitted schedule requests without a callback target",
                ))
            })?;
            result.into_reaction_output_for_callback(target)
        };
        callback_payloads
            .lock()
            .map_err(|_| {
                EvalError::Message(String::from(
                    "transaction interaction payload lock poisoned",
                ))
            })?
            .remove(&token);
        Ok(output)
    })?;
    Ok(TransactionInteractionSender {
        sender,
        reaction: callback,
        payloads,
        next_token: Arc::new(AtomicU64::new(1)),
    })
}

#[derive(Clone, Copy)]
struct InvocationBindings<'a> {
    receiver: Option<&'a ReactionArgument>,
    arguments: &'a [ReactionArgument],
}

fn merge_preparation(
    target: &mut ReactionHostPreparation,
    extra: &ReactionHostPreparation,
) -> Result<(), EvalError> {
    target.stores.extend(extra.stores.iter().copied());
    for (handle, object) in &extra.objects {
        if target.objects.insert(*handle, object.clone()).is_some() {
            return Err(EvalError::Message(format!(
                "duplicate prepared object handle {handle}"
            )));
        }
    }
    for (handle, capabilities) in &extra.capabilities {
        target
            .capabilities
            .entry(*handle)
            .or_default()
            .extend(capabilities.iter().cloned());
    }
    target
        .host_queries
        .extend(extra.host_queries.iter().cloned());
    target.host_calls.extend(extra.host_calls.iter().cloned());
    Ok(())
}

fn execute_prepared_transaction(
    adapter: &TransactionReactionAdapter,
    context: &EvalContext<'_>,
    preparation: &ReactionHostPreparation,
    invocation: InvocationBindings<'_>,
) -> Result<TransactionReactionResult, EvalError> {
    let mut all_stores = adapter.stores.clone();
    all_stores.extend(preparation.stores.iter().copied());
    validate_store_bindings(&all_stores).map_err(EvalError::Message)?;
    let mut resolved_stores = Vec::new();
    let mut snapshot_ids = Vec::new();
    let lazy_stores: BTreeMap<_, _> = all_stores
        .iter()
        .map(|binding| (binding.local_handle, *binding))
        .collect();
    let reserved_handle = preparation_reserved_handle(&all_stores, preparation);
    loop {
        match execute_resolved_transaction(
            adapter,
            &resolved_stores,
            &snapshot_ids,
            &lazy_stores,
            reserved_handle,
            preparation,
            invocation,
        )? {
            DemandExecution::Complete(result) => return Ok(result),
            DemandExecution::NeedStore(binding) => {
                snapshot_ids.push(context.read_atom(binding.node)?);
                resolved_stores.push(binding);
            }
        }
    }
}

enum DemandExecution {
    Complete(TransactionReactionResult),
    NeedStore(ReactionStoreBinding),
}

fn execute_resolved_transaction(
    adapter: &TransactionReactionAdapter,
    stores: &[ReactionStoreBinding],
    snapshot_ids: &[u64],
    lazy_stores: &BTreeMap<u64, ReactionStoreBinding>,
    reserved_handle: u64,
    preparation: &ReactionHostPreparation,
    invocation: InvocationBindings<'_>,
) -> Result<DemandExecution, EvalError> {
    let mut interner = adapter
        .interner
        .lock()
        .map_err(|_| EvalError::Message(String::from("snapshot interner lock poisoned")))?;
    let inner = build_host(
        &interner,
        stores,
        snapshot_ids,
        &adapter.quantity_component,
        reserved_handle,
    )
    .map_err(EvalError::Message)?;
    let resolved_handles: BTreeSet<_> = stores.iter().map(|binding| binding.local_handle).collect();
    let unresolved_stores = lazy_stores
        .keys()
        .filter(|handle| !resolved_handles.contains(handle))
        .copied()
        .collect();
    let mut host = DemandTransactionHost {
        inner,
        unresolved_stores,
    };
    apply_preparation(&mut host.inner.state, preparation.clone());
    let receiver = invocation
        .receiver
        .map(|argument| lower_argument(argument, &adapter.stores))
        .transpose()
        .map_err(EvalError::Message)?;
    let arguments = invocation
        .arguments
        .iter()
        .map(|argument| lower_argument(argument, &adapter.stores))
        .collect::<Result<Vec<_>, _>>()
        .map_err(EvalError::Message)?;
    match execute_transaction(
        &adapter.program,
        &mut host,
        receiver,
        arguments,
        TransactionExecutionLimits::default(),
    ) {
        Ok(execution) => {
            let state_values = host.inner.state.state_values.clone();
            let performed_host_calls = host.inner.state.performed_host_calls.clone();
            let mut writes = publish_stores(&mut interner, stores, snapshot_ids, &host.inner.state)
                .map_err(EvalError::Message)?;
            writes.extend(publish_host_calls(adapter, &performed_host_calls)?);
            let schedule_requests = lower_schedule_requests(
                adapter,
                &host.inner.state,
                &host.inner.state.scheduled_once,
            )?;
            Ok(DemandExecution::Complete(TransactionReactionResult {
                execution,
                writes,
                state_values,
                schedule_requests,
                performed_host_calls,
            }))
        }
        Err(TransactionExecutionError::Host(DemandHostError::NeedStore(handle))) => lazy_stores
            .get(&handle)
            .copied()
            .map(DemandExecution::NeedStore)
            .ok_or_else(|| {
                EvalError::Message(format!("transaction requested unknown lazy store {handle}"))
            }),
        Err(error) => Err(EvalError::Message(error.to_string())),
    }
}

fn publish_host_calls(
    adapter: &TransactionReactionAdapter,
    calls: &[MemoryTransactionPerformedHostCall],
) -> Result<Vec<StateWrite>, EvalError> {
    if calls.is_empty() {
        return Ok(Vec::new());
    }
    let publisher = adapter.host_call_publisher.as_ref().ok_or_else(|| {
        EvalError::Message(String::from(
            "transaction committed host calls without a configured host-call publisher",
        ))
    })?;
    let mut writes = Vec::new();
    for call in calls {
        writes.extend(publisher(call)?);
    }
    Ok(writes)
}

fn lower_schedule_requests(
    adapter: &TransactionReactionAdapter,
    state: &MemoryTransactionState,
    requests: &[MemoryTransactionScheduleOnce],
) -> Result<Vec<TransactionScheduleRequest>, EvalError> {
    if requests.is_empty() {
        return Ok(Vec::new());
    }
    let unit = adapter.schedule_unit.ok_or_else(|| {
        EvalError::Message(String::from(
            "transaction emitted schedule_once without a configured source-time unit",
        ))
    })?;
    let mut keys = adapter
        .schedule_keys
        .lock()
        .map_err(|_| EvalError::Message(String::from("schedule-key interner lock poisoned")))?;
    requests
        .iter()
        .map(|request| {
            if request.delay_units < 0 {
                return Err(EvalError::Message(format!(
                    "negative source schedule delay {} is not yet supported",
                    request.delay_units
                )));
            }
            let nanos = unit
                .as_nanos()
                .checked_mul(request.delay_units)
                .ok_or_else(|| EvalError::Message(String::from("schedule delay overflow")))?;
            let identity_parts = request
                .key
                .iter()
                .map(|value| stable_schedule_datum(adapter, state, value))
                .collect::<Result<Vec<_>, _>>()?;
            let canonical_key = identity_parts
                .iter()
                .map(|(canonical, _)| canonical.clone())
                .collect();
            let identity = identity_parts.into_iter().map(|(_, value)| value).collect();
            let key = keys
                .intern(canonical_key, identity)
                .map_err(EvalError::Message)?;
            Ok(TransactionScheduleRequest {
                key,
                after: SimDuration::from_nanos(nanos),
                priority: request.priority,
            })
        })
        .collect()
}

fn stable_schedule_datum(
    adapter: &TransactionReactionAdapter,
    state: &MemoryTransactionState,
    value: &TransactionDatum,
) -> Result<(CanonicalDatumKey, TransactionDatum), EvalError> {
    let stable = if let TransactionDatum::Handle(handle) = value {
        let component = adapter.schedule_identity_component.as_deref().ok_or_else(|| {
            EvalError::Message(format!(
                "schedule key contains ephemeral handle {handle} without a stable identity component"
            ))
        })?;
        state
            .objects
            .get(handle)
            .and_then(|object| object.components.get(component))
            .cloned()
            .ok_or_else(|| {
                EvalError::Message(format!(
                    "schedule key handle {handle} has no stable identity component {component:?}"
                ))
            })?
    } else {
        value.clone()
    };
    let canonical = canonical_datum_key(&stable).map_err(EvalError::Message)?;
    Ok((canonical, stable))
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
    reserved_handle: u64,
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
        next_handle: max_store_handle.max(reserved_handle).saturating_add(1),
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
        max_quantity_component: String::from("max_quantity"),
    })
}

fn validate_store_bindings(bindings: &[ReactionStoreBinding]) -> Result<(), String> {
    let mut handles = BTreeSet::new();
    let mut nodes = BTreeSet::new();
    for binding in bindings {
        if !handles.insert(binding.local_handle) {
            return Err(format!(
                "duplicate transaction store handle {}",
                binding.local_handle
            ));
        }
        if !nodes.insert(binding.node) {
            return Err(format!("duplicate graph store node {:?}", binding.node));
        }
    }
    Ok(())
}

fn preparation_reserved_handle(
    stores: &[ReactionStoreBinding],
    preparation: &ReactionHostPreparation,
) -> u64 {
    let mut maximum = stores
        .iter()
        .map(|binding| binding.local_handle)
        .max()
        .unwrap_or(0);
    maximum = maximum.max(preparation.objects.keys().copied().max().unwrap_or(0));
    maximum = maximum.max(preparation.capabilities.keys().copied().max().unwrap_or(0));
    for query in &preparation.host_queries {
        if let Some(receiver) = &query.receiver {
            maximum = maximum.max(max_datum_handle(receiver));
        }
        for argument in &query.arguments {
            maximum = maximum.max(max_datum_handle(argument));
        }
        maximum = maximum.max(max_datum_handle(&query.result));
    }
    maximum
}

fn max_datum_handle(value: &TransactionDatum) -> u64 {
    match value {
        TransactionDatum::Handle(handle) => *handle,
        TransactionDatum::Sequence(values) => {
            values.iter().map(max_datum_handle).max().unwrap_or(0)
        }
        _ => 0,
    }
}

fn apply_preparation(state: &mut MemoryTransactionState, preparation: ReactionHostPreparation) {
    state.objects.extend(preparation.objects);
    for (handle, capabilities) in preparation.capabilities {
        state
            .capabilities
            .entry(handle)
            .or_default()
            .extend(capabilities);
    }
    state.host_queries.extend(preparation.host_queries);
    state.host_calls.extend(preparation.host_calls);
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
    use quiescence_runtime::{Graph, ReactionOutput, RuntimeConfig, interaction_channel};

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

    fn no_store_access_program() -> TransactionProgram {
        program(
            vec![statement(0, TransactionOperation::Return { value: None })],
            0,
        )
    }

    fn host_call_program() -> TransactionProgram {
        program(
            vec![
                statement(
                    0,
                    TransactionOperation::Evaluate {
                        value: TransactionValue::HostCall {
                            operation: String::from("world.write"),
                            receiver: None,
                            arguments: vec![TransactionValue::Integer { value: 7 }],
                        },
                        next: TransactionStatementId(1),
                    },
                ),
                statement(1, TransactionOperation::Return { value: None }),
            ],
            0,
        )
    }

    fn parameter_host_call_program() -> TransactionProgram {
        program(
            vec![
                statement(
                    0,
                    TransactionOperation::Evaluate {
                        value: TransactionValue::HostCall {
                            operation: String::from("world.write"),
                            receiver: None,
                            arguments: vec![TransactionValue::Parameter { index: 0 }],
                        },
                        next: TransactionStatementId(1),
                    },
                ),
                statement(1, TransactionOperation::Return { value: None }),
            ],
            0,
        )
    }

    fn schedule_program(key: TransactionValue) -> TransactionProgram {
        program(
            vec![
                statement(
                    0,
                    TransactionOperation::Effect {
                        effect: TransactionEffect::ScheduleOnce {
                            key: vec![key],
                            delay: TransactionValue::Integer { value: 4 },
                            priority: TransactionValue::Integer { value: -1 },
                        },
                        next: TransactionStatementId(1),
                    },
                ),
                statement(1, TransactionOperation::Return { value: None }),
            ],
            0,
        )
    }

    fn store_read_program() -> TransactionProgram {
        program(
            vec![
                statement(
                    0,
                    TransactionOperation::Evaluate {
                        value: TransactionValue::StoreRead {
                            store: Box::new(TransactionValue::Parameter { index: 0 }),
                            key: vec![TransactionValue::Integer { value: 0 }],
                        },
                        next: TransactionStatementId(1),
                    },
                ),
                statement(1, TransactionOperation::Return { value: None }),
            ],
            0,
        )
    }

    fn prepared_store_program() -> TransactionProgram {
        TransactionProgram {
            entry: TransactionFunctionId(0),
            functions: vec![TransactionFunction {
                id: TransactionFunctionId(0),
                parameter_count: 0,
                local_count: 2,
                entry_statement: TransactionStatementId(0),
                statements: vec![
                    statement(
                        0,
                        TransactionOperation::Assign {
                            target: TransactionLocalId(0),
                            value: TransactionValue::HostQuery {
                                operation: String::from("topology.source"),
                                receiver: None,
                                arguments: Vec::new(),
                            },
                            next: TransactionStatementId(1),
                        },
                    ),
                    statement(
                        1,
                        TransactionOperation::Assign {
                            target: TransactionLocalId(1),
                            value: TransactionValue::StoreTake {
                                store: Box::new(TransactionValue::Local {
                                    local: TransactionLocalId(0),
                                }),
                                key: vec![TransactionValue::Integer { value: 0 }],
                                amount: Box::new(TransactionValue::Integer { value: 1 }),
                            },
                            next: TransactionStatementId(2),
                        },
                    ),
                    statement(2, TransactionOperation::Return { value: None }),
                ],
            }],
        }
    }

    fn prepared_store_read_program() -> TransactionProgram {
        TransactionProgram {
            entry: TransactionFunctionId(0),
            functions: vec![TransactionFunction {
                id: TransactionFunctionId(0),
                parameter_count: 0,
                local_count: 1,
                entry_statement: TransactionStatementId(0),
                statements: vec![
                    statement(
                        0,
                        TransactionOperation::Assign {
                            target: TransactionLocalId(0),
                            value: TransactionValue::HostQuery {
                                operation: String::from("topology.used"),
                                receiver: None,
                                arguments: Vec::new(),
                            },
                            next: TransactionStatementId(1),
                        },
                    ),
                    statement(
                        1,
                        TransactionOperation::Evaluate {
                            value: TransactionValue::StoreRead {
                                store: Box::new(TransactionValue::Local {
                                    local: TransactionLocalId(0),
                                }),
                                key: vec![TransactionValue::Integer { value: 0 }],
                            },
                            next: TransactionStatementId(2),
                        },
                    ),
                    statement(2, TransactionOperation::Return { value: None }),
                ],
            }],
        }
    }

    #[test]
    fn prepared_stores_become_dependencies_only_when_touched()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut interner = SnapshotInterner::new();
        let first = interner.intern(snapshot(5))?;
        let second = interner.intern(snapshot(6))?;
        let interner = Arc::new(Mutex::new(interner));
        let mut graph = Graph::new(RuntimeConfig::default());
        let (interactions, inbox) = interaction_channel();
        let used = graph.add_atom_input("used", first)?;
        let unused = graph.add_atom_input("unused", first)?;
        let adapter = TransactionReactionAdapter::new(
            Arc::new(prepared_store_read_program()),
            interner,
            Vec::new(),
            Vec::new(),
            None,
            "quantity",
        )
        .with_host_preparer(move |_| {
            Ok(ReactionHostPreparation {
                stores: vec![
                    ReactionStoreBinding {
                        node: used,
                        local_handle: 2,
                    },
                    ReactionStoreBinding {
                        node: unused,
                        local_handle: 3,
                    },
                ],
                host_queries: vec![MemoryTransactionHostQuery {
                    operation: String::from("topology.used"),
                    receiver: None,
                    arguments: Vec::new(),
                    result: TransactionDatum::Handle(2),
                    consume: false,
                }],
                ..ReactionHostPreparation::default()
            })
        });
        let evaluations = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&evaluations);
        graph.add_reaction("lazy prepared stores", move |context| {
            observed.fetch_add(1, Ordering::Relaxed);
            let result = adapter.execute(context)?;
            Ok(ReactionOutput::immediate(result.writes))
        })?;

        graph.settle()?;
        assert_eq!(evaluations.load(Ordering::Relaxed), 1);
        interactions.state_at(
            SimTime::from_millis(10),
            0,
            vec![StateWrite::atom(unused, second)],
        )?;
        graph.process_interactions(&inbox)?;
        graph.advance_to(SimTime::from_millis(10))?;
        assert_eq!(evaluations.load(Ordering::Relaxed), 1);
        interactions.state_at(
            SimTime::from_millis(20),
            0,
            vec![StateWrite::atom(used, second)],
        )?;
        graph.process_interactions(&inbox)?;
        graph.advance_to(SimTime::from_millis(20))?;
        assert_eq!(evaluations.load(Ordering::Relaxed), 2);
        Ok(())
    }

    #[test]
    fn prepared_graph_store_is_materialized_and_published() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut interner = SnapshotInterner::new();
        let initial = interner.intern(snapshot(5))?;
        let interner = Arc::new(Mutex::new(interner));
        let mut graph = Graph::new(RuntimeConfig::default());
        let source = graph.add_atom_input("source", initial)?;
        let adapter = TransactionReactionAdapter::new(
            Arc::new(prepared_store_program()),
            Arc::clone(&interner),
            Vec::new(),
            Vec::new(),
            None,
            "quantity",
        )
        .with_host_preparer(move |_| {
            Ok(ReactionHostPreparation {
                stores: vec![ReactionStoreBinding {
                    node: source,
                    local_handle: 2,
                }],
                host_queries: vec![MemoryTransactionHostQuery {
                    operation: String::from("topology.source"),
                    receiver: None,
                    arguments: Vec::new(),
                    result: TransactionDatum::Handle(2),
                    consume: false,
                }],
                ..ReactionHostPreparation::default()
            })
        });
        let evaluations = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&evaluations);
        graph.add_reaction("prepared source", move |context| {
            if observed.fetch_add(1, Ordering::Relaxed) != 0 {
                return Ok(ReactionOutput::idle());
            }
            let result = adapter.execute(context)?;
            Ok(ReactionOutput::immediate(result.writes))
        })?;
        graph.settle()?;

        let snapshot_id = graph.value_atom(source)?;
        assert_ne!(snapshot_id, initial);
        let snapshots = interner
            .lock()
            .map_err(|_| String::from("snapshot interner lock poisoned"))?;
        let snapshot = snapshots
            .get(snapshot_id)
            .ok_or_else(|| String::from("published snapshot missing"))?;
        assert_eq!(
            snapshot.slots[0]
                .as_ref()
                .and_then(|value| value.components.get("quantity")),
            Some(&TransactionDatum::Integer(4))
        );
        Ok(())
    }

    #[test]
    fn transaction_interaction_port_carries_event_local_arguments_and_host_evidence()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut graph = Graph::new(RuntimeConfig::default());
        let world_state = graph.add_atom_input("world state", 0)?;
        let adapter = TransactionReactionAdapter::new(
            Arc::new(parameter_host_call_program()),
            Arc::new(Mutex::new(SnapshotInterner::new())),
            Vec::new(),
            vec![ReactionArgument::Datum(TransactionDatum::Integer(-1))],
            None,
            "quantity",
        )
        .with_host_call_publisher(move |call| {
            let [TransactionDatum::Integer(value)] = call.arguments.as_slice() else {
                return Err(EvalError::Message(String::from(
                    "interaction host call did not carry one integer argument",
                )));
            };
            let value = u64::try_from(*value)
                .map_err(|_| EvalError::Message(String::from("negative world state")))?;
            Ok(vec![StateWrite::atom(world_state, value)])
        });
        let (ingress, inbox) = interaction_channel();
        let port = install_transaction_interaction(
            &mut graph,
            ingress,
            "transaction interaction",
            adapter,
            None,
        )?;
        graph.quiesce()?;

        let invocation = |value| {
            TransactionInvocation::new(vec![ReactionArgument::Datum(TransactionDatum::Integer(
                value,
            ))])
            .with_preparation(ReactionHostPreparation {
                host_calls: vec![MemoryTransactionHostCall {
                    operation: String::from("world.write"),
                    receiver: None,
                    arguments: vec![TransactionDatum::Integer(value)],
                    result: TransactionDatum::Boolean(true),
                    consume: false,
                }],
                ..ReactionHostPreparation::default()
            })
        };

        let first = port.submit_now(0, invocation(7))?;
        assert_eq!(port.pending_payload_count()?, 1);
        assert_eq!(graph.process_interactions(&inbox)?, 1);
        assert_eq!(graph.value_atom(world_state)?, 7);
        assert_eq!(port.pending_payload_count()?, 0);
        assert!(matches!(
            port.retry_now(first, 0),
            Err(TransactionInteractionError::UnknownToken(token)) if token == first
        ));

        let future = port.submit_at(SimTime::from_millis(20), 0, invocation(9))?;
        assert_eq!(graph.process_interactions(&inbox)?, 1);
        assert_eq!(graph.value_atom(world_state)?, 7);
        assert_eq!(port.pending_payload_count()?, 1);
        graph.advance_to(SimTime::from_millis(20))?;
        assert_eq!(graph.value_atom(world_state)?, 9);
        assert_eq!(port.pending_payload_count()?, 0);
        assert!(matches!(
            port.retry_at(future, SimTime::from_millis(30), 0),
            Err(TransactionInteractionError::UnknownToken(token)) if token == future
        ));
        Ok(())
    }

    #[test]
    fn failed_transaction_interaction_payload_can_retry_same_token()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut graph = Graph::new(RuntimeConfig::default());
        let world_state = graph.add_atom_input("world state", 0)?;
        let attempts = Arc::new(AtomicUsize::new(0));
        let observed_attempts = Arc::clone(&attempts);
        let adapter = TransactionReactionAdapter::new(
            Arc::new(parameter_host_call_program()),
            Arc::new(Mutex::new(SnapshotInterner::new())),
            Vec::new(),
            vec![ReactionArgument::Datum(TransactionDatum::Integer(-1))],
            None,
            "quantity",
        )
        .with_host_call_publisher(move |call| {
            if observed_attempts.fetch_add(1, Ordering::Relaxed) == 0 {
                return Err(EvalError::Message(String::from(
                    "transient publisher failure",
                )));
            }
            let [TransactionDatum::Integer(value)] = call.arguments.as_slice() else {
                return Err(EvalError::Message(String::from("unexpected retry payload")));
            };
            Ok(vec![StateWrite::atom(
                world_state,
                u64::try_from(*value)
                    .map_err(|_| EvalError::Message(String::from("negative world state")))?,
            )])
        });
        let (ingress, inbox) = interaction_channel();
        let port = install_transaction_interaction(
            &mut graph,
            ingress,
            "retryable transaction interaction",
            adapter,
            None,
        )?;
        graph.quiesce()?;
        let invocation = TransactionInvocation::new(vec![ReactionArgument::Datum(
            TransactionDatum::Integer(11),
        )])
        .with_preparation(ReactionHostPreparation {
            host_calls: vec![MemoryTransactionHostCall {
                operation: String::from("world.write"),
                receiver: None,
                arguments: vec![TransactionDatum::Integer(11)],
                result: TransactionDatum::Boolean(true),
                consume: false,
            }],
            ..ReactionHostPreparation::default()
        });
        let token = port.submit_now(0, invocation)?;
        assert!(matches!(
            graph.process_interactions(&inbox),
            Err(RuntimeError::ReactionEvaluation { .. })
        ));
        assert_eq!(port.pending_payload_count()?, 1);
        assert_eq!(graph.value_atom(world_state)?, 0);
        port.retry_now(token, 0)?;
        assert_eq!(graph.process_interactions(&inbox)?, 1);
        assert_eq!(graph.value_atom(world_state)?, 11);
        assert_eq!(port.pending_payload_count()?, 0);
        assert_eq!(attempts.load(Ordering::Relaxed), 2);
        Ok(())
    }

    #[test]
    fn committed_host_calls_publish_graph_writes_atomically()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut graph = Graph::new(RuntimeConfig::default());
        let world_state = graph.add_atom_input("world state", 1)?;
        let adapter = TransactionReactionAdapter::new(
            Arc::new(host_call_program()),
            Arc::new(Mutex::new(SnapshotInterner::new())),
            Vec::new(),
            vec![ReactionArgument::Datum(TransactionDatum::Integer(0))],
            None,
            "quantity",
        )
        .with_host_preparer(|_| {
            Ok(ReactionHostPreparation {
                host_calls: vec![MemoryTransactionHostCall {
                    operation: String::from("world.write"),
                    receiver: None,
                    arguments: vec![TransactionDatum::Integer(7)],
                    result: TransactionDatum::Boolean(true),
                    consume: false,
                }],
                ..ReactionHostPreparation::default()
            })
        })
        .with_host_call_publisher(move |call| {
            if call.operation != "world.write"
                || call.arguments != [TransactionDatum::Integer(7)]
                || call.result != TransactionDatum::Boolean(true)
            {
                return Err(EvalError::Message(String::from(
                    "unexpected published host call",
                )));
            }
            Ok(vec![StateWrite::atom(world_state, 42)])
        });
        let captured = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&captured);
        graph.add_reaction("host call bridge", move |context| {
            let result = adapter.execute(context)?;
            *observed
                .lock()
                .map_err(|_| EvalError::Message(String::from("capture lock poisoned")))? =
                result.performed_host_calls.clone();
            Ok(ReactionOutput::immediate(result.writes))
        })?;
        graph.settle()?;
        assert_eq!(graph.value_atom(world_state)?, 42);
        let captured = captured
            .lock()
            .map_err(|_| String::from("capture lock poisoned"))?;
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].operation, "world.write");
        Ok(())
    }

    #[test]
    fn committed_host_calls_require_a_publisher() -> Result<(), Box<dyn std::error::Error>> {
        let adapter = TransactionReactionAdapter::new(
            Arc::new(host_call_program()),
            Arc::new(Mutex::new(SnapshotInterner::new())),
            Vec::new(),
            vec![ReactionArgument::Datum(TransactionDatum::Integer(0))],
            None,
            "quantity",
        );
        let error = publish_host_calls(
            &adapter,
            &[MemoryTransactionPerformedHostCall {
                operation: String::from("world.write"),
                receiver: None,
                arguments: vec![TransactionDatum::Integer(7)],
                result: TransactionDatum::Boolean(true),
            }],
        )
        .err()
        .ok_or_else(|| String::from("host calls disappeared without a publisher"))?;
        assert!(
            error
                .to_string()
                .contains("without a configured host-call publisher")
        );
        Ok(())
    }

    #[test]
    fn schedule_requests_convert_source_units_and_reuse_exact_keys()
    -> Result<(), Box<dyn std::error::Error>> {
        let interner = Arc::new(Mutex::new(SnapshotInterner::new()));
        let adapter = TransactionReactionAdapter::new(
            Arc::new(schedule_program(TransactionValue::String {
                value: String::from("block:repeater"),
            })),
            interner,
            Vec::new(),
            vec![ReactionArgument::Datum(TransactionDatum::Integer(0))],
            None,
            "quantity",
        )
        .with_schedule_unit(SimDuration::from_millis(50));
        let identity_adapter = adapter.clone();
        let observed = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&observed);
        let mut graph = Graph::new(RuntimeConfig::default());
        let (interactions, inbox) = interaction_channel();
        let reaction = graph.add_reaction("schedule bridge", move |context| {
            let result = adapter.execute(context)?;
            captured
                .lock()
                .map_err(|_| EvalError::Message(String::from("capture lock poisoned")))?
                .push(result.schedule_requests[0]);
            Ok(ReactionOutput::idle())
        })?;
        graph.quiesce()?;
        interactions.callback_at(reaction, SimTime::from_millis(10), 0)?;
        graph.process_interactions(&inbox)?;
        graph.advance_to(SimTime::from_millis(10))?;
        let observed = observed
            .lock()
            .map_err(|_| String::from("capture lock poisoned"))?;
        assert_eq!(observed.len(), 2);
        assert_eq!(observed[0].key, observed[1].key);
        assert_eq!(observed[0].after, SimDuration::from_millis(200));
        assert_eq!(observed[0].priority, -1);
        assert_eq!(
            identity_adapter.schedule_identity(observed[0].key)?,
            vec![TransactionDatum::String(String::from("block:repeater"))]
        );
        Ok(())
    }

    #[test]
    fn duplicate_transaction_schedules_preserve_first_pending_callback()
    -> Result<(), Box<dyn std::error::Error>> {
        let adapter = TransactionReactionAdapter::new(
            Arc::new(schedule_program(TransactionValue::String {
                value: String::from("block:repeater"),
            })),
            Arc::new(Mutex::new(SnapshotInterner::new())),
            Vec::new(),
            vec![ReactionArgument::Datum(TransactionDatum::Integer(0))],
            None,
            "quantity",
        )
        .with_schedule_unit(SimDuration::from_millis(50));
        let producer_evaluations = Arc::new(AtomicUsize::new(0));
        let observed_producer = Arc::clone(&producer_evaluations);
        let callback_evaluations = Arc::new(AtomicUsize::new(0));
        let observed_callback = Arc::clone(&callback_evaluations);
        let mut graph = Graph::new(RuntimeConfig::default());
        let (interactions, inbox) = interaction_channel();
        let callback = graph.add_event_callback("transaction callback target", move |_| {
            observed_callback.fetch_add(1, Ordering::Relaxed);
            Ok(ReactionOutput::idle())
        })?;
        let producer = graph.add_reaction("transaction schedule producer", move |context| {
            let evaluation = observed_producer.fetch_add(1, Ordering::Relaxed);
            if evaluation >= 2 {
                return Ok(ReactionOutput::idle());
            }
            Ok(adapter
                .execute(context)?
                .into_reaction_output_for_callback(callback))
        })?;

        graph.quiesce()?;
        interactions.callback_at(producer, SimTime::from_millis(50), 0)?;
        graph.process_interactions(&inbox)?;
        graph.advance_to(SimTime::from_millis(50))?;
        assert_eq!(producer_evaluations.load(Ordering::Relaxed), 2);
        assert_eq!(callback_evaluations.load(Ordering::Relaxed), 0);
        graph.advance_to(SimTime::from_millis(199))?;
        assert_eq!(callback_evaluations.load(Ordering::Relaxed), 0);
        graph.advance_to(SimTime::from_millis(200))?;
        assert_eq!(callback_evaluations.load(Ordering::Relaxed), 1);
        Ok(())
    }

    #[test]
    fn schedule_key_handles_use_configured_stable_identity()
    -> Result<(), Box<dyn std::error::Error>> {
        let adapter = TransactionReactionAdapter::new(
            Arc::new(schedule_program(TransactionValue::Parameter { index: 0 })),
            Arc::new(Mutex::new(SnapshotInterner::new())),
            Vec::new(),
            vec![ReactionArgument::Datum(TransactionDatum::Handle(99))],
            None,
            "quantity",
        )
        .with_schedule_unit(SimDuration::from_millis(50))
        .with_schedule_identity_component("semantic_id")
        .with_host_preparer(|_| {
            Ok(ReactionHostPreparation {
                objects: [(
                    99,
                    MemoryTransactionObject {
                        components: [(
                            String::from("semantic_id"),
                            TransactionDatum::String(String::from("minecraft:unpowered_repeater")),
                        )]
                        .into_iter()
                        .collect(),
                        semantic_tag: None,
                    },
                )]
                .into_iter()
                .collect(),
                ..ReactionHostPreparation::default()
            })
        });
        let identity_adapter = adapter.clone();
        let observed = Arc::new(Mutex::new(None));
        let captured = Arc::clone(&observed);
        let mut graph = Graph::new(RuntimeConfig::default());
        graph.add_reaction("stable schedule identity", move |context| {
            let result = adapter.execute(context)?;
            *captured
                .lock()
                .map_err(|_| EvalError::Message(String::from("capture lock poisoned")))? =
                Some(result.schedule_requests[0]);
            Ok(ReactionOutput::idle())
        })?;
        graph.settle()?;
        let scheduled = observed
            .lock()
            .map_err(|_| String::from("capture lock poisoned"))?
            .ok_or_else(|| String::from("schedule result missing"))?;
        assert_eq!(scheduled.after, SimDuration::from_millis(200));
        assert_eq!(scheduled.priority, -1);
        assert_eq!(
            identity_adapter.schedule_identity(scheduled.key)?,
            vec![TransactionDatum::String(String::from(
                "minecraft:unpowered_repeater"
            ))]
        );
        Ok(())
    }

    #[test]
    fn schedule_keys_reject_ephemeral_transaction_handles() -> Result<(), Box<dyn std::error::Error>>
    {
        let adapter = TransactionReactionAdapter::new(
            Arc::new(schedule_program(TransactionValue::Parameter { index: 0 })),
            Arc::new(Mutex::new(SnapshotInterner::new())),
            Vec::new(),
            vec![ReactionArgument::Datum(TransactionDatum::Handle(99))],
            None,
            "quantity",
        )
        .with_schedule_unit(SimDuration::from_millis(50));
        let rejected = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&rejected);
        let mut graph = Graph::new(RuntimeConfig::default());
        graph.add_reaction("invalid persistent key", move |context| {
            match adapter.execute(context) {
                Ok(_) => Err(EvalError::Message(String::from(
                    "ephemeral schedule key accepted",
                ))),
                Err(error) => {
                    if error.to_string().contains("ephemeral handle 99") {
                        observed.store(1, Ordering::Relaxed);
                        Ok(ReactionOutput::idle())
                    } else {
                        Err(error)
                    }
                }
            }
        })?;
        graph.settle()?;
        assert_eq!(rejected.load(Ordering::Relaxed), 1);
        Ok(())
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
    fn fixed_store_argument_is_lazy_until_semantically_accessed()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut interner = SnapshotInterner::new();
        let first = interner.intern(snapshot(5))?;
        let second = interner.intern(snapshot(4))?;
        let interner = Arc::new(Mutex::new(interner));
        let mut graph = Graph::new(RuntimeConfig::default());
        let (interactions, inbox) = interaction_channel();
        let store = graph.add_atom_input("store", first)?;
        let adapter = adapter(no_store_access_program(), interner, store);
        let evaluations = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&evaluations);
        graph.add_reaction("unused fixed store", move |context| {
            observed.fetch_add(1, Ordering::Relaxed);
            let result = adapter.execute(context)?;
            Ok(ReactionOutput::immediate(result.writes))
        })?;

        graph.settle()?;
        assert_eq!(evaluations.load(Ordering::Relaxed), 1);
        interactions.state_at(
            SimTime::from_millis(10),
            0,
            vec![StateWrite::atom(store, second)],
        )?;
        graph.process_interactions(&inbox)?;
        graph.advance_to(SimTime::from_millis(10))?;
        assert_eq!(evaluations.load(Ordering::Relaxed), 1);
        Ok(())
    }

    #[test]
    fn store_dependency_appears_only_after_deadline() -> Result<(), Box<dyn std::error::Error>> {
        let mut interner = SnapshotInterner::new();
        let first = interner.intern(snapshot(5))?;
        let second = interner.intern(snapshot(4))?;
        let interner = Arc::new(Mutex::new(interner));
        let mut graph = Graph::new(RuntimeConfig::default());
        let (interactions, inbox) = interaction_channel();
        let deadline = graph.add_atom_input("deadline_ms", 100)?;
        let store = graph.add_atom_input("store", first)?;
        let adapter = adapter(store_read_program(), interner, store);
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
        interactions.state_at(
            SimTime::from_millis(50),
            0,
            vec![StateWrite::atom(store, second)],
        )?;
        graph.process_interactions(&inbox)?;
        graph.advance_to(SimTime::from_millis(50))?;
        assert_eq!(evaluations.load(Ordering::Relaxed), 1);
        graph.advance_to(SimTime::from_millis(100))?;
        assert_eq!(evaluations.load(Ordering::Relaxed), 2);
        interactions.state_at(
            SimTime::from_millis(110),
            0,
            vec![StateWrite::atom(store, first)],
        )?;
        graph.process_interactions(&inbox)?;
        graph.advance_to(SimTime::from_millis(110))?;
        assert_eq!(evaluations.load(Ordering::Relaxed), 3);
        Ok(())
    }
}
