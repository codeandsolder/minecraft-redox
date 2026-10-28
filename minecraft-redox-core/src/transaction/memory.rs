//! Serializable reference host for transaction differential tests and archaeology.
//!
//! This is not a game inventory implementation. It is a small source-neutral host that makes the
//! transaction runtime inspectable: objects are component maps, stores are indexed handle arrays,
//! and every execution still uses the normal isolated-draft/commit contract.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{TransactionDatum, TransactionDraft, TransactionHost, TransactionStoreQueryKind};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MemoryTransactionObject {
    #[serde(default)]
    pub components: BTreeMap<String, TransactionDatum>,
    #[serde(default)]
    pub semantic_tag: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MemoryTransactionStore {
    pub slots: Vec<Option<u64>>,
    #[serde(default = "default_true")]
    pub allows_read: bool,
    #[serde(default = "default_true")]
    pub allows_write: bool,
}

const fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MemoryTransactionHostQuery {
    pub operation: String,
    pub receiver: Option<TransactionDatum>,
    #[serde(default)]
    pub arguments: Vec<TransactionDatum>,
    pub result: TransactionDatum,
    #[serde(default)]
    pub consume: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MemoryTransactionScheduleOnce {
    pub key: Vec<TransactionDatum>,
    pub delay_units: i64,
    pub priority: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MemoryTransactionState {
    #[serde(default)]
    pub objects: BTreeMap<u64, MemoryTransactionObject>,
    #[serde(default)]
    pub stores: BTreeMap<u64, MemoryTransactionStore>,
    #[serde(default)]
    pub capabilities: BTreeMap<u64, BTreeSet<String>>,
    #[serde(default)]
    pub state_values: BTreeMap<u64, BTreeMap<String, TransactionDatum>>,
    #[serde(default)]
    pub touched_stores: BTreeSet<u64>,
    #[serde(default)]
    pub host_queries: Vec<MemoryTransactionHostQuery>,
    #[serde(default)]
    pub scheduled_once: Vec<MemoryTransactionScheduleOnce>,
    pub next_handle: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MemoryTransactionHost {
    pub state: MemoryTransactionState,
    pub quantity_component: String,
    #[serde(default = "default_max_quantity_component")]
    pub max_quantity_component: String,
}

fn default_max_quantity_component() -> String {
    String::from("max_quantity")
}

pub struct MemoryTransactionDraft<'a> {
    host: &'a mut MemoryTransactionHost,
    state: MemoryTransactionState,
    quantity_component: String,
    max_quantity_component: String,
}

impl TransactionHost for MemoryTransactionHost {
    type Error = String;
    type Draft<'a> = MemoryTransactionDraft<'a>;

    fn begin(&mut self) -> Result<Self::Draft<'_>, Self::Error> {
        Ok(MemoryTransactionDraft {
            state: self.state.clone(),
            quantity_component: self.quantity_component.clone(),
            max_quantity_component: self.max_quantity_component.clone(),
            host: self,
        })
    }
}

impl MemoryTransactionDraft<'_> {
    fn handle(value: &TransactionDatum) -> Result<u64, String> {
        match value {
            TransactionDatum::Handle(handle) => Ok(*handle),
            value => Err(format!("expected handle, got {value:?}")),
        }
    }

    fn integer(value: &TransactionDatum) -> Result<i64, String> {
        match value {
            TransactionDatum::Integer(value) => Ok(*value),
            value => Err(format!("expected integer, got {value:?}")),
        }
    }

    fn store_key(
        store: &TransactionDatum,
        key: &[TransactionDatum],
    ) -> Result<(u64, usize), String> {
        let [key] = key else {
            return Err(String::from("reference host expects one store key"));
        };
        let store = Self::handle(store)?;
        let key = Self::integer(key)?;
        let key = usize::try_from(key).map_err(|_| String::from("store key is negative"))?;
        Ok((store, key))
    }

    fn allocate_object(&mut self, object: MemoryTransactionObject) -> u64 {
        let handle = self.state.next_handle;
        self.state.next_handle = self.state.next_handle.saturating_add(1);
        self.state.objects.insert(handle, object);
        handle
    }
}

