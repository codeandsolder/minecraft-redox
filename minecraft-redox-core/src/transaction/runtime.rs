//! Runtime for source-neutral transaction programs.
//!
//! Execution always happens against a host-provided draft. The draft is committed only after the
//! top-level function returns successfully; dropping it after any execution/host error must leave
//! externally visible state unchanged.

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

use super::{
    TransactionEffect, TransactionFunction, TransactionFunctionId, TransactionLocalId,
    TransactionNumericType, TransactionOperation, TransactionProgram, TransactionStoreQueryKind,
    TransactionValue,
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum TransactionDatum {
    Integer(i64),
    Float(f64),
    Boolean(bool),
    String(String),
    Null,
    Sequence(Vec<TransactionDatum>),
    Handle(u64),
}

/// Creates isolated drafts for transaction execution.
pub trait TransactionHost {
    type Error;
    type Draft<'a>: TransactionDraft<Error = Self::Error>
    where
        Self: 'a;

    /// Start an isolated draft whose drop has no externally visible effect.
    ///
    /// # Errors
    ///
    /// Returns a host-defined error when a draft cannot be created.
    fn begin(&mut self) -> Result<Self::Draft<'_>, Self::Error>;
}

/// Semantic operations available to a transaction while it executes in isolation.
pub trait TransactionDraft {
    type Error;

    /// Query a target capability.
    ///
    /// # Errors
    /// Returns a host-defined error when the value cannot be inspected.
    fn has_capability(
        &self,
        value: &TransactionDatum,
        capability: &str,
    ) -> Result<bool, Self::Error>;

    /// Clone a semantic value with independent mutable component identity.
    ///
    /// # Errors
    /// Returns a host-defined error when the value cannot be cloned.
    fn clone_value(&mut self, value: &TransactionDatum) -> Result<TransactionDatum, Self::Error>;

    /// Read one semantic value component.
    ///
    /// # Errors
    /// Returns a host-defined error for an unsupported value/component pair.
    fn component(
        &self,
        value: &TransactionDatum,
        component: &str,
    ) -> Result<TransactionDatum, Self::Error>;

    /// Mutate one transaction-local semantic value component.
    ///
    /// # Errors
    /// Returns a host-defined error for an unsupported value/component pair.
    fn set_component(
        &mut self,
        value: &TransactionDatum,
        component: &str,
        new_value: TransactionDatum,
    ) -> Result<(), Self::Error>;

    /// Evaluate a source-bound semantic equality operation.
    ///
    /// # Errors
    /// Returns a host-defined error when the values are not comparable.
    fn semantic_equal(
        &self,
        left: &TransactionDatum,
        right: &TransactionDatum,
    ) -> Result<bool, Self::Error>;

    /// Execute one source-neutral semantic host query.
    ///
    /// Queries may be stateful (for example an RNG draw), so the draft is mutable even when a
    /// particular operation is observational.
    ///
    /// # Errors
    /// Returns a host-defined error when the operation or operands are unsupported.
    fn host_query(
        &mut self,
        operation: &str,
        receiver: Option<&TransactionDatum>,
        arguments: &[TransactionDatum],
    ) -> Result<TransactionDatum, Self::Error>;

    /// Read one indexed store value from the draft.
    ///
    /// # Errors
    /// Returns a host-defined error for an invalid store/key.
    fn store_read(
        &self,
        store: &TransactionDatum,
        key: &[TransactionDatum],
    ) -> Result<TransactionDatum, Self::Error>;

    /// Remove an amount from one indexed store value and return the removed value.
    ///
    /// # Errors
    /// Returns a host-defined error for an invalid store/key/amount.
    fn store_take(
        &mut self,
        store: &TransactionDatum,
        key: &[TransactionDatum],
        amount: &TransactionDatum,
    ) -> Result<TransactionDatum, Self::Error>;

    /// Write one indexed store value in the draft.
    ///
    /// # Errors
    /// Returns a host-defined error for an invalid store/key/value.
    fn store_write(
        &mut self,
        store: &TransactionDatum,
        key: &[TransactionDatum],
        value: TransactionDatum,
    ) -> Result<(), Self::Error>;

    /// Mark a store as semantically changed in the draft.
    ///
    /// # Errors
    /// Returns a host-defined error for an invalid store.
    fn store_touch(&mut self, store: &TransactionDatum) -> Result<(), Self::Error>;

    /// Execute one semantic indexed-store query.
    ///
    /// # Errors
    /// Returns a host-defined error when the query arguments are invalid.
    fn store_query(
        &self,
        operation: TransactionStoreQueryKind,
        store: &TransactionDatum,
        key: &[TransactionDatum],
        value: Option<&TransactionDatum>,
        context: &[TransactionDatum],
    ) -> Result<TransactionDatum, Self::Error>;

    /// Write one semantic object-state value in the draft.
    ///
    /// # Errors
    /// Returns a host-defined error for an invalid target/state/value.
    fn state_write(
        &mut self,
        target: &TransactionDatum,
        state: &str,
        value: TransactionDatum,
    ) -> Result<(), Self::Error>;

    /// Atomically publish this completed draft.
    ///
    /// # Errors
    /// Returns a host-defined error when the draft cannot be committed.
    fn commit(self) -> Result<(), Self::Error>
    where
        Self: Sized;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TransactionExecution {
    pub return_value: Option<TransactionDatum>,
    pub steps: usize,
}

#[derive(Debug)]
pub enum TransactionExecutionError<E> {
    Host(E),
    InvalidProgram(String),
    InvalidValue(String),
    StepLimit { limit: usize },
    CallDepthLimit { limit: usize },
}

impl<E: fmt::Display> fmt::Display for TransactionExecutionError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Host(error) => write!(formatter, "transaction host error: {error}"),
            Self::InvalidProgram(message) => {
                write!(formatter, "invalid transaction program: {message}")
            }
            Self::InvalidValue(message) => {
                write!(formatter, "invalid transaction value: {message}")
            }
            Self::StepLimit { limit } => write!(formatter, "transaction exceeded {limit} steps"),
            Self::CallDepthLimit { limit } => {
                write!(formatter, "transaction exceeded {limit} call frames")
            }
        }
    }
}

