//! Source-neutral executable transaction control-flow IR.
//!
//! This IR is intentionally smaller than JVM/Jimple. Importers erase source-language types,
//! method names, field names, and mutable implementation objects into semantic capabilities,
//! indexed stores, value components, and object-state operations before runtime execution.

use serde::{Deserialize, Serialize};

mod memory;
mod runtime;
pub use memory::{
    MemoryTransactionHost, MemoryTransactionHostCall, MemoryTransactionHostQuery,
    MemoryTransactionObject, MemoryTransactionPerformedHostCall, MemoryTransactionScheduleOnce,
    MemoryTransactionState, MemoryTransactionStore,
};
pub use runtime::{
    TransactionDatum, TransactionDraft, TransactionExecution, TransactionExecutionError,
    TransactionExecutionLimits, TransactionHost, execute_transaction,
};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct TransactionFunctionId(pub u32);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct TransactionStatementId(pub u32);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct TransactionLocalId(pub u32);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TransactionProgram {
    pub entry: TransactionFunctionId,
    pub functions: Vec<TransactionFunction>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TransactionFunction {
    pub id: TransactionFunctionId,
    pub parameter_count: usize,
    pub local_count: u32,
    pub entry_statement: TransactionStatementId,
    pub statements: Vec<TransactionStatement>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TransactionStatement {
    pub id: TransactionStatementId,
    #[serde(flatten)]
    pub operation: TransactionOperation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TransactionSwitchCase {
    pub value: i64,
    pub target: TransactionStatementId,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TransactionOperation {
    Assign {
        target: TransactionLocalId,
        value: TransactionValue,
        next: TransactionStatementId,
    },
    SetComponent {
        target: TransactionValue,
        component: String,
        value: TransactionValue,
        next: TransactionStatementId,
    },
    Effect {
        effect: TransactionEffect,
        next: TransactionStatementId,
    },
    Evaluate {
        value: TransactionValue,
        next: TransactionStatementId,
    },
    Branch {
        condition: TransactionValue,
        when_false: TransactionStatementId,
        when_true: TransactionStatementId,
    },
    Switch {
        key: TransactionValue,
        cases: Vec<TransactionSwitchCase>,
        default_target: TransactionStatementId,
    },
    Jump {
        target: TransactionStatementId,
    },
    Return {
        value: Option<TransactionValue>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransactionNumericType {
    I32,
    F64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransactionStoreQueryKind {
    Extent,
    Keys,
    Empty,
    Full,
    AllowsRead,
    AllowsWrite,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TransactionEffect {
    StoreWrite {
        store: TransactionValue,
        key: Vec<TransactionValue>,
        value: TransactionValue,
    },
    StoreTouch {
        store: TransactionValue,
    },
    StateWrite {
        target: TransactionValue,
        state: String,
        value: TransactionValue,
    },
    ScheduleOnce {
        key: Vec<TransactionValue>,
        delay: TransactionValue,
        priority: TransactionValue,
    },
    Call {
        function: TransactionFunctionId,
        receiver: Option<TransactionValue>,
        arguments: Vec<TransactionValue>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TransactionValue {
    Local {
        local: TransactionLocalId,
    },
    Parameter {
        index: usize,
    },
    This,
    Integer {
        value: i64,
    },
    Float {
        value: f64,
    },
    String {
        value: String,
    },
    Null,
    Sequence {
        values: Vec<Self>,
    },
    NumericCast {
        target: TransactionNumericType,
        value: Box<Self>,
    },
    Binary {
        operator: String,
        left: Box<Self>,
        right: Box<Self>,
    },
    Unary {
        operator: String,
        value: Box<Self>,
    },
    Index {
        sequence: Box<Self>,
        index: Box<Self>,
    },
    Length {
        sequence: Box<Self>,
    },
    HasCapability {
        value: Box<Self>,
        capability: String,
    },
    HostQuery {
        operation: String,
        receiver: Option<Box<Self>>,
        arguments: Vec<Self>,
    },
    HostCall {
        operation: String,
        receiver: Option<Box<Self>>,
        arguments: Vec<Self>,
    },
    StoreRead {
        store: Box<Self>,
        key: Vec<Self>,
    },
    StoreTake {
        store: Box<Self>,
        key: Vec<Self>,
        amount: Box<Self>,
    },
    StoreQuery {
        operation: TransactionStoreQueryKind,
        store: Box<Self>,
        key: Vec<Self>,
        value: Option<Box<Self>>,
        context: Vec<Self>,
    },
    Component {
        value: Box<Self>,
        component: String,
    },
    CloneValue {
        value: Box<Self>,
    },
    Equal {
        left: Box<Self>,
        right: Box<Self>,
    },
    Min {
        left: Box<Self>,
        right: Box<Self>,
    },
    Max {
        left: Box<Self>,
        right: Box<Self>,
    },
    Call {
        function: TransactionFunctionId,
        receiver: Option<Box<Self>>,
        arguments: Vec<Self>,
    },
}
