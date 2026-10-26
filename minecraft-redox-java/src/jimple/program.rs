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
    AbstractInvocation, AbstractObjectSnapshot, AbstractValue, ExecutionState, JimpleMethod,
    JimpleMethodSignature, JimpleOperation, JimpleStatement, JimpleValue, JimpleValueKind,
    MutableArray, MutableObject, abstract_array_identity, abstract_binary, abstract_cast,
    abstract_default_value, abstract_truth, abstract_unary,
};

#[derive(Clone, Debug, Default)]
pub struct JimpleProgram {
    methods: Vec<JimpleMethod>,
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
    pub static_fields: Vec<AbstractStaticFieldValue>,
    pub arrays: Vec<AbstractArraySnapshot>,
    pub objects: Vec<AbstractObjectSnapshot>,
    pub maps: Vec<AbstractMapSnapshot>,
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
        for (owner, name) in sequence {
            let method = self.unique_method(owner, name)?;
            vm.execute_method(method, None, Vec::new(), 0)?;
        }
        Ok(vm.finish())
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
    opaque_call_shapes: Vec<AbstractOpaqueCallShape>,
    unresolved_array_writes: Vec<AbstractArrayWrite>,
    exceptional_paths: Vec<AbstractValue>,
    steps: usize,
    max_steps: usize,
    path_splits: usize,
    max_path_splits: usize,
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
            opaque_call_shapes: Vec::new(),
            unresolved_array_writes: Vec::new(),
            exceptional_paths: Vec::new(),
            steps: 0,
            max_steps,
            path_splits: 0,
            max_path_splits: max_steps,
        }
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
        self.opaque_call_shapes = branch.opaque_call_shapes;
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
            JimpleValueKind::New { class_type } => self.allocate_object(class_type),
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
            _ => self.evaluate_simple(frame, value).with_context(|| {
                format!("unsupported simple Jimple value kind in {}", value.text)
            })?,
        };
        Ok(result)
    }

    fn evaluate_simple(&self, frame: &Frame, value: &JimpleValue) -> Option<AbstractValue> {
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
            JimpleValueKind::StaticField { field } => Some(
                self.state
                    .static_fields
                    .get(field)
                    .cloned()
                    .unwrap_or_else(|| AbstractValue::StaticField {
                        field: field.clone(),
                    }),
            ),
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
        if let Some(value) = self.invoke_jdk(method, base.as_ref(), &arguments, depth)? {
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
            && is_jdk_map(class_type)
        {
            return self.invoke_map(*id, &method.name, arguments).map(Some);
        }
        Ok(None)
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
        if is_jdk_map(class_type) {
            self.maps.insert(id, Vec::new());
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
            opaque_call_shapes: self.opaque_call_shapes,
            unresolved_array_writes: self.unresolved_array_writes,
            exceptional_paths: self.exceptional_paths,
        }
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

fn is_jdk_map(class_type: &str) -> bool {
    matches!(
        class_type,
        "java.util.HashMap" | "java.util.LinkedHashMap" | "java.util.EnumMap"
    )
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