impl TransactionDraft for MemoryTransactionDraft<'_> {
    type Error = String;

    fn has_capability(
        &self,
        value: &TransactionDatum,
        capability: &str,
    ) -> Result<bool, Self::Error> {
        let handle = Self::handle(value)?;
        Ok(self
            .state
            .capabilities
            .get(&handle)
            .is_some_and(|capabilities| capabilities.contains(capability)))
    }

    fn clone_value(&mut self, value: &TransactionDatum) -> Result<TransactionDatum, Self::Error> {
        let TransactionDatum::Handle(handle) = value else {
            return Ok(value.clone());
        };
        let object = self
            .state
            .objects
            .get(handle)
            .cloned()
            .ok_or_else(|| format!("object handle {handle} is missing"))?;
        Ok(TransactionDatum::Handle(self.allocate_object(object)))
    }

    fn component(
        &self,
        value: &TransactionDatum,
        component: &str,
    ) -> Result<TransactionDatum, Self::Error> {
        let handle = Self::handle(value)?;
        self.state
            .objects
            .get(&handle)
            .and_then(|object| object.components.get(component))
            .cloned()
            .ok_or_else(|| format!("component {component:?} missing from object {handle}"))
    }

    fn set_component(
        &mut self,
        value: &TransactionDatum,
        component: &str,
        new_value: TransactionDatum,
    ) -> Result<(), Self::Error> {
        let handle = Self::handle(value)?;
        let object = self
            .state
            .objects
            .get_mut(&handle)
            .ok_or_else(|| format!("object handle {handle} is missing"))?;
        object.components.insert(component.to_owned(), new_value);
        Ok(())
    }

    fn semantic_equal(
        &self,
        left: &TransactionDatum,
        right: &TransactionDatum,
    ) -> Result<bool, Self::Error> {
        match (left, right) {
            (TransactionDatum::Handle(left), TransactionDatum::Handle(right)) => {
                let left = self
                    .state
                    .objects
                    .get(left)
                    .ok_or_else(|| String::from("left semantic object is missing"))?;
                let right = self
                    .state
                    .objects
                    .get(right)
                    .ok_or_else(|| String::from("right semantic object is missing"))?;
                Ok(left.semantic_tag == right.semantic_tag)
            }
            _ => Ok(left == right),
        }
    }

    fn host_query(
        &mut self,
        operation: &str,
        receiver: Option<&TransactionDatum>,
        arguments: &[TransactionDatum],
    ) -> Result<TransactionDatum, Self::Error> {
        let index = self
            .state
            .host_queries
            .iter()
            .position(|query| {
                query.operation == operation
                    && query.receiver.as_ref() == receiver
                    && query.arguments == arguments
            })
            .ok_or_else(|| {
                format!(
                    "unconfigured host query {operation:?} receiver={receiver:?} arguments={arguments:?}"
                )
            })?;
        let query = &self.state.host_queries[index];
        let result = query.result.clone();
        if query.consume {
            self.state.host_queries.remove(index);
        }
        Ok(result)
    }

    fn store_read(
        &self,
        store: &TransactionDatum,
        key: &[TransactionDatum],
    ) -> Result<TransactionDatum, Self::Error> {
        let (store, key) = Self::store_key(store, key)?;
        let store = self
            .state
            .stores
            .get(&store)
            .ok_or_else(|| String::from("store is missing"))?;
        Ok(store
            .slots
            .get(key)
            .copied()
            .flatten()
            .map_or(TransactionDatum::Null, TransactionDatum::Handle))
    }

    fn store_take(
        &mut self,
        store: &TransactionDatum,
        key: &[TransactionDatum],
        amount: &TransactionDatum,
    ) -> Result<TransactionDatum, Self::Error> {
        let (store_handle, key) = Self::store_key(store, key)?;
        let amount = Self::integer(amount)?;
        if amount < 0 {
            return Err(String::from("take amount is negative"));
        }
        let source_handle = self
            .state
            .stores
            .get(&store_handle)
            .and_then(|store| store.slots.get(key))
            .copied()
            .flatten()
            .ok_or_else(|| String::from("source slot is empty"))?;
        let source = self
            .state
            .objects
            .get_mut(&source_handle)
            .ok_or_else(|| String::from("source object is missing"))?;
        let quantity = source
            .components
            .get(&self.quantity_component)
            .ok_or_else(|| String::from("source quantity is missing"))?;
        let quantity = Self::integer(quantity)?;
        if quantity < amount {
            return Err(String::from("take amount exceeds source quantity"));
        }
        let mut taken = source.clone();
        source.components.insert(
            self.quantity_component.clone(),
            TransactionDatum::Integer(quantity - amount),
        );
        taken.components.insert(
            self.quantity_component.clone(),
            TransactionDatum::Integer(amount),
        );
        let source_empty = quantity == amount;
        let taken_handle = self.allocate_object(taken);
        if source_empty {
            let store = self
                .state
                .stores
                .get_mut(&store_handle)
                .ok_or_else(|| String::from("store disappeared"))?;
            store.slots[key] = None;
        }
        Ok(TransactionDatum::Handle(taken_handle))
    }

    fn store_write(
        &mut self,
        store: &TransactionDatum,
        key: &[TransactionDatum],
        value: TransactionDatum,
    ) -> Result<(), Self::Error> {
        let (store, key) = Self::store_key(store, key)?;
        let store = self
            .state
            .stores
            .get_mut(&store)
            .ok_or_else(|| String::from("store is missing"))?;
        let slot = store
            .slots
            .get_mut(key)
            .ok_or_else(|| String::from("store key is out of bounds"))?;
        *slot = match value {
            TransactionDatum::Null => None,
            TransactionDatum::Handle(handle) => Some(handle),
            value => return Err(format!("store write requires handle/null, got {value:?}")),
        };
        Ok(())
    }

    fn store_touch(&mut self, store: &TransactionDatum) -> Result<(), Self::Error> {
        self.state.touched_stores.insert(Self::handle(store)?);
        Ok(())
    }

    fn store_query(
        &self,
        operation: TransactionStoreQueryKind,
        store: &TransactionDatum,
        _key: &[TransactionDatum],
        _value: Option<&TransactionDatum>,
        _context: &[TransactionDatum],
    ) -> Result<TransactionDatum, Self::Error> {
        let handle = Self::handle(store)?;
        let store = self
            .state
            .stores
            .get(&handle)
            .ok_or_else(|| String::from("store is missing"))?;
        match operation {
            TransactionStoreQueryKind::Extent => Ok(TransactionDatum::Integer(
                i64::try_from(store.slots.len())
                    .map_err(|_| String::from("store extent exceeds i64"))?,
            )),
            TransactionStoreQueryKind::Keys => Ok(TransactionDatum::Sequence(
                (0..store.slots.len())
                    .map(|index| {
                        i64::try_from(index)
                            .map(TransactionDatum::Integer)
                            .map_err(|_| String::from("store key exceeds i64"))
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            )),
            TransactionStoreQueryKind::Empty => Ok(TransactionDatum::Boolean(
                store.slots.iter().all(Option::is_none),
            )),
            TransactionStoreQueryKind::Full => {
                let full = store.slots.iter().all(|slot| {
                    let Some(handle) = slot else {
                        return false;
                    };
                    let Some(object) = self.state.objects.get(handle) else {
                        return false;
                    };
                    let Some(TransactionDatum::Integer(quantity)) =
                        object.components.get(&self.quantity_component)
                    else {
                        return false;
                    };
                    let Some(TransactionDatum::Integer(max_quantity)) =
                        object.components.get(&self.max_quantity_component)
                    else {
                        return false;
                    };
                    quantity >= max_quantity
                });
                Ok(TransactionDatum::Boolean(full))
            }
            TransactionStoreQueryKind::AllowsRead => {
                Ok(TransactionDatum::Boolean(store.allows_read))
            }
            TransactionStoreQueryKind::AllowsWrite => {
                Ok(TransactionDatum::Boolean(store.allows_write))
            }
        }
    }

    fn schedule_once(
        &mut self,
        key: &[TransactionDatum],
        delay: &TransactionDatum,
        priority: &TransactionDatum,
    ) -> Result<(), Self::Error> {
        let delay_units = Self::integer(delay)?;
        let priority = i32::try_from(Self::integer(priority)?)
            .map_err(|_| String::from("schedule priority exceeds i32"))?;
        self.state
            .scheduled_once
            .push(MemoryTransactionScheduleOnce {
                key: key.to_vec(),
                delay_units,
                priority,
            });
        Ok(())
    }

    fn state_write(
        &mut self,
        target: &TransactionDatum,
        state: &str,
        value: TransactionDatum,
    ) -> Result<(), Self::Error> {
        let target = Self::handle(target)?;
        self.state
            .state_values
            .entry(target)
            .or_default()
            .insert(state.to_owned(), value);
        Ok(())
    }

    fn commit(self) -> Result<(), Self::Error> {
        self.host.state = self.state;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(
        operation: &str,
        result: TransactionDatum,
        consume: bool,
    ) -> MemoryTransactionHostQuery {
        MemoryTransactionHostQuery {
            operation: operation.to_owned(),
            receiver: Some(TransactionDatum::Handle(1)),
            arguments: vec![TransactionDatum::Integer(2)],
            result,
            consume,
        }
    }

    #[test]
    fn store_empty_and_full_queries_follow_draft_state() -> Result<(), String> {
        let mut host = MemoryTransactionHost {
            state: MemoryTransactionState {
                objects: [(
                    7,
                    MemoryTransactionObject {
                        components: [
                            (String::from("quantity"), TransactionDatum::Integer(64)),
                            (String::from("max_quantity"), TransactionDatum::Integer(64)),
                        ]
                        .into_iter()
                        .collect(),
                        semantic_tag: Some(String::from("item")),
                    },
                )]
                .into_iter()
                .collect(),
                stores: [(
                    1,
                    MemoryTransactionStore {
                        slots: vec![None],
                        allows_read: true,
                        allows_write: true,
                    },
                )]
                .into_iter()
                .collect(),
                next_handle: 8,
                ..MemoryTransactionState::default()
            },
            quantity_component: String::from("quantity"),
            max_quantity_component: String::from("max_quantity"),
        };
        let store = TransactionDatum::Handle(1);
        let key = [TransactionDatum::Integer(0)];
        let mut draft = host.begin()?;
        assert_eq!(
            draft.store_query(TransactionStoreQueryKind::Empty, &store, &[], None, &[])?,
            TransactionDatum::Boolean(true)
        );
        assert_eq!(
            draft.store_query(TransactionStoreQueryKind::Full, &store, &[], None, &[])?,
            TransactionDatum::Boolean(false)
        );
        draft.store_write(&store, &key, TransactionDatum::Handle(7))?;
        assert_eq!(
            draft.store_query(TransactionStoreQueryKind::Empty, &store, &[], None, &[])?,
            TransactionDatum::Boolean(false)
        );
        assert_eq!(
            draft.store_query(TransactionStoreQueryKind::Full, &store, &[], None, &[])?,
            TransactionDatum::Boolean(true)
        );
        draft.set_component(
            &TransactionDatum::Handle(7),
            "quantity",
            TransactionDatum::Integer(63),
        )?;
        assert_eq!(
            draft.store_query(TransactionStoreQueryKind::Full, &store, &[], None, &[])?,
            TransactionDatum::Boolean(false)
        );
        Ok(())
    }

    #[test]
    fn schedule_requests_publish_only_on_draft_commit() -> Result<(), String> {
        let mut host = MemoryTransactionHost {
            state: MemoryTransactionState::default(),
            quantity_component: String::from("quantity"),
            max_quantity_component: String::from("max_quantity"),
        };
        {
            let mut draft = host.begin()?;
            draft.schedule_once(
                &[
                    TransactionDatum::Integer(1),
                    TransactionDatum::String(String::from("block")),
                ],
                &TransactionDatum::Integer(4),
                &TransactionDatum::Integer(-1),
            )?;
        }
        assert_eq!(host.state.scheduled_once, Vec::new());
        let mut draft = host.begin()?;
        draft.schedule_once(
            &[
                TransactionDatum::Integer(1),
                TransactionDatum::String(String::from("block")),
            ],
            &TransactionDatum::Integer(4),
            &TransactionDatum::Integer(-1),
        )?;
        draft.commit()?;
        assert_eq!(
            host.state.scheduled_once,
            vec![MemoryTransactionScheduleOnce {
                key: vec![
                    TransactionDatum::Integer(1),
                    TransactionDatum::String(String::from("block")),
                ],
                delay_units: 4,
                priority: -1,
            }]
        );
        Ok(())
    }

    #[test]
    fn host_queries_can_be_repeatable_or_consumed() -> Result<(), String> {
        let mut host = MemoryTransactionHost {
            state: MemoryTransactionState {
                host_queries: vec![
                    query("topology.lookup", TransactionDatum::Handle(7), false),
                    query("random.draw", TransactionDatum::Integer(3), true),
                ],
                ..MemoryTransactionState::default()
            },
            quantity_component: String::from("quantity"),
            max_quantity_component: String::from("max_quantity"),
        };
        let receiver = TransactionDatum::Handle(1);
        let arguments = [TransactionDatum::Integer(2)];
        let mut draft = host.begin()?;
        assert_eq!(
            draft.host_query("topology.lookup", Some(&receiver), &arguments)?,
            TransactionDatum::Handle(7)
        );
        assert_eq!(
            draft.host_query("topology.lookup", Some(&receiver), &arguments)?,
            TransactionDatum::Handle(7)
        );
        assert_eq!(
            draft.host_query("random.draw", Some(&receiver), &arguments)?,
            TransactionDatum::Integer(3)
        );
        assert!(
            draft
                .host_query("random.draw", Some(&receiver), &arguments)
                .is_err()
        );
        Ok(())
    }
}
