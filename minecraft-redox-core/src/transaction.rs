//! Source-neutral executable transaction control-flow IR.
//!
//! This IR is intentionally smaller than JVM/Jimple. Importers erase source-language types,
//! method names, field names, and mutable implementation objects into semantic capabilities,
//! indexed stores, value components, and object-state operations before runtime execution.

use serde::{Deserialize, Serialize};

mod memory;
mod runtime;
pub use memory::{
    MemoryTransactionHost, MemoryTransactionHostQuery, MemoryTransactionObject,
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
        values: Vec<TransactionValue>,
    },
    NumericCast {
        target: TransactionNumericType,
        value: Box<TransactionValue>,
    },
    Binary {
        operator: String,
        left: Box<TransactionValue>,
        right: Box<TransactionValue>,
    },
    Unary {
        operator: String,
        value: Box<TransactionValue>,
    },
    Index {
        sequence: Box<TransactionValue>,
        index: Box<TransactionValue>,
    },
    Length {
        sequence: Box<TransactionValue>,
    },
    HasCapability {
        value: Box<TransactionValue>,
        capability: String,
    },
    HostQuery {
        operation: String,
        receiver: Option<Box<TransactionValue>>,
        arguments: Vec<TransactionValue>,
    },
    StoreRead {
        store: Box<TransactionValue>,
        key: Vec<TransactionValue>,
    },
    StoreTake {
        store: Box<TransactionValue>,
        key: Vec<TransactionValue>,
        amount: Box<TransactionValue>,
    },
    StoreQuery {
        operation: TransactionStoreQueryKind,
        store: Box<TransactionValue>,
        key: Vec<TransactionValue>,
        value: Option<Box<TransactionValue>>,
        context: Vec<TransactionValue>,
    },
    Component {
        value: Box<TransactionValue>,
        component: String,
    },
    CloneValue {
        value: Box<TransactionValue>,
    },
    Equal {
        left: Box<TransactionValue>,
        right: Box<TransactionValue>,
    },
    Min {
        left: Box<TransactionValue>,
        right: Box<TransactionValue>,
    },
    Call {
        function: TransactionFunctionId,
        receiver: Option<Box<TransactionValue>>,
        arguments: Vec<TransactionValue>,
    },
}
