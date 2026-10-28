//! Lower structured Jimple into the source-neutral transaction CFG.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail};
use minecraft_redox_core::transaction::{
    TransactionEffect, TransactionFunction, TransactionFunctionId, TransactionLocalId,
    TransactionNumericType, TransactionOperation, TransactionProgram, TransactionStatement,
    TransactionStatementId, TransactionStoreQueryKind, TransactionSwitchCase, TransactionValue,
};

use super::{
    AbstractArrayIdentity, AbstractValue, JimpleCallOperandBinding, JimpleFieldSignature,
    JimpleIndexedStoreOperationKind, JimpleIndexedStoreQueryKind, JimpleMethod,
    JimpleMethodSignature, JimpleOperation, JimpleProgram, JimplePureValueOperation,
    JimpleStatement, JimpleStaticConstantBinding, JimpleStaticConstantValue,
    JimpleTransactionBindings, JimpleValue, JimpleValueKind, ProgramExecution,
};

impl JimpleProgram {
    /// Lower one exact Jimple entrypoint and its exact in-program helper calls into a
    /// source-language-free transaction CFG.
    ///
    /// Source API calls must be covered by `bindings`. Dynamic source dispatch that is not
    /// explicitly semantic-bound stays fail-closed rather than guessing a runtime target.
    ///
    /// # Errors
    ///
    /// Returns an error for missing methods, ambiguous semantic bindings, unsupported source
    /// values/control flow, or any reached call that is neither semantic-bound nor an exact
    /// in-program static/special helper.
    pub fn lower_transaction_program(
        &self,
        entry: &JimpleMethodSignature,
        bindings: &JimpleTransactionBindings,
    ) -> Result<TransactionProgram> {
        self.lower_transaction_program_impl(entry, None, bindings)
    }

    /// Lower one entrypoint assuming an exact runtime type for its `this` receiver.
    ///
    /// This enables virtual/interface calls on `this` to dispatch through exported class hierarchy
    /// metadata. Calls on other dynamic receivers remain fail-closed.
    ///
    /// # Errors
    ///
    /// Returns an error when the runtime type is not a known subtype, dispatch is ambiguous or
    /// missing, one helper would require conflicting runtime specializations, or ordinary
    /// transaction lowering fails.
    pub fn lower_transaction_program_for_runtime_this(
        &self,
        entry: &JimpleMethodSignature,
        runtime_type: &str,
        bindings: &JimpleTransactionBindings,
    ) -> Result<TransactionProgram> {
        if !self.is_known_subtype(runtime_type, &entry.owner) {
            bail!(
                "runtime transaction type {runtime_type:?} is not a known subtype of {:?}",
                entry.owner
            );
        }
        self.lower_transaction_program_impl(entry, Some(runtime_type), bindings)
    }

