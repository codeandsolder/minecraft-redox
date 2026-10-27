use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
};

use anyhow::{Context, Result, bail};
use num_bigint::BigInt;
use serde::Serialize;

use super::{
    AbstractArrayEntry, AbstractArrayIdentity, AbstractArraySnapshot, AbstractFieldValue,
    AbstractInvocation, AbstractObjectSnapshot, AbstractValue, ExecutionState, JimpleDirectEffect,
    JimpleGuard, JimpleInvocation, JimpleMethod, JimpleMethodSignature, JimpleOperation,
    JimpleStatement, JimpleValue, JimpleValueKind, MutableArray, MutableObject,
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

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct JimpleUnresolvedEffectCall {
    pub call_path: Vec<JimpleEffectCallFrame>,
    pub guards: Vec<JimpleEffectGuard>,
    pub receiver: Option<SymbolicValue>,
    pub arguments: Vec<SymbolicValue>,
    pub reason: JimpleUnresolvedEffectReason,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct JimpleProgramEffectSummary {
    pub entry: JimpleMethodSignature,
    pub possible_field_writes: Vec<JimpleProgramFieldWrite>,
    pub unresolved_calls: Vec<JimpleUnresolvedEffectCall>,
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
        let method = self
            .method(entry)
            .with_context(|| format!("Jimple method {entry:?} does not exist"))?;
        let mut summary = JimpleProgramEffectSummary {
            entry: entry.clone(),
            possible_field_writes: Vec::new(),
            unresolved_calls: Vec::new(),
        };
        let mut active = vec![entry.clone()];
        self.expand_structural_effects(
            method,
            StructuralEffectContext {
                this_value: None,
                arguments: &[],
                call_path: &[],
                guards: &[],
            },
            &mut active,
            &mut summary,
        );
        Ok(summary)
    }

    fn expand_structural_effects(
        &self,
        method: &JimpleMethod,
        context: StructuralEffectContext<'_>,
        active: &mut Vec<JimpleMethodSignature>,
        summary: &mut JimpleProgramEffectSummary,
    ) {
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
                        value: substitute_symbolic(
                            &write.value,
                            context.this_value,
                            context.arguments,
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

    fn expand_structural_invocation(
        &self,
        current_method: &JimpleMethod,
        invocation: &JimpleInvocation,
        context: &StructuralEffectContext<'_>,
        active: &mut Vec<JimpleMethodSignature>,
        summary: &mut JimpleProgramEffectSummary,
    ) {
        let receiver = invocation
            .receiver
            .as_ref()
            .map(|receiver| substitute_symbolic(receiver, context.this_value, context.arguments));
        let call_arguments: Vec<_> = invocation
            .arguments
            .iter()
            .map(|argument| substitute_symbolic(argument, context.this_value, context.arguments))
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

        let Some(target_method) = self.method(&invocation.method) else {
            summary.unresolved_calls.push(JimpleUnresolvedEffectCall {
                call_path: next_path,
                guards: invocation_guards,
                receiver,
                arguments: call_arguments,
                reason: JimpleUnresolvedEffectReason::MissingMethod,
            });
            return;
        };

        if !structurally_exact_dispatch(
            &invocation.invoke_kind,
            receiver.as_ref(),
            &invocation.method,
        ) {
            summary.unresolved_calls.push(JimpleUnresolvedEffectCall {
                call_path: next_path,
                guards: invocation_guards,
                receiver,
                arguments: call_arguments,
                reason: JimpleUnresolvedEffectReason::DynamicDispatch,
            });
            return;
        }

        if active.contains(&target_method.signature) {
            summary.unresolved_calls.push(JimpleUnresolvedEffectCall {
                call_path: next_path,
                guards: invocation_guards,
                receiver,
                arguments: call_arguments,
                reason: JimpleUnresolvedEffectReason::RecursiveCycle,
            });
            return;
        }

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
        let exact_instance =
            exact_symbolic_runtime_type(base.as_deref()) == Some(method.owner.as_str());
        let exact_static = base.is_none()
            && self
                .method(&method)
                .is_some_and(|target| !method_has_this(target));
        if (exact_instance || exact_static)
            && let Some(target) = self.method(&method)
            && let Some(returned) = target.boolean_materialized_return()
        {
            let substituted = substitute_symbolic(&returned, base.as_deref(), &arguments);
            return self.simplify_guard_value(substituted, depth + 1);
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

    fn class_initializer(&self, owner: &str) -> Option<&JimpleMethod> {
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

fn simplify_boolean_comparison(
    operator: String,
    left: SymbolicValue,
    right: SymbolicValue,
) -> SymbolicValue {
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

fn structurally_exact_dispatch(
    invoke_kind: &str,
    receiver: Option<&SymbolicValue>,
    method: &JimpleMethodSignature,
) -> bool {
    match invoke_kind {
        "JStaticInvokeExpr" | "JSpecialInvokeExpr" => true,
        "JVirtualInvokeExpr" => {
            exact_symbolic_runtime_type(receiver) == Some(method.owner.as_str())
        }
        _ => false,
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
        assert!(
            effects
                .possible_field_writes
                .iter()
                .all(|write| write.field == timer)
        );
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