impl<E: Error + 'static> Error for TransactionExecutionError<E> {}

impl<E> TransactionExecutionError<E> {
    fn host(error: E) -> Self {
        Self::Host(error)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct TransactionExecutionLimits {
    pub max_steps: usize,
    pub max_call_depth: usize,
}

impl Default for TransactionExecutionLimits {
    fn default() -> Self {
        Self {
            max_steps: 100_000,
            max_call_depth: 64,
        }
    }
}

/// Execute a transaction in an isolated host draft and commit only after a successful return.
///
/// # Errors
///
/// Returns host errors, malformed-program/value errors, or configured execution-limit failures.
pub fn execute_transaction<H: TransactionHost>(
    program: &TransactionProgram,
    host: &mut H,
    receiver: Option<TransactionDatum>,
    arguments: Vec<TransactionDatum>,
    limits: TransactionExecutionLimits,
) -> Result<TransactionExecution, TransactionExecutionError<H::Error>> {
    let mut draft = host.begin().map_err(TransactionExecutionError::host)?;
    let (return_value, steps) = {
        let mut executor = Executor {
            program,
            draft: &mut draft,
            steps: 0,
            limits,
        };
        let return_value = executor.execute_function(program.entry, receiver, arguments, 0)?;
        (return_value, executor.steps)
    };
    draft.commit().map_err(TransactionExecutionError::host)?;
    Ok(TransactionExecution {
        return_value,
        steps,
    })
}

struct Executor<'a, D: TransactionDraft> {
    program: &'a TransactionProgram,
    draft: &'a mut D,
    steps: usize,
    limits: TransactionExecutionLimits,
}

struct Frame {
    receiver: Option<TransactionDatum>,
    arguments: Vec<TransactionDatum>,
    locals: Vec<Option<TransactionDatum>>,
}

impl<D: TransactionDraft> Executor<'_, D> {
    fn execute_function(
        &mut self,
        function_id: TransactionFunctionId,
        receiver: Option<TransactionDatum>,
        arguments: Vec<TransactionDatum>,
        depth: usize,
    ) -> Result<Option<TransactionDatum>, TransactionExecutionError<D::Error>> {
        if depth > self.limits.max_call_depth {
            return Err(TransactionExecutionError::CallDepthLimit {
                limit: self.limits.max_call_depth,
            });
        }
        let function = self.function(function_id)?.clone();
        if arguments.len() != function.parameter_count {
            return Err(TransactionExecutionError::InvalidProgram(format!(
                "function {:?} expects {} parameters, got {}",
                function.id,
                function.parameter_count,
                arguments.len()
            )));
        }
        let local_count = usize::try_from(function.local_count).map_err(|_| {
            TransactionExecutionError::InvalidProgram(String::from(
                "local count does not fit usize",
            ))
        })?;
        let mut frame = Frame {
            receiver,
            arguments,
            locals: vec![None; local_count],
        };
        let mut pc = function.entry_statement;
        loop {
            self.bump_step()?;
            let statement = function
                .statements
                .iter()
                .find(|statement| statement.id == pc)
                .ok_or_else(|| {
                    TransactionExecutionError::InvalidProgram(format!(
                        "statement {:?} missing from function {:?}",
                        pc, function.id
                    ))
                })?;
            match &statement.operation {
                TransactionOperation::Assign {
                    target,
                    value,
                    next,
                } => {
                    let value = self.evaluate(&frame, value, depth)?;
                    Self::set_local(&mut frame, *target, value)?;
                    pc = *next;
                }
                TransactionOperation::SetComponent {
                    target,
                    component,
                    value,
                    next,
                } => {
                    let target = self.evaluate(&frame, target, depth)?;
                    let value = self.evaluate(&frame, value, depth)?;
                    self.draft
                        .set_component(&target, component, value)
                        .map_err(TransactionExecutionError::host)?;
                    pc = *next;
                }
                TransactionOperation::Effect { effect, next } => {
                    self.execute_effect(&frame, effect, depth)?;
                    pc = *next;
                }
                TransactionOperation::Evaluate { value, next } => {
                    self.evaluate(&frame, value, depth)?;
                    pc = *next;
                }
                TransactionOperation::Branch {
                    condition,
                    when_false,
                    when_true,
                } => {
                    let condition = self.evaluate(&frame, condition, depth)?;
                    pc = if truthy(&condition)? {
                        *when_true
                    } else {
                        *when_false
                    };
                }
                TransactionOperation::Jump { target } => pc = *target,
                TransactionOperation::Return { value } => {
                    return value
                        .as_ref()
                        .map(|value| self.evaluate(&frame, value, depth))
                        .transpose();
                }
            }
        }
    }