    fn lower_transaction_program_impl(
        &self,
        entry: &JimpleMethodSignature,
        runtime_type: Option<&str>,
        bindings: &JimpleTransactionBindings,
    ) -> Result<TransactionProgram> {
        let methods = discover_transaction_methods(self, entry, runtime_type, bindings)?;
        let function_ids: Vec<_> = methods
            .iter()
            .enumerate()
            .map(|(index, method)| {
                let id = u32::try_from(index).context("transaction function count exceeds u32")?;
                Ok((method.signature.clone(), TransactionFunctionId(id)))
            })
            .collect::<Result<_>>()?;
        let functions = methods
            .iter()
            .map(|discovered| {
                let method = self.method(&discovered.signature).with_context(|| {
                    format!("Jimple method {:?} does not exist", discovered.signature)
                })?;
                lower_transaction_function(
                    self,
                    method,
                    discovered.runtime_this.as_deref(),
                    bindings,
                    &function_ids,
                )
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(TransactionProgram {
            entry: TransactionFunctionId(0),
            functions,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DiscoveredTransactionMethod {
    signature: JimpleMethodSignature,
    runtime_this: Option<String>,
}

fn discover_transaction_methods(
    program: &JimpleProgram,
    entry: &JimpleMethodSignature,
    runtime_type: Option<&str>,
    bindings: &JimpleTransactionBindings,
) -> Result<Vec<DiscoveredTransactionMethod>> {
    program
        .method(entry)
        .with_context(|| format!("Jimple method {entry:?} does not exist"))?;
    let mut discovered = vec![DiscoveredTransactionMethod {
        signature: entry.clone(),
        runtime_this: runtime_type.map(str::to_owned),
    }];
    let mut index = 0;
    while index < discovered.len() {
        let current = discovered[index].clone();
        let method = program
            .method(&current.signature)
            .with_context(|| format!("Jimple method {:?} disappeared", current.signature))?;
        let this_aliases = this_alias_locals(method);
        let mut calls = Vec::new();
        for statement in &method.statements {
            collect_statement_calls(statement, &mut calls);
        }
        for call in calls {
            if semantic_call_binding_count(call.callee, bindings) > 0 {
                continue;
            }
            let (target, target_runtime_this) = resolve_transaction_call_target(
                program,
                &current,
                call.invoke_kind,
                call.base
                    .is_some_and(|base| value_is_this_alias(base, &this_aliases)),
                call.callee,
            )?;
            if let Some(existing) = discovered.iter().find(|item| item.signature == target) {
                if existing.runtime_this != target_runtime_this {
                    bail!(
                        "transaction helper {target:?} requires conflicting runtime-this specializations {:?} and {:?}",
                        existing.runtime_this,
                        target_runtime_this
                    );
                }
            } else {
                discovered.push(DiscoveredTransactionMethod {
                    signature: target,
                    runtime_this: target_runtime_this,
                });
            }
        }
        index += 1;
    }
    Ok(discovered)
}

struct TransactionSourceCall<'a> {
    invoke_kind: &'a str,
    callee: &'a JimpleMethodSignature,
    base: Option<&'a JimpleValue>,
}

fn resolve_transaction_call_target(
    program: &JimpleProgram,
    current: &DiscoveredTransactionMethod,
    invoke_kind: &str,
    base_is_this: bool,
    callee: &JimpleMethodSignature,
) -> Result<(JimpleMethodSignature, Option<String>)> {
    match invoke_kind {
        "JStaticInvokeExpr" => {
            let target = program
                .method(callee)
                .or_else(|| program.dispatched_method_in_hierarchy(&callee.owner, callee))
                .with_context(|| format!("unbound external transaction call {callee:?}"))?;
            Ok((target.signature.clone(), None))
        }
        "JSpecialInvokeExpr" => {
            program
                .method(callee)
                .with_context(|| format!("unbound external transaction call {callee:?}"))?;
            let runtime_this = base_is_this.then(|| current.runtime_this.clone()).flatten();
            Ok((callee.clone(), runtime_this))
        }
        "JVirtualInvokeExpr" | "JInterfaceInvokeExpr" if base_is_this => {
            let runtime_this = current.runtime_this.as_deref().with_context(|| {
                format!("dynamic transaction call {callee:?} requires exact runtime `this`")
            })?;
            let target = program
                .dispatched_method_in_hierarchy(runtime_this, callee)
                .with_context(|| {
                    format!(
                        "runtime dispatch from {runtime_this:?} cannot resolve transaction call {callee:?}"
                    )
                })?;
            Ok((target.signature.clone(), Some(runtime_this.to_owned())))
        }
        _ => bail!("unbound dynamic transaction call {callee:?}"),
    }
}

fn this_alias_locals(method: &JimpleMethod) -> BTreeSet<String> {
    let candidates: BTreeSet<_> = method
        .statements
        .iter()
        .filter_map(|statement| match &statement.operation {
            JimpleOperation::Identity { left, right }
                if matches!(right.kind, JimpleValueKind::This) =>
            {
                match &left.kind {
                    JimpleValueKind::Local { name } => Some(name.clone()),
                    _ => None,
                }
            }
            _ => None,
        })
        .collect();
    candidates
        .into_iter()
        .filter(|candidate| {
            !method.statements.iter().any(|statement| {
                matches!(
                    &statement.operation,
                    JimpleOperation::Assign {
                        left: JimpleValue {
                            kind: JimpleValueKind::Local { name },
                            ..
                        },
                        ..
                    } if name == candidate
                )
            })
        })
        .collect()
}

fn value_is_this_alias(value: &JimpleValue, aliases: &BTreeSet<String>) -> bool {
    match &value.kind {
        JimpleValueKind::This => true,
        JimpleValueKind::Local { name } => aliases.contains(name),
        _ => false,
    }
}

fn collect_statement_calls<'a>(
    statement: &'a JimpleStatement,
    calls: &mut Vec<TransactionSourceCall<'a>>,
) {
    match &statement.operation {
        JimpleOperation::Assign { left, right } | JimpleOperation::Identity { left, right } => {
            collect_value_calls(left, calls);
            collect_value_calls(right, calls);
        }
        JimpleOperation::If { condition }
        | JimpleOperation::Return { value: condition }
        | JimpleOperation::Throw { value: condition } => collect_value_calls(condition, calls),
        JimpleOperation::Invoke {
            expression: Some(expression),
        } => collect_value_calls(expression, calls),
        JimpleOperation::Switch { key, .. } => collect_value_calls(key, calls),
        JimpleOperation::Invoke { expression: None }
        | JimpleOperation::ReturnVoid
        | JimpleOperation::Goto
        | JimpleOperation::Opaque => {}
    }
}

fn collect_value_calls<'a>(value: &'a JimpleValue, calls: &mut Vec<TransactionSourceCall<'a>>) {
    match &value.kind {
        JimpleValueKind::Invoke {
            invoke_kind,
            method,
            base,
            arguments,
            bootstrap_arguments,
            ..
        } => {
            calls.push(TransactionSourceCall {
                invoke_kind,
                callee: method,
                base: base.as_deref(),
            });
            if let Some(base) = base {
                collect_value_calls(base, calls);
            }
            for argument in arguments.iter().chain(bootstrap_arguments) {
                collect_value_calls(argument, calls);
            }
        }
        JimpleValueKind::ArrayRef { base, index }
        | JimpleValueKind::Binary {
            left: base,
            right: index,
            ..
        } => {
            collect_value_calls(base, calls);
            collect_value_calls(index, calls);
        }
        JimpleValueKind::InstanceField { base, .. }
        | JimpleValueKind::Cast { value: base, .. }
        | JimpleValueKind::InstanceOf { value: base, .. }
        | JimpleValueKind::Unary { value: base, .. }
        | JimpleValueKind::ArrayLength { value: base }
        | JimpleValueKind::NewArray { size: base, .. } => collect_value_calls(base, calls),
        JimpleValueKind::NewMultiArray { sizes, .. } | JimpleValueKind::Phi { values: sizes } => {
            for value in sizes {
                collect_value_calls(value, calls);
            }
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
}

fn semantic_call_binding_count(
    callee: &JimpleMethodSignature,
    bindings: &JimpleTransactionBindings,
) -> usize {
    bindings
        .indexed_stores
        .iter()
        .filter(|binding| binding.callee == *callee)
        .count()
        + bindings
            .snapshot_clones
            .iter()
            .filter(|binding| binding.callee == *callee)
            .count()
        + bindings
            .pure_values
            .iter()
            .filter(|binding| binding.callee == *callee)
            .count()
        + bindings
            .store_queries
            .iter()
            .filter(|binding| binding.callee == *callee)
            .count()
        + bindings
            .state_writes
            .iter()
            .filter(|binding| binding.callee == *callee)
            .count()
        + bindings
            .host_queries
            .iter()
            .filter(|binding| binding.callee == *callee)
            .count()
        + bindings
            .host_calls
            .iter()
            .filter(|binding| binding.callee == *callee)
            .count()
        + bindings
            .schedule_once
            .iter()
            .filter(|binding| binding.callee == *callee)
            .count()
}

struct FunctionLowerer<'a> {
    program: &'a JimpleProgram,
    bindings: &'a JimpleTransactionBindings,
    function_ids: &'a [(JimpleMethodSignature, TransactionFunctionId)],
    runtime_this: Option<&'a str>,
    this_aliases: BTreeSet<String>,
    locals: Vec<String>,
    elided_assignments: BTreeSet<usize>,
    static_values: BTreeMap<JimpleFieldSignature, TransactionValue>,
}

fn lower_transaction_function(
    program: &JimpleProgram,
    method: &JimpleMethod,
    runtime_this: Option<&str>,
    bindings: &JimpleTransactionBindings,
    function_ids: &[(JimpleMethodSignature, TransactionFunctionId)],
) -> Result<TransactionFunction> {
    let id = function_id(function_ids, &method.signature)
        .with_context(|| format!("transaction function ID missing for {:?}", method.signature))?;
    let first = method
        .statements
        .first()
        .with_context(|| format!("cannot lower empty method {}", method.display_signature))?;
    let mut lowerer = FunctionLowerer {
        program,
        bindings,
        function_ids,
        runtime_this,
        this_aliases: this_alias_locals(method),
        locals: Vec::new(),
        elided_assignments: elidable_local_assignments(method, bindings),
        static_values: BTreeMap::new(),
    };
    let statements = method
        .statements
        .iter()
        .map(|statement| lowerer.lower_statement(statement))
        .collect::<Result<Vec<_>>>()?;
    let local_count =
        u32::try_from(lowerer.locals.len()).context("transaction local count exceeds u32")?;
    Ok(TransactionFunction {
        id,
        parameter_count: method.signature.parameter_types.len(),
        local_count,
        entry_statement: statement_id(first.id)?,
        statements,
    })
}

fn elidable_local_assignments(
    method: &JimpleMethod,
    bindings: &JimpleTransactionBindings,
) -> BTreeSet<usize> {
    let index_by_id: BTreeMap<_, _> = method
        .statements
        .iter()
        .enumerate()
        .map(|(index, statement)| (statement.id, index))
        .collect();
    let uses: Vec<_> = method
        .statements
        .iter()
        .map(|statement| semantic_statement_local_uses(statement, bindings))
        .collect();
    let defs: Vec<_> = method.statements.iter().map(local_definition).collect();
    let mut live_in = vec![BTreeSet::new(); method.statements.len()];
    let mut live_out = vec![BTreeSet::new(); method.statements.len()];
    loop {
        let mut changed = false;
        for index in (0..method.statements.len()).rev() {
            let statement = &method.statements[index];
            let mut next_out = BTreeSet::new();
            for successor in &statement.successors {
                if let Some(successor_index) = index_by_id.get(successor) {
                    next_out.extend(live_in[*successor_index].iter().cloned());
                }
            }
            let mut next_in = uses[index].clone();
            let mut surviving = next_out.clone();
            if let Some(definition) = &defs[index] {
                surviving.remove(definition);
            }
            next_in.extend(surviving);
            if next_out != live_out[index] || next_in != live_in[index] {
                live_out[index] = next_out;
                live_in[index] = next_in;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    method
        .statements
        .iter()
        .enumerate()
        .filter_map(|(index, statement)| {
            let JimpleOperation::Assign { left, right } = &statement.operation else {
                return None;
            };
            let JimpleValueKind::Local { name } = &left.kind else {
                return None;
            };
            (!live_out[index].contains(name) && !value_contains_invoke(right))
                .then_some(statement.id)
        })
        .collect()
}

fn local_definition(statement: &JimpleStatement) -> Option<String> {
    match &statement.operation {
        JimpleOperation::Assign { left, .. } | JimpleOperation::Identity { left, .. } => {
            let JimpleValueKind::Local { name } = &left.kind else {
                return None;
            };
            Some(name.clone())
        }
        _ => None,
    }
}

fn semantic_statement_local_uses(
    statement: &JimpleStatement,
    bindings: &JimpleTransactionBindings,
) -> BTreeSet<String> {
    let mut uses = BTreeSet::new();
    match &statement.operation {
        JimpleOperation::Assign { right, .. } | JimpleOperation::Identity { right, .. } => {
            collect_semantic_local_uses(right, bindings, &mut uses);
        }
        JimpleOperation::If { condition }
        | JimpleOperation::Return { value: condition }
        | JimpleOperation::Throw { value: condition } => {
            collect_semantic_local_uses(condition, bindings, &mut uses);
        }
        JimpleOperation::Invoke {
            expression: Some(expression),
        } => collect_semantic_local_uses(expression, bindings, &mut uses),
        JimpleOperation::Switch { key, .. } => {
            collect_semantic_local_uses(key, bindings, &mut uses);
        }
        JimpleOperation::Invoke { expression: None }
        | JimpleOperation::ReturnVoid
        | JimpleOperation::Goto
        | JimpleOperation::Opaque => {}
    }
    uses
}

fn collect_semantic_local_uses(
    value: &JimpleValue,
    bindings: &JimpleTransactionBindings,
    uses: &mut BTreeSet<String>,
) {
    match &value.kind {
        JimpleValueKind::Local { name } => {
            uses.insert(name.clone());
        }
        JimpleValueKind::Invoke {
            method,
            base,
            arguments,
            bootstrap_arguments,
            ..
        } => {
            let query_matches: Vec<_> = bindings
                .host_queries
                .iter()
                .filter(|binding| binding.callee == *method)
                .map(|binding| (binding.receiver.as_ref(), binding.arguments.as_slice()))
                .collect();
            let call_matches: Vec<_> = bindings
                .host_calls
                .iter()
                .filter(|binding| binding.callee == *method)
                .map(|binding| (binding.receiver.as_ref(), binding.arguments.as_slice()))
                .collect();
            let semantic_matches: Vec<_> = query_matches.into_iter().chain(call_matches).collect();
            if let [(receiver_selector, argument_selectors)] = semantic_matches.as_slice() {
                if let Some(selector) = receiver_selector
                    && let Some(selected) = select_raw_operand(base.as_deref(), arguments, selector)
                {
                    collect_semantic_local_uses(selected, bindings, uses);
                }
                for selector in *argument_selectors {
                    if let Some(selected) = select_raw_operand(base.as_deref(), arguments, selector)
                    {
                        collect_semantic_local_uses(selected, bindings, uses);
                    }
                }
                return;
            }
            if let Some(base) = base {
                collect_semantic_local_uses(base, bindings, uses);
            }
            for argument in arguments.iter().chain(bootstrap_arguments) {
                collect_semantic_local_uses(argument, bindings, uses);
            }
        }
        JimpleValueKind::ArrayRef { base, index }
        | JimpleValueKind::Binary {
            left: base,
            right: index,
            ..
        } => {
            collect_semantic_local_uses(base, bindings, uses);
            collect_semantic_local_uses(index, bindings, uses);
        }
        JimpleValueKind::InstanceField { base, .. }
        | JimpleValueKind::Cast { value: base, .. }
        | JimpleValueKind::InstanceOf { value: base, .. }
        | JimpleValueKind::Unary { value: base, .. }
        | JimpleValueKind::ArrayLength { value: base }
        | JimpleValueKind::NewArray { size: base, .. } => {
            collect_semantic_local_uses(base, bindings, uses);
        }
        JimpleValueKind::NewMultiArray { sizes, .. } | JimpleValueKind::Phi { values: sizes } => {
            for item in sizes {
                collect_semantic_local_uses(item, bindings, uses);
            }
        }
        JimpleValueKind::IntegerConstant { .. }
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
}

fn select_raw_operand<'a>(
    receiver: Option<&'a JimpleValue>,
    arguments: &'a [JimpleValue],
    selector: &JimpleCallOperandBinding,
) -> Option<&'a JimpleValue> {
    match selector {
        JimpleCallOperandBinding::Receiver => receiver,
        JimpleCallOperandBinding::Argument { index } => arguments.get(*index),
    }
}

fn value_contains_invoke(value: &JimpleValue) -> bool {
    match &value.kind {
        JimpleValueKind::Invoke { .. } => true,
        JimpleValueKind::ArrayRef { base, index }
        | JimpleValueKind::Binary {
            left: base,
            right: index,
            ..
        } => value_contains_invoke(base) || value_contains_invoke(index),
        JimpleValueKind::InstanceField { base, .. }
        | JimpleValueKind::Cast { value: base, .. }
        | JimpleValueKind::InstanceOf { value: base, .. }
        | JimpleValueKind::Unary { value: base, .. }
        | JimpleValueKind::ArrayLength { value: base }
        | JimpleValueKind::NewArray { size: base, .. } => value_contains_invoke(base),
        JimpleValueKind::NewMultiArray { sizes, .. } | JimpleValueKind::Phi { values: sizes } => {
            sizes.iter().any(value_contains_invoke)
        }
        _ => false,
    }
}

impl FunctionLowerer<'_> {
    fn lower_statement(&mut self, statement: &JimpleStatement) -> Result<TransactionStatement> {
        let id = statement_id(statement.id)?;
        if self.elided_assignments.contains(&statement.id) {
            return Ok(TransactionStatement {
                id,
                operation: TransactionOperation::Jump {
                    target: single_successor(statement)?,
                },
            });
        }
        let operation = match &statement.operation {
            JimpleOperation::Assign { left, right } | JimpleOperation::Identity { left, right } => {
                self.lower_assignment(statement, left, right)?
            }
            JimpleOperation::If { condition } => {
                let [when_false, when_true] = statement.successors.as_slice() else {
                    bail!(
                        "transaction branch {} does not have two successors",
                        statement.id
                    );
                };
                TransactionOperation::Branch {
                    condition: self.lower_value(condition)?,
                    when_false: statement_id(*when_false)?,
                    when_true: statement_id(*when_true)?,
                }
            }
            JimpleOperation::Invoke {
                expression: Some(expression),
            } => {
                let next = single_successor(statement)?;
                match self.lower_invoke(expression)? {
                    LoweredInvoke::Value(value) => TransactionOperation::Evaluate { value, next },
                    LoweredInvoke::Effect(effect) => TransactionOperation::Effect { effect, next },
                }
            }
            JimpleOperation::Invoke { expression: None } => {
                bail!(
                    "transaction invoke statement {} has no expression",
                    statement.id
                );
            }
            JimpleOperation::Return { value } => TransactionOperation::Return {
                value: Some(self.lower_value(value)?),
            },
            JimpleOperation::ReturnVoid => TransactionOperation::Return { value: None },
            JimpleOperation::Goto => {
                let target = single_successor(statement)?;
                TransactionOperation::Jump { target }
            }
            JimpleOperation::Throw { .. } => {
                bail!(
                    "transaction lowering does not yet support throw at {}",
                    statement.id
                );
            }
            JimpleOperation::Switch {
                key,
                cases,
                default_target,
            } => TransactionOperation::Switch {
                key: self.lower_value(key)?,
                cases: cases
                    .iter()
                    .map(|case| {
                        Ok(TransactionSwitchCase {
                            value: case.value,
                            target: statement_id(case.target)?,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?,
                default_target: statement_id(*default_target)?,
            },
            JimpleOperation::Opaque => {
                bail!(
                    "transaction lowering reached opaque statement {}",
                    statement.id
                );
            }
        };
        Ok(TransactionStatement { id, operation })
    }

    fn lower_assignment(
        &mut self,
        statement: &JimpleStatement,
        left: &JimpleValue,
        right: &JimpleValue,
    ) -> Result<TransactionOperation> {
        let next = single_successor(statement)?;
        match &left.kind {
            JimpleValueKind::Local { name } => Ok(TransactionOperation::Assign {
                target: self.local_id(name)?,
                value: self.lower_value(right)?,
                next,
            }),
            JimpleValueKind::InstanceField { base, field } => {
                let component = unique_component(self.program, field, self.bindings)?;
                Ok(TransactionOperation::SetComponent {
                    target: self.lower_value(base)?,
                    component,
                    value: self.lower_value(right)?,
                    next,
                })
            }
            _ => bail!(
                "unsupported transaction assignment target {:?} at statement {}",
                left.kind,
                statement.id
            ),
        }
    }

    fn lower_value(&mut self, value: &JimpleValue) -> Result<TransactionValue> {
        match &value.kind {
            JimpleValueKind::Local { name } => Ok(TransactionValue::Local {
                local: self.local_id(name)?,
            }),
            JimpleValueKind::IntegerConstant { value } => {
                Ok(TransactionValue::Integer { value: *value })
            }
            JimpleValueKind::FloatConstant { value } => {
                Ok(TransactionValue::Float { value: *value })
            }
            JimpleValueKind::StringConstant { value } => Ok(TransactionValue::String {
                value: value.clone(),
            }),
            JimpleValueKind::NullConstant => Ok(TransactionValue::Null),
            JimpleValueKind::Parameter { index } => {
                Ok(TransactionValue::Parameter { index: *index })
            }
            JimpleValueKind::This => Ok(TransactionValue::This),
            JimpleValueKind::StaticField { field } => self.lower_static_field(field),
            JimpleValueKind::ArrayRef { base, index } => Ok(TransactionValue::Index {
                sequence: Box::new(self.lower_value(base)?),
                index: Box::new(self.lower_value(index)?),
            }),
            JimpleValueKind::InstanceField { base, field } => Ok(TransactionValue::Component {
                value: Box::new(self.lower_value(base)?),
                component: unique_component(self.program, field, self.bindings)?,
            }),
            JimpleValueKind::Binary {
                operator,
                left,
                right,
            } => Ok(TransactionValue::Binary {
                operator: operator.trim().to_owned(),
                left: Box::new(self.lower_value(left)?),
                right: Box::new(self.lower_value(right)?),
            }),
            JimpleValueKind::Cast {
                cast_type,
                value: inner,
            } => match cast_type.as_str() {
                "int" => Ok(TransactionValue::NumericCast {
                    target: TransactionNumericType::I32,
                    value: Box::new(self.lower_value(inner)?),
                }),
                "double" => Ok(TransactionValue::NumericCast {
                    target: TransactionNumericType::F64,
                    value: Box::new(self.lower_value(inner)?),
                }),
                _ => {
                    unique_capability(cast_type, self.bindings)?;
                    self.lower_value(inner)
                }
            },
            JimpleValueKind::InstanceOf {
                value: inner,
                check_type,
            } => Ok(TransactionValue::HasCapability {
                value: Box::new(self.lower_value(inner)?),
                capability: unique_capability(check_type, self.bindings)?,
            }),
            JimpleValueKind::Unary {
                operator,
                value: inner,
            } => Ok(TransactionValue::Unary {
                operator: operator.clone(),
                value: Box::new(self.lower_value(inner)?),
            }),
            JimpleValueKind::ArrayLength { value: inner } => Ok(TransactionValue::Length {
                sequence: Box::new(self.lower_value(inner)?),
            }),
            JimpleValueKind::Invoke { .. } => match self.lower_invoke(value)? {
                LoweredInvoke::Value(value) => Ok(value),
                LoweredInvoke::Effect(_) => {
                    bail!(
                        "void/effectful transaction call used as a value: {}",
                        value.text
                    )
                }
            },
            JimpleValueKind::ClassConstant { .. }
            | JimpleValueKind::MethodHandle { .. }
            | JimpleValueKind::MethodType { .. }
            | JimpleValueKind::New { .. }
            | JimpleValueKind::NewArray { .. }
            | JimpleValueKind::NewMultiArray { .. }
            | JimpleValueKind::Phi { .. }
            | JimpleValueKind::Opaque => {
                bail!(
                    "unsupported transaction value {:?}: {}",
                    value.kind,
                    value.text
                )
            }
        }
    }

    fn lower_static_field(&mut self, field: &JimpleFieldSignature) -> Result<TransactionValue> {
        if let Some(binding) = unique_static_constant(field, &self.bindings.static_constants)? {
            return lower_static_constant(&binding.value);
        }
        if let Some(value) = self.static_values.get(field) {
            return Ok(value.clone());
        }
        self.program
            .class_initializer(&field.owner)
            .with_context(|| {
                format!("transaction static field {field:?} has no reachable class initializer")
            })?;
        let execution = self
            .program
            .execute_named_sequence(&[(field.owner.as_str(), "<clinit>")], 100_000)
            .with_context(|| format!("failed to recover startup value for {field:?}"))?;
        for candidate in &execution.static_fields {
            if let Ok(lowered) = lower_abstract_constant(&candidate.value, &execution) {
                self.static_values
                    .entry(candidate.field.clone())
                    .or_insert(lowered);
            }
        }
        if let Some(lowered) = self.static_values.get(field) {
            return Ok(lowered.clone());
        }
        let value = execution
            .static_field_value(&field.owner, &field.name)
            .with_context(|| format!("class initializer did not materialize {field:?}"))?;
        lower_abstract_constant(value, &execution)
            .with_context(|| format!("static field {field:?} is not a target constant"))
    }

    fn lower_invoke(&mut self, value: &JimpleValue) -> Result<LoweredInvoke> {
        let JimpleValueKind::Invoke {
            invoke_kind,
            method,
            base,
            arguments,
            ..
        } = &value.kind
        else {
            bail!("expected invocation value, got {}", value.text);
        };
        if semantic_call_binding_count(method, self.bindings) > 1 {
            bail!("ambiguous transaction semantic bindings for {method:?}");
        }
        if let Some(lowered) =
            self.lower_raw_host_call_invoke(method, base.as_deref(), arguments)?
        {
            return Ok(lowered);
        }
        if let Some(lowered) =
            self.lower_raw_host_query_invoke(method, base.as_deref(), arguments)?
        {
            return Ok(lowered);
        }
        let receiver = base
            .as_deref()
            .map(|base| self.lower_value(base))
            .transpose()?;
        let arguments = arguments
            .iter()
            .map(|argument| self.lower_value(argument))
            .collect::<Result<Vec<_>>>()?;
        if let Some(lowered) = self.lower_store_invoke(method, receiver.as_ref(), &arguments)? {
            return Ok(lowered);
        }
        if self
            .bindings
            .snapshot_clones
            .iter()
            .any(|binding| binding.callee == *method)
        {
            let value = receiver.context("snapshot clone call is missing receiver")?;
            if !arguments.is_empty() {
                bail!("snapshot clone call unexpectedly has arguments");
            }
            return Ok(LoweredInvoke::Value(TransactionValue::CloneValue {
                value: Box::new(value),
            }));
        }
        if let Some(lowered) =
            self.lower_pure_value_invoke(method, receiver.as_ref(), &arguments)?
        {
            return Ok(lowered);
        }
        if let Some(lowered) =
            self.lower_store_query_invoke(method, receiver.as_ref(), &arguments)?
        {
            return Ok(lowered);
        }
        if let Some(lowered) =
            self.lower_state_write_invoke(method, receiver.as_ref(), &arguments)?
        {
            return Ok(lowered);
        }
        if let Some(lowered) = self.lower_schedule_once_invoke(method, &arguments)? {
            return Ok(lowered);
        }
        self.lower_internal_invoke(
            invoke_kind,
            method,
            base.as_deref()
                .is_some_and(|base| value_is_this_alias(base, &self.this_aliases)),
            receiver,
            arguments,
        )
    }

    fn lower_store_invoke(
        &self,
        method: &JimpleMethodSignature,
        receiver: Option<&TransactionValue>,
        arguments: &[TransactionValue],
    ) -> Result<Option<LoweredInvoke>> {
        let Some(binding) = self
            .bindings
            .indexed_stores
            .iter()
            .find(|binding| binding.callee == *method)
        else {
            return Ok(None);
        };
        let store = select_operand(receiver, arguments, &binding.store, "store")?;
        let key = select_operands(receiver, arguments, &binding.key, "key")?;
        let lowered = match binding.operation {
            JimpleIndexedStoreOperationKind::Read => {
                LoweredInvoke::Value(TransactionValue::StoreRead {
                    store: Box::new(store),
                    key,
                })
            }
            JimpleIndexedStoreOperationKind::Take => {
                let amount = binding
                    .amount
                    .as_ref()
                    .context("store take binding is missing amount")?;
                LoweredInvoke::Value(TransactionValue::StoreTake {
                    store: Box::new(store),
                    key,
                    amount: Box::new(select_operand(receiver, arguments, amount, "amount")?),
                })
            }
            JimpleIndexedStoreOperationKind::Write => {
                let value = binding
                    .value
                    .as_ref()
                    .context("store write binding is missing value")?;
                LoweredInvoke::Effect(TransactionEffect::StoreWrite {
                    store,
                    key,
                    value: select_operand(receiver, arguments, value, "value")?,
                })
            }
            JimpleIndexedStoreOperationKind::Touch => {
                LoweredInvoke::Effect(TransactionEffect::StoreTouch { store })
            }
        };
        Ok(Some(lowered))
    }

    fn lower_pure_value_invoke(
        &self,
        method: &JimpleMethodSignature,
        receiver: Option<&TransactionValue>,
        arguments: &[TransactionValue],
    ) -> Result<Option<LoweredInvoke>> {
        let Some(binding) = self
            .bindings
            .pure_values
            .iter()
            .find(|binding| binding.callee == *method)
        else {
            return Ok(None);
        };
        let lowered = match &binding.operation {
            JimplePureValueOperation::Component { component } => {
                let receiver = receiver.context("component query is missing receiver")?;
                if !arguments.is_empty() {
                    bail!("component query unexpectedly has arguments");
                }
                TransactionValue::Component {
                    value: Box::new(receiver.clone()),
                    component: component.clone(),
                }
            }
            JimplePureValueOperation::Equal => {
                let [left, right] = arguments else {
                    bail!("semantic equality requires two arguments");
                };
                TransactionValue::Equal {
                    left: Box::new(left.clone()),
                    right: Box::new(right.clone()),
                }
            }
            JimplePureValueOperation::Min => {
                let [left, right] = arguments else {
                    bail!("semantic min requires two arguments");
                };
                TransactionValue::Min {
                    left: Box::new(left.clone()),
                    right: Box::new(right.clone()),
                }
            }
            JimplePureValueOperation::Max => {
                let [left, right] = arguments else {
                    bail!("semantic max requires two arguments");
                };
                TransactionValue::Max {
                    left: Box::new(left.clone()),
                    right: Box::new(right.clone()),
                }
            }
            JimplePureValueOperation::Sequence => {
                if receiver.is_some() {
                    bail!("semantic sequence unexpectedly has a receiver");
                }
                TransactionValue::Sequence {
                    values: arguments.to_vec(),
                }
            }
            JimplePureValueOperation::Length => {
                let receiver = receiver.context("semantic length is missing receiver")?;
                if !arguments.is_empty() {
                    bail!("semantic length unexpectedly has arguments");
                }
                TransactionValue::Length {
                    sequence: Box::new(receiver.clone()),
                }
            }
            JimplePureValueOperation::Index => {
                let receiver = receiver.context("semantic index is missing receiver")?;
                let [index] = arguments else {
                    bail!("semantic index requires one argument");
                };
                TransactionValue::Index {
                    sequence: Box::new(receiver.clone()),
                    index: Box::new(index.clone()),
                }
            }
        };
        Ok(Some(LoweredInvoke::Value(lowered)))
    }

    fn lower_store_query_invoke(
        &self,
        method: &JimpleMethodSignature,
        receiver: Option<&TransactionValue>,
        arguments: &[TransactionValue],
    ) -> Result<Option<LoweredInvoke>> {
        let Some(binding) = self
            .bindings
            .store_queries
            .iter()
            .find(|binding| binding.callee == *method)
        else {
            return Ok(None);
        };
        let operation = match binding.operation {
            JimpleIndexedStoreQueryKind::Extent => TransactionStoreQueryKind::Extent,
            JimpleIndexedStoreQueryKind::Keys => TransactionStoreQueryKind::Keys,
            JimpleIndexedStoreQueryKind::Empty => TransactionStoreQueryKind::Empty,
            JimpleIndexedStoreQueryKind::Full => TransactionStoreQueryKind::Full,
            JimpleIndexedStoreQueryKind::AllowsRead => TransactionStoreQueryKind::AllowsRead,
            JimpleIndexedStoreQueryKind::AllowsWrite => TransactionStoreQueryKind::AllowsWrite,
        };
        let value = binding
            .value
            .as_ref()
            .map(|selector| {
                select_operand(receiver, arguments, selector, "query value").map(Box::new)
            })
            .transpose()?;
        Ok(Some(LoweredInvoke::Value(TransactionValue::StoreQuery {
            operation,
            store: Box::new(select_operand(
                receiver,
                arguments,
                &binding.store,
                "store",
            )?),
            key: select_operands(receiver, arguments, &binding.key, "key")?,
            value,
            context: select_operands(receiver, arguments, &binding.context, "query context")?,
        })))
    }

    fn lower_schedule_once_invoke(
        &self,
        method: &JimpleMethodSignature,
        arguments: &[TransactionValue],
    ) -> Result<Option<LoweredInvoke>> {
        let Some(binding) = self
            .bindings
            .schedule_once
            .iter()
            .find(|binding| binding.callee == *method)
        else {
            return Ok(None);
        };
        let key = binding
            .key_arguments
            .iter()
            .map(|index| {
                arguments
                    .get(*index)
                    .cloned()
                    .with_context(|| format!("schedule key argument {index} is missing"))
            })
            .collect::<Result<Vec<_>>>()?;
        let delay = arguments
            .get(binding.delay_argument)
            .cloned()
            .with_context(|| {
                format!(
                    "schedule delay argument {} is missing",
                    binding.delay_argument
                )
            })?;
        let priority = binding
            .priority_argument
            .map(|index| {
                arguments
                    .get(index)
                    .cloned()
                    .with_context(|| format!("schedule priority argument {index} is missing"))
            })
            .transpose()?
            .unwrap_or(TransactionValue::Integer { value: 0 });
        Ok(Some(LoweredInvoke::Effect(
            TransactionEffect::ScheduleOnce {
                key,
                delay,
                priority,
            },
        )))
    }

    fn lower_state_write_invoke(
        &self,
        method: &JimpleMethodSignature,
        receiver: Option<&TransactionValue>,
        arguments: &[TransactionValue],
    ) -> Result<Option<LoweredInvoke>> {
        let Some(binding) = self
            .bindings
            .state_writes
            .iter()
            .find(|binding| binding.callee == *method)
        else {
            return Ok(None);
        };
        Ok(Some(LoweredInvoke::Effect(TransactionEffect::StateWrite {
            target: select_operand(receiver, arguments, &binding.target, "state target")?,
            state: binding.state.clone(),
            value: select_operand(receiver, arguments, &binding.value, "state value")?,
        })))
    }

    fn lower_raw_host_call_invoke(
        &mut self,
        method: &JimpleMethodSignature,
        receiver: Option<&JimpleValue>,
        arguments: &[JimpleValue],
    ) -> Result<Option<LoweredInvoke>> {
        let Some(binding) = self
            .bindings
            .host_calls
            .iter()
            .find(|binding| binding.callee == *method)
            .cloned()
        else {
            return Ok(None);
        };
        let call_receiver = binding
            .receiver
            .as_ref()
            .map(|selector| {
                self.lower_raw_operand(receiver, arguments, selector, "host call receiver")
            })
            .transpose()?
            .map(Box::new);
        let call_arguments = binding
            .arguments
            .iter()
            .map(|selector| {
                self.lower_raw_operand(receiver, arguments, selector, "host call argument")
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Some(LoweredInvoke::Value(TransactionValue::HostCall {
            operation: binding.operation,
            receiver: call_receiver,
            arguments: call_arguments,
        })))
    }

    fn lower_raw_host_query_invoke(
        &mut self,
        method: &JimpleMethodSignature,
        receiver: Option<&JimpleValue>,
        arguments: &[JimpleValue],
    ) -> Result<Option<LoweredInvoke>> {
        let Some(binding) = self
            .bindings
            .host_queries
            .iter()
            .find(|binding| binding.callee == *method)
            .cloned()
        else {
            return Ok(None);
        };
        let query_receiver = binding
            .receiver
            .as_ref()
            .map(|selector| {
                self.lower_raw_operand(receiver, arguments, selector, "host query receiver")
            })
            .transpose()?
            .map(Box::new);
        let query_arguments = binding
            .arguments
            .iter()
            .map(|selector| {
                self.lower_raw_operand(receiver, arguments, selector, "host query argument")
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Some(LoweredInvoke::Value(TransactionValue::HostQuery {
            operation: binding.operation,
            receiver: query_receiver,
            arguments: query_arguments,
        })))
    }

    fn lower_raw_operand(
        &mut self,
        receiver: Option<&JimpleValue>,
        arguments: &[JimpleValue],
        selector: &JimpleCallOperandBinding,
        role: &str,
    ) -> Result<TransactionValue> {
        let value = match selector {
            JimpleCallOperandBinding::Receiver => {
                receiver.with_context(|| format!("transaction {role} receiver is missing"))?
            }
            JimpleCallOperandBinding::Argument { index } => arguments
                .get(*index)
                .with_context(|| format!("transaction {role} argument {index} is missing"))?,
        };
        self.lower_value(value)
    }

    fn lower_internal_invoke(
        &self,
        invoke_kind: &str,
        method: &JimpleMethodSignature,
        base_is_this: bool,
        receiver: Option<TransactionValue>,
        arguments: Vec<TransactionValue>,
    ) -> Result<LoweredInvoke> {
        let target = match invoke_kind {
            "JStaticInvokeExpr" => self
                .program
                .method(method)
                .or_else(|| {
                    self.program
                        .dispatched_method_in_hierarchy(&method.owner, method)
                })
                .with_context(|| format!("unbound external transaction call {method:?}"))?,
            "JSpecialInvokeExpr" => self
                .program
                .method(method)
                .with_context(|| format!("unbound external transaction call {method:?}"))?,
            "JVirtualInvokeExpr" | "JInterfaceInvokeExpr" if base_is_this => {
                let runtime_this = self.runtime_this.with_context(|| {
                    format!("dynamic transaction call {method:?} requires exact runtime `this`")
                })?;
                self.program
                    .dispatched_method_in_hierarchy(runtime_this, method)
                    .with_context(|| {
                        format!(
                            "runtime dispatch from {runtime_this:?} cannot resolve transaction call {method:?}"
                        )
                    })?
            }
            _ => bail!("unbound dynamic transaction call {method:?}"),
        };
        let function = function_id(self.function_ids, &target.signature)
            .with_context(|| format!("transaction helper ID missing for {:?}", target.signature))?;
        if target.signature.return_type == "void" {
            Ok(LoweredInvoke::Effect(TransactionEffect::Call {
                function,
                receiver,
                arguments,
            }))
        } else {
            Ok(LoweredInvoke::Value(TransactionValue::Call {
                function,
                receiver: receiver.map(Box::new),
                arguments,
            }))
        }
    }

    fn local_id(&mut self, name: &str) -> Result<TransactionLocalId> {
        if let Some(index) = self.locals.iter().position(|candidate| candidate == name) {
            return Ok(TransactionLocalId(
                u32::try_from(index).context("transaction local index exceeds u32")?,
            ));
        }
        let index =
            u32::try_from(self.locals.len()).context("transaction local count exceeds u32")?;
        self.locals.push(name.to_owned());
        Ok(TransactionLocalId(index))
    }
}

fn lower_abstract_constant(
    value: &AbstractValue,
    execution: &ProgramExecution,
) -> Result<TransactionValue> {
    match value {
        AbstractValue::Integer { value } => Ok(TransactionValue::Integer { value: *value }),
        AbstractValue::Float { value } => Ok(TransactionValue::Float { value: *value }),
        AbstractValue::String { value } => Ok(TransactionValue::String {
            value: value.clone(),
        }),
        AbstractValue::Null => Ok(TransactionValue::Null),
        AbstractValue::Sequence { values, .. } => Ok(TransactionValue::Sequence {
            values: values
                .iter()
                .map(|value| lower_abstract_constant(value, execution))
                .collect::<Result<Vec<_>>>()?,
        }),
        AbstractValue::ArrayRef { id, element_type } => {
            let array = execution
                .arrays
                .iter()
                .find(|array| array.identity == AbstractArrayIdentity::Allocated { id: *id })
                .with_context(|| format!("startup array {id} is missing"))?;
            let length = array
                .length
                .with_context(|| format!("startup array {id} has unknown length"))?;
            let mut values = (0..length)
                .map(|_| default_array_element(element_type))
                .collect::<Result<Vec<_>>>()?;
            for entry in &array.entries {
                let index = usize::try_from(entry.index).with_context(|| {
                    format!("startup array {id} has negative index {}", entry.index)
                })?;
                let slot = values.get_mut(index).with_context(|| {
                    format!("startup array {id} index {index} is out of bounds")
                })?;
                *slot = lower_abstract_constant(&entry.value, execution)?;
            }
            Ok(TransactionValue::Sequence { values })
        }
        other => bail!("unsupported startup constant {other:?}"),
    }
}

fn default_array_element(element_type: &str) -> Result<TransactionValue> {
    match element_type {
        "boolean" | "byte" | "char" | "short" | "int" | "long" => {
            Ok(TransactionValue::Integer { value: 0 })
        }
        "float" | "double" => Ok(TransactionValue::Float { value: 0.0 }),
        value if value.ends_with("[]") || value.contains('.') => Ok(TransactionValue::Null),
        _ => bail!("unsupported startup array element type {element_type:?}"),
    }
}

enum LoweredInvoke {
    Value(TransactionValue),
    Effect(TransactionEffect),
}

fn unique_component(
    program: &JimpleProgram,
    field: &super::JimpleFieldSignature,
    bindings: &JimpleTransactionBindings,
) -> Result<String> {
    let mut exact = bindings
        .value_components
        .iter()
        .filter(|binding| binding.field == *field);
    if let Some(binding) = exact.next() {
        if exact.next().is_some() {
            bail!("ambiguous transaction component binding for {field:?}");
        }
        return Ok(binding.component.clone());
    }
    let mut inherited = bindings.value_components.iter().filter(|binding| {
        binding.field.name == field.name
            && binding.field.type_name == field.type_name
            && program.is_known_subtype(&field.owner, &binding.field.owner)
    });
    let binding = inherited
        .next()
        .with_context(|| format!("unbound transaction field {field:?}"))?;
    if inherited.next().is_some() {
        bail!("ambiguous inherited transaction component binding for {field:?}");
    }
    Ok(binding.component.clone())
}

fn unique_capability(type_name: &str, bindings: &JimpleTransactionBindings) -> Result<String> {
    let mut matching = bindings
        .capabilities
        .iter()
        .filter(|binding| binding.source_type == type_name);
    let binding = matching
        .next()
        .with_context(|| format!("unbound transaction capability source type {type_name:?}"))?;
    if matching.next().is_some() {
        bail!("ambiguous transaction capability binding for {type_name:?}");
    }
    Ok(binding.capability.clone())
}

fn select_operands(
    receiver: Option<&TransactionValue>,
    arguments: &[TransactionValue],
    selectors: &[JimpleCallOperandBinding],
    role: &str,
) -> Result<Vec<TransactionValue>> {
    selectors
        .iter()
        .map(|selector| select_operand(receiver, arguments, selector, role))
        .collect()
}

fn select_operand(
    receiver: Option<&TransactionValue>,
    arguments: &[TransactionValue],
    selector: &JimpleCallOperandBinding,
    role: &str,
) -> Result<TransactionValue> {
    match selector {
        JimpleCallOperandBinding::Receiver => receiver
            .cloned()
            .with_context(|| format!("transaction {role} receiver is missing")),
        JimpleCallOperandBinding::Argument { index } => arguments
            .get(*index)
            .cloned()
            .with_context(|| format!("transaction {role} argument {index} is missing")),
    }
}

fn function_id(
    ids: &[(JimpleMethodSignature, TransactionFunctionId)],
    signature: &JimpleMethodSignature,
) -> Option<TransactionFunctionId> {
    ids.iter()
        .find_map(|(candidate, id)| (candidate == signature).then_some(*id))
}

fn single_successor(statement: &JimpleStatement) -> Result<TransactionStatementId> {
    let [successor] = statement.successors.as_slice() else {
        bail!(
            "transaction statement {} expected one successor, found {}",
            statement.id,
            statement.successors.len()
        );
    };
    statement_id(*successor)
}

fn unique_static_constant<'a>(
    field: &JimpleFieldSignature,
    bindings: &'a [JimpleStaticConstantBinding],
) -> Result<Option<&'a JimpleStaticConstantBinding>> {
    let mut matches = bindings.iter().filter(|binding| binding.field == *field);
    let first = matches.next();
    if matches.next().is_some() {
        bail!("ambiguous transaction static constant binding for {field:?}");
    }
    Ok(first)
}

fn lower_static_constant(value: &JimpleStaticConstantValue) -> Result<TransactionValue> {
    Ok(match value {
        JimpleStaticConstantValue::Integer { value } => TransactionValue::Integer { value: *value },
        JimpleStaticConstantValue::Boolean { value } => TransactionValue::Integer {
            value: i64::from(*value),
        },
        JimpleStaticConstantValue::String { value } => TransactionValue::String {
            value: value.clone(),
        },
        JimpleStaticConstantValue::Null => TransactionValue::Null,
        JimpleStaticConstantValue::Sequence { values } => TransactionValue::Sequence {
            values: values
                .iter()
                .map(lower_static_constant)
                .collect::<Result<Vec<_>>>()?,
        },
    })
}

fn statement_id(value: usize) -> Result<TransactionStatementId> {
    Ok(TransactionStatementId(
        u32::try_from(value).context("transaction statement ID exceeds u32")?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jimple::{
        JimpleCapabilityBinding, JimpleHostCallBinding, JimpleHostQueryBinding, JimpleMethod,
        JimpleScheduleOnceBinding, JimpleStatement,
    };

    fn value(type_name: &str, text: &str, kind: JimpleValueKind) -> JimpleValue {
        JimpleValue {
            jimple_kind: String::from("fixture"),
            type_name: type_name.to_owned(),
            text: text.to_owned(),
            kind,
        }
    }

    fn local(name: &str, type_name: &str) -> JimpleValue {
        value(
            type_name,
            name,
            JimpleValueKind::Local {
                name: name.to_owned(),
            },
        )
    }

    fn integer(value_: i64) -> JimpleValue {
        value(
            "int",
            &value_.to_string(),
            JimpleValueKind::IntegerConstant { value: value_ },
        )
    }

    fn statement(id: usize, successors: Vec<usize>, operation: JimpleOperation) -> JimpleStatement {
        JimpleStatement {
            id,
            kind: String::from("fixture"),
            text: format!("statement {id}"),
            def: None,
            uses: Vec::new(),
            successors,
            operation,
        }
    }

    fn helper_signature() -> JimpleMethodSignature {
        JimpleMethodSignature {
            owner: String::from("demo.SourceHelper"),
            name: String::from("sourceHelper"),
            return_type: String::from("int"),
            parameter_types: Vec::new(),
        }
    }

    fn fixture_program() -> (JimpleProgram, JimpleMethodSignature) {
        let entry = JimpleMethodSignature {
            owner: String::from("demo.SourceEntry"),
            name: String::from("run"),
            return_type: String::from("int"),
            parameter_types: vec![String::from("demo.Store")],
        };
        let store = local("store", "demo.Store");
        let supported = local("supported", "boolean");
        let result = local("result", "int");
        let helper = helper_signature();
        let methods = vec![
            JimpleMethod {
                signature: entry.clone(),
                display_signature: String::from("source entry"),
                declaring_superclass: None,
                declaring_interfaces: Vec::new(),
                statements: vec![
                    statement(
                        0,
                        vec![1],
                        JimpleOperation::Identity {
                            left: store.clone(),
                            right: value(
                                "demo.Store",
                                "@parameter0",
                                JimpleValueKind::Parameter { index: 0 },
                            ),
                        },
                    ),
                    statement(
                        1,
                        vec![2],
                        JimpleOperation::Assign {
                            left: supported.clone(),
                            right: value(
                                "boolean",
                                "store instanceof demo.SidedStore",
                                JimpleValueKind::InstanceOf {
                                    value: Box::new(store),
                                    check_type: String::from("demo.SidedStore"),
                                },
                            ),
                        },
                    ),
                    statement(
                        2,
                        vec![3, 5],
                        JimpleOperation::If {
                            condition: supported,
                        },
                    ),
                    statement(
                        3,
                        vec![4],
                        JimpleOperation::Assign {
                            left: result.clone(),
                            right: value(
                                "int",
                                "source helper call",
                                JimpleValueKind::Invoke {
                                    invoke_kind: String::from("JStaticInvokeExpr"),
                                    method: helper.clone(),
                                    base: None,
                                    arguments: Vec::new(),
                                    bootstrap_method: None,
                                    bootstrap_arguments: Vec::new(),
                                },
                            ),
                        },
                    ),
                    statement(4, Vec::new(), JimpleOperation::Return { value: result }),
                    statement(5, Vec::new(), JimpleOperation::Return { value: integer(0) }),
                ],
            },
            JimpleMethod {
                signature: helper,
                display_signature: String::from("source helper"),
                declaring_superclass: None,
                declaring_interfaces: Vec::new(),
                statements: vec![statement(
                    0,
                    Vec::new(),
                    JimpleOperation::Return { value: integer(1) },
                )],
            },
        ];
        (JimpleProgram::new(methods), entry)
    }

    fn host_query_fixture() -> (JimpleProgram, JimpleMethodSignature, JimpleMethodSignature) {
        let query = JimpleMethodSignature {
            owner: String::from("demo.SourceWorld"),
            name: String::from("sourceOnlyLookup"),
            return_type: String::from("int"),
            parameter_types: vec![String::from("int"), String::from("demo.SourceSelector")],
        };
        let entry = JimpleMethodSignature {
            owner: String::from("demo.SourceEntry"),
            name: String::from("query"),
            return_type: String::from("int"),
            parameter_types: vec![String::from("demo.SourceWorld"), String::from("int")],
        };
        let world = local("world", "demo.SourceWorld");
        let selector = local("selector", "demo.SourceSelector");
        let result = local("result", "int");
        let method = JimpleMethod {
            signature: entry.clone(),
            display_signature: String::from("source query"),
            declaring_superclass: None,
            declaring_interfaces: Vec::new(),
            statements: vec![
                statement(
                    0,
                    vec![1],
                    JimpleOperation::Identity {
                        left: world.clone(),
                        right: value(
                            "demo.SourceWorld",
                            "@parameter0",
                            JimpleValueKind::Parameter { index: 0 },
                        ),
                    },
                ),
                statement(
                    1,
                    vec![2],
                    JimpleOperation::Assign {
                        left: selector.clone(),
                        right: value(
                            "demo.SourceSelector",
                            "source selector",
                            JimpleValueKind::StaticField {
                                field: super::super::JimpleFieldSignature {
                                    owner: String::from("demo.SourceSelector"),
                                    name: String::from("ALL"),
                                    type_name: String::from("demo.SourceSelector"),
                                },
                            },
                        ),
                    },
                ),
                statement(
                    2,
                    vec![3],
                    JimpleOperation::Assign {
                        left: result.clone(),
                        right: value(
                            "int",
                            "dynamic source query",
                            JimpleValueKind::Invoke {
                                invoke_kind: String::from("JVirtualInvokeExpr"),
                                method: query.clone(),
                                base: Some(Box::new(world)),
                                arguments: vec![
                                    value(
                                        "int",
                                        "@parameter1",
                                        JimpleValueKind::Parameter { index: 1 },
                                    ),
                                    selector,
                                ],
                                bootstrap_method: None,
                                bootstrap_arguments: Vec::new(),
                            },
                        ),
                    },
                ),
                statement(3, Vec::new(), JimpleOperation::Return { value: result }),
            ],
        };
        (JimpleProgram::new(vec![method]), entry, query)
    }

    #[test]
    fn switch_lowers_to_source_neutral_control_flow() -> Result<()> {
        let entry = JimpleMethodSignature {
            owner: String::from("demo.Switcher"),
            name: String::from("run"),
            return_type: String::from("int"),
            parameter_types: vec![String::from("int")],
        };
        let key = local("key", "int");
        let program = JimpleProgram::new(vec![JimpleMethod {
            signature: entry.clone(),
            display_signature: String::from("switch fixture"),
            declaring_superclass: None,
            declaring_interfaces: Vec::new(),
            statements: vec![
                statement(
                    0,
                    vec![1],
                    JimpleOperation::Identity {
                        left: key.clone(),
                        right: value(
                            "int",
                            "@parameter0",
                            JimpleValueKind::Parameter { index: 0 },
                        ),
                    },
                ),
                statement(
                    1,
                    vec![2, 3],
                    JimpleOperation::Switch {
                        key,
                        cases: vec![crate::jimple::JimpleSwitchCase {
                            value: 1,
                            target: 2,
                        }],
                        default_target: 3,
                    },
                ),
                statement(2, Vec::new(), JimpleOperation::Return { value: integer(7) }),
                statement(3, Vec::new(), JimpleOperation::Return { value: integer(9) }),
            ],
        }]);
        let lowered =
            program.lower_transaction_program(&entry, &JimpleTransactionBindings::default())?;
        assert!(matches!(
            lowered.functions[0].statements[1].operation,
            TransactionOperation::Switch { ref cases, default_target, .. }
                if cases == &[TransactionSwitchCase {
                    value: 1,
                    target: TransactionStatementId(2),
                }] && default_target == TransactionStatementId(3)
        ));
        Ok(())
    }

    #[test]
    fn schedule_binding_lowers_to_source_neutral_effect() -> Result<()> {
        let scheduler = JimpleMethodSignature {
            owner: String::from("demo.SourceWorld"),
            name: String::from("sourceSchedule"),
            return_type: String::from("void"),
            parameter_types: vec![
                String::from("int"),
                String::from("int"),
                String::from("int"),
                String::from("java.lang.String"),
                String::from("int"),
                String::from("int"),
            ],
        };
        let entry = JimpleMethodSignature {
            owner: String::from("demo.SourceEntry"),
            name: String::from("schedule"),
            return_type: String::from("void"),
            parameter_types: Vec::new(),
        };
        let schedule = value(
            "void",
            "source schedule",
            JimpleValueKind::Invoke {
                invoke_kind: String::from("JStaticInvokeExpr"),
                method: scheduler.clone(),
                base: None,
                arguments: vec![
                    integer(1),
                    integer(2),
                    integer(3),
                    value(
                        "java.lang.String",
                        "block",
                        JimpleValueKind::StringConstant {
                            value: String::from("block"),
                        },
                    ),
                    integer(4),
                    integer(-1),
                ],
                bootstrap_method: None,
                bootstrap_arguments: Vec::new(),
            },
        );
        let program = JimpleProgram::new(vec![JimpleMethod {
            signature: entry.clone(),
            display_signature: String::from("schedule fixture"),
            declaring_superclass: None,
            declaring_interfaces: Vec::new(),
            statements: vec![
                statement(
                    0,
                    vec![1],
                    JimpleOperation::Invoke {
                        expression: Some(schedule),
                    },
                ),
                statement(1, Vec::new(), JimpleOperation::ReturnVoid),
            ],
        }]);
        let lowered = program.lower_transaction_program(
            &entry,
            &JimpleTransactionBindings {
                schedule_once: vec![JimpleScheduleOnceBinding {
                    callee: scheduler,
                    key_arguments: vec![0, 1, 2, 3],
                    delay_argument: 4,
                    priority_argument: Some(5),
                }],
                ..JimpleTransactionBindings::default()
            },
        )?;
        let TransactionOperation::Effect {
            effect:
                TransactionEffect::ScheduleOnce {
                    key,
                    delay,
                    priority,
                },
            ..
        } = &lowered.functions[0].statements[0].operation
        else {
            bail!("schedule call did not lower to schedule_once effect");
        };
        assert_eq!(key.len(), 4);
        assert!(matches!(delay, TransactionValue::Integer { value: 4 }));
        assert!(matches!(priority, TransactionValue::Integer { value: -1 }));
        let serialized = serde_json::to_string(&lowered)?;
        assert!(!serialized.contains("SourceWorld"));
        assert!(!serialized.contains("sourceSchedule"));
        Ok(())
    }

    #[test]
    fn host_call_binding_lowers_to_transactional_host_call() -> Result<()> {
        let (program, entry, query) = host_query_fixture();
        let lowered = program.lower_transaction_program(
            &entry,
            &JimpleTransactionBindings {
                host_calls: vec![JimpleHostCallBinding {
                    callee: query,
                    operation: String::from("world.write"),
                    receiver: Some(JimpleCallOperandBinding::Receiver),
                    arguments: vec![JimpleCallOperandBinding::Argument { index: 0 }],
                }],
                ..JimpleTransactionBindings::default()
            },
        )?;
        let serialized = serde_json::to_string(&lowered)?;
        assert!(serialized.contains("host_call"));
        assert!(serialized.contains("world.write"));
        assert!(!serialized.contains("SourceWorld"));
        assert!(!serialized.contains("SourceSelector"));
        Ok(())
    }

    #[test]
    fn host_query_binding_erases_ignored_source_scaffolding() -> Result<()> {
        let (program, entry, query) = host_query_fixture();
        let lowered = program.lower_transaction_program(
            &entry,
            &JimpleTransactionBindings {
                host_queries: vec![JimpleHostQueryBinding {
                    callee: query,
                    operation: String::from("topology.lookup"),
                    receiver: Some(JimpleCallOperandBinding::Receiver),
                    arguments: vec![JimpleCallOperandBinding::Argument { index: 0 }],
                }],
                ..JimpleTransactionBindings::default()
            },
        )?;
        let serialized = serde_json::to_string(&lowered)?;
        assert!(serialized.contains("topology.lookup"));
        assert!(!serialized.contains("demo.SourceWorld"));
        assert!(!serialized.contains("demo.SourceSelector"));
        assert!(!serialized.contains("sourceOnlyLookup"));
        Ok(())
    }

    fn static_array_initializer(static_value: &JimpleValue) -> JimpleMethod {
        let array = local("array", "int[]");
        JimpleMethod {
            signature: JimpleMethodSignature {
                owner: String::from("demo.Constants"),
                name: String::from("<clinit>"),
                return_type: String::from("void"),
                parameter_types: Vec::new(),
            },
            display_signature: String::from("constants clinit"),
            declaring_superclass: None,
            declaring_interfaces: Vec::new(),
            statements: vec![
                statement(
                    0,
                    vec![1],
                    JimpleOperation::Assign {
                        left: array.clone(),
                        right: value(
                            "int[]",
                            "new int[3]",
                            JimpleValueKind::NewArray {
                                element_type: String::from("int"),
                                size: Box::new(integer(3)),
                            },
                        ),
                    },
                ),
                statement(
                    1,
                    vec![2],
                    JimpleOperation::Assign {
                        left: value(
                            "int",
                            "array[0]",
                            JimpleValueKind::ArrayRef {
                                base: Box::new(array.clone()),
                                index: Box::new(integer(0)),
                            },
                        ),
                        right: integer(4),
                    },
                ),
                statement(
                    2,
                    vec![3],
                    JimpleOperation::Assign {
                        left: value(
                            "int",
                            "array[2]",
                            JimpleValueKind::ArrayRef {
                                base: Box::new(array.clone()),
                                index: Box::new(integer(2)),
                            },
                        ),
                        right: integer(9),
                    },
                ),
                statement(
                    3,
                    vec![4],
                    JimpleOperation::Assign {
                        left: static_value.clone(),
                        right: array,
                    },
                ),
                statement(4, Vec::new(), JimpleOperation::ReturnVoid),
            ],
        }
    }

    fn static_array_reader(
        entry: &JimpleMethodSignature,
        static_value: JimpleValue,
    ) -> JimpleMethod {
        let result = local("result", "int");
        JimpleMethod {
            signature: entry.clone(),
            display_signature: String::from("reader"),
            declaring_superclass: None,
            declaring_interfaces: Vec::new(),
            statements: vec![
                statement(
                    0,
                    vec![1],
                    JimpleOperation::Assign {
                        left: local("values", "int[]"),
                        right: static_value,
                    },
                ),
                statement(
                    1,
                    vec![2],
                    JimpleOperation::Assign {
                        left: result.clone(),
                        right: value(
                            "int",
                            "values[2]",
                            JimpleValueKind::ArrayRef {
                                base: Box::new(local("values", "int[]")),
                                index: Box::new(integer(2)),
                            },
                        ),
                    },
                ),
                statement(2, Vec::new(), JimpleOperation::Return { value: result }),
            ],
        }
    }

    fn static_array_fixture() -> (JimpleProgram, JimpleMethodSignature) {
        let field = JimpleFieldSignature {
            owner: String::from("demo.Constants"),
            name: String::from("VALUES"),
            type_name: String::from("int[]"),
        };
        let static_value = value(
            "int[]",
            "demo.Constants.VALUES",
            JimpleValueKind::StaticField { field },
        );
        let entry = JimpleMethodSignature {
            owner: String::from("demo.Reader"),
            name: String::from("read"),
            return_type: String::from("int"),
            parameter_types: Vec::new(),
        };
        let program = JimpleProgram::new(vec![
            static_array_initializer(&static_value),
            static_array_reader(&entry, static_value),
        ]);
        (program, entry)
    }

    #[test]
    fn static_primitive_arrays_lower_from_recovered_class_initialization() -> Result<()> {
        let (program, entry) = static_array_fixture();
        let lowered =
            program.lower_transaction_program(&entry, &JimpleTransactionBindings::default())?;
        let TransactionOperation::Assign { value, .. } =
            &lowered.functions[0].statements[0].operation
        else {
            bail!("static array did not lower to an assignment");
        };
        assert_eq!(
            value,
            &TransactionValue::Sequence {
                values: vec![
                    TransactionValue::Integer { value: 4 },
                    TransactionValue::Integer { value: 0 },
                    TransactionValue::Integer { value: 9 },
                ]
            }
        );
        Ok(())
    }

    #[test]
    fn static_semantic_constant_binding_avoids_source_initializer() -> Result<()> {
        let field = JimpleFieldSignature {
            owner: String::from("demo.Registry"),
            name: String::from("ITEM"),
            type_name: String::from("demo.Item"),
        };
        let entry = JimpleMethodSignature {
            owner: String::from("demo.Reader"),
            name: String::from("read"),
            return_type: String::from("demo.Item"),
            parameter_types: Vec::new(),
        };
        let program = JimpleProgram::new(vec![JimpleMethod {
            signature: entry.clone(),
            display_signature: String::from("reader"),
            declaring_superclass: None,
            declaring_interfaces: Vec::new(),
            statements: vec![statement(
                0,
                Vec::new(),
                JimpleOperation::Return {
                    value: value(
                        "demo.Item",
                        "demo.Registry.ITEM",
                        JimpleValueKind::StaticField {
                            field: field.clone(),
                        },
                    ),
                },
            )],
        }]);
        let lowered = program.lower_transaction_program(
            &entry,
            &JimpleTransactionBindings {
                static_constants: vec![JimpleStaticConstantBinding {
                    field,
                    value: JimpleStaticConstantValue::String {
                        value: String::from("semantic:item"),
                    },
                }],
                ..JimpleTransactionBindings::default()
            },
        )?;
        let TransactionOperation::Return { value: Some(value) } =
            &lowered.functions[0].statements[0].operation
        else {
            bail!("static semantic constant did not lower to return value");
        };
        assert_eq!(
            value,
            &TransactionValue::String {
                value: String::from("semantic:item")
            }
        );
        Ok(())
    }

    #[test]
    fn inherited_static_call_resolves_through_declared_owner_hierarchy() -> Result<()> {
        let entry = JimpleMethodSignature {
            owner: String::from("demo.Child"),
            name: String::from("run"),
            return_type: String::from("int"),
            parameter_types: Vec::new(),
        };
        let declared = JimpleMethodSignature {
            owner: String::from("demo.Child"),
            name: String::from("staticValue"),
            return_type: String::from("int"),
            parameter_types: Vec::new(),
        };
        let implementation = JimpleMethodSignature {
            owner: String::from("demo.Base"),
            name: String::from("staticValue"),
            return_type: String::from("int"),
            parameter_types: Vec::new(),
        };
        let result = local("result", "int");
        let program = JimpleProgram::new(vec![
            JimpleMethod {
                signature: entry.clone(),
                display_signature: String::from("child run"),
                declaring_superclass: Some(String::from("demo.Base")),
                declaring_interfaces: Vec::new(),
                statements: vec![
                    statement(
                        0,
                        vec![1],
                        JimpleOperation::Assign {
                            left: result.clone(),
                            right: value(
                                "int",
                                "static child value",
                                JimpleValueKind::Invoke {
                                    invoke_kind: String::from("JStaticInvokeExpr"),
                                    method: declared,
                                    base: None,
                                    arguments: Vec::new(),
                                    bootstrap_method: None,
                                    bootstrap_arguments: Vec::new(),
                                },
                            ),
                        },
                    ),
                    statement(1, Vec::new(), JimpleOperation::Return { value: result }),
                ],
            },
            JimpleMethod {
                signature: implementation,
                display_signature: String::from("base static value"),
                declaring_superclass: None,
                declaring_interfaces: Vec::new(),
                statements: vec![statement(
                    0,
                    Vec::new(),
                    JimpleOperation::Return { value: integer(7) },
                )],
            },
        ]);
        let lowered =
            program.lower_transaction_program(&entry, &JimpleTransactionBindings::default())?;
        assert_eq!(lowered.functions.len(), 2);
        assert!(matches!(
            lowered.functions[1].statements[0].operation,
            TransactionOperation::Return {
                value: Some(TransactionValue::Integer { value: 7 })
            }
        ));
        Ok(())
    }

    #[test]
    fn runtime_this_dispatches_through_identity_alias_and_superclass() -> Result<()> {
        let entry = JimpleMethodSignature {
            owner: String::from("demo.Child"),
            name: String::from("run"),
            return_type: String::from("int"),
            parameter_types: Vec::new(),
        };
        let declared = JimpleMethodSignature {
            owner: String::from("demo.Child"),
            name: String::from("value"),
            return_type: String::from("int"),
            parameter_types: Vec::new(),
        };
        let implementation = JimpleMethodSignature {
            owner: String::from("demo.Base"),
            name: String::from("value"),
            return_type: String::from("int"),
            parameter_types: Vec::new(),
        };
        let this_local = local("self", "demo.Child");
        let result = local("result", "int");
        let program = JimpleProgram::new(vec![
            JimpleMethod {
                signature: entry.clone(),
                display_signature: String::from("child run"),
                declaring_superclass: Some(String::from("demo.Base")),
                declaring_interfaces: Vec::new(),
                statements: vec![
                    statement(
                        0,
                        vec![1],
                        JimpleOperation::Identity {
                            left: this_local.clone(),
                            right: value("demo.Child", "@this", JimpleValueKind::This),
                        },
                    ),
                    statement(
                        1,
                        vec![2],
                        JimpleOperation::Assign {
                            left: result.clone(),
                            right: value(
                                "int",
                                "virtual value",
                                JimpleValueKind::Invoke {
                                    invoke_kind: String::from("JVirtualInvokeExpr"),
                                    method: declared,
                                    base: Some(Box::new(this_local)),
                                    arguments: Vec::new(),
                                    bootstrap_method: None,
                                    bootstrap_arguments: Vec::new(),
                                },
                            ),
                        },
                    ),
                    statement(2, Vec::new(), JimpleOperation::Return { value: result }),
                ],
            },
            JimpleMethod {
                signature: implementation,
                display_signature: String::from("base value"),
                declaring_superclass: None,
                declaring_interfaces: Vec::new(),
                statements: vec![statement(
                    0,
                    Vec::new(),
                    JimpleOperation::Return { value: integer(7) },
                )],
            },
        ]);
        assert!(
            program
                .lower_transaction_program(&entry, &JimpleTransactionBindings::default())
                .is_err()
        );
        let lowered = program.lower_transaction_program_for_runtime_this(
            &entry,
            "demo.Child",
            &JimpleTransactionBindings::default(),
        )?;
        assert_eq!(lowered.functions.len(), 2);
        assert_eq!(
            lowered.functions[1].statements[0].operation,
            TransactionOperation::Return {
                value: Some(TransactionValue::Integer { value: 7 })
            }
        );
        Ok(())
    }

    #[test]
    fn lowering_erases_source_types_and_helper_names() -> Result<()> {
        let (program, entry) = fixture_program();
        let lowered = program.lower_transaction_program(
            &entry,
            &JimpleTransactionBindings {
                capabilities: vec![JimpleCapabilityBinding {
                    source_type: String::from("demo.SidedStore"),
                    capability: String::from("sided_access"),
                }],
                ..JimpleTransactionBindings::default()
            },
        )?;
        assert_eq!(lowered.functions.len(), 2);
        let serialized = serde_json::to_string(&lowered)?;
        assert!(serialized.contains("sided_access"));
        assert!(!serialized.contains("demo.SourceEntry"));
        assert!(!serialized.contains("demo.SourceHelper"));
        assert!(!serialized.contains("demo.SidedStore"));
        assert!(!serialized.contains("sourceHelper"));
        Ok(())
    }
}
