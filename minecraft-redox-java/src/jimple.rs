//! Structured Jimple interchange and generic provenance recovery.
//!
//! This module intentionally contains no `GregTech` vocabulary. The Java-side
//! exporter lowers JVM bytecode to structured Jimple; Rust then follows defs,
//! uses and expression structure without parsing Java or Jimple text.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

mod program;
pub use program::{
    AbstractMapEntry, AbstractMapSnapshot, AbstractOpaqueCallShape, AbstractStaticFieldValue,
    JimpleClockedStateCandidate, JimpleEffectCallFrame, JimpleEffectGuard, JimpleProgram,
    JimpleProgramEffectSummary, JimpleProgramFieldWrite, JimpleUnresolvedEffectCall,
    JimpleUnresolvedEffectReason, ProgramExecution,
};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Deserialize, Serialize)]
pub struct JimpleFieldSignature {
    pub owner: String,
    pub name: String,
    #[serde(rename = "type")]
    pub type_name: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct JimpleMethodSignature {
    pub owner: String,
    pub name: String,
    pub return_type: String,
    pub parameter_types: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct JimpleMethod {
    pub signature: JimpleMethodSignature,
    pub display_signature: String,
    pub statements: Vec<JimpleStatement>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct JimpleStatement {
    pub id: usize,
    pub kind: String,
    pub text: String,
    pub def: Option<String>,
    pub uses: Vec<JimpleValueSummary>,
    pub successors: Vec<usize>,
    pub operation: JimpleOperation,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct JimpleValueSummary {
    pub kind: String,
    #[serde(rename = "type")]
    pub type_name: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct JimpleValue {
    pub jimple_kind: String,
    #[serde(rename = "type")]
    pub type_name: String,
    pub text: String,
    #[serde(flatten)]
    pub kind: JimpleValueKind,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JimpleValueKind {
    Local {
        name: String,
    },
    IntegerConstant {
        value: i64,
    },
    FloatConstant {
        value: f64,
    },
    StringConstant {
        value: String,
    },
    ClassConstant {
        class_type: String,
    },
    MethodHandle {
        handle_kind: String,
        method: JimpleMethodSignature,
    },
    MethodType {
        return_type: String,
        parameter_types: Vec<String>,
    },
    NullConstant,
    Parameter {
        index: usize,
    },
    This,
    ArrayRef {
        base: Box<JimpleValue>,
        index: Box<JimpleValue>,
    },
    InstanceField {
        base: Box<JimpleValue>,
        field: JimpleFieldSignature,
    },
    StaticField {
        field: JimpleFieldSignature,
    },
    Binary {
        operator: String,
        left: Box<JimpleValue>,
        right: Box<JimpleValue>,
    },
    Cast {
        cast_type: String,
        value: Box<JimpleValue>,
    },
    Unary {
        operator: String,
        value: Box<JimpleValue>,
    },
    ArrayLength {
        value: Box<JimpleValue>,
    },
    New {
        class_type: String,
    },
    NewArray {
        element_type: String,
        size: Box<JimpleValue>,
    },
    NewMultiArray {
        array_type: String,
        sizes: Vec<JimpleValue>,
    },
    Phi {
        values: Vec<JimpleValue>,
    },
    Invoke {
        invoke_kind: String,
        method: JimpleMethodSignature,
        base: Option<Box<JimpleValue>>,
        arguments: Vec<JimpleValue>,
        #[serde(default)]
        bootstrap_method: Option<JimpleMethodSignature>,
        #[serde(default)]
        bootstrap_arguments: Vec<JimpleValue>,
    },
    Opaque,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct JimpleSwitchCase {
    pub value: i64,
    pub target: usize,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JimpleOperation {
    Assign {
        left: JimpleValue,
        right: JimpleValue,
    },
    Identity {
        left: JimpleValue,
        right: JimpleValue,
    },
    If {
        condition: JimpleValue,
    },
    Invoke {
        expression: Option<JimpleValue>,
    },
    Return {
        value: JimpleValue,
    },
    ReturnVoid,
    Throw {
        value: JimpleValue,
    },
    Switch {
        key: JimpleValue,
        cases: Vec<JimpleSwitchCase>,
        default_target: usize,
    },
    Goto,
    Opaque,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum StorageLocation {
    Local(String),
    StaticField(JimpleFieldSignature),
    InstanceField {
        base: Box<StorageLocation>,
        field: JimpleFieldSignature,
    },
    ArrayElement {
        base: Box<StorageLocation>,
        index: i64,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SymbolicValue {
    Integer {
        value: i64,
        type_name: String,
    },
    Float {
        value: f64,
        type_name: String,
    },
    String {
        value: String,
    },
    Null,
    Parameter {
        index: usize,
        type_name: String,
    },
    Local {
        name: String,
        type_name: String,
    },
    This {
        type_name: String,
    },
    StaticField {
        field: JimpleFieldSignature,
    },
    InstanceField {
        base: Box<SymbolicValue>,
        field: JimpleFieldSignature,
    },
    ArrayElement {
        base: Box<SymbolicValue>,
        index: Box<SymbolicValue>,
    },
    Binary {
        operator: String,
        left: Box<SymbolicValue>,
        right: Box<SymbolicValue>,
    },
    Cast {
        type_name: String,
        value: Box<SymbolicValue>,
    },
    Unary {
        operator: String,
        value: Box<SymbolicValue>,
    },
    ArrayLength {
        value: Box<SymbolicValue>,
    },
    New {
        class_type: String,
    },
    NewArray {
        element_type: String,
        size: Box<SymbolicValue>,
    },
    NewMultiArray {
        array_type: String,
        sizes: Vec<SymbolicValue>,
    },
    Phi {
        values: Vec<SymbolicValue>,
    },
    Piecewise {
        arms: Vec<SymbolicPiecewiseArm>,
    },
    Invoke {
        method: JimpleMethodSignature,
        base: Option<Box<SymbolicValue>>,
        arguments: Vec<SymbolicValue>,
    },
    Unknown {
        type_name: String,
        text: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SymbolicPiecewiseArm {
    pub guards: Vec<JimpleGuard>,
    pub value: SymbolicValue,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SymbolicAssignment {
    pub statement_id: usize,
    pub target: SymbolicValue,
    pub value: SymbolicValue,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct JimpleGuard {
    pub branch_statement_id: usize,
    pub expected: bool,
    pub condition: SymbolicValue,
}

#[derive(Clone)]
struct PiecewisePathState {
    statement_id: usize,
    value: Option<SymbolicValue>,
    guards: Vec<JimpleGuard>,
    visited: BTreeSet<usize>,
}

impl PiecewisePathState {
    fn new(statement_id: usize) -> Self {
        Self {
            statement_id,
            value: None,
            guards: Vec::new(),
            visited: BTreeSet::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct JimpleFieldWrite {
    pub statement_id: usize,
    pub guards: Vec<JimpleGuard>,
    pub field: JimpleFieldSignature,
    pub receiver: Option<SymbolicValue>,
    pub value: SymbolicValue,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct JimpleInvocation {
    pub statement_id: usize,
    pub guards: Vec<JimpleGuard>,
    pub invoke_kind: String,
    pub method: JimpleMethodSignature,
    pub receiver: Option<SymbolicValue>,
    pub arguments: Vec<SymbolicValue>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", content = "effect", rename_all = "snake_case")]
pub enum JimpleDirectEffect {
    FieldWrite(JimpleFieldWrite),
    Invocation(JimpleInvocation),
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct JimpleEffectSummary {
    pub method: JimpleMethodSignature,
    pub effects: Vec<JimpleDirectEffect>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct JimpleCounterUpdate {
    pub statement_id: usize,
    pub field: JimpleFieldSignature,
    pub receiver: Option<SymbolicValue>,
    pub delta: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct JimpleFieldConstantWrite {
    pub statement_id: usize,
    pub field: JimpleFieldSignature,
    pub receiver: Option<SymbolicValue>,
    pub value: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct JimpleFieldComparison {
    pub statement_id: usize,
    pub field: JimpleFieldSignature,
    pub receiver: Option<SymbolicValue>,
    pub operator: String,
    pub constant: i64,
    pub field_on_left: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct JimpleTemporalSummary {
    pub method: JimpleMethodSignature,
    pub counter_updates: Vec<JimpleCounterUpdate>,
    pub constant_writes: Vec<JimpleFieldConstantWrite>,
    pub comparisons: Vec<JimpleFieldComparison>,
}

/// Decode methods produced by the structured Jimple exporter.
///
/// # Errors
///
/// Returns an error when the input is not valid structured Jimple JSON.
pub fn parse_methods(json: &str) -> Result<Vec<JimpleMethod>> {
    serde_json::from_str(json).context("failed to decode structured Jimple JSON")
}

impl JimpleMethod {
    #[must_use]
    pub fn statement_containing(&self, text: &str) -> Option<&JimpleStatement> {
        self.statements
            .iter()
            .find(|statement| statement.text.contains(text))
    }

    #[must_use]
    pub fn backward_slice(&self, sink_id: usize) -> Vec<usize> {
        let Some(sink) = self
            .statements
            .iter()
            .find(|statement| statement.id == sink_id)
        else {
            return Vec::new();
        };
        let mut included = BTreeSet::from([sink_id]);
        let mut pending = Vec::new();
        operation_inputs_before(&sink.operation, sink.id, &mut pending);
        while let Some((before_id, value)) = pending.pop() {
            self.add_value_dependencies(before_id, value, &mut included, &mut pending);
        }
        included.into_iter().collect()
    }

    /// Recover direct structural effects from this method without interpreting domain names.
    ///
    /// Local temporaries are resolved at each program point, while heap fields remain state facts.
    /// Calls are reported separately so a program-level analysis can compose exact callees without
    /// pretending ambiguous dynamic dispatch is known.
    #[must_use]
    pub fn effect_summary(&self) -> JimpleEffectSummary {
        let mut effects = Vec::new();

        for statement in &self.statements {
            for value in operation_input_values(&statement.operation) {
                collect_invocations(value, &mut |invoke_kind, method, base, arguments| {
                    let mut visiting = BTreeSet::new();
                    let receiver = base
                        .map(|base| self.resolve_locals_only(statement.id, base, &mut visiting));
                    let arguments = arguments
                        .iter()
                        .map(|argument| {
                            self.resolve_locals_only(statement.id, argument, &mut visiting)
                        })
                        .collect();
                    effects.push(JimpleDirectEffect::Invocation(JimpleInvocation {
                        statement_id: statement.id,
                        guards: self.control_guards(statement.id),
                        invoke_kind: invoke_kind.to_owned(),
                        method: method.clone(),
                        receiver,
                        arguments,
                    }));
                });
            }

            if let JimpleOperation::Assign { left, right } = &statement.operation
                && let Some((field, receiver)) = self.field_target(statement.id, left)
            {
                let mut visiting = BTreeSet::new();
                effects.push(JimpleDirectEffect::FieldWrite(JimpleFieldWrite {
                    statement_id: statement.id,
                    guards: self.control_guards(statement.id),
                    field,
                    receiver,
                    value: self.resolve_locals_only(statement.id, right, &mut visiting),
                }));
            }
        }

        JimpleEffectSummary {
            method: self.signature.clone(),
            effects,
        }
    }

    /// Recover source-independent temporal counter structure from this method.
    ///
    /// The detector follows local definitions before matching, so it recognizes JVM/Jimple
    /// shapes such as `tmp = this.counter; tmp2 = tmp - 1; this.counter = tmp2` without
    /// knowing the source field or class name.
    #[must_use]
    pub fn temporal_summary(&self) -> JimpleTemporalSummary {
        let effects = self.effect_summary();
        let mut counter_updates = Vec::new();
        let mut constant_writes = Vec::new();
        let mut comparisons = Vec::new();

        for effect in effects.effects {
            let JimpleDirectEffect::FieldWrite(write) = effect else {
                continue;
            };
            if let Some(value) = symbolic_integer(&write.value) {
                constant_writes.push(JimpleFieldConstantWrite {
                    statement_id: write.statement_id,
                    field: write.field,
                    receiver: write.receiver,
                    value,
                });
                continue;
            }
            if let Some(delta) = counter_delta(&write.field, write.receiver.as_ref(), &write.value)
            {
                counter_updates.push(JimpleCounterUpdate {
                    statement_id: write.statement_id,
                    field: write.field,
                    receiver: write.receiver,
                    delta,
                });
            }
        }

        for statement in &self.statements {
            if let JimpleOperation::If { condition } = &statement.operation {
                let mut visiting = BTreeSet::new();
                let resolved = self.resolve_locals_only(statement.id, condition, &mut visiting);
                if let Some(comparison) = field_comparison(statement.id, &resolved) {
                    comparisons.push(comparison);
                }
            }
        }

        JimpleTemporalSummary {
            method: self.signature.clone(),
            counter_updates,
            constant_writes,
            comparisons,
        }
    }

    fn field_target(
        &self,
        before_id: usize,
        value: &JimpleValue,
    ) -> Option<(JimpleFieldSignature, Option<SymbolicValue>)> {
        match &value.kind {
            JimpleValueKind::InstanceField { base, field } => {
                let mut visiting = BTreeSet::new();
                Some((
                    field.clone(),
                    Some(self.resolve_locals_only(before_id, base, &mut visiting)),
                ))
            }
            JimpleValueKind::StaticField { field } => Some((field.clone(), None)),
            _ => None,
        }
    }

    /// Resolve an assignment RHS through preceding structured definitions.
    ///
    /// # Errors
    ///
    /// Returns an error when the statement does not exist or is not an assignment.
    pub fn symbolic_assignment(&self, statement_id: usize) -> Result<SymbolicAssignment> {
        let statement = self
            .statements
            .iter()
            .find(|statement| statement.id == statement_id)
            .with_context(|| format!("Jimple statement {statement_id} does not exist"))?;
        let JimpleOperation::Assign { left, right } = &statement.operation else {
            bail!("Jimple statement {statement_id} is not an assignment");
        };
        let mut visiting = BTreeSet::new();
        Ok(SymbolicAssignment {
            statement_id,
            target: self.resolve_value(statement_id, left, &mut visiting),
            value: self.resolve_value(statement_id, right, &mut visiting),
        })
    }

    fn resolve_locals_only(
        &self,
        before_id: usize,
        value: &JimpleValue,
        visiting: &mut BTreeSet<(usize, String)>,
    ) -> SymbolicValue {
        self.resolve_locals_impl(before_id, value, visiting, true)
    }

    fn resolve_locals_raw(
        &self,
        before_id: usize,
        value: &JimpleValue,
        visiting: &mut BTreeSet<(usize, String)>,
    ) -> SymbolicValue {
        self.resolve_locals_impl(before_id, value, visiting, false)
    }

    fn resolve_locals_impl(
        &self,
        before_id: usize,
        value: &JimpleValue,
        visiting: &mut BTreeSet<(usize, String)>,
        allow_piecewise: bool,
    ) -> SymbolicValue {
        if let JimpleValueKind::Local { name } = &value.kind {
            if let Some(definition) = self.unique_reaching_local_definition(before_id, name) {
                let visit = (definition.id, name.clone());
                if visiting.insert(visit.clone()) {
                    let resolved = match &definition.operation {
                        JimpleOperation::Assign { right, .. }
                        | JimpleOperation::Identity { right, .. } => self.resolve_locals_impl(
                            definition.id,
                            right,
                            visiting,
                            allow_piecewise,
                        ),
                        _ => symbolic_leaf(value),
                    };
                    visiting.remove(&visit);
                    return resolved;
                }
            } else if allow_piecewise
                && let Some(piecewise) =
                    self.piecewise_local_value(before_id, name, &value.type_name, visiting)
            {
                return piecewise;
            }
        }
        self.resolve_locals_composite(before_id, value, visiting, allow_piecewise)
    }

    fn resolve_locals_composite(
        &self,
        before_id: usize,
        value: &JimpleValue,
        visiting: &mut BTreeSet<(usize, String)>,
        allow_piecewise: bool,
    ) -> SymbolicValue {
        match &value.kind {
            JimpleValueKind::ArrayRef { base, index } => SymbolicValue::ArrayElement {
                base: Box::new(self.resolve_locals_impl(
                    before_id,
                    base,
                    visiting,
                    allow_piecewise,
                )),
                index: Box::new(self.resolve_locals_impl(
                    before_id,
                    index,
                    visiting,
                    allow_piecewise,
                )),
            },
            JimpleValueKind::InstanceField { base, field } => SymbolicValue::InstanceField {
                base: Box::new(self.resolve_locals_impl(
                    before_id,
                    base,
                    visiting,
                    allow_piecewise,
                )),
                field: field.clone(),
            },
            JimpleValueKind::Binary {
                operator,
                left,
                right,
            } => SymbolicValue::Binary {
                operator: operator.trim().to_owned(),
                left: Box::new(self.resolve_locals_impl(
                    before_id,
                    left,
                    visiting,
                    allow_piecewise,
                )),
                right: Box::new(self.resolve_locals_impl(
                    before_id,
                    right,
                    visiting,
                    allow_piecewise,
                )),
            },
            JimpleValueKind::Cast { cast_type, value } => SymbolicValue::Cast {
                type_name: cast_type.clone(),
                value: Box::new(self.resolve_locals_impl(
                    before_id,
                    value,
                    visiting,
                    allow_piecewise,
                )),
            },
            JimpleValueKind::Unary { operator, value } => SymbolicValue::Unary {
                operator: operator.clone(),
                value: Box::new(self.resolve_locals_impl(
                    before_id,
                    value,
                    visiting,
                    allow_piecewise,
                )),
            },
            JimpleValueKind::ArrayLength { value } => SymbolicValue::ArrayLength {
                value: Box::new(self.resolve_locals_impl(
                    before_id,
                    value,
                    visiting,
                    allow_piecewise,
                )),
            },
            _ => self.resolve_locals_tail(before_id, value, visiting, allow_piecewise),
        }
    }

    fn resolve_locals_tail(
        &self,
        before_id: usize,
        value: &JimpleValue,
        visiting: &mut BTreeSet<(usize, String)>,
        allow_piecewise: bool,
    ) -> SymbolicValue {
        match &value.kind {
            JimpleValueKind::NewArray { element_type, size } => SymbolicValue::NewArray {
                element_type: element_type.clone(),
                size: Box::new(self.resolve_locals_impl(
                    before_id,
                    size,
                    visiting,
                    allow_piecewise,
                )),
            },
            JimpleValueKind::NewMultiArray { array_type, sizes } => SymbolicValue::NewMultiArray {
                array_type: array_type.clone(),
                sizes: sizes
                    .iter()
                    .map(|size| {
                        self.resolve_locals_impl(before_id, size, visiting, allow_piecewise)
                    })
                    .collect(),
            },
            JimpleValueKind::Phi { values } => SymbolicValue::Phi {
                values: values
                    .iter()
                    .map(|item| {
                        self.resolve_locals_impl(before_id, item, visiting, allow_piecewise)
                    })
                    .collect(),
            },
            JimpleValueKind::Invoke {
                method,
                base,
                arguments,
                ..
            } => SymbolicValue::Invoke {
                method: method.clone(),
                base: base.as_deref().map(|item| {
                    Box::new(self.resolve_locals_impl(before_id, item, visiting, allow_piecewise))
                }),
                arguments: arguments
                    .iter()
                    .map(|item| {
                        self.resolve_locals_impl(before_id, item, visiting, allow_piecewise)
                    })
                    .collect(),
            },
            JimpleValueKind::ArrayRef { .. }
            | JimpleValueKind::InstanceField { .. }
            | JimpleValueKind::Binary { .. }
            | JimpleValueKind::Cast { .. }
            | JimpleValueKind::Unary { .. }
            | JimpleValueKind::ArrayLength { .. } => {
                unreachable!("composite value handled earlier")
            }
            JimpleValueKind::Local { .. }
            | JimpleValueKind::IntegerConstant { .. }
            | JimpleValueKind::FloatConstant { .. }
            | JimpleValueKind::StringConstant { .. }
            | JimpleValueKind::ClassConstant { .. }
            | JimpleValueKind::MethodHandle { .. }
            | JimpleValueKind::MethodType { .. }
            | JimpleValueKind::NullConstant
            | JimpleValueKind::Parameter { .. }
            | JimpleValueKind::This
            | JimpleValueKind::StaticField { .. }
            | JimpleValueKind::New { .. }
            | JimpleValueKind::Opaque => symbolic_leaf(value),
        }
    }

    fn piecewise_local_value(
        &self,
        before_id: usize,
        name: &str,
        type_name: &str,
        visiting: &mut BTreeSet<(usize, String)>,
    ) -> Option<SymbolicValue> {
        const MAX_PATH_STATES: usize = 256;
        const MAX_ARMS: usize = 64;

        let predecessors = self.predecessors();
        let relevant = ancestors_of(before_id, &predecessors);
        let entries: Vec<_> = self
            .statements
            .iter()
            .filter(|statement| {
                relevant.contains(&statement.id)
                    && predecessors.get(&statement.id).is_none_or(Vec::is_empty)
            })
            .map(|statement| statement.id)
            .collect();
        if entries.is_empty() {
            return None;
        }

        let mut pending: Vec<_> = entries.into_iter().map(PiecewisePathState::new).collect();
        let mut arms = Vec::new();
        let mut explored = 0_usize;
        while let Some(mut path) = pending.pop() {
            explored = explored.saturating_add(1);
            if explored > MAX_PATH_STATES {
                return None;
            }
            if path.statement_id == before_id {
                let value = path.value?;
                arms.push(SymbolicPiecewiseArm {
                    guards: path.guards,
                    value,
                });
                if arms.len() > MAX_ARMS {
                    return None;
                }
                continue;
            }
            if !path.visited.insert(path.statement_id) {
                return None;
            }
            let statement = self
                .statements
                .iter()
                .find(|statement| statement.id == path.statement_id)?;
            self.update_piecewise_local(statement, name, &mut path, visiting);
            self.extend_piecewise_paths(statement, name, &path, &relevant, visiting, &mut pending)?;
        }

        normalize_piecewise_arms(&mut arms);
        match arms.as_slice() {
            [] => None,
            [arm] => Some(arm.value.clone()),
            _ if arms.iter().all(|arm| arm.value == arms[0].value) => Some(arms[0].value.clone()),
            _ => Some(SymbolicValue::Piecewise { arms }),
        }
        .or_else(|| {
            Some(SymbolicValue::Local {
                name: name.to_owned(),
                type_name: type_name.to_owned(),
            })
        })
    }

    fn update_piecewise_local(
        &self,
        statement: &JimpleStatement,
        name: &str,
        path: &mut PiecewisePathState,
        visiting: &mut BTreeSet<(usize, String)>,
    ) {
        let Some(target) = operation_definition(&statement.operation) else {
            return;
        };
        let JimpleValueKind::Local { name: defined } = &target.kind else {
            return;
        };
        if defined != name {
            return;
        }
        let (JimpleOperation::Assign { right, .. } | JimpleOperation::Identity { right, .. }) =
            &statement.operation
        else {
            return;
        };
        let raw = self.resolve_locals_raw(statement.id, right, visiting);
        path.value = Some(substitute_local_symbolic(&raw, name, path.value.as_ref()));
    }

    fn extend_piecewise_paths(
        &self,
        statement: &JimpleStatement,
        name: &str,
        path: &PiecewisePathState,
        relevant: &BTreeSet<usize>,
        visiting: &mut BTreeSet<(usize, String)>,
        pending: &mut Vec<PiecewisePathState>,
    ) -> Option<()> {
        let successors: Vec<_> = statement
            .successors
            .iter()
            .copied()
            .filter(|successor| relevant.contains(successor))
            .collect();
        if successors.is_empty() {
            return Some(());
        }
        if let JimpleOperation::If { condition } = &statement.operation {
            let [when_false, when_true] = statement.successors.as_slice() else {
                return None;
            };
            let raw = self.resolve_locals_raw(statement.id, condition, visiting);
            let condition = substitute_local_symbolic(&raw, name, path.value.as_ref());
            for (successor, expected) in [(*when_false, false), (*when_true, true)] {
                if relevant.contains(&successor) {
                    let mut branch = path.clone();
                    branch.statement_id = successor;
                    branch.guards.push(JimpleGuard {
                        branch_statement_id: statement.id,
                        expected,
                        condition: condition.clone(),
                    });
                    pending.push(branch);
                }
            }
            return Some(());
        }
        if matches!(statement.operation, JimpleOperation::Switch { .. }) && successors.len() > 1 {
            return None;
        }
        for successor in successors {
            let mut next = path.clone();
            next.statement_id = successor;
            pending.push(next);
        }
        Some(())
    }

    fn control_guards(&self, statement_id: usize) -> Vec<JimpleGuard> {
        let mut guards = Vec::new();
        for branch in &self.statements {
            let JimpleOperation::If { condition } = &branch.operation else {
                continue;
            };
            let [when_false, when_true] = branch.successors.as_slice() else {
                continue;
            };
            let merge = self.immediate_postdominator(branch.id);
            let false_region = self.reachable_until(*when_false, merge);
            let true_region = self.reachable_until(*when_true, merge);
            let in_false = false_region.contains(&statement_id);
            let in_true = true_region.contains(&statement_id);
            if in_false == in_true {
                continue;
            }
            let mut visiting = BTreeSet::new();
            guards.push(JimpleGuard {
                branch_statement_id: branch.id,
                expected: in_true,
                condition: self.resolve_locals_only(branch.id, condition, &mut visiting),
            });
        }
        guards.sort_by_key(|guard| guard.branch_statement_id);
        guards
    }

    fn reaching_local_definitions(&self, before_id: usize, name: &str) -> Vec<&JimpleStatement> {
        let predecessors = self.predecessors();
        let mut pending: Vec<_> = predecessors
            .get(&before_id)
            .into_iter()
            .flatten()
            .copied()
            .collect();
        let mut visited = BTreeSet::new();
        let mut definition_ids = BTreeSet::new();
        while let Some(statement_id) = pending.pop() {
            if !visited.insert(statement_id) {
                continue;
            }
            let Some(statement) = self
                .statements
                .iter()
                .find(|statement| statement.id == statement_id)
            else {
                continue;
            };
            let defines_name = operation_definition(&statement.operation)
                .is_some_and(|value| matches!(&value.kind, JimpleValueKind::Local { name: defined } if defined == name));
            if defines_name {
                definition_ids.insert(statement_id);
                continue;
            }
            if let Some(previous) = predecessors.get(&statement_id) {
                pending.extend(previous.iter().copied());
            }
        }
        definition_ids
            .into_iter()
            .filter_map(|id| self.statements.iter().find(|statement| statement.id == id))
            .collect()
    }

    fn unique_reaching_local_definition(
        &self,
        before_id: usize,
        name: &str,
    ) -> Option<&JimpleStatement> {
        let definitions = self.reaching_local_definitions(before_id, name);
        let [definition] = definitions.as_slice() else {
            return None;
        };
        Some(*definition)
    }

    pub(super) fn boolean_materialized_return(&self) -> Option<SymbolicValue> {
        if self.signature.return_type != "boolean" {
            return None;
        }
        let returns: Vec<_> = self
            .statements
            .iter()
            .filter_map(|statement| match &statement.operation {
                JimpleOperation::Return { value } => Some((statement, value)),
                _ => None,
            })
            .collect();
        let [(return_statement, return_value)] = returns.as_slice() else {
            return None;
        };
        let JimpleValueKind::Local { name } = &return_value.kind else {
            return None;
        };
        let definitions = self.reaching_local_definitions(return_statement.id, name);
        let [left, right] = definitions.as_slice() else {
            return None;
        };
        let arm = |definition: &JimpleStatement| -> Option<(i64, JimpleGuard)> {
            let (JimpleOperation::Assign { right, .. } | JimpleOperation::Identity { right, .. }) =
                &definition.operation
            else {
                return None;
            };
            let mut visiting = BTreeSet::new();
            let value =
                symbolic_integer(&self.resolve_locals_only(definition.id, right, &mut visiting))?;
            if !matches!(value, 0 | 1) {
                return None;
            }
            let guards = self.control_guards(definition.id);
            let [guard] = guards.as_slice() else {
                return None;
            };
            Some((value, guard.clone()))
        };
        let left = arm(left)?;
        let right = arm(right)?;
        if left.0 == right.0
            || left.1.branch_statement_id != right.1.branch_statement_id
            || left.1.expected == right.1.expected
            || left.1.condition != right.1.condition
        {
            return None;
        }
        let one_guard = if left.0 == 1 { &left.1 } else { &right.1 };
        if one_guard.expected {
            Some(one_guard.condition.clone())
        } else {
            Some(SymbolicValue::Unary {
                operator: String::from("!"),
                value: Box::new(one_guard.condition.clone()),
            })
        }
    }

    fn predecessors(&self) -> BTreeMap<usize, Vec<usize>> {
        let mut predecessors: BTreeMap<usize, Vec<usize>> = self
            .statements
            .iter()
            .map(|statement| (statement.id, Vec::new()))
            .collect();
        for statement in &self.statements {
            for successor in &statement.successors {
                if let Some(entries) = predecessors.get_mut(successor) {
                    entries.push(statement.id);
                }
            }
        }
        predecessors
    }

    fn reachable_until(&self, start: usize, stop: Option<usize>) -> BTreeSet<usize> {
        let mut reached = BTreeSet::new();
        let mut pending = vec![start];
        while let Some(statement_id) = pending.pop() {
            if Some(statement_id) == stop || !reached.insert(statement_id) {
                continue;
            }
            if let Some(statement) = self
                .statements
                .iter()
                .find(|statement| statement.id == statement_id)
            {
                pending.extend(statement.successors.iter().copied());
            }
        }
        reached
    }

    fn immediate_postdominator(&self, statement_id: usize) -> Option<usize> {
        const EXIT: usize = usize::MAX;
        let ids: BTreeSet<_> = self
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
            for statement in self.statements.iter().rev() {
                let successors: Vec<_> = if statement.successors.is_empty() {
                    vec![EXIT]
                } else {
                    statement.successors.clone()
                };
                let mut intersection = universe.clone();
                for successor in successors {
                    let Some(successor_set) = postdominators.get(&successor) else {
                        continue;
                    };
                    intersection.retain(|candidate| successor_set.contains(candidate));
                }
                intersection.insert(statement.id);
                if postdominators.get(&statement.id) != Some(&intersection) {
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
                        .is_some_and(|set| set.contains(candidate))
            })
        })
    }

    fn add_value_dependencies<'a>(
        &'a self,
        before_id: usize,
        value: &'a JimpleValue,
        included: &mut BTreeSet<usize>,
        pending: &mut Vec<(usize, &'a JimpleValue)>,
    ) {
        if let Some(location) = storage_location(value)
            && let Some(definition) = self.latest_definition_before(before_id, &location)
            && included.insert(definition.id)
        {
            operation_inputs_before(&definition.operation, definition.id, pending);
            return;
        }
        for child in value_children(value) {
            pending.push((before_id, child));
        }
    }

    fn latest_definition_before(
        &self,
        before_id: usize,
        location: &StorageLocation,
    ) -> Option<&JimpleStatement> {
        self.statements.iter().rev().find(|statement| {
            statement.id < before_id
                && operation_definition(&statement.operation).and_then(storage_location)
                    == Some(location.clone())
        })
    }

    fn resolve_value(
        &self,
        before_id: usize,
        value: &JimpleValue,
        visiting: &mut BTreeSet<(usize, StorageLocation)>,
    ) -> SymbolicValue {
        if let Some(location) = storage_location(value)
            && let Some(definition) = self.latest_definition_before(before_id, &location)
        {
            let visit = (definition.id, location.clone());
            if visiting.insert(visit.clone()) {
                let resolved = match &definition.operation {
                    JimpleOperation::Assign { right, .. }
                    | JimpleOperation::Identity { right, .. } => {
                        self.resolve_value(definition.id, right, visiting)
                    }
                    _ => symbolic_leaf(value),
                };
                visiting.remove(&visit);
                return resolved;
            }
        }
        match &value.kind {
            JimpleValueKind::ArrayRef { base, index } => SymbolicValue::ArrayElement {
                base: Box::new(self.resolve_value(before_id, base, visiting)),
                index: Box::new(self.resolve_value(before_id, index, visiting)),
            },
            JimpleValueKind::InstanceField { base, field } => SymbolicValue::InstanceField {
                base: Box::new(self.resolve_value(before_id, base, visiting)),
                field: field.clone(),
            },
            JimpleValueKind::Binary {
                operator,
                left,
                right,
            } => SymbolicValue::Binary {
                operator: operator.trim().to_owned(),
                left: Box::new(self.resolve_value(before_id, left, visiting)),
                right: Box::new(self.resolve_value(before_id, right, visiting)),
            },
            JimpleValueKind::Cast { cast_type, value } => SymbolicValue::Cast {
                type_name: cast_type.clone(),
                value: Box::new(self.resolve_value(before_id, value, visiting)),
            },
            JimpleValueKind::Unary { operator, value } => SymbolicValue::Unary {
                operator: operator.clone(),
                value: Box::new(self.resolve_value(before_id, value, visiting)),
            },
            JimpleValueKind::ArrayLength { value } => SymbolicValue::ArrayLength {
                value: Box::new(self.resolve_value(before_id, value, visiting)),
            },
            JimpleValueKind::NewArray { element_type, size } => SymbolicValue::NewArray {
                element_type: element_type.clone(),
                size: Box::new(self.resolve_value(before_id, size, visiting)),
            },
            JimpleValueKind::NewMultiArray { array_type, sizes } => SymbolicValue::NewMultiArray {
                array_type: array_type.clone(),
                sizes: sizes
                    .iter()
                    .map(|size| self.resolve_value(before_id, size, visiting))
                    .collect(),
            },
            JimpleValueKind::Phi { values } => SymbolicValue::Phi {
                values: values
                    .iter()
                    .map(|value| self.resolve_value(before_id, value, visiting))
                    .collect(),
            },
            JimpleValueKind::Invoke {
                method,
                base,
                arguments,
                ..
            } => SymbolicValue::Invoke {
                method: method.clone(),
                base: base
                    .as_deref()
                    .map(|base| Box::new(self.resolve_value(before_id, base, visiting))),
                arguments: arguments
                    .iter()
                    .map(|argument| self.resolve_value(before_id, argument, visiting))
                    .collect(),
            },
            _ => symbolic_leaf(value),
        }
    }
}

fn symbolic_integer(value: &SymbolicValue) -> Option<i64> {
    match value {
        SymbolicValue::Integer { value, .. } => Some(*value),
        SymbolicValue::Cast { value, .. } => symbolic_integer(value),
        _ => None,
    }
}

fn symbolic_field(
    value: &SymbolicValue,
) -> Option<(&JimpleFieldSignature, Option<&SymbolicValue>)> {
    match value {
        SymbolicValue::InstanceField { base, field } => Some((field, Some(base))),
        SymbolicValue::StaticField { field } => Some((field, None)),
        SymbolicValue::Cast { value, .. } => symbolic_field(value),
        _ => None,
    }
}

fn same_field(
    expected_field: &JimpleFieldSignature,
    expected_receiver: Option<&SymbolicValue>,
    value: &SymbolicValue,
) -> bool {
    let Some((field, receiver)) = symbolic_field(value) else {
        return false;
    };
    field == expected_field && receiver == expected_receiver
}

fn counter_delta(
    field: &JimpleFieldSignature,
    receiver: Option<&SymbolicValue>,
    value: &SymbolicValue,
) -> Option<i64> {
    let SymbolicValue::Binary {
        operator,
        left,
        right,
    } = value
    else {
        return None;
    };
    if same_field(field, receiver, left) {
        let constant = symbolic_integer(right)?;
        return match operator.as_str() {
            "+" => Some(constant),
            "-" => constant.checked_neg(),
            _ => None,
        };
    }
    if operator == "+" && same_field(field, receiver, right) {
        return symbolic_integer(left);
    }
    None
}

fn field_comparison(statement_id: usize, value: &SymbolicValue) -> Option<JimpleFieldComparison> {
    let SymbolicValue::Binary {
        operator,
        left,
        right,
    } = value
    else {
        return None;
    };
    if let Some((field, receiver)) = symbolic_field(left)
        && let Some(constant) = symbolic_integer(right)
    {
        return Some(JimpleFieldComparison {
            statement_id,
            field: field.clone(),
            receiver: receiver.cloned(),
            operator: operator.clone(),
            constant,
            field_on_left: true,
        });
    }
    if let Some((field, receiver)) = symbolic_field(right)
        && let Some(constant) = symbolic_integer(left)
    {
        return Some(JimpleFieldComparison {
            statement_id,
            field: field.clone(),
            receiver: receiver.cloned(),
            operator: operator.clone(),
            constant,
            field_on_left: false,
        });
    }
    None
}

fn operation_input_values(operation: &JimpleOperation) -> Vec<&JimpleValue> {
    match operation {
        JimpleOperation::Assign { right, .. } | JimpleOperation::Identity { right, .. } => {
            vec![right]
        }
        JimpleOperation::If { condition } => vec![condition],
        JimpleOperation::Invoke {
            expression: Some(expression),
        } => vec![expression],
        JimpleOperation::Return { value } | JimpleOperation::Throw { value } => vec![value],
        JimpleOperation::Switch { key, .. } => vec![key],
        JimpleOperation::Invoke { expression: None }
        | JimpleOperation::ReturnVoid
        | JimpleOperation::Goto
        | JimpleOperation::Opaque => Vec::new(),
    }
}

fn collect_invocations<'a>(
    value: &'a JimpleValue,
    visitor: &mut impl FnMut(
        &'a str,
        &'a JimpleMethodSignature,
        Option<&'a JimpleValue>,
        &'a [JimpleValue],
    ),
) {
    if let JimpleValueKind::Invoke {
        invoke_kind,
        method,
        base,
        arguments,
        ..
    } = &value.kind
    {
        visitor(invoke_kind, method, base.as_deref(), arguments);
    }
    for child in value_children(value) {
        collect_invocations(child, visitor);
    }
}

fn ancestors_of(
    statement_id: usize,
    predecessors: &BTreeMap<usize, Vec<usize>>,
) -> BTreeSet<usize> {
    let mut reached = BTreeSet::from([statement_id]);
    let mut pending = vec![statement_id];
    while let Some(current) = pending.pop() {
        if let Some(previous) = predecessors.get(&current) {
            for predecessor in previous {
                if reached.insert(*predecessor) {
                    pending.push(*predecessor);
                }
            }
        }
    }
    reached
}

fn normalize_piecewise_arms(arms: &mut Vec<SymbolicPiecewiseArm>) {
    for arm in arms.iter_mut() {
        arm.guards.sort_by_key(|guard| guard.branch_statement_id);
        arm.guards.dedup_by(|left, right| {
            left.branch_statement_id == right.branch_statement_id
                && left.expected == right.expected
                && left.condition == right.condition
        });
    }
    let mut index = 0;
    while index < arms.len() {
        let duplicate = arms[..index]
            .iter()
            .any(|existing| existing == &arms[index]);
        if duplicate {
            arms.remove(index);
        } else {
            index += 1;
        }
    }
}

fn substitute_local_symbolic(
    value: &SymbolicValue,
    local_name: &str,
    replacement: Option<&SymbolicValue>,
) -> SymbolicValue {
    if let SymbolicValue::Local { name, .. } = value
        && name == local_name
    {
        return replacement.cloned().unwrap_or_else(|| value.clone());
    }
    match value {
        SymbolicValue::InstanceField { base, field } => SymbolicValue::InstanceField {
            base: Box::new(substitute_local_symbolic(base, local_name, replacement)),
            field: field.clone(),
        },
        SymbolicValue::ArrayElement { base, index } => SymbolicValue::ArrayElement {
            base: Box::new(substitute_local_symbolic(base, local_name, replacement)),
            index: Box::new(substitute_local_symbolic(index, local_name, replacement)),
        },
        SymbolicValue::Binary {
            operator,
            left,
            right,
        } => SymbolicValue::Binary {
            operator: operator.clone(),
            left: Box::new(substitute_local_symbolic(left, local_name, replacement)),
            right: Box::new(substitute_local_symbolic(right, local_name, replacement)),
        },
        SymbolicValue::Cast { type_name, value } => SymbolicValue::Cast {
            type_name: type_name.clone(),
            value: Box::new(substitute_local_symbolic(value, local_name, replacement)),
        },
        SymbolicValue::Unary { operator, value } => SymbolicValue::Unary {
            operator: operator.clone(),
            value: Box::new(substitute_local_symbolic(value, local_name, replacement)),
        },
        SymbolicValue::ArrayLength { value } => SymbolicValue::ArrayLength {
            value: Box::new(substitute_local_symbolic(value, local_name, replacement)),
        },
        SymbolicValue::NewArray { element_type, size } => SymbolicValue::NewArray {
            element_type: element_type.clone(),
            size: Box::new(substitute_local_symbolic(size, local_name, replacement)),
        },
        SymbolicValue::NewMultiArray { array_type, sizes } => SymbolicValue::NewMultiArray {
            array_type: array_type.clone(),
            sizes: sizes
                .iter()
                .map(|size| substitute_local_symbolic(size, local_name, replacement))
                .collect(),
        },
        SymbolicValue::Phi { values } => SymbolicValue::Phi {
            values: values
                .iter()
                .map(|item| substitute_local_symbolic(item, local_name, replacement))
                .collect(),
        },
        SymbolicValue::Piecewise { arms } => SymbolicValue::Piecewise {
            arms: arms
                .iter()
                .map(|arm| SymbolicPiecewiseArm {
                    guards: arm
                        .guards
                        .iter()
                        .map(|guard| JimpleGuard {
                            branch_statement_id: guard.branch_statement_id,
                            expected: guard.expected,
                            condition: substitute_local_symbolic(
                                &guard.condition,
                                local_name,
                                replacement,
                            ),
                        })
                        .collect(),
                    value: substitute_local_symbolic(&arm.value, local_name, replacement),
                })
                .collect(),
        },
        SymbolicValue::Invoke {
            method,
            base,
            arguments,
        } => SymbolicValue::Invoke {
            method: method.clone(),
            base: base
                .as_deref()
                .map(|base| Box::new(substitute_local_symbolic(base, local_name, replacement))),
            arguments: arguments
                .iter()
                .map(|argument| substitute_local_symbolic(argument, local_name, replacement))
                .collect(),
        },
        SymbolicValue::Integer { .. }
        | SymbolicValue::Float { .. }
        | SymbolicValue::String { .. }
        | SymbolicValue::Null
        | SymbolicValue::Parameter { .. }
        | SymbolicValue::Local { .. }
        | SymbolicValue::This { .. }
        | SymbolicValue::StaticField { .. }
        | SymbolicValue::New { .. }
        | SymbolicValue::Unknown { .. } => value.clone(),
    }
}

pub(super) fn substitute_symbolic(
    value: &SymbolicValue,
    this_value: Option<&SymbolicValue>,
    arguments: &[SymbolicValue],
) -> SymbolicValue {
    match value {
        SymbolicValue::This { .. } => this_value.cloned().unwrap_or_else(|| value.clone()),
        SymbolicValue::Parameter { index, .. } => arguments
            .get(*index)
            .cloned()
            .unwrap_or_else(|| value.clone()),
        SymbolicValue::InstanceField { base, field } => SymbolicValue::InstanceField {
            base: Box::new(substitute_symbolic(base, this_value, arguments)),
            field: field.clone(),
        },
        SymbolicValue::ArrayElement { base, index } => SymbolicValue::ArrayElement {
            base: Box::new(substitute_symbolic(base, this_value, arguments)),
            index: Box::new(substitute_symbolic(index, this_value, arguments)),
        },
        SymbolicValue::Binary {
            operator,
            left,
            right,
        } => SymbolicValue::Binary {
            operator: operator.clone(),
            left: Box::new(substitute_symbolic(left, this_value, arguments)),
            right: Box::new(substitute_symbolic(right, this_value, arguments)),
        },
        SymbolicValue::Cast { type_name, value } => SymbolicValue::Cast {
            type_name: type_name.clone(),
            value: Box::new(substitute_symbolic(value, this_value, arguments)),
        },
        SymbolicValue::Unary { operator, value } => SymbolicValue::Unary {
            operator: operator.clone(),
            value: Box::new(substitute_symbolic(value, this_value, arguments)),
        },
        SymbolicValue::ArrayLength { value } => SymbolicValue::ArrayLength {
            value: Box::new(substitute_symbolic(value, this_value, arguments)),
        },
        SymbolicValue::NewArray { element_type, size } => SymbolicValue::NewArray {
            element_type: element_type.clone(),
            size: Box::new(substitute_symbolic(size, this_value, arguments)),
        },
        SymbolicValue::NewMultiArray { array_type, sizes } => SymbolicValue::NewMultiArray {
            array_type: array_type.clone(),
            sizes: sizes
                .iter()
                .map(|size| substitute_symbolic(size, this_value, arguments))
                .collect(),
        },
        SymbolicValue::Phi { values } => SymbolicValue::Phi {
            values: values
                .iter()
                .map(|item| substitute_symbolic(item, this_value, arguments))
                .collect(),
        },
        SymbolicValue::Piecewise { arms } => SymbolicValue::Piecewise {
            arms: arms
                .iter()
                .map(|arm| SymbolicPiecewiseArm {
                    guards: arm
                        .guards
                        .iter()
                        .map(|guard| JimpleGuard {
                            branch_statement_id: guard.branch_statement_id,
                            expected: guard.expected,
                            condition: substitute_symbolic(&guard.condition, this_value, arguments),
                        })
                        .collect(),
                    value: substitute_symbolic(&arm.value, this_value, arguments),
                })
                .collect(),
        },
        SymbolicValue::Invoke {
            method,
            base,
            arguments: inner_arguments,
        } => SymbolicValue::Invoke {
            method: method.clone(),
            base: base
                .as_deref()
                .map(|base| Box::new(substitute_symbolic(base, this_value, arguments))),
            arguments: inner_arguments
                .iter()
                .map(|argument| substitute_symbolic(argument, this_value, arguments))
                .collect(),
        },
        SymbolicValue::Integer { .. }
        | SymbolicValue::Float { .. }
        | SymbolicValue::String { .. }
        | SymbolicValue::Local { .. }
        | SymbolicValue::Null
        | SymbolicValue::StaticField { .. }
        | SymbolicValue::New { .. }
        | SymbolicValue::Unknown { .. } => value.clone(),
    }
}

fn operation_definition(operation: &JimpleOperation) -> Option<&JimpleValue> {
    match operation {
        JimpleOperation::Assign { left, .. } | JimpleOperation::Identity { left, .. } => Some(left),
        _ => None,
    }
}

fn operation_inputs_before<'a>(
    operation: &'a JimpleOperation,
    before_id: usize,
    pending: &mut Vec<(usize, &'a JimpleValue)>,
) {
    match operation {
        JimpleOperation::Assign { right, .. } | JimpleOperation::Identity { right, .. } => {
            pending.push((before_id, right));
        }
        JimpleOperation::If { condition } => pending.push((before_id, condition)),
        JimpleOperation::Invoke {
            expression: Some(expression),
        } => pending.push((before_id, expression)),
        JimpleOperation::Return { value } | JimpleOperation::Throw { value } => {
            pending.push((before_id, value));
        }
        JimpleOperation::Switch { key, .. } => pending.push((before_id, key)),
        JimpleOperation::Invoke { expression: None }
        | JimpleOperation::ReturnVoid
        | JimpleOperation::Goto
        | JimpleOperation::Opaque => {}
    }
}

fn storage_location(value: &JimpleValue) -> Option<StorageLocation> {
    match &value.kind {
        JimpleValueKind::Local { name } => Some(StorageLocation::Local(name.clone())),
        JimpleValueKind::StaticField { field } => Some(StorageLocation::StaticField(field.clone())),
        JimpleValueKind::InstanceField { base, field } => Some(StorageLocation::InstanceField {
            base: Box::new(storage_location(base)?),
            field: field.clone(),
        }),
        JimpleValueKind::ArrayRef { base, index } => {
            let JimpleValueKind::IntegerConstant { value: index } = index.kind else {
                return None;
            };
            Some(StorageLocation::ArrayElement {
                base: Box::new(storage_location(base)?),
                index,
            })
        }
        _ => None,
    }
}

fn value_children(value: &JimpleValue) -> Vec<&JimpleValue> {
    let mut children = Vec::new();
    match &value.kind {
        JimpleValueKind::ArrayRef { base, index }
        | JimpleValueKind::Binary {
            left: base,
            right: index,
            ..
        } => {
            children.push(base.as_ref());
            children.push(index.as_ref());
        }
        JimpleValueKind::InstanceField { base, .. }
        | JimpleValueKind::Cast { value: base, .. }
        | JimpleValueKind::Unary { value: base, .. }
        | JimpleValueKind::ArrayLength { value: base }
        | JimpleValueKind::NewArray { size: base, .. } => children.push(base.as_ref()),
        JimpleValueKind::NewMultiArray { sizes, .. } | JimpleValueKind::Phi { values: sizes } => {
            children.extend(sizes.iter());
        }
        JimpleValueKind::Invoke {
            base, arguments, ..
        } => {
            if let Some(base) = base {
                children.push(base.as_ref());
            }
            children.extend(arguments.iter());
        }
        JimpleValueKind::Local { .. }
        | JimpleValueKind::IntegerConstant { .. }
        | JimpleValueKind::FloatConstant { .. }
        | JimpleValueKind::StringConstant { .. }
        | JimpleValueKind::ClassConstant { .. }
        | JimpleValueKind::MethodHandle { .. }
        | JimpleValueKind::MethodType { .. }
        | JimpleValueKind::NullConstant
        | JimpleValueKind::Parameter { .. }
        | JimpleValueKind::This
        | JimpleValueKind::StaticField { .. }
        | JimpleValueKind::New { .. }
        | JimpleValueKind::Opaque => {}
    }
    children
}

fn symbolic_leaf(value: &JimpleValue) -> SymbolicValue {
    match &value.kind {
        JimpleValueKind::IntegerConstant { value: integer } => SymbolicValue::Integer {
            value: *integer,
            type_name: value.type_name.clone(),
        },
        JimpleValueKind::FloatConstant { value: float } => SymbolicValue::Float {
            value: *float,
            type_name: value.type_name.clone(),
        },
        JimpleValueKind::StringConstant { value } => SymbolicValue::String {
            value: value.clone(),
        },
        JimpleValueKind::NullConstant => SymbolicValue::Null,
        JimpleValueKind::Parameter { index } => SymbolicValue::Parameter {
            index: *index,
            type_name: value.type_name.clone(),
        },
        JimpleValueKind::Local { name } => SymbolicValue::Local {
            name: name.clone(),
            type_name: value.type_name.clone(),
        },
        JimpleValueKind::This => SymbolicValue::This {
            type_name: value.type_name.clone(),
        },
        JimpleValueKind::StaticField { field } => SymbolicValue::StaticField {
            field: field.clone(),
        },
        JimpleValueKind::New { class_type } => SymbolicValue::New {
            class_type: class_type.clone(),
        },
        _ => SymbolicValue::Unknown {
            type_name: value.type_name.clone(),
            text: value.text.clone(),
        },
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AbstractValue {
    Integer {
        value: i64,
    },
    Float {
        value: f64,
    },
    String {
        value: String,
    },
    Class {
        class_type: String,
    },
    BigInteger {
        decimal: String,
    },
    MethodHandle {
        handle_kind: String,
        method: JimpleMethodSignature,
    },
    Lambda {
        handle_kind: String,
        target: JimpleMethodSignature,
        captures: Vec<AbstractValue>,
    },
    Sequence {
        element_type: String,
        values: Vec<AbstractValue>,
    },
    Null,
    Parameter {
        index: usize,
        type_name: String,
    },
    This {
        type_name: String,
    },
    StaticField {
        field: JimpleFieldSignature,
    },
    InstanceField {
        base: Box<AbstractValue>,
        field: JimpleFieldSignature,
    },
    ArrayElement {
        base: Box<AbstractValue>,
        index: Box<AbstractValue>,
    },
    Binary {
        operator: String,
        left: Box<AbstractValue>,
        right: Box<AbstractValue>,
    },
    Cast {
        type_name: String,
        value: Box<AbstractValue>,
    },
    Unary {
        operator: String,
        value: Box<AbstractValue>,
    },
    ArrayLength {
        value: Box<AbstractValue>,
    },
    ObjectRef {
        id: u64,
        class_type: String,
    },
    ArrayRef {
        id: u64,
        element_type: String,
    },
    Invoke {
        method: JimpleMethodSignature,
        base: Option<Box<AbstractValue>>,
        arguments: Vec<AbstractValue>,
    },
    Unknown {
        type_name: String,
        text: String,
    },
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AbstractArrayIdentity {
    Allocated { id: u64 },
    StaticField { field: JimpleFieldSignature },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AbstractArrayEntry {
    pub index: i64,
    pub value: AbstractValue,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AbstractArraySnapshot {
    pub identity: AbstractArrayIdentity,
    pub length: Option<usize>,
    pub entries: Vec<AbstractArrayEntry>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AbstractInvocation {
    pub method: JimpleMethodSignature,
    pub arguments: Vec<AbstractValue>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AbstractFieldValue {
    pub field: JimpleFieldSignature,
    pub value: AbstractValue,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AbstractObjectSnapshot {
    pub id: u64,
    pub class_type: String,
    pub constructor: Option<AbstractInvocation>,
    pub fields_may_be_opaque: bool,
    pub fields: Vec<AbstractFieldValue>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FiniteExecution {
    pub steps: usize,
    pub arrays: Vec<AbstractArraySnapshot>,
    pub objects: Vec<AbstractObjectSnapshot>,
}

impl FiniteExecution {
    #[must_use]
    pub fn static_array(&self, owner: &str, name: &str) -> Option<&AbstractArraySnapshot> {
        self.arrays.iter().find(|array| {
            matches!(
                &array.identity,
                AbstractArrayIdentity::StaticField { field }
                    if field.owner == owner && field.name == name
            )
        })
    }

    #[must_use]
    pub fn object(&self, id: u64) -> Option<&AbstractObjectSnapshot> {
        self.objects.iter().find(|object| object.id == id)
    }
}

#[derive(Clone, Debug, Default)]
struct MutableArray {
    length: Option<usize>,
    entries: BTreeMap<i64, AbstractValue>,
}

#[derive(Clone, Debug)]
struct MutableObject {
    class_type: String,
    constructor: Option<AbstractInvocation>,
    fields_may_be_opaque: bool,
    fields: BTreeMap<JimpleFieldSignature, AbstractValue>,
}

#[derive(Clone, Debug, Default)]
struct ExecutionState {
    locals: BTreeMap<String, AbstractValue>,
    static_fields: BTreeMap<JimpleFieldSignature, AbstractValue>,
    arrays: BTreeMap<AbstractArrayIdentity, MutableArray>,
    objects: BTreeMap<u64, MutableObject>,
    next_id: u64,
}

impl JimpleMethod {
    /// Execute a method when all encountered branches can be decided finitely.
    ///
    /// Unknown values are preserved symbolically; execution only fails if one
    /// of them controls a branch or the step limit is exceeded.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown branch, malformed control flow, an
    /// unsupported statement kind reached during execution, or step exhaustion.
    pub fn execute_finite(&self, max_steps: usize) -> Result<FiniteExecution> {
        let first = self
            .statements
            .first()
            .context("cannot execute an empty Jimple method")?;
        let mut state = ExecutionState::default();
        let mut pc = Some(first.id);
        let mut steps = 0usize;
        while let Some(statement_id) = pc {
            if steps >= max_steps {
                bail!("finite Jimple execution exceeded {max_steps} steps");
            }
            let statement = self
                .statements
                .iter()
                .find(|statement| statement.id == statement_id)
                .with_context(|| format!("Jimple statement {statement_id} does not exist"))?;
            pc = state.execute_statement(statement)?;
            steps += 1;
        }
        Ok(state.finish(steps))
    }
}

impl ExecutionState {
    fn execute_statement(&mut self, statement: &JimpleStatement) -> Result<Option<usize>> {
        match &statement.operation {
            JimpleOperation::Assign { left, right } | JimpleOperation::Identity { left, right } => {
                let value = self.evaluate(right);
                self.assign(left, value)?;
                Self::fallthrough(statement)
            }
            JimpleOperation::Invoke { expression } => {
                if let Some(expression) = expression {
                    self.evaluate(expression);
                }
                Self::fallthrough(statement)
            }
            JimpleOperation::If { condition } => self.branch(statement, condition),
            JimpleOperation::Goto => Self::jump(statement),
            JimpleOperation::Return { value } => {
                self.evaluate(value);
                Ok(None)
            }
            JimpleOperation::ReturnVoid => Ok(None),
            JimpleOperation::Throw { value } => {
                let value = self.evaluate(value);
                bail!("finite Jimple execution threw {value:?}")
            }
            JimpleOperation::Switch {
                key,
                cases,
                default_target,
            } => {
                let AbstractValue::Integer { value } = self.evaluate(key) else {
                    bail!("finite Jimple switch key is not concrete")
                };
                Ok(Some(
                    cases
                        .iter()
                        .find(|case| case.value == value)
                        .map_or(*default_target, |case| case.target),
                ))
            }
            JimpleOperation::Opaque => {
                bail!(
                    "cannot finitely execute opaque Jimple statement {}",
                    statement.id
                )
            }
        }
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

    fn branch(
        &mut self,
        statement: &JimpleStatement,
        condition: &JimpleValue,
    ) -> Result<Option<usize>> {
        let condition = self.evaluate(condition);
        let truth = abstract_truth(&condition).with_context(|| {
            format!(
                "branch at statement {} is not concrete: {condition:?}",
                statement.id
            )
        })?;
        let [when_false, when_true] = statement.successors.as_slice() else {
            bail!("if statement {} does not have two targets", statement.id);
        };
        Ok(Some(if truth { *when_true } else { *when_false }))
    }

    fn assign(&mut self, target: &JimpleValue, value: AbstractValue) -> Result<()> {
        match &target.kind {
            JimpleValueKind::Local { name } => {
                self.locals.insert(name.clone(), value);
            }
            JimpleValueKind::StaticField { field } => {
                self.static_fields.insert(field.clone(), value);
            }
            JimpleValueKind::InstanceField { base, field } => {
                let base = self.evaluate(base);
                if let AbstractValue::ObjectRef { id, .. } = base
                    && let Some(object) = self.objects.get_mut(&id)
                {
                    object.fields.insert(field.clone(), value);
                }
            }
            JimpleValueKind::ArrayRef { base, index } => {
                let base = self.evaluate(base);
                let index = self.evaluate(index);
                let AbstractValue::Integer { value: index } = index else {
                    bail!("array write index is not concrete: {index:?}");
                };
                let identity = abstract_array_identity(&base)
                    .with_context(|| format!("array write base is not addressable: {base:?}"))?;
                self.arrays
                    .entry(identity)
                    .or_default()
                    .entries
                    .insert(index, value);
            }
            _ => bail!("unsupported Jimple assignment target: {}", target.text),
        }
        Ok(())
    }

    fn evaluate(&mut self, value: &JimpleValue) -> AbstractValue {
        match &value.kind {
            JimpleValueKind::Local { name } => {
                self.locals
                    .get(name)
                    .cloned()
                    .unwrap_or_else(|| AbstractValue::Unknown {
                        type_name: value.type_name.clone(),
                        text: value.text.clone(),
                    })
            }
            JimpleValueKind::IntegerConstant { value } => AbstractValue::Integer { value: *value },
            JimpleValueKind::FloatConstant { value } => AbstractValue::Float { value: *value },
            JimpleValueKind::StringConstant { value } => AbstractValue::String {
                value: value.clone(),
            },
            JimpleValueKind::ClassConstant { class_type } => AbstractValue::Class {
                class_type: class_type.clone(),
            },
            JimpleValueKind::MethodHandle {
                handle_kind,
                method,
            } => AbstractValue::MethodHandle {
                handle_kind: handle_kind.clone(),
                method: method.clone(),
            },
            JimpleValueKind::NullConstant => AbstractValue::Null,
            JimpleValueKind::Parameter { index } => AbstractValue::Parameter {
                index: *index,
                type_name: value.type_name.clone(),
            },
            JimpleValueKind::This => AbstractValue::This {
                type_name: value.type_name.clone(),
            },
            JimpleValueKind::StaticField { field } => {
                self.static_fields.get(field).cloned().unwrap_or_else(|| {
                    AbstractValue::StaticField {
                        field: field.clone(),
                    }
                })
            }
            JimpleValueKind::InstanceField { base, field } => {
                self.evaluate_instance_field(base, field)
            }
            JimpleValueKind::ArrayRef { base, index } => self.evaluate_array_ref(base, index),
            JimpleValueKind::Binary {
                operator,
                left,
                right,
            } => self.evaluate_binary(operator, left, right),
            JimpleValueKind::Cast { cast_type, value } => {
                let value = self.evaluate(value);
                abstract_cast(cast_type, value)
            }
            JimpleValueKind::Unary {
                operator,
                value: operand,
            } => {
                let operand = self.evaluate(operand);
                abstract_unary(operator, operand, &value.type_name)
            }
            JimpleValueKind::ArrayLength { value } => self.evaluate_array_length(value),
            JimpleValueKind::New { class_type } => self.allocate_object(class_type),
            JimpleValueKind::NewArray { element_type, size } => {
                self.allocate_array(element_type, size)
            }
            JimpleValueKind::Invoke {
                invoke_kind,
                method,
                base,
                arguments,
                ..
            } => self.evaluate_invoke(invoke_kind, method, base.as_deref(), arguments),
            JimpleValueKind::NewMultiArray { .. }
            | JimpleValueKind::Phi { .. }
            | JimpleValueKind::MethodType { .. }
            | JimpleValueKind::Opaque => AbstractValue::Unknown {
                type_name: value.type_name.clone(),
                text: value.text.clone(),
            },
        }
    }

    fn evaluate_instance_field(
        &mut self,
        base: &JimpleValue,
        field: &JimpleFieldSignature,
    ) -> AbstractValue {
        let base = self.evaluate(base);
        if let AbstractValue::ObjectRef { id, .. } = &base
            && let Some(object) = self.objects.get(id)
        {
            if let Some(value) = object.fields.get(field) {
                return value.clone();
            }
            if !object.fields_may_be_opaque {
                return abstract_default_value(&field.type_name);
            }
        }
        AbstractValue::InstanceField {
            base: Box::new(base),
            field: field.clone(),
        }
    }

    fn evaluate_array_ref(&mut self, base: &JimpleValue, index: &JimpleValue) -> AbstractValue {
        let base = self.evaluate(base);
        let index = self.evaluate(index);
        if let AbstractValue::Integer { value: integer } = index
            && let Some(identity) = abstract_array_identity(&base)
            && let Some(array) = self.arrays.get(&identity)
        {
            if let Some(value) = array.entries.get(&integer) {
                return value.clone();
            }
            if let (
                AbstractArrayIdentity::Allocated { .. },
                AbstractValue::ArrayRef { element_type, .. },
                Some(length),
                Ok(index),
            ) = (&identity, &base, array.length, usize::try_from(integer))
                && index < length
            {
                return abstract_default_value(element_type);
            }
        }
        AbstractValue::ArrayElement {
            base: Box::new(base),
            index: Box::new(index),
        }
    }

    fn evaluate_binary(
        &mut self,
        operator: &str,
        left: &JimpleValue,
        right: &JimpleValue,
    ) -> AbstractValue {
        let result_type = left.type_name.as_str();
        let left = self.evaluate(left);
        let right = self.evaluate(right);
        abstract_binary(operator.trim(), left, right, result_type)
    }

    fn evaluate_array_length(&mut self, value: &JimpleValue) -> AbstractValue {
        let value = self.evaluate(value);
        if let Some(identity) = abstract_array_identity(&value)
            && let Some(length) = self.arrays.get(&identity).and_then(|array| array.length)
            && let Ok(length) = i64::try_from(length)
        {
            return AbstractValue::Integer { value: length };
        }
        AbstractValue::ArrayLength {
            value: Box::new(value),
        }
    }

    fn allocate_object(&mut self, class_type: &str) -> AbstractValue {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.objects.insert(
            id,
            MutableObject {
                class_type: class_type.to_owned(),
                constructor: None,
                fields_may_be_opaque: false,
                fields: BTreeMap::new(),
            },
        );
        AbstractValue::ObjectRef {
            id,
            class_type: class_type.to_owned(),
        }
    }

    fn allocate_array(&mut self, element_type: &str, size: &JimpleValue) -> AbstractValue {
        let size = self.evaluate(size);
        let length = match size {
            AbstractValue::Integer { value } => usize::try_from(value).ok(),
            _ => None,
        };
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.arrays.insert(
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

    fn evaluate_invoke(
        &mut self,
        invoke_kind: &str,
        method: &JimpleMethodSignature,
        base: Option<&JimpleValue>,
        arguments: &[JimpleValue],
    ) -> AbstractValue {
        let base = base.map(|base| self.evaluate(base));
        let arguments: Vec<_> = arguments
            .iter()
            .map(|argument| self.evaluate(argument))
            .collect();
        if method.name == "<init>" && invoke_kind == "JSpecialInvokeExpr" {
            if let Some(AbstractValue::ObjectRef { id, .. }) = &base
                && let Some(object) = self.objects.get_mut(id)
            {
                if object.constructor.is_none() {
                    object.constructor = Some(AbstractInvocation {
                        method: method.clone(),
                        arguments,
                    });
                }
                if method.owner != "java.lang.Object" {
                    object.fields_may_be_opaque = true;
                }
            }
            return base.unwrap_or(AbstractValue::Null);
        }
        AbstractValue::Invoke {
            method: method.clone(),
            base: base.map(Box::new),
            arguments,
        }
    }

    fn finish(self, steps: usize) -> FiniteExecution {
        let arrays = self
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
            .collect();
        let objects = self
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
            .collect();
        FiniteExecution {
            steps,
            arrays,
            objects,
        }
    }
}

fn abstract_default_value(type_name: &str) -> AbstractValue {
    match type_name {
        "boolean" | "byte" | "char" | "short" | "int" | "long" => {
            AbstractValue::Integer { value: 0 }
        }
        "float" | "double" => AbstractValue::Float { value: 0.0 },
        _ => AbstractValue::Null,
    }
}

fn abstract_array_identity(value: &AbstractValue) -> Option<AbstractArrayIdentity> {
    match value {
        AbstractValue::ArrayRef { id, .. } => Some(AbstractArrayIdentity::Allocated { id: *id }),
        AbstractValue::StaticField { field } => Some(AbstractArrayIdentity::StaticField {
            field: field.clone(),
        }),
        _ => None,
    }
}

fn abstract_truth(value: &AbstractValue) -> Option<bool> {
    match value {
        AbstractValue::Integer { value } => Some(*value != 0),
        _ => None,
    }
}

fn abstract_cast(type_name: &str, value: AbstractValue) -> AbstractValue {
    match value {
        AbstractValue::Integer { value } => AbstractValue::Integer {
            value: java_integer_cast(type_name, value),
        },
        AbstractValue::Float { value } => AbstractValue::Float { value },
        value @ (AbstractValue::ObjectRef { .. }
        | AbstractValue::ArrayRef { .. }
        | AbstractValue::String { .. }
        | AbstractValue::Null) => value,
        other => AbstractValue::Cast {
            type_name: type_name.to_owned(),
            value: Box::new(other),
        },
    }
}

fn java_integer_cast(type_name: &str, value: i64) -> i64 {
    match type_name {
        "byte" => i64::from(low_i8(value)),
        "short" => i64::from(low_i16(value)),
        "char" => i64::from(low_u16(value)),
        "int" => i64::from(low_i32(value)),
        _ => value,
    }
}

fn low_i8(value: i64) -> i8 {
    i8::from_le_bytes([value.to_le_bytes()[0]])
}

fn low_i16(value: i64) -> i16 {
    let bytes = value.to_le_bytes();
    i16::from_le_bytes([bytes[0], bytes[1]])
}

fn low_u16(value: i64) -> u16 {
    let bytes = value.to_le_bytes();
    u16::from_le_bytes([bytes[0], bytes[1]])
}

fn low_i32(value: i64) -> i32 {
    let bytes = value.to_le_bytes();
    i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn abstract_unary(operator: &str, value: AbstractValue, type_name: &str) -> AbstractValue {
    if operator == "-"
        && let AbstractValue::Integer { value: integer } = value
    {
        return AbstractValue::Integer {
            value: match type_name {
                "int" => i64::from(low_i32(integer).wrapping_neg()),
                "long" => integer.wrapping_neg(),
                _ => integer.checked_neg().unwrap_or(integer),
            },
        };
    }
    AbstractValue::Unary {
        operator: operator.to_owned(),
        value: Box::new(value),
    }
}

fn abstract_binary(
    operator: &str,
    left: AbstractValue,
    right: AbstractValue,
    type_name: &str,
) -> AbstractValue {
    if let (AbstractValue::Integer { value: left_int }, AbstractValue::Integer { value: right_int }) =
        (&left, &right)
        && let Some(value) = integer_binary(operator, *left_int, *right_int, type_name)
    {
        return AbstractValue::Integer { value };
    }
    if matches!(operator, "==" | "!=")
        && let Some(equal) = abstract_reference_equal(&left, &right)
    {
        return AbstractValue::Integer {
            value: i64::from(if operator == "==" { equal } else { !equal }),
        };
    }
    AbstractValue::Binary {
        operator: operator.to_owned(),
        left: Box::new(left),
        right: Box::new(right),
    }
}

fn abstract_reference_equal(left: &AbstractValue, right: &AbstractValue) -> Option<bool> {
    if matches!(left, AbstractValue::Null) {
        return Some(matches!(right, AbstractValue::Null));
    }
    if matches!(right, AbstractValue::Null) {
        return abstract_definitely_non_null(left).then_some(false);
    }
    match (left, right) {
        (AbstractValue::ObjectRef { id: left, .. }, AbstractValue::ObjectRef { id: right, .. })
        | (AbstractValue::ArrayRef { id: left, .. }, AbstractValue::ArrayRef { id: right, .. }) => {
            Some(left == right)
        }
        (AbstractValue::ObjectRef { .. }, AbstractValue::ArrayRef { .. })
        | (AbstractValue::ArrayRef { .. }, AbstractValue::ObjectRef { .. }) => Some(false),
        (AbstractValue::String { value: left }, AbstractValue::String { value: right })
        | (AbstractValue::Class { class_type: left }, AbstractValue::Class { class_type: right }) => {
            Some(left == right)
        }
        _ => None,
    }
}

fn abstract_definitely_non_null(value: &AbstractValue) -> bool {
    matches!(
        value,
        AbstractValue::ObjectRef { .. }
            | AbstractValue::ArrayRef { .. }
            | AbstractValue::String { .. }
            | AbstractValue::Class { .. }
            | AbstractValue::BigInteger { .. }
            | AbstractValue::MethodHandle { .. }
            | AbstractValue::Lambda { .. }
            | AbstractValue::Sequence { .. }
            | AbstractValue::This { .. }
    )
}

fn integer_binary(operator: &str, left: i64, right: i64, type_name: &str) -> Option<i64> {
    match operator {
        "+" => java_wrapping_binary(left, right, type_name, i32::wrapping_add, i64::wrapping_add),
        "-" => java_wrapping_binary(left, right, type_name, i32::wrapping_sub, i64::wrapping_sub),
        "*" => java_wrapping_binary(left, right, type_name, i32::wrapping_mul, i64::wrapping_mul),
        "/" => java_divide(left, right, type_name),
        "%" => java_remainder(left, right, type_name),
        "==" => Some(i64::from(left == right)),
        "!=" => Some(i64::from(left != right)),
        "<" => Some(i64::from(left < right)),
        "<=" => Some(i64::from(left <= right)),
        ">" => Some(i64::from(left > right)),
        ">=" => Some(i64::from(left >= right)),
        "cmp" => Some(match left.cmp(&right) {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        }),
        _ => None,
    }
}

fn java_wrapping_binary(
    left: i64,
    right: i64,
    type_name: &str,
    int_operation: fn(i32, i32) -> i32,
    long_operation: fn(i64, i64) -> i64,
) -> Option<i64> {
    match type_name {
        "int" => Some(i64::from(int_operation(low_i32(left), low_i32(right)))),
        "long" => Some(long_operation(left, right)),
        _ => None,
    }
}

fn java_divide(left: i64, right: i64, type_name: &str) -> Option<i64> {
    if right == 0 {
        return None;
    }
    match type_name {
        "int" => {
            let left = low_i32(left);
            let right = low_i32(right);
            Some(i64::from(left.checked_div(right).unwrap_or(i32::MIN)))
        }
        "long" => Some(left.checked_div(right).unwrap_or(i64::MIN)),
        _ => None,
    }
}

fn java_remainder(left: i64, right: i64, type_name: &str) -> Option<i64> {
    if right == 0 {
        return None;
    }
    match type_name {
        "int" => {
            let left = low_i32(left);
            let right = low_i32(right);
            Some(i64::from(left.checked_rem(right).unwrap_or(0)))
        }
        "long" => Some(left.checked_rem(right).unwrap_or(0)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{JimpleValueKind, SymbolicValue, parse_methods};

    const SAMPLE: &str = r#"[
      {
        "signature":{"owner":"demo.M","name":"f","return_type":"void","parameter_types":["demo.R"]},
        "display_signature":"<demo.M: void f(demo.R)>",
        "statements":[
          {"id":0,"kind":"JIdentityStmt","text":"r := @parameter0: demo.R","def":"r","uses":[],"successors":[1],"operation":{"kind":"identity","left":{"jimple_kind":"JavaLocal","type":"demo.R","text":"r","kind":"local","name":"r"},"right":{"jimple_kind":"JParameterRef","type":"demo.R","text":"@parameter0: demo.R","kind":"parameter","index":0}}},
          {"id":1,"kind":"JAssignStmt","text":"a = r.<demo.R: long x>","def":"a","uses":[],"successors":[2],"operation":{"kind":"assign","left":{"jimple_kind":"JavaLocal","type":"long","text":"a","kind":"local","name":"a"},"right":{"jimple_kind":"JInstanceFieldRef","type":"long","text":"r.<demo.R: long x>","kind":"instance_field","base":{"jimple_kind":"JavaLocal","type":"demo.R","text":"r","kind":"local","name":"r"},"field":{"owner":"demo.R","name":"x","type":"long"}}}},
          {"id":2,"kind":"JAssignStmt","text":"b = a * 10L","def":"b","uses":[],"successors":[],"operation":{"kind":"assign","left":{"jimple_kind":"JavaLocal","type":"long","text":"b","kind":"local","name":"b"},"right":{"jimple_kind":"JMulExpr","type":"long","text":"a * 10L","kind":"binary","operator":" * ","left":{"jimple_kind":"JavaLocal","type":"long","text":"a","kind":"local","name":"a"},"right":{"jimple_kind":"LongConstant","type":"long","text":"10L","kind":"integer_constant","value":10}}}}
        ]
      }
    ]"#;

    #[test]
    fn slices_structured_defs_without_parsing_text() -> anyhow::Result<()> {
        let methods = parse_methods(SAMPLE)?;
        let method = methods
            .first()
            .ok_or_else(|| anyhow::anyhow!("method missing"))?;
        assert_eq!(method.backward_slice(2), vec![0, 1, 2]);
        Ok(())
    }

    #[test]
    fn resolves_symbolic_expression_through_locals() -> anyhow::Result<()> {
        let methods = parse_methods(SAMPLE)?;
        let method = methods
            .first()
            .ok_or_else(|| anyhow::anyhow!("method missing"))?;
        let assignment = method.symbolic_assignment(2)?;
        let SymbolicValue::Binary { left, right, .. } = assignment.value else {
            return Err(anyhow::anyhow!("expected binary symbolic value"));
        };
        let SymbolicValue::InstanceField { base, field } = *left else {
            return Err(anyhow::anyhow!("expected field provenance"));
        };
        assert_eq!(field.name, "x");
        assert!(matches!(*base, SymbolicValue::Parameter { index: 0, .. }));
        assert!(matches!(*right, SymbolicValue::Integer { value: 10, .. }));
        Ok(())
    }

    fn local(name: &str, type_name: &str) -> super::JimpleValue {
        super::JimpleValue {
            jimple_kind: String::from("JavaLocal"),
            type_name: type_name.to_owned(),
            text: name.to_owned(),
            kind: super::JimpleValueKind::Local {
                name: name.to_owned(),
            },
        }
    }

    fn this_value(type_name: &str) -> super::JimpleValue {
        super::JimpleValue {
            jimple_kind: String::from("JThisRef"),
            type_name: type_name.to_owned(),
            text: String::from("@this"),
            kind: super::JimpleValueKind::This,
        }
    }

    fn instance_field(base: super::JimpleValue, owner: &str, name: &str) -> super::JimpleValue {
        super::JimpleValue {
            jimple_kind: String::from("JInstanceFieldRef"),
            type_name: String::from("int"),
            text: format!("{name} field"),
            kind: super::JimpleValueKind::InstanceField {
                base: Box::new(base),
                field: super::JimpleFieldSignature {
                    owner: owner.to_owned(),
                    name: name.to_owned(),
                    type_name: String::from("int"),
                },
            },
        }
    }

    fn integer(value: i64) -> super::JimpleValue {
        super::JimpleValue {
            jimple_kind: String::from("IntConstant"),
            type_name: String::from("int"),
            text: value.to_string(),
            kind: super::JimpleValueKind::IntegerConstant { value },
        }
    }

    fn binary(
        operator: &str,
        left: super::JimpleValue,
        right: super::JimpleValue,
    ) -> super::JimpleValue {
        super::JimpleValue {
            jimple_kind: String::from("test_binary"),
            type_name: String::from("int"),
            text: String::from("test binary"),
            kind: super::JimpleValueKind::Binary {
                operator: operator.to_owned(),
                left: Box::new(left),
                right: Box::new(right),
            },
        }
    }

    fn statement(
        id: usize,
        successors: Vec<usize>,
        operation: super::JimpleOperation,
    ) -> super::JimpleStatement {
        super::JimpleStatement {
            id,
            kind: String::from("test"),
            text: format!("statement {id}"),
            def: None,
            uses: Vec::new(),
            successors,
            operation,
        }
    }

    #[test]
    fn recovers_temporal_counter_shape_through_jimple_locals() {
        let this_local = local("this", "demo.Timer");
        let counter = instance_field(this_local.clone(), "demo.Timer", "counter");
        let loaded = local("loaded", "int");
        let decremented = local("decremented", "int");
        let checked = local("checked", "int");
        let method = super::JimpleMethod {
            signature: super::JimpleMethodSignature {
                owner: String::from("demo.Timer"),
                name: String::from("tick"),
                return_type: String::from("void"),
                parameter_types: Vec::new(),
            },
            display_signature: String::from("<demo.Timer: void tick()>"),
            statements: vec![
                statement(
                    0,
                    vec![1],
                    super::JimpleOperation::Identity {
                        left: this_local.clone(),
                        right: this_value("demo.Timer"),
                    },
                ),
                statement(
                    1,
                    vec![2],
                    super::JimpleOperation::Assign {
                        left: loaded.clone(),
                        right: counter.clone(),
                    },
                ),
                statement(
                    2,
                    vec![3],
                    super::JimpleOperation::Assign {
                        left: decremented.clone(),
                        right: binary("-", loaded, integer(1)),
                    },
                ),
                statement(
                    3,
                    vec![4],
                    super::JimpleOperation::Assign {
                        left: counter.clone(),
                        right: decremented,
                    },
                ),
                statement(
                    4,
                    vec![5],
                    super::JimpleOperation::Assign {
                        left: checked.clone(),
                        right: counter.clone(),
                    },
                ),
                statement(
                    5,
                    vec![6],
                    super::JimpleOperation::If {
                        condition: binary("<=", checked, integer(0)),
                    },
                ),
                statement(
                    6,
                    Vec::new(),
                    super::JimpleOperation::Assign {
                        left: counter,
                        right: integer(400),
                    },
                ),
            ],
        };

        let summary = method.temporal_summary();
        assert_eq!(summary.counter_updates.len(), 1);
        assert_eq!(summary.counter_updates[0].field.name, "counter");
        assert_eq!(summary.counter_updates[0].delta, -1);
        assert_eq!(summary.comparisons.len(), 1);
        assert_eq!(summary.comparisons[0].operator, "<=");
        assert_eq!(summary.comparisons[0].constant, 0);
        assert_eq!(summary.constant_writes.len(), 1);
        assert_eq!(summary.constant_writes[0].value, 400);
    }

    #[test]
    fn recovers_boolean_materialization_as_a_predicate() -> anyhow::Result<()> {
        let this_local = local("this", "demo.Timer");
        let counter = instance_field(this_local.clone(), "demo.Timer", "counter");
        let loaded = local("loaded", "int");
        let result = local("result", "boolean");
        let mut method = super::JimpleMethod {
            signature: super::JimpleMethodSignature {
                owner: String::from("demo.Timer"),
                name: String::from("ready"),
                return_type: String::from("boolean"),
                parameter_types: Vec::new(),
            },
            display_signature: String::from("<demo.Timer: boolean ready()>"),
            statements: vec![
                statement(
                    0,
                    vec![1],
                    super::JimpleOperation::Identity {
                        left: this_local,
                        right: this_value("demo.Timer"),
                    },
                ),
                statement(
                    1,
                    vec![2],
                    super::JimpleOperation::Assign {
                        left: loaded.clone(),
                        right: counter,
                    },
                ),
                statement(
                    2,
                    vec![3, 5],
                    super::JimpleOperation::If {
                        condition: binary("<=", loaded, integer(0)),
                    },
                ),
                statement(
                    3,
                    vec![4],
                    super::JimpleOperation::Assign {
                        left: result.clone(),
                        right: integer(1),
                    },
                ),
                statement(4, vec![6], super::JimpleOperation::Goto),
                statement(
                    5,
                    vec![6],
                    super::JimpleOperation::Assign {
                        left: result.clone(),
                        right: integer(0),
                    },
                ),
                statement(
                    6,
                    Vec::new(),
                    super::JimpleOperation::Return { value: result },
                ),
            ],
        };
        method.signature.return_type = String::from("boolean");

        let Some(SymbolicValue::Unary { operator, value }) = method.boolean_materialized_return()
        else {
            return Err(anyhow::anyhow!("expected recovered boolean predicate"));
        };
        assert_eq!(operator, "!");
        let SymbolicValue::Binary {
            operator,
            left,
            right,
        } = *value
        else {
            return Err(anyhow::anyhow!("expected comparison under negation"));
        };
        assert_eq!(operator, "<=");
        assert!(matches!(*left, SymbolicValue::InstanceField { .. }));
        assert!(matches!(*right, SymbolicValue::Integer { value: 0, .. }));
        Ok(())
    }

    #[test]
    fn guards_use_cfg_reaching_definitions_and_reconstructs_piecewise_locals() -> anyhow::Result<()>
    {
        let this_local = local("this", "demo.Timer");
        let counter = instance_field(this_local.clone(), "demo.Timer", "counter");
        let gate = instance_field(this_local.clone(), "demo.Timer", "gate");
        let output = instance_field(this_local.clone(), "demo.Timer", "output");
        let merged = local("merged", "int");
        let method = super::JimpleMethod {
            signature: super::JimpleMethodSignature {
                owner: String::from("demo.Timer"),
                name: String::from("tick"),
                return_type: String::from("void"),
                parameter_types: Vec::new(),
            },
            display_signature: String::from("<demo.Timer: void tick()>"),
            statements: vec![
                statement(
                    0,
                    vec![1],
                    super::JimpleOperation::Identity {
                        left: this_local,
                        right: this_value("demo.Timer"),
                    },
                ),
                statement(
                    1,
                    vec![2, 4],
                    super::JimpleOperation::If {
                        condition: binary("<=", counter, integer(0)),
                    },
                ),
                statement(
                    2,
                    vec![3],
                    super::JimpleOperation::Assign {
                        left: gate,
                        right: integer(1),
                    },
                ),
                statement(
                    3,
                    vec![5],
                    super::JimpleOperation::Assign {
                        left: merged.clone(),
                        right: integer(1),
                    },
                ),
                statement(
                    4,
                    vec![5],
                    super::JimpleOperation::Assign {
                        left: merged.clone(),
                        right: integer(0),
                    },
                ),
                statement(
                    5,
                    vec![6],
                    super::JimpleOperation::Assign {
                        left: output,
                        right: merged,
                    },
                ),
                statement(6, Vec::new(), super::JimpleOperation::ReturnVoid),
            ],
        };

        let effects = method.effect_summary().effects;
        let writes: Vec<_> = effects
            .iter()
            .filter_map(|effect| match effect {
                super::JimpleDirectEffect::FieldWrite(write) => Some(write),
                super::JimpleDirectEffect::Invocation(_) => None,
            })
            .collect();
        assert_eq!(writes.len(), 2);
        assert_eq!(writes[0].field.name, "gate");
        assert_eq!(writes[0].guards.len(), 1);
        assert!(!writes[0].guards[0].expected);
        assert_eq!(writes[1].field.name, "output");
        assert_eq!(writes[1].guards, Vec::new());
        let SymbolicValue::Piecewise { arms } = &writes[1].value else {
            return Err(anyhow::anyhow!(
                "expected piecewise merged local, got {:?}",
                writes[1].value
            ));
        };
        assert_eq!(arms.len(), 2);
        assert!(arms.iter().any(|arm| {
            matches!(arm.value, SymbolicValue::Integer { value: 1, .. })
                && arm.guards.iter().any(|guard| !guard.expected)
        }));
        assert!(arms.iter().any(|arm| {
            matches!(arm.value, SymbolicValue::Integer { value: 0, .. })
                && arm.guards.iter().any(|guard| guard.expected)
        }));
        Ok(())
    }

    #[test]
    fn piecewise_local_tracks_conditional_overwrite_after_initialization() -> anyhow::Result<()> {
        let this_local = local("this", "demo.Timer");
        let condition = instance_field(this_local.clone(), "demo.Timer", "condition");
        let output = instance_field(this_local.clone(), "demo.Timer", "output");
        let merged = local("merged", "int");
        let method = super::JimpleMethod {
            signature: super::JimpleMethodSignature {
                owner: String::from("demo.Timer"),
                name: String::from("tick"),
                return_type: String::from("void"),
                parameter_types: Vec::new(),
            },
            display_signature: String::from("<demo.Timer: void tick()>"),
            statements: vec![
                statement(
                    0,
                    vec![1],
                    super::JimpleOperation::Identity {
                        left: this_local,
                        right: this_value("demo.Timer"),
                    },
                ),
                statement(
                    1,
                    vec![2],
                    super::JimpleOperation::Assign {
                        left: merged.clone(),
                        right: integer(0),
                    },
                ),
                statement(
                    2,
                    vec![3, 4],
                    super::JimpleOperation::If {
                        condition: binary("!=", condition, integer(0)),
                    },
                ),
                statement(
                    3,
                    vec![4],
                    super::JimpleOperation::Assign {
                        left: merged.clone(),
                        right: integer(1),
                    },
                ),
                statement(
                    4,
                    vec![5],
                    super::JimpleOperation::Assign {
                        left: output,
                        right: merged,
                    },
                ),
                statement(5, Vec::new(), super::JimpleOperation::ReturnVoid),
            ],
        };

        let effects = method.effect_summary().effects;
        let write = effects
            .iter()
            .find_map(|effect| match effect {
                super::JimpleDirectEffect::FieldWrite(write) if write.field.name == "output" => {
                    Some(write)
                }
                _ => None,
            })
            .ok_or_else(|| anyhow::anyhow!("output write missing"))?;
        let SymbolicValue::Piecewise { arms } = &write.value else {
            return Err(anyhow::anyhow!(
                "expected piecewise overwrite, got {:?}",
                write.value
            ));
        };
        assert_eq!(arms.len(), 2);
        assert!(arms.iter().any(|arm| {
            matches!(arm.value, SymbolicValue::Integer { value: 1, .. })
                && arm
                    .guards
                    .iter()
                    .any(|guard| guard.branch_statement_id == 2 && !guard.expected)
        }));
        assert!(arms.iter().any(|arm| {
            matches!(arm.value, SymbolicValue::Integer { value: 0, .. })
                && arm
                    .guards
                    .iter()
                    .any(|guard| guard.branch_statement_id == 2 && guard.expected)
        }));
        Ok(())
    }

    #[test]
    fn finitely_executes_bounded_array_loop() -> anyhow::Result<()> {
        let array = local("array", "int[]");
        let index = local("i", "int");
        let array_element = super::JimpleValue {
            jimple_kind: String::from("JArrayRef"),
            type_name: String::from("int"),
            text: String::from("array[i]"),
            kind: super::JimpleValueKind::ArrayRef {
                base: Box::new(array.clone()),
                index: Box::new(index.clone()),
            },
        };
        let method = super::JimpleMethod {
            signature: super::JimpleMethodSignature {
                owner: String::from("demo.Loop"),
                name: String::from("run"),
                return_type: String::from("void"),
                parameter_types: Vec::new(),
            },
            display_signature: String::from("<demo.Loop: void run()>"),
            statements: vec![
                statement(
                    0,
                    vec![1],
                    super::JimpleOperation::Assign {
                        left: array.clone(),
                        right: super::JimpleValue {
                            jimple_kind: String::from("JNewArrayExpr"),
                            type_name: String::from("int[]"),
                            text: String::from("newarray (int)[3]"),
                            kind: super::JimpleValueKind::NewArray {
                                element_type: String::from("int"),
                                size: Box::new(integer(3)),
                            },
                        },
                    },
                ),
                statement(
                    1,
                    vec![2],
                    super::JimpleOperation::Assign {
                        left: index.clone(),
                        right: integer(0),
                    },
                ),
                statement(
                    2,
                    vec![3, 6],
                    super::JimpleOperation::If {
                        condition: binary(">=", index.clone(), integer(3)),
                    },
                ),
                statement(
                    3,
                    vec![4],
                    super::JimpleOperation::Assign {
                        left: array_element,
                        right: index.clone(),
                    },
                ),
                statement(
                    4,
                    vec![5],
                    super::JimpleOperation::Assign {
                        left: index.clone(),
                        right: binary("+", index, integer(1)),
                    },
                ),
                statement(5, vec![2], super::JimpleOperation::Goto),
                statement(6, Vec::new(), super::JimpleOperation::ReturnVoid),
            ],
        };
        let execution = method.execute_finite(100)?;
        assert_eq!(execution.steps, 16);
        let array = execution
            .arrays
            .iter()
            .find(|array| {
                matches!(
                    array.identity,
                    super::AbstractArrayIdentity::Allocated { .. }
                )
            })
            .ok_or_else(|| anyhow::anyhow!("allocated array missing"))?;
        assert_eq!(array.length, Some(3));
        assert_eq!(array.entries.len(), 3);
        for (expected, entry) in (0_i64..3).zip(&array.entries) {
            assert_eq!(entry.index, expected);
            assert_eq!(
                entry.value,
                super::AbstractValue::Integer { value: expected }
            );
        }
        Ok(())
    }

    #[test]
    fn models_jvm_zero_and_null_defaults() {
        assert_eq!(
            super::abstract_default_value("boolean"),
            super::AbstractValue::Integer { value: 0 }
        );
        assert_eq!(
            super::abstract_default_value("long"),
            super::AbstractValue::Integer { value: 0 }
        );
        assert_eq!(
            super::abstract_default_value("double"),
            super::AbstractValue::Float { value: 0.0 }
        );
        assert_eq!(
            super::abstract_default_value("demo.Reference"),
            super::AbstractValue::Null
        );
    }

    #[test]
    fn compares_concrete_reference_identity_and_nulls() {
        let first = super::AbstractValue::ObjectRef {
            id: 7,
            class_type: String::from("demo.Value"),
        };
        let same = first.clone();
        let other = super::AbstractValue::ObjectRef {
            id: 8,
            class_type: String::from("demo.Value"),
        };
        assert_eq!(
            super::abstract_binary("==", first.clone(), same, "boolean"),
            super::AbstractValue::Integer { value: 1 }
        );
        assert_eq!(
            super::abstract_binary("==", first.clone(), other, "boolean"),
            super::AbstractValue::Integer { value: 0 }
        );
        assert_eq!(
            super::abstract_binary("!=", first, super::AbstractValue::Null, "boolean"),
            super::AbstractValue::Integer { value: 1 }
        );
    }

    #[test]
    fn reference_cast_preserves_array_identity() {
        let value = super::AbstractValue::ArrayRef {
            id: 7,
            element_type: String::from("demo.Value"),
        };
        let cast = super::abstract_cast("demo.Value[]", value.clone());
        assert_eq!(cast, value);
        assert_eq!(
            super::abstract_array_identity(&cast),
            Some(super::AbstractArrayIdentity::Allocated { id: 7 })
        );
    }

    #[test]
    fn parses_integer_constant_shape() -> anyhow::Result<()> {
        let methods = parse_methods(SAMPLE)?;
        let method = methods
            .first()
            .ok_or_else(|| anyhow::anyhow!("method missing"))?;
        let statement = method
            .statements
            .iter()
            .find(|statement| statement.id == 2)
            .ok_or_else(|| anyhow::anyhow!("statement missing"))?;
        let super::JimpleOperation::Assign { right, .. } = &statement.operation else {
            return Err(anyhow::anyhow!("assignment missing"));
        };
        let JimpleValueKind::Binary { right, .. } = &right.kind else {
            return Err(anyhow::anyhow!("binary missing"));
        };
        assert!(matches!(
            right.kind,
            JimpleValueKind::IntegerConstant { value: 10 }
        ));
        Ok(())
    }
}