    fn evaluate(
        &mut self,
        frame: &Frame,
        value: &TransactionValue,
        depth: usize,
    ) -> Result<TransactionDatum, TransactionExecutionError<D::Error>> {
        match value {
            TransactionValue::Local { local } => Self::local(frame, *local),
            TransactionValue::Parameter { index } => {
                frame.arguments.get(*index).cloned().ok_or_else(|| {
                    TransactionExecutionError::InvalidProgram(format!(
                        "parameter {index} is missing"
                    ))
                })
            }
            TransactionValue::This => frame.receiver.clone().ok_or_else(|| {
                TransactionExecutionError::InvalidProgram(String::from(
                    "transaction `this` is missing",
                ))
            }),
            TransactionValue::Integer { value } => Ok(TransactionDatum::Integer(*value)),
            TransactionValue::Float { value } => Ok(TransactionDatum::Float(*value)),
            TransactionValue::String { value } => Ok(TransactionDatum::String(value.clone())),
            TransactionValue::Null => Ok(TransactionDatum::Null),
            TransactionValue::Sequence { values } => self
                .evaluate_many(frame, values, depth)
                .map(TransactionDatum::Sequence),
            TransactionValue::NumericCast { target, value } => {
                let value = self.evaluate(frame, value, depth)?;
                numeric_cast(*target, value)
            }
            TransactionValue::Binary {
                operator,
                left,
                right,
            } => {
                let left = self.evaluate(frame, left, depth)?;
                let right = self.evaluate(frame, right, depth)?;
                binary(operator, left, right)
            }
            TransactionValue::Unary { operator, value } => {
                let value = self.evaluate(frame, value, depth)?;
                unary(operator, value)
            }
            TransactionValue::Index { sequence, index } => {
                let sequence = self.evaluate(frame, sequence, depth)?;
                let index = self.evaluate(frame, index, depth)?;
                index_value(sequence, &index)
            }
            TransactionValue::Length { sequence } => {
                let sequence = self.evaluate(frame, sequence, depth)?;
                length_value(sequence)
            }
            TransactionValue::Min { left, right } => {
                let left = self.evaluate(frame, left, depth)?;
                let right = self.evaluate(frame, right, depth)?;
                min_value(left, right)
            }
            TransactionValue::HasCapability { .. }
            | TransactionValue::HostQuery { .. }
            | TransactionValue::StoreRead { .. }
            | TransactionValue::StoreTake { .. }
            | TransactionValue::StoreQuery { .. }
            | TransactionValue::Component { .. }
            | TransactionValue::CloneValue { .. }
            | TransactionValue::Equal { .. }
            | TransactionValue::Call { .. } => self.evaluate_host_value(frame, value, depth),
        }
    }

    fn evaluate_host_value(
        &mut self,
        frame: &Frame,
        value: &TransactionValue,
        depth: usize,
    ) -> Result<TransactionDatum, TransactionExecutionError<D::Error>> {
        match value {
            TransactionValue::HostQuery {
                operation,
                receiver,
                arguments,
            } => {
                let receiver = receiver
                    .as_ref()
                    .map(|receiver| self.evaluate(frame, receiver, depth))
                    .transpose()?;
                let arguments = self.evaluate_many(frame, arguments, depth)?;
                self.draft
                    .host_query(operation, receiver.as_ref(), &arguments)
                    .map_err(TransactionExecutionError::host)
            }
            TransactionValue::HasCapability { value, capability } => {
                let value = self.evaluate(frame, value, depth)?;
                let has = self
                    .draft
                    .has_capability(&value, capability)
                    .map_err(TransactionExecutionError::host)?;
                Ok(TransactionDatum::Boolean(has))
            }
            TransactionValue::StoreRead { store, key } => {
                let store = self.evaluate(frame, store, depth)?;
                let key = self.evaluate_many(frame, key, depth)?;
                self.draft
                    .store_read(&store, &key)
                    .map_err(TransactionExecutionError::host)
            }
            TransactionValue::StoreTake { store, key, amount } => {
                let store = self.evaluate(frame, store, depth)?;
                let key = self.evaluate_many(frame, key, depth)?;
                let amount = self.evaluate(frame, amount, depth)?;
                self.draft
                    .store_take(&store, &key, &amount)
                    .map_err(TransactionExecutionError::host)
            }
            TransactionValue::StoreQuery {
                operation,
                store,
                key,
                value,
                context,
            } => {
                let store = self.evaluate(frame, store, depth)?;
                let key = self.evaluate_many(frame, key, depth)?;
                let value = value
                    .as_ref()
                    .map(|value| self.evaluate(frame, value, depth))
                    .transpose()?;
                let context = self.evaluate_many(frame, context, depth)?;
                self.draft
                    .store_query(*operation, &store, &key, value.as_ref(), &context)
                    .map_err(TransactionExecutionError::host)
            }
            TransactionValue::Component { value, component } => {
                let value = self.evaluate(frame, value, depth)?;
                self.draft
                    .component(&value, component)
                    .map_err(TransactionExecutionError::host)
            }
            TransactionValue::CloneValue { value } => {
                let value = self.evaluate(frame, value, depth)?;
                self.draft
                    .clone_value(&value)
                    .map_err(TransactionExecutionError::host)
            }
            TransactionValue::Equal { left, right } => {
                let left = self.evaluate(frame, left, depth)?;
                let right = self.evaluate(frame, right, depth)?;
                let equal = self
                    .draft
                    .semantic_equal(&left, &right)
                    .map_err(TransactionExecutionError::host)?;
                Ok(TransactionDatum::Boolean(equal))
            }
            TransactionValue::Call {
                function,
                receiver,
                arguments,
            } => {
                let receiver = receiver
                    .as_ref()
                    .map(|receiver| self.evaluate(frame, receiver, depth))
                    .transpose()?;
                let arguments = self.evaluate_many(frame, arguments, depth)?;
                self.execute_function(*function, receiver, arguments, depth + 1)?
                    .ok_or_else(|| {
                        TransactionExecutionError::InvalidProgram(format!(
                            "value call to function {function:?} returned void"
                        ))
                    })
            }
            _ => Err(TransactionExecutionError::InvalidProgram(String::from(
                "non-host transaction value reached host evaluator",
            ))),
        }
    }

