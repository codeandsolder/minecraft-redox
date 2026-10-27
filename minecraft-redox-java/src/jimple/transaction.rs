//! Lower structured Jimple into the source-neutral transaction CFG.

use anyhow::{Context, Result, bail};
use minecraft_redox_core::transaction::{
    TransactionEffect, TransactionFunction, TransactionFunctionId, TransactionLocalId,
    TransactionOperation, TransactionProgram, TransactionStatement, TransactionStatementId,
    TransactionStoreQueryKind, TransactionValue,
};

use super::{
    JimpleCallOperandBinding, JimpleIndexedStoreOperationKind, JimpleIndexedStoreQueryKind,
    JimpleMethod, JimpleMethodSignature, JimpleOperation, JimpleProgram, JimplePureValueOperation,
    JimpleStatement, JimpleTransactionBindings, JimpleValue, JimpleValueKind,
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
        let signatures = discover_transaction_methods(self, entry, bindings)?;
        let function_ids: Vec<_> = signatures
            .iter()
            .enumerate()
            .map(|(index, signature)| {
                let id = u32::try_from(index).context("transaction function count exceeds u32")?;
                Ok((signature.clone(), TransactionFunctionId(id)))
            })
            .collect::<Result<_>>()?;
        let functions = signatures
            .iter()
            .map(|signature| {
                let method = self
                    .method(signature)
                    .with_context(|| format!("Jimple method {signature:?} does not exist"))?;
                lower_transaction_function(self, method, bindings, &function_ids)
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(TransactionProgram {
            entry: TransactionFunctionId(0),
            functions,
        })
    }
}

fn discover_transaction_methods(
    program: &JimpleProgram,
    entry: &JimpleMethodSignature,
    bindings: &JimpleTransactionBindings,
) -> Result<Vec<JimpleMethodSignature>> {
    program
        .method(entry)
        .with_context(|| format!("Jimple method {entry:?} does not exist"))?;
    let mut discovered = vec![entry.clone()];
    let mut index = 0;
    while index < discovered.len() {
        let signature = discovered[index].clone();
        let method = program
            .method(&signature)
            .with_context(|| format!("Jimple method {signature:?} disappeared"))?;
        let mut calls = Vec::new();
        for statement in &method.statements {
            collect_statement_calls(statement, &mut calls);
        }
        for (invoke_kind, callee) in calls {
            if semantic_call_binding_count(callee, bindings) > 0 {
                continue;
            }
            if !matches!(invoke_kind, "JStaticInvokeExpr" | "JSpecialInvokeExpr") {
                bail!(
                    "unbound dynamic transaction call {callee:?} in {}",
                    method.display_signature
                );
            }
            if program.method(callee).is_none() {
                bail!(
                    "unbound external transaction call {callee:?} in {}",
                    method.display_signature
                );
            }
            if !discovered.contains(callee) {
                discovered.push(callee.clone());
            }
        }
        index += 1;
    }
    Ok(discovered)
}

fn collect_statement_calls<'a>(
    statement: &'a JimpleStatement,
    calls: &mut Vec<(&'a str, &'a JimpleMethodSignature)>,
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

fn collect_value_calls<'a>(
    value: &'a JimpleValue,
    calls: &mut Vec<(&'a str, &'a JimpleMethodSignature)>,
) {
    match &value.kind {
        JimpleValueKind::Invoke {
            invoke_kind,
            method,
            base,
            arguments,
            bootstrap_arguments,
            ..
        } => {
            calls.push((invoke_kind, method));
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
}

struct FunctionLowerer<'a> {
    program: &'a JimpleProgram,
    bindings: &'a JimpleTransactionBindings,
    function_ids: &'a [(JimpleMethodSignature, TransactionFunctionId)],
    locals: Vec<String>,
}

fn lower_transaction_function(
    program: &JimpleProgram,
    method: &JimpleMethod,
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
        locals: Vec::new(),
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

impl FunctionLowerer<'_> {
    fn lower_statement(&mut self, statement: &JimpleStatement) -> Result<TransactionStatement> {
        let id = statement_id(statement.id)?;
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
            JimpleOperation::Switch { .. } => {
                bail!(
                    "transaction lowering does not yet support switch at {}",
                    statement.id
                );
            }
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
                let component = unique_component(field, self.bindings)?;
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
            JimpleValueKind::ArrayRef { base, index } => Ok(TransactionValue::Index {
                sequence: Box::new(self.lower_value(base)?),
                index: Box::new(self.lower_value(index)?),
            }),
            JimpleValueKind::InstanceField { base, field } => Ok(TransactionValue::Component {
                value: Box::new(self.lower_value(base)?),
                component: unique_component(field, self.bindings)?,
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
            } => {
                unique_capability(cast_type, self.bindings)?;
                self.lower_value(inner)
            }
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
            | JimpleValueKind::StaticField { .. }
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
        let receiver = base
            .as_deref()
            .map(|base| self.lower_value(base))
            .transpose()?;
        let arguments = arguments
            .iter()
            .map(|argument| self.lower_value(argument))
            .collect::<Result<Vec<_>>>()?;

        if semantic_call_binding_count(method, self.bindings) > 1 {
            bail!("ambiguous transaction semantic bindings for {method:?}");
        }
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
        self.lower_internal_invoke(invoke_kind, method, receiver, arguments)
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

    fn lower_internal_invoke(
        &self,
        invoke_kind: &str,
        method: &JimpleMethodSignature,
        receiver: Option<TransactionValue>,
        arguments: Vec<TransactionValue>,
    ) -> Result<LoweredInvoke> {
        if !matches!(invoke_kind, "JStaticInvokeExpr" | "JSpecialInvokeExpr") {
            bail!("unbound dynamic transaction call {method:?}");
        }
        self.program
            .method(method)
            .with_context(|| format!("unbound external transaction call {method:?}"))?;
        let function = function_id(self.function_ids, method)
            .with_context(|| format!("transaction helper ID missing for {method:?}"))?;
        if method.return_type == "void" {
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

enum LoweredInvoke {
    Value(TransactionValue),
    Effect(TransactionEffect),
}

fn unique_component(
    field: &super::JimpleFieldSignature,
    bindings: &JimpleTransactionBindings,
) -> Result<String> {
    let mut matching = bindings
        .value_components
        .iter()
        .filter(|binding| binding.field == *field);
    let binding = matching
        .next()
        .with_context(|| format!("unbound transaction field {field:?}"))?;
    if matching.next().is_some() {
        bail!("ambiguous transaction component binding for {field:?}");
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

fn statement_id(value: usize) -> Result<TransactionStatementId> {
    Ok(TransactionStatementId(
        u32::try_from(value).context("transaction statement ID exceeds u32")?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jimple::{JimpleCapabilityBinding, JimpleMethod, JimpleStatement};

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