    fn execute_effect(
        &mut self,
        frame: &Frame,
        effect: &TransactionEffect,
        depth: usize,
    ) -> Result<(), TransactionExecutionError<D::Error>> {
        match effect {
            TransactionEffect::StoreWrite { store, key, value } => {
                let store = self.evaluate(frame, store, depth)?;
                let key = self.evaluate_many(frame, key, depth)?;
                let value = self.evaluate(frame, value, depth)?;
                self.draft
                    .store_write(&store, &key, value)
                    .map_err(TransactionExecutionError::host)
            }
            TransactionEffect::StoreTouch { store } => {
                let store = self.evaluate(frame, store, depth)?;
                self.draft
                    .store_touch(&store)
                    .map_err(TransactionExecutionError::host)
            }
            TransactionEffect::StateWrite {
                target,
                state,
                value,
            } => {
                let target = self.evaluate(frame, target, depth)?;
                let value = self.evaluate(frame, value, depth)?;
                self.draft
                    .state_write(&target, state, value)
                    .map_err(TransactionExecutionError::host)
            }
            TransactionEffect::Call {
                function,
                receiver,
                arguments,
            } => {
                let receiver = receiver
                    .as_ref()
                    .map(|receiver| self.evaluate(frame, receiver, depth))
                    .transpose()?;
                let arguments = self.evaluate_many(frame, arguments, depth)?;
                self.execute_function(*function, receiver, arguments, depth + 1)?;
                Ok(())
            }
        }
    }

    fn evaluate_many(
        &mut self,
        frame: &Frame,
        values: &[TransactionValue],
        depth: usize,
    ) -> Result<Vec<TransactionDatum>, TransactionExecutionError<D::Error>> {
        values
            .iter()
            .map(|value| self.evaluate(frame, value, depth))
            .collect()
    }

    fn function(
        &self,
        id: TransactionFunctionId,
    ) -> Result<&TransactionFunction, TransactionExecutionError<D::Error>> {
        self.program
            .functions
            .iter()
            .find(|function| function.id == id)
            .ok_or_else(|| {
                TransactionExecutionError::InvalidProgram(format!(
                    "transaction function {id:?} is missing"
                ))
            })
    }

    fn local(
        frame: &Frame,
        id: TransactionLocalId,
    ) -> Result<TransactionDatum, TransactionExecutionError<D::Error>> {
        let index = usize::try_from(id.0).map_err(|_| {
            TransactionExecutionError::InvalidProgram(String::from("local ID does not fit usize"))
        })?;
        frame
            .locals
            .get(index)
            .and_then(Clone::clone)
            .ok_or_else(|| {
                TransactionExecutionError::InvalidProgram(format!(
                    "transaction local {id:?} is uninitialized"
                ))
            })
    }

    fn set_local(
        frame: &mut Frame,
        id: TransactionLocalId,
        value: TransactionDatum,
    ) -> Result<(), TransactionExecutionError<D::Error>> {
        let index = usize::try_from(id.0).map_err(|_| {
            TransactionExecutionError::InvalidProgram(String::from("local ID does not fit usize"))
        })?;
        let local = frame.locals.get_mut(index).ok_or_else(|| {
            TransactionExecutionError::InvalidProgram(format!(
                "transaction local {id:?} is outside the frame"
            ))
        })?;
        *local = Some(value);
        Ok(())
    }

    fn bump_step(&mut self) -> Result<(), TransactionExecutionError<D::Error>> {
        if self.steps >= self.limits.max_steps {
            return Err(TransactionExecutionError::StepLimit {
                limit: self.limits.max_steps,
            });
        }
        self.steps += 1;
        Ok(())
    }
}

fn truthy<E>(value: &TransactionDatum) -> Result<bool, TransactionExecutionError<E>> {
    match value {
        TransactionDatum::Boolean(value) => Ok(*value),
        TransactionDatum::Integer(value) => Ok(*value != 0),
        other => Err(TransactionExecutionError::InvalidValue(format!(
            "branch condition is not boolean/integer: {other:?}"
        ))),
    }
}

fn binary<E>(
    operator: &str,
    left: TransactionDatum,
    right: TransactionDatum,
) -> Result<TransactionDatum, TransactionExecutionError<E>> {
    match operator {
        "==" => Ok(TransactionDatum::Boolean(cfg_equal(&left, &right))),
        "!=" => Ok(TransactionDatum::Boolean(!cfg_equal(&left, &right))),
        "+" | "-" | "*" | "/" | "%" | "<" | "<=" | ">" | ">=" | "cmpg" | "cmpl" => {
            binary_numeric(operator, left, right)
        }
        _ => Err(TransactionExecutionError::InvalidValue(format!(
            "unsupported binary operator {operator:?}"
        ))),
    }
}

fn cfg_equal(left: &TransactionDatum, right: &TransactionDatum) -> bool {
    match (left, right) {
        (TransactionDatum::Boolean(left), TransactionDatum::Integer(right))
        | (TransactionDatum::Integer(right), TransactionDatum::Boolean(left)) => {
            *right == i64::from(*left)
        }
        _ => left == right,
    }
}

fn binary_numeric<E>(
    operator: &str,
    left: TransactionDatum,
    right: TransactionDatum,
) -> Result<TransactionDatum, TransactionExecutionError<E>> {
    match (left, right) {
        (TransactionDatum::Integer(left), TransactionDatum::Integer(right)) => {
            integer_binary(operator, left, right)
        }
        (TransactionDatum::Float(left), TransactionDatum::Float(right)) => {
            float_binary(operator, left, right)
        }
        (left, right) => Err(TransactionExecutionError::InvalidValue(format!(
            "numeric operator {operator:?} received {left:?} and {right:?}"
        ))),
    }
}

fn integer_binary<E>(
    operator: &str,
    left: i64,
    right: i64,
) -> Result<TransactionDatum, TransactionExecutionError<E>> {
    let value = match operator {
        "+" => TransactionDatum::Integer(left.wrapping_add(right)),
        "-" => TransactionDatum::Integer(left.wrapping_sub(right)),
        "*" => TransactionDatum::Integer(left.wrapping_mul(right)),
        "/" if right != 0 => TransactionDatum::Integer(left.wrapping_div(right)),
        "%" if right != 0 => TransactionDatum::Integer(left.wrapping_rem(right)),
        "<" => TransactionDatum::Boolean(left < right),
        "<=" => TransactionDatum::Boolean(left <= right),
        ">" => TransactionDatum::Boolean(left > right),
        ">=" => TransactionDatum::Boolean(left >= right),
        "/" | "%" => {
            return Err(TransactionExecutionError::InvalidValue(String::from(
                "integer division by zero",
            )));
        }
        _ => {
            return Err(TransactionExecutionError::InvalidValue(format!(
                "unsupported integer operator {operator:?}"
            )));
        }
    };
    Ok(value)
}

fn float_binary<E>(
    operator: &str,
    left: f64,
    right: f64,
) -> Result<TransactionDatum, TransactionExecutionError<E>> {
    let value = match operator {
        "+" => TransactionDatum::Float(left + right),
        "-" => TransactionDatum::Float(left - right),
        "*" => TransactionDatum::Float(left * right),
        "/" => TransactionDatum::Float(left / right),
        "%" => TransactionDatum::Float(left % right),
        "<" => TransactionDatum::Boolean(left < right),
        "<=" => TransactionDatum::Boolean(left <= right),
        ">" => TransactionDatum::Boolean(left > right),
        ">=" => TransactionDatum::Boolean(left >= right),
        "cmpg" => TransactionDatum::Integer(i64::from(float_compare(left, right, 1))),
        "cmpl" => TransactionDatum::Integer(i64::from(float_compare(left, right, -1))),
        _ => {
            return Err(TransactionExecutionError::InvalidValue(format!(
                "unsupported float operator {operator:?}"
            )));
        }
    };
    Ok(value)
}

fn float_compare(left: f64, right: f64, nan_result: i32) -> i32 {
    if left.is_nan() || right.is_nan() {
        nan_result
    } else if left > right {
        1
    } else if left < right {
        -1
    } else {
        0
    }
}

fn numeric_cast<E>(
    target: TransactionNumericType,
    value: TransactionDatum,
) -> Result<TransactionDatum, TransactionExecutionError<E>> {
    match (target, value) {
        (TransactionNumericType::I32, TransactionDatum::Integer(value)) => {
            let low = value & 0xffff_ffff;
            let narrowed = if low >= 0x8000_0000 {
                low - 0x1_0000_0000
            } else {
                low
            };
            Ok(TransactionDatum::Integer(narrowed))
        }
        (TransactionNumericType::I32, TransactionDatum::Float(value)) => {
            Ok(TransactionDatum::Integer(java_f64_to_i32(value)))
        }
        (TransactionNumericType::F64, TransactionDatum::Integer(value)) => {
            let value = i32::try_from(value).map_err(|_| {
                TransactionExecutionError::InvalidValue(String::from(
                    "i64-to-f64 transaction cast requires an i32 source",
                ))
            })?;
            Ok(TransactionDatum::Float(f64::from(value)))
        }
        (TransactionNumericType::F64, TransactionDatum::Float(value)) => {
            Ok(TransactionDatum::Float(value))
        }
        (target, value) => Err(TransactionExecutionError::InvalidValue(format!(
            "unsupported numeric cast {target:?} for {value:?}"
        ))),
    }
}

fn java_f64_to_i32(value: f64) -> i64 {
    if value.is_nan() {
        return 0;
    }
    if value >= f64::from(i32::MAX) {
        return i64::from(i32::MAX);
    }
    if value <= f64::from(i32::MIN) {
        return i64::from(i32::MIN);
    }
    let bits = value.to_bits();
    let negative = (bits >> 63) != 0;
    let exponent = i32::try_from((bits >> 52) & 0x7ff).unwrap_or(0) - 1023;
    if exponent < 0 {
        return 0;
    }
    let significand = (1_u64 << 52) | (bits & ((1_u64 << 52) - 1));
    let magnitude = if exponent >= 52 {
        significand << u32::try_from(exponent - 52).unwrap_or(0)
    } else {
        significand >> u32::try_from(52 - exponent).unwrap_or(0)
    };
    let magnitude = i64::try_from(magnitude).unwrap_or(i64::MAX);
    if negative { -magnitude } else { magnitude }
}

fn unary<E>(
    operator: &str,
    value: TransactionDatum,
) -> Result<TransactionDatum, TransactionExecutionError<E>> {
    match (operator, value) {
        ("-", TransactionDatum::Integer(value)) => Ok(TransactionDatum::Integer(-value)),
        ("-", TransactionDatum::Float(value)) => Ok(TransactionDatum::Float(-value)),
        ("!", TransactionDatum::Boolean(value)) => Ok(TransactionDatum::Boolean(!value)),
        ("!", TransactionDatum::Integer(value)) => Ok(TransactionDatum::Boolean(value == 0)),
        (_, value) => Err(TransactionExecutionError::InvalidValue(format!(
            "unsupported unary operator {operator:?} for {value:?}"
        ))),
    }
}

fn index_value<E>(
    sequence: TransactionDatum,
    index: &TransactionDatum,
) -> Result<TransactionDatum, TransactionExecutionError<E>> {
    let TransactionDatum::Sequence(values) = sequence else {
        return Err(TransactionExecutionError::InvalidValue(String::from(
            "index target is not a sequence",
        )));
    };
    let TransactionDatum::Integer(index) = index else {
        return Err(TransactionExecutionError::InvalidValue(String::from(
            "sequence index is not an integer",
        )));
    };
    let index = usize::try_from(*index).map_err(|_| {
        TransactionExecutionError::InvalidValue(String::from("sequence index is negative"))
    })?;
    values.get(index).cloned().ok_or_else(|| {
        TransactionExecutionError::InvalidValue(format!("sequence index {index} is out of bounds"))
    })
}

fn length_value<E>(
    sequence: TransactionDatum,
) -> Result<TransactionDatum, TransactionExecutionError<E>> {
    let TransactionDatum::Sequence(values) = sequence else {
        return Err(TransactionExecutionError::InvalidValue(String::from(
            "length target is not a sequence",
        )));
    };
    let length = i64::try_from(values.len()).map_err(|_| {
        TransactionExecutionError::InvalidValue(String::from("sequence length exceeds i64"))
    })?;
    Ok(TransactionDatum::Integer(length))
}

fn min_value<E>(
    left: TransactionDatum,
    right: TransactionDatum,
) -> Result<TransactionDatum, TransactionExecutionError<E>> {
    match (left, right) {
        (TransactionDatum::Integer(left), TransactionDatum::Integer(right)) => {
            Ok(TransactionDatum::Integer(left.min(right)))
        }
        (TransactionDatum::Float(left), TransactionDatum::Float(right)) => {
            Ok(TransactionDatum::Float(left.min(right)))
        }
        (left, right) => Err(TransactionExecutionError::InvalidValue(format!(
            "min received {left:?} and {right:?}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::transaction::{TransactionStatement, TransactionStatementId};

    #[derive(Clone, Debug, PartialEq)]
    struct TestItem {
        quantity: i64,
    }

    #[derive(Clone, Debug, PartialEq)]
    struct TestState {
        slots: BTreeMap<(u64, i64), u64>,
        items: BTreeMap<u64, TestItem>,
        next_handle: u64,
    }

    struct TestHost {
        state: TestState,
    }

    struct TestDraft<'a> {
        host: &'a mut TestHost,
        state: TestState,
    }

    impl TransactionHost for TestHost {
        type Error = String;
        type Draft<'a> = TestDraft<'a>;

        fn begin(&mut self) -> Result<Self::Draft<'_>, Self::Error> {
            Ok(TestDraft {
                state: self.state.clone(),
                host: self,
            })
        }
    }

    impl TransactionDraft for TestDraft<'_> {
        type Error = String;

        fn has_capability(
            &self,
            _value: &TransactionDatum,
            _capability: &str,
        ) -> Result<bool, Self::Error> {
            Ok(false)
        }

        fn clone_value(
            &mut self,
            value: &TransactionDatum,
        ) -> Result<TransactionDatum, Self::Error> {
            let handle = handle(value)?;
            let item = self
                .state
                .items
                .get(&handle)
                .cloned()
                .ok_or_else(|| format!("item handle {handle} is missing"))?;
            let clone = self.state.next_handle;
            self.state.next_handle += 1;
            self.state.items.insert(clone, item);
            Ok(TransactionDatum::Handle(clone))
        }

        fn component(
            &self,
            value: &TransactionDatum,
            component: &str,
        ) -> Result<TransactionDatum, Self::Error> {
            if component != "quantity" {
                return Err(format!("unknown component {component}"));
            }
            let handle = handle(value)?;
            let item = self
                .state
                .items
                .get(&handle)
                .ok_or_else(|| format!("item handle {handle} is missing"))?;
            Ok(TransactionDatum::Integer(item.quantity))
        }

        fn set_component(
            &mut self,
            value: &TransactionDatum,
            component: &str,
            new_value: TransactionDatum,
        ) -> Result<(), Self::Error> {
            if component != "quantity" {
                return Err(format!("unknown component {component}"));
            }
            let handle = handle(value)?;
            let quantity = integer(&new_value)?;
            let item = self
                .state
                .items
                .get_mut(&handle)
                .ok_or_else(|| format!("item handle {handle} is missing"))?;
            item.quantity = quantity;
            Ok(())
        }

        fn semantic_equal(
            &self,
            left: &TransactionDatum,
            right: &TransactionDatum,
        ) -> Result<bool, Self::Error> {
            Ok(left == right)
        }

        fn host_query(
            &mut self,
            operation: &str,
            _receiver: Option<&TransactionDatum>,
            _arguments: &[TransactionDatum],
        ) -> Result<TransactionDatum, Self::Error> {
            Err(format!("unsupported test host query {operation}"))
        }

        fn store_read(
            &self,
            store: &TransactionDatum,
            key: &[TransactionDatum],
        ) -> Result<TransactionDatum, Self::Error> {
            let slot = store_key(store, key)?;
            Ok(self
                .state
                .slots
                .get(&slot)
                .copied()
                .map_or(TransactionDatum::Null, TransactionDatum::Handle))
        }

        fn store_take(
            &mut self,
            store: &TransactionDatum,
            key: &[TransactionDatum],
            amount: &TransactionDatum,
        ) -> Result<TransactionDatum, Self::Error> {
            let slot = store_key(store, key)?;
            let amount = integer(amount)?;
            let source = *self
                .state
                .slots
                .get(&slot)
                .ok_or_else(|| String::from("source slot is empty"))?;
            let source_item = self
                .state
                .items
                .get_mut(&source)
                .ok_or_else(|| String::from("source item is missing"))?;
            if amount < 0 || source_item.quantity < amount {
                return Err(String::from("invalid take amount"));
            }
            source_item.quantity -= amount;
            let source_empty = source_item.quantity == 0;
            let taken = self.state.next_handle;
            self.state.next_handle += 1;
            self.state
                .items
                .insert(taken, TestItem { quantity: amount });
            if source_empty {
                self.state.slots.remove(&slot);
            }
            Ok(TransactionDatum::Handle(taken))
        }

        fn store_write(
            &mut self,
            store: &TransactionDatum,
            key: &[TransactionDatum],
            value: TransactionDatum,
        ) -> Result<(), Self::Error> {
            let slot = store_key(store, key)?;
            match value {
                TransactionDatum::Handle(handle) => {
                    self.state.slots.insert(slot, handle);
                }
                TransactionDatum::Null => {
                    self.state.slots.remove(&slot);
                }
                value => return Err(format!("invalid store value {value:?}")),
            }
            Ok(())
        }

        fn store_touch(&mut self, _store: &TransactionDatum) -> Result<(), Self::Error> {
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
            match operation {
                TransactionStoreQueryKind::Extent => {
                    let store = handle(store)?;
                    let extent = self
                        .state
                        .slots
                        .keys()
                        .filter(|(candidate, _)| *candidate == store)
                        .map(|(_, key)| *key)
                        .max()
                        .map_or(0, |key| key + 1);
                    Ok(TransactionDatum::Integer(extent))
                }
                _ => Err(String::from("query not implemented by test host")),
            }
        }

        fn state_write(
            &mut self,
            _target: &TransactionDatum,
            _state: &str,
            _value: TransactionDatum,
        ) -> Result<(), Self::Error> {
            Ok(())
        }

        fn commit(self) -> Result<(), Self::Error> {
            self.host.state = self.state;
            Ok(())
        }
    }

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

    fn store_key(store: &TransactionDatum, key: &[TransactionDatum]) -> Result<(u64, i64), String> {
        let [key] = key else {
            return Err(String::from("expected one store key"));
        };
        Ok((handle(store)?, integer(key)?))
    }

    fn host_with_slot(quantity: i64) -> TestHost {
        TestHost {
            state: TestState {
                slots: BTreeMap::from([((1, 0), 100)]),
                items: BTreeMap::from([
                    (100, TestItem { quantity }),
                    (200, TestItem { quantity: 99 }),
                ]),
                next_handle: 1000,
            },
        }
    }

    fn statement(id: u32, operation: TransactionOperation) -> TransactionStatement {
        TransactionStatement {
            id: TransactionStatementId(id),
            operation,
        }
    }

    fn program(statements: Vec<TransactionStatement>, local_count: u32) -> TransactionProgram {
        TransactionProgram {
            entry: TransactionFunctionId(0),
            functions: vec![TransactionFunction {
                id: TransactionFunctionId(0),
                parameter_count: 2,
                local_count,
                entry_statement: TransactionStatementId(0),
                statements,
            }],
        }
    }

    fn store_write_effect(value: TransactionValue) -> TransactionEffect {
        TransactionEffect::StoreWrite {
            store: TransactionValue::Parameter { index: 0 },
            key: vec![TransactionValue::Integer { value: 0 }],
            value,
        }
    }

    #[test]
    fn java_numeric_casts_and_float_compare_preserve_jvm_edges() -> Result<(), String> {
        let cast = |value| {
            numeric_cast::<String>(TransactionNumericType::I32, value)
                .map_err(|error| error.to_string())
        };
        assert_eq!(
            cast(TransactionDatum::Float(-1.75))?,
            TransactionDatum::Integer(-1)
        );
        assert_eq!(
            cast(TransactionDatum::Float(f64::NAN))?,
            TransactionDatum::Integer(0)
        );
        assert_eq!(
            cast(TransactionDatum::Float(f64::INFINITY))?,
            TransactionDatum::Integer(i64::from(i32::MAX))
        );
        assert_eq!(
            binary::<String>(
                "cmpg",
                TransactionDatum::Float(f64::NAN),
                TransactionDatum::Float(0.0),
            )
            .map_err(|error| error.to_string())?,
            TransactionDatum::Integer(1)
        );
        assert_eq!(
            binary::<String>(
                "cmpl",
                TransactionDatum::Float(f64::NAN),
                TransactionDatum::Float(0.0),
            )
            .map_err(|error| error.to_string())?,
            TransactionDatum::Integer(-1)
        );
        assert_eq!(
            binary::<String>(
                "==",
                TransactionDatum::Boolean(false),
                TransactionDatum::Integer(0),
            )
            .map_err(|error| error.to_string())?,
            TransactionDatum::Boolean(true)
        );
        assert_eq!(
            binary::<String>(
                "!=",
                TransactionDatum::Boolean(true),
                TransactionDatum::Integer(1),
            )
            .map_err(|error| error.to_string())?,
            TransactionDatum::Boolean(false)
        );
        Ok(())
    }

    #[test]
    fn draft_commits_only_after_successful_top_level_return() -> Result<(), String> {
        let success = program(
            vec![
                statement(
                    0,
                    TransactionOperation::Effect {
                        effect: store_write_effect(TransactionValue::Parameter { index: 1 }),
                        next: TransactionStatementId(1),
                    },
                ),
                statement(1, TransactionOperation::Return { value: None }),
            ],
            0,
        );
        let mut host = host_with_slot(5);
        execute_transaction(
            &success,
            &mut host,
            None,
            vec![TransactionDatum::Handle(1), TransactionDatum::Handle(200)],
            TransactionExecutionLimits::default(),
        )
        .map_err(|error| error.to_string())?;
        assert_eq!(host.state.slots.get(&(1, 0)), Some(&200));

        let failure = program(
            vec![statement(
                0,
                TransactionOperation::Effect {
                    effect: store_write_effect(TransactionValue::Parameter { index: 1 }),
                    next: TransactionStatementId(99),
                },
            )],
            0,
        );
        let mut host = host_with_slot(5);
        let result = execute_transaction(
            &failure,
            &mut host,
            None,
            vec![TransactionDatum::Handle(1), TransactionDatum::Handle(200)],
            TransactionExecutionLimits::default(),
        );
        assert!(matches!(
            result,
            Err(TransactionExecutionError::InvalidProgram(_))
        ));
        assert_eq!(host.state.slots.get(&(1, 0)), Some(&100));
        Ok(())
    }

    #[test]
    fn speculative_take_and_private_mutation_can_restore_snapshot_before_commit()
    -> Result<(), String> {
        let store_read = TransactionValue::StoreRead {
            store: Box::new(TransactionValue::Parameter { index: 0 }),
            key: vec![TransactionValue::Integer { value: 0 }],
        };
        let transaction = program(
            vec![
                statement(
                    0,
                    TransactionOperation::Assign {
                        target: TransactionLocalId(0),
                        value: TransactionValue::CloneValue {
                            value: Box::new(store_read),
                        },
                        next: TransactionStatementId(1),
                    },
                ),
                statement(
                    1,
                    TransactionOperation::Assign {
                        target: TransactionLocalId(1),
                        value: TransactionValue::StoreTake {
                            store: Box::new(TransactionValue::Parameter { index: 0 }),
                            key: vec![TransactionValue::Integer { value: 0 }],
                            amount: Box::new(TransactionValue::Integer { value: 1 }),
                        },
                        next: TransactionStatementId(2),
                    },
                ),
                statement(
                    2,
                    TransactionOperation::SetComponent {
                        target: TransactionValue::Local {
                            local: TransactionLocalId(1),
                        },
                        component: String::from("quantity"),
                        value: TransactionValue::Integer { value: 0 },
                        next: TransactionStatementId(3),
                    },
                ),
                statement(
                    3,
                    TransactionOperation::Effect {
                        effect: store_write_effect(TransactionValue::Local {
                            local: TransactionLocalId(0),
                        }),
                        next: TransactionStatementId(4),
                    },
                ),
                statement(
                    4,
                    TransactionOperation::Return {
                        value: Some(TransactionValue::Integer { value: 0 }),
                    },
                ),
            ],
            2,
        );
        let mut host = host_with_slot(5);
        let execution = execute_transaction(
            &transaction,
            &mut host,
            None,
            vec![TransactionDatum::Handle(1), TransactionDatum::Null],
            TransactionExecutionLimits::default(),
        )
        .map_err(|error| error.to_string())?;
        assert_eq!(execution.return_value, Some(TransactionDatum::Integer(0)));
        let restored = *host
            .state
            .slots
            .get(&(1, 0))
            .ok_or_else(|| String::from("slot must be restored"))?;
        assert_eq!(
            host.state.items.get(&restored),
            Some(&TestItem { quantity: 5 })
        );
        assert_ne!(
            restored, 100,
            "restore should use the independent cloned snapshot"
        );
        Ok(())
    }
}
