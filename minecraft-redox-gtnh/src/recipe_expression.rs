use minecraft_redox_java::{
    JavaBindings, JavaExpression, JavaProjectIndex, JavaValueKind, substitute_references,
};
use serde::Serialize;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ItemSemantic {
    OreDict {
        prefix: String,
        material: JavaExpression,
        amount: JavaExpression,
    },
    OreDictQuery {
        key: JavaExpression,
        amount: JavaExpression,
    },
    OreDictUnify {
        item: JavaExpression,
        inner: Option<Box<ItemSemantic>>,
    },
    OreDictHelper {
        method: String,
        arguments: Vec<JavaExpression>,
    },
    ItemList {
        item: String,
        accessor: String,
        arguments: Vec<JavaExpression>,
    },
    MaterialItem {
        material: String,
        accessor: String,
        arguments: Vec<JavaExpression>,
    },
    ModItem {
        provider: String,
        arguments: Vec<JavaExpression>,
    },
    CopyAmount {
        amount: JavaExpression,
        meta: Option<JavaExpression>,
        item: JavaExpression,
        inner: Option<Box<ItemSemantic>>,
        unsafe_copy: bool,
    },
    ItemStack {
        item: JavaExpression,
        amount: Option<JavaExpression>,
        meta: Option<JavaExpression>,
    },
    OreDictStack {
        arguments: Vec<JavaExpression>,
    },
    OreDictAlternativeInput {
        key: JavaExpression,
        amount: JavaExpression,
    },
    ExplicitAlternatives {
        alternatives: Vec<JavaExpression>,
    },
    ModuleConditionalItem {
        module: String,
        when_present: Option<Box<ItemSemantic>>,
        when_absent: Option<Box<ItemSemantic>>,
    },
    LinkedMethod {
        method_id: String,
        return_type: String,
        receiver: Option<JavaExpression>,
        receiver_type: Option<String>,
        arguments: Vec<JavaExpression>,
    },
    LinkedValue {
        value_id: String,
        value_kind: JavaValueKind,
        name: String,
        declared_type: String,
    },
    TypedExpression {
        expression_type: String,
    },
    InlinedMethod {
        method_id: String,
        result: Box<ItemSemantic>,
    },
    InlinedValue {
        value_id: String,
        result: Box<ItemSemantic>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FluidSemantic {
    FluidAccessor {
        source_ref: String,
        accessor: String,
        amount: JavaExpression,
    },
    FluidStack {
        fluid: JavaExpression,
        amount: JavaExpression,
    },
    RegistryFluid {
        provider: String,
        arguments: Vec<JavaExpression>,
    },
    SolderingSubstitute {
        base_amount: Option<JavaExpression>,
    },
    LinkedMethod {
        method_id: String,
        return_type: String,
        receiver: Option<JavaExpression>,
        receiver_type: Option<String>,
        arguments: Vec<JavaExpression>,
    },
    InlinedValue {
        value_id: String,
        result: Box<FluidSemantic>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RecoveredExpression<T> {
    pub source: String,
    pub expression: JavaExpression,
    pub semantic: Option<T>,
}

#[must_use]
pub fn recover_items(
    sources: &[String],
    expressions: &[JavaExpression],
    bindings: &JavaBindings,
) -> Vec<RecoveredExpression<ItemSemantic>> {
    recover_values(sources, expressions, |expression| {
        item_semantic(expression, bindings)
    })
}

#[must_use]
pub fn recover_item_inputs(
    sources: &[String],
    expressions: &[JavaExpression],
    bindings: &JavaBindings,
) -> Vec<RecoveredExpression<ItemSemantic>> {
    let nested_array_is_slot = expressions.len() > 1;
    recover_values(sources, expressions, |expression| {
        recipe_input_semantic(expression, bindings, nested_array_is_slot)
            .or_else(|| item_semantic(expression, bindings))
    })
}

#[must_use]
pub fn recover_item_inputs_with_project(
    sources: &[String],
    expressions: &[JavaExpression],
    bindings: &JavaBindings,
    project: &JavaProjectIndex,
    source_path: &str,
    byte_position: usize,
) -> Vec<RecoveredExpression<ItemSemantic>> {
    let nested_array_is_slot = expressions.len() > 1;
    recover_values(sources, expressions, |expression| {
        recipe_input_semantic(expression, bindings, nested_array_is_slot)
            .or_else(|| item_semantic(expression, bindings))
            .or_else(|| {
                project_input_item_semantic(
                    expression,
                    bindings,
                    project,
                    source_path,
                    byte_position,
                    nested_array_is_slot,
                )
            })
            .map(|semantic| {
                enrich_item_semantic_with_project(
                    semantic,
                    bindings,
                    project,
                    source_path,
                    byte_position,
                )
            })
    })
}

#[must_use]
pub fn recover_items_with_project(
    sources: &[String],
    expressions: &[JavaExpression],
    bindings: &JavaBindings,
    project: &JavaProjectIndex,
    source_path: &str,
    byte_position: usize,
) -> Vec<RecoveredExpression<ItemSemantic>> {
    recover_values(sources, expressions, |expression| {
        item_semantic(expression, bindings)
            .or_else(|| {
                project_item_semantic(expression, bindings, project, source_path, byte_position)
            })
            .map(|semantic| {
                enrich_item_semantic_with_project(
                    semantic,
                    bindings,
                    project,
                    source_path,
                    byte_position,
                )
            })
    })
}

#[must_use]
pub fn recover_fluids(
    sources: &[String],
    expressions: &[JavaExpression],
    bindings: &JavaBindings,
) -> Vec<RecoveredExpression<FluidSemantic>> {
    recover_values(sources, expressions, |expression| {
        fluid_semantic(expression, bindings)
    })
}

#[must_use]
pub fn recover_fluids_with_project(
    sources: &[String],
    expressions: &[JavaExpression],
    bindings: &JavaBindings,
    project: &JavaProjectIndex,
    source_path: &str,
    byte_position: usize,
) -> Vec<RecoveredExpression<FluidSemantic>> {
    recover_values(sources, expressions, |expression| {
        fluid_semantic(expression, bindings)
            .map(|semantic| {
                enrich_fluid_semantic_with_project(
                    semantic,
                    expression,
                    bindings,
                    project,
                    source_path,
                    byte_position,
                )
            })
            .or_else(|| {
                project_fluid_semantic(expression, bindings, project, source_path, byte_position)
            })
    })
}

#[must_use]
pub fn duration_ticks(expression: &JavaExpression, bindings: &JavaBindings) -> Option<i64> {
    eval_i64(expression, bindings)
}

fn recover_values<T, F>(
    sources: &[String],
    expressions: &[JavaExpression],
    mut classify: F,
) -> Vec<RecoveredExpression<T>>
where
    F: FnMut(&JavaExpression) -> Option<T>,
{
    sources
        .iter()
        .zip(expressions)
        .map(|(source, expression)| RecoveredExpression {
            source: source.clone(),
            expression: expression.clone(),
            semantic: classify(expression),
        })
        .collect()
}

fn recipe_input_semantic(
    expression: &JavaExpression,
    bindings: &JavaBindings,
    nested_array_is_slot: bool,
) -> Option<ItemSemantic> {
    if !nested_array_is_slot {
        return None;
    }
    let JavaExpression::Array {
        element_type: Some(element_type),
        elements,
        ..
    } = expression
    else {
        return None;
    };
    let resolved_type = bindings.resolve_reference(element_type);
    let simple_type = resolved_type.rsplit('.').next().unwrap_or(&resolved_type);
    match simple_type {
        "Object" if elements.len() == 2 => Some(ItemSemantic::OreDictAlternativeInput {
            key: elements[0].clone(),
            amount: elements[1].clone(),
        }),
        "ItemStack" => Some(ItemSemantic::ExplicitAlternatives {
            alternatives: elements.clone(),
        }),
        _ => None,
    }
}

pub(crate) fn item_semantic(expression: &JavaExpression, bindings: &JavaBindings) -> Option<ItemSemantic> {
    match expression {
        JavaExpression::Call {
            receiver: Some(receiver),
            method,
            arguments,
        } => classify_item_call(receiver, method, arguments, bindings),
        JavaExpression::Call {
            receiver: None,
            method,
            arguments,
        } => classify_implicit_item_call(method, arguments, bindings),
        JavaExpression::New {
            type_name,
            arguments,
        } => classify_new_item(type_name, arguments, bindings),
        JavaExpression::Parenthesized(inner) => item_semantic(inner, bindings),
        _ => None,
    }
}

fn project_input_item_semantic(
    expression: &JavaExpression,
    bindings: &JavaBindings,
    project: &JavaProjectIndex,
    source_path: &str,
    byte_position: usize,
    nested_array_is_slot: bool,
) -> Option<ItemSemantic> {
    if nested_array_is_slot
        && let JavaExpression::Call {
            receiver,
            arguments,
            ..
        } = expression
        && let Some((resolved, receiver_type)) =
            project.resolve_call_at(source_path, byte_position, expression, bindings)
        && is_recipe_input_container_type(&resolved.return_type)
    {
        let method_id = resolved.stable_id();
        if receiver_type.is_none()
            && resolved.returns.len() == 1
            && let Some(replacements) = resolved.argument_bindings(arguments)
        {
            let substituted = substitute_references(&resolved.returns[0].expression, &replacements);
            if let Some(result) = recipe_input_semantic(&substituted, bindings, true)
                .or_else(|| item_semantic(&substituted, bindings))
            {
                return Some(ItemSemantic::InlinedMethod {
                    method_id,
                    result: Box::new(result),
                });
            }
        }
        return Some(ItemSemantic::LinkedMethod {
            method_id,
            return_type: resolved.return_type.clone(),
            receiver: receiver.as_deref().cloned(),
            receiver_type,
            arguments: arguments.clone(),
        });
    }
    project_item_semantic(expression, bindings, project, source_path, byte_position)
}

fn is_recipe_input_container_type(type_name: &str) -> bool {
    type_name.ends_with("Object[]") || type_name.ends_with("ItemStack[]")
}

fn project_item_semantic(
    expression: &JavaExpression,
    bindings: &JavaBindings,
    project: &JavaProjectIndex,
    source_path: &str,
    byte_position: usize,
) -> Option<ItemSemantic> {
    match expression {
        JavaExpression::Reference(reference) => {
            let value =
                project.resolve_value_at(source_path, byte_position, reference, bindings)?;
            if !is_item_value_type(&value.type_name) {
                return None;
            }
            if let Some(alternatives) =
                project.stable_enhanced_for_elements_at(source_path, byte_position, reference)
            {
                return Some(ItemSemantic::ExplicitAlternatives { alternatives });
            }
            if let Some(initializer) =
                project.stable_local_initializer_at(source_path, byte_position, reference)
            {
                if let Some(result) = module_conditional_item_semantic(
                    &initializer,
                    bindings,
                    project,
                    source_path,
                    byte_position,
                ) {
                    return Some(ItemSemantic::InlinedValue {
                        value_id: value.value_id,
                        result: Box::new(result),
                    });
                }
                if let Some(result) = item_semantic(&initializer, bindings) {
                    return Some(ItemSemantic::InlinedValue {
                        value_id: value.value_id,
                        result: Box::new(result),
                    });
                }
            }
            Some(ItemSemantic::LinkedValue {
                value_id: value.value_id,
                value_kind: value.kind,
                name: value.name,
                declared_type: value.type_name,
            })
        }
        JavaExpression::Call {
            receiver,
            arguments,
            ..
        } => {
            if let Some((resolved, receiver_type)) =
                project.resolve_call_at(source_path, byte_position, expression, bindings)
            {
                let qualified_receiver = receiver.as_deref().map(|receiver| {
                    qualify_project_item_reference(
                        receiver,
                        bindings,
                        project,
                        source_path,
                        byte_position,
                    )
                });
                let qualified_arguments = arguments
                    .iter()
                    .map(|argument| {
                        qualify_project_item_reference(
                            argument,
                            bindings,
                            project,
                            source_path,
                            byte_position,
                        )
                    })
                    .collect::<Vec<_>>();
                if let Some(semantic) = linked_method_semantic(
                    resolved,
                    qualified_receiver.as_ref(),
                    &qualified_arguments,
                    bindings,
                    receiver_type,
                ) {
                    return Some(semantic);
                }
            }
            typed_item_expression(expression, bindings, project, source_path, byte_position)
        }
        JavaExpression::Parenthesized(inner) => {
            project_item_semantic(inner, bindings, project, source_path, byte_position)
        }
        _ => typed_item_expression(expression, bindings, project, source_path, byte_position),
    }
}

fn module_conditional_item_semantic(
    expression: &JavaExpression,
    bindings: &JavaBindings,
    project: &JavaProjectIndex,
    source_path: &str,
    byte_position: usize,
) -> Option<ItemSemantic> {
    let JavaExpression::Conditional {
        condition,
        when_true,
        when_false,
    } = expression
    else {
        return None;
    };
    let module = content_module_presence_condition(condition, bindings)?;
    let present_elements = stable_item_array_elements(when_true, bindings, project)?;
    let absent_elements = stable_item_array_elements(when_false, bindings, project)?;
    if present_elements.len() > 1 || absent_elements.len() > 1 {
        return None;
    }
    let recover_branch = |elements: &[JavaExpression]| -> Option<Option<Box<ItemSemantic>>> {
        let Some(element) = elements.first() else {
            return Some(None);
        };
        let semantic = item_semantic(element, bindings)
            .or_else(|| project_item_semantic(element, bindings, project, source_path, byte_position))?;
        Some(Some(Box::new(enrich_item_semantic_with_project(
            semantic,
            bindings,
            project,
            source_path,
            byte_position,
        ))))
    };
    let when_present = recover_branch(&present_elements)?;
    let when_absent = recover_branch(&absent_elements)?;
    (when_present.is_some() || when_absent.is_some()).then_some(ItemSemantic::ModuleConditionalItem {
        module,
        when_present,
        when_absent,
    })
}

fn content_module_presence_condition(
    expression: &JavaExpression,
    bindings: &JavaBindings,
) -> Option<String> {
    let JavaExpression::Call {
        receiver: Some(receiver),
        method,
        arguments,
    } = expression
    else {
        return None;
    };
    if method != "isModLoaded" || !arguments.is_empty() {
        return None;
    }
    let module = resolved_reference_name(receiver, bindings)?;
    module.contains(".Mods.").then_some(module)
}

fn stable_item_array_elements(
    expression: &JavaExpression,
    bindings: &JavaBindings,
    project: &JavaProjectIndex,
) -> Option<Vec<JavaExpression>> {
    match expression {
        JavaExpression::Array {
            elements,
            dimensions,
            ..
        } if !elements.is_empty() && dimensions.is_empty() => Some(elements.clone()),
        JavaExpression::Array {
            elements,
            dimensions,
            ..
        } if elements.is_empty() && dimensions.is_empty() => Some(Vec::new()),
        JavaExpression::Array {
            elements,
            dimensions,
            ..
        } if elements.is_empty()
            && matches!(dimensions.as_slice(), [dimension] if literal_i64(dimension) == Some(0)) =>
        {
            Some(Vec::new())
        }
        JavaExpression::Reference(_) | JavaExpression::FieldAccess { .. } => {
            let reference = resolved_reference_name(expression, bindings)?;
            let (owner, field_name) = reference.rsplit_once('.')?;
            let field = project.resolve_field(owner, field_name, Some(true))?;
            if !field.is_final {
                return None;
            }
            stable_item_array_elements(field.initializer.as_ref()?, bindings, project)
        }
        JavaExpression::Parenthesized(inner) => stable_item_array_elements(inner, bindings, project),
        _ => None,
    }
}

fn qualify_project_item_reference(
    receiver: &JavaExpression,
    bindings: &JavaBindings,
    project: &JavaProjectIndex,
    source_path: &str,
    byte_position: usize,
) -> JavaExpression {
    let JavaExpression::Reference(reference) = receiver else {
        return receiver.clone();
    };
    project
        .qualified_field_reference_at(source_path, byte_position, reference, bindings)
        .map_or_else(|| receiver.clone(), JavaExpression::Reference)
}

fn enrich_item_semantic_with_project(
    semantic: ItemSemantic,
    bindings: &JavaBindings,
    project: &JavaProjectIndex,
    source_path: &str,
    byte_position: usize,
) -> ItemSemantic {
    match semantic {
        ItemSemantic::CopyAmount {
            amount,
            meta,
            item,
            inner,
            unsafe_copy,
        } => {
            let inner = inner.or_else(|| {
                item_semantic(&item, bindings)
                    .or_else(|| {
                        project_item_semantic(&item, bindings, project, source_path, byte_position)
                    })
                    .map(|semantic| {
                        Box::new(enrich_item_semantic_with_project(
                            semantic,
                            bindings,
                            project,
                            source_path,
                            byte_position,
                        ))
                    })
            });
            ItemSemantic::CopyAmount {
                amount,
                meta,
                item,
                inner,
                unsafe_copy,
            }
        }
        ItemSemantic::OreDictUnify { item, inner } => {
            let inner = inner.or_else(|| {
                item_semantic(&item, bindings)
                    .or_else(|| {
                        project_item_semantic(&item, bindings, project, source_path, byte_position)
                    })
                    .map(|semantic| {
                        Box::new(enrich_item_semantic_with_project(
                            semantic,
                            bindings,
                            project,
                            source_path,
                            byte_position,
                        ))
                    })
            });
            ItemSemantic::OreDictUnify { item, inner }
        }
        ItemSemantic::InlinedMethod { method_id, result } => ItemSemantic::InlinedMethod {
            method_id,
            result: Box::new(enrich_item_semantic_with_project(
                *result,
                bindings,
                project,
                source_path,
                byte_position,
            )),
        },
        ItemSemantic::InlinedValue { value_id, result } => ItemSemantic::InlinedValue {
            value_id,
            result: Box::new(enrich_item_semantic_with_project(
                *result,
                bindings,
                project,
                source_path,
                byte_position,
            )),
        },
        semantic => semantic,
    }
}

fn typed_item_expression(
    expression: &JavaExpression,
    bindings: &JavaBindings,
    project: &JavaProjectIndex,
    source_path: &str,
    byte_position: usize,
) -> Option<ItemSemantic> {
    let expression_type =
        project.expression_type_at(source_path, byte_position, expression, bindings)?;
    is_item_value_type(&expression_type)
        .then_some(ItemSemantic::TypedExpression { expression_type })
}

fn is_item_value_type(type_name: &str) -> bool {
    type_name
        .trim()
        .trim_end_matches("[]")
        .ends_with("ItemStack")
}

fn linked_method_semantic(
    resolved: &minecraft_redox_java::JavaMethod,
    receiver: Option<&JavaExpression>,
    arguments: &[JavaExpression],
    bindings: &JavaBindings,
    receiver_type: Option<String>,
) -> Option<ItemSemantic> {
    if !resolved.return_type.ends_with("ItemStack") {
        return None;
    }
    let method_id = resolved.stable_id();
    if receiver_type.is_none()
        && resolved.returns.len() == 1
        && let Some(replacements) = resolved.argument_bindings(arguments)
    {
        let substituted = substitute_references(&resolved.returns[0].expression, &replacements);
        if let Some(result) = item_semantic(&substituted, bindings) {
            return Some(ItemSemantic::InlinedMethod {
                method_id,
                result: Box::new(result),
            });
        }
    }
    Some(ItemSemantic::LinkedMethod {
        method_id,
        return_type: resolved.return_type.clone(),
        receiver: receiver.cloned(),
        receiver_type,
        arguments: arguments.to_vec(),
    })
}

fn enrich_fluid_semantic_with_project(
    semantic: FluidSemantic,
    expression: &JavaExpression,
    bindings: &JavaBindings,
    project: &JavaProjectIndex,
    source_path: &str,
    byte_position: usize,
) -> FluidSemantic {
    match (semantic, expression) {
        (
            FluidSemantic::FluidAccessor {
                source_ref,
                accessor,
                amount,
            },
            JavaExpression::Call {
                receiver: Some(receiver),
                ..
            },
        ) if !source_ref.contains('.') => {
            let JavaExpression::Reference(reference) = receiver.as_ref() else {
                return FluidSemantic::FluidAccessor {
                    source_ref,
                    accessor,
                    amount,
                };
            };
            let source_ref = project
                .qualified_field_reference_at(source_path, byte_position, reference, bindings)
                .unwrap_or(source_ref);
            FluidSemantic::FluidAccessor {
                source_ref,
                accessor,
                amount,
            }
        }
        (semantic, _) => semantic,
    }
}

fn project_fluid_semantic(
    expression: &JavaExpression,
    bindings: &JavaBindings,
    project: &JavaProjectIndex,
    source_path: &str,
    byte_position: usize,
) -> Option<FluidSemantic> {
    match expression {
        JavaExpression::Reference(reference) => {
            let value =
                project.resolve_value_at(source_path, byte_position, reference, bindings)?;
            let initializer =
                project.stable_local_initializer_at(source_path, byte_position, reference)?;
            let result = fluid_semantic(&initializer, bindings)?;
            let result = enrich_fluid_semantic_with_project(
                result,
                &initializer,
                bindings,
                project,
                source_path,
                byte_position,
            );
            Some(FluidSemantic::InlinedValue {
                value_id: value.value_id,
                result: Box::new(result),
            })
        }
        JavaExpression::Call {
            receiver,
            arguments,
            ..
        } => {
            let (resolved, receiver_type) =
                project.resolve_call_at(source_path, byte_position, expression, bindings)?;
            if !resolved.return_type.ends_with("FluidStack") {
                return None;
            }
            Some(FluidSemantic::LinkedMethod {
                method_id: resolved.stable_id(),
                return_type: resolved.return_type.clone(),
                receiver: receiver.as_deref().cloned(),
                receiver_type,
                arguments: arguments.clone(),
            })
        }
        JavaExpression::Parenthesized(inner) => {
            project_fluid_semantic(inner, bindings, project, source_path, byte_position)
        }
        _ => None,
    }
}

fn classify_item_call(
    receiver: &JavaExpression,
    method: &str,
    arguments: &[JavaExpression],
    bindings: &JavaBindings,
) -> Option<ItemSemantic> {
    let receiver_name = reference_name(receiver, bindings)?;
    let resolved_receiver = bindings.resolve_reference(&receiver_name);

    if resolved_receiver.ends_with("GTOreDictUnificator") {
        if method == "get" && arguments.len() >= 3 {
            let prefix = reference_name(arguments.first()?, bindings)?;
            return Some(ItemSemantic::OreDict {
                prefix: bindings.resolve_reference(&prefix),
                material: arguments.get(1)?.clone(),
                amount: arguments.get(2)?.clone(),
            });
        }
        if method == "get" && arguments.len() == 2 {
            return Some(ItemSemantic::OreDictQuery {
                key: arguments.first()?.clone(),
                amount: arguments.get(1)?.clone(),
            });
        }
        if method == "get" && arguments.len() == 1 {
            let item = arguments.first()?.clone();
            let inner = item_semantic(&item, bindings).map(Box::new);
            return Some(ItemSemantic::OreDictUnify { item, inner });
        }
        if matches!(method, "getDust" | "getDustTiny" | "getDustSmall") {
            return Some(ItemSemantic::OreDictHelper {
                method: method.to_owned(),
                arguments: arguments.to_vec(),
            });
        }
    }

    if resolved_receiver.ends_with("GTUtility") {
        if matches!(method, "copyAmount" | "copyAmountUnsafe") && arguments.len() >= 2 {
            let item = arguments.get(1)?.clone();
            let inner = item_semantic(&item, bindings).map(Box::new);
            return Some(ItemSemantic::CopyAmount {
                amount: arguments.first()?.clone(),
                meta: None,
                item,
                inner,
                unsafe_copy: method == "copyAmountUnsafe",
            });
        }
        if method == "copyAmountAndMetaData" && arguments.len() >= 3 {
            let item = arguments.get(2)?.clone();
            let inner = item_semantic(&item, bindings).map(Box::new);
            return Some(ItemSemantic::CopyAmount {
                amount: arguments.first()?.clone(),
                meta: arguments.get(1).cloned(),
                item,
                inner,
                unsafe_copy: false,
            });
        }
    }

    if looks_like_item_list_member(&resolved_receiver) && is_item_list_accessor(method) {
        return Some(ItemSemantic::ItemList {
            item: resolved_receiver,
            accessor: method.to_owned(),
            arguments: arguments.to_vec(),
        });
    }

    if looks_like_material_member(&resolved_receiver) && is_material_item_accessor(method) {
        return Some(ItemSemantic::MaterialItem {
            material: resolved_receiver,
            accessor: method.to_owned(),
            arguments: arguments.to_vec(),
        });
    }

    if resolved_receiver.ends_with("GTModHandler") && matches!(method, "getModItem" | "getIC2Item")
    {
        return Some(ItemSemantic::ModItem {
            provider: format!("{resolved_receiver}.{method}"),
            arguments: arguments.to_vec(),
        });
    }

    None
}

fn classify_implicit_item_call(
    method: &str,
    arguments: &[JavaExpression],
    bindings: &JavaBindings,
) -> Option<ItemSemantic> {
    let provider = bindings.static_imports.get(method)?;
    if matches!(method, "getModItem" | "getIC2Item") {
        return Some(ItemSemantic::ModItem {
            provider: provider.clone(),
            arguments: arguments.to_vec(),
        });
    }
    None
}

fn classify_new_item(
    type_name: &str,
    arguments: &[JavaExpression],
    bindings: &JavaBindings,
) -> Option<ItemSemantic> {
    let resolved_type = bindings.resolve_reference(type_name);
    if resolved_type.ends_with("OreDictItemStack") {
        return Some(ItemSemantic::OreDictStack {
            arguments: arguments.to_vec(),
        });
    }
    if resolved_type.ends_with("ItemStack") {
        return Some(ItemSemantic::ItemStack {
            item: arguments.first()?.clone(),
            amount: arguments.get(1).cloned(),
            meta: arguments.get(2).cloned(),
        });
    }
    None
}

pub(crate) fn fluid_semantic(expression: &JavaExpression, bindings: &JavaBindings) -> Option<FluidSemantic> {
    match expression {
        JavaExpression::Call {
            receiver: Some(receiver),
            method,
            arguments,
        } if matches!(method.as_str(), "getFluidStack" | "getFluidStackFromName")
            && arguments.len() >= 2
            && reference_name(receiver, bindings)?.ends_with("FluidRegistry") =>
        {
            let provider = format!("{}.{}", reference_name(receiver, bindings)?, method);
            Some(FluidSemantic::RegistryFluid {
                provider,
                arguments: arguments.clone(),
            })
        }
        JavaExpression::Call {
            receiver: Some(receiver),
            method,
            arguments,
        } if method == "soldering"
            && arguments.len() <= 1
            && reference_name(receiver, bindings)?
                == "gregtech.api.objects.SubstituteFluidStack" =>
        {
            Some(FluidSemantic::SolderingSubstitute {
                base_amount: arguments.first().cloned(),
            })
        }
        JavaExpression::Call {
            receiver: Some(receiver),
            method,
            arguments,
        } if is_fluid_accessor(method) && arguments.len() == 1 => {
            let material = reference_name(receiver, bindings)?;
            Some(FluidSemantic::FluidAccessor {
                source_ref: bindings.resolve_reference(&material),
                accessor: method.clone(),
                amount: arguments.first()?.clone(),
            })
        }
        JavaExpression::Call {
            receiver: None,
            method,
            arguments,
        } if matches!(method.as_str(), "getFluidStack" | "getFluidStackFromName") => {
            Some(FluidSemantic::RegistryFluid {
                provider: bindings
                    .static_imports
                    .get(method)
                    .cloned()
                    .unwrap_or_else(|| method.clone()),
                arguments: arguments.clone(),
            })
        }
        JavaExpression::New {
            type_name,
            arguments,
        } if bindings
            .resolve_reference(type_name)
            .ends_with("FluidStack")
            && arguments.len() >= 2 =>
        {
            Some(FluidSemantic::FluidStack {
                fluid: arguments.first()?.clone(),
                amount: arguments.get(1)?.clone(),
            })
        }
        JavaExpression::Parenthesized(inner) => fluid_semantic(inner, bindings),
        _ => None,
    }
}

fn reference_name(expression: &JavaExpression, bindings: &JavaBindings) -> Option<String> {
    match expression {
        JavaExpression::Reference(reference) => Some(bindings.resolve_reference(reference)),
        JavaExpression::FieldAccess { receiver, field } => {
            Some(format!("{}.{}", reference_name(receiver, bindings)?, field))
        }
        JavaExpression::Parenthesized(inner) => reference_name(inner, bindings),
        _ => None,
    }
}

fn looks_like_item_list_member(reference: &str) -> bool {
    reference.contains("ItemList.") || reference.contains("ItemList$")
}

fn looks_like_material_member(reference: &str) -> bool {
    reference.contains("Materials.")
        || reference.contains("MaterialsAlloy.")
        || reference.contains("MaterialsElements.")
        || reference.contains("MaterialsOres.")
        || reference.contains("MaterialsNuclides.")
        || reference.contains("MaterialMisc.")
        || reference.contains("WerkstoffLoader.")
        || reference.contains("GGMaterial.")
}

fn is_item_list_accessor(method: &str) -> bool {
    matches!(
        method,
        "get"
            | "getWildcard"
            | "getUndamaged"
            | "getAlmostBroken"
            | "getWithName"
            | "getWithCharge"
            | "getWithDamage"
            | "getInternalStack_unsafe"
    )
}

fn is_material_item_accessor(method: &str) -> bool {
    method.starts_with("get")
        && !is_fluid_accessor(method)
        && !matches!(
            method,
            "getMass" | "getProtons" | "getNeutrons" | "getDensity"
        )
}

fn is_fluid_accessor(method: &str) -> bool {
    matches!(
        method,
        "getFluid"
            | "getGas"
            | "getMolten"
            | "getPlasma"
            | "getFluidOrGas"
            | "getFluidStack"
            | "getSolid"
    )
}

pub(crate) fn literal_i64(expression: &JavaExpression) -> Option<i64> {
    match expression {
        JavaExpression::Literal(value) => parse_integer_literal(value),
        JavaExpression::Binary {
            left,
            operator,
            right,
        } => {
            let left = literal_i64(left)?;
            let right = literal_i64(right)?;
            match operator.as_str() {
                "+" => left.checked_add(right),
                "-" => left.checked_sub(right),
                "*" => left.checked_mul(right),
                "/" if right != 0 => left.checked_div(right),
                "%" if right != 0 => left.checked_rem(right),
                _ => None,
            }
        }
        JavaExpression::Parenthesized(inner) => literal_i64(inner),
        _ => None,
    }
}

pub(crate) fn resolved_reference_name(
    expression: &JavaExpression,
    bindings: &JavaBindings,
) -> Option<String> {
    reference_name(expression, bindings)
}

fn eval_i64(expression: &JavaExpression, bindings: &JavaBindings) -> Option<i64> {
    match expression {
        JavaExpression::Literal(value) => parse_integer_literal(value),
        JavaExpression::Reference(reference) => {
            known_integer_constant(&bindings.resolve_reference(reference))
        }
        JavaExpression::FieldAccess { .. } => reference_name(expression, bindings)
            .and_then(|reference| known_integer_constant(&reference)),
        JavaExpression::Binary {
            left,
            operator,
            right,
        } => {
            let left = eval_i64(left, bindings)?;
            let right = eval_i64(right, bindings)?;
            match operator.as_str() {
                "+" => left.checked_add(right),
                "-" => left.checked_sub(right),
                "*" => left.checked_mul(right),
                "/" if right != 0 => left.checked_div(right),
                "%" if right != 0 => left.checked_rem(right),
                _ => None,
            }
        }
        JavaExpression::Parenthesized(inner) => eval_i64(inner, bindings),
        _ => None,
    }
}

fn parse_integer_literal(value: &str) -> Option<i64> {
    let compact: String = value
        .chars()
        .filter(|character| *character != '_')
        .collect();
    let compact = compact
        .strip_suffix('L')
        .or_else(|| compact.strip_suffix('l'))
        .unwrap_or(&compact);
    if let Some(hex) = compact
        .strip_prefix("0x")
        .or_else(|| compact.strip_prefix("0X"))
    {
        return i64::from_str_radix(hex, 16).ok();
    }
    if let Some(binary) = compact
        .strip_prefix("0b")
        .or_else(|| compact.strip_prefix("0B"))
    {
        return i64::from_str_radix(binary, 2).ok();
    }
    if compact.len() > 1 && compact.starts_with('0') {
        return i64::from_str_radix(&compact[1..], 8).ok();
    }
    compact.parse().ok()
}

fn known_integer_constant(reference: &str) -> Option<i64> {
    match reference {
        "gregtech.api.util.GTRecipeBuilder.TICKS"
        | "GTRecipeBuilder.TICKS"
        | "gregtech.api.enums.TickTime.TICK"
        | "TickTime.TICK" => Some(1),
        "gregtech.api.util.GTRecipeBuilder.SECONDS"
        | "GTRecipeBuilder.SECONDS"
        | "gregtech.api.enums.TickTime.SECOND"
        | "TickTime.SECOND" => Some(20),
        "gregtech.api.util.GTRecipeBuilder.MINUTES"
        | "GTRecipeBuilder.MINUTES"
        | "gregtech.api.enums.TickTime.MINUTE"
        | "TickTime.MINUTE" => Some(20 * 60),
        "gregtech.api.util.GTRecipeBuilder.HOURS"
        | "GTRecipeBuilder.HOURS"
        | "gregtech.api.enums.TickTime.HOUR"
        | "TickTime.HOUR" => Some(20 * 60 * 60),
        "gregtech.api.enums.TickTime.DAY" | "TickTime.DAY" => Some(20 * 60 * 60 * 24),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FluidSemantic, ItemSemantic, duration_ticks, parse_integer_literal, recover_fluids,
        recover_fluids_with_project, recover_item_inputs, recover_item_inputs_with_project,
        recover_items,
    };
    use minecraft_redox_java::{JavaExpression, JavaProjectIndex, analyze_source};

    fn recipe_call<'a>(
        analysis: &'a minecraft_redox_java::JavaAnalysis,
        name: &str,
    ) -> anyhow::Result<&'a minecraft_redox_java::JavaCall> {
        analysis
            .call_chains
            .iter()
            .flat_map(|chain| &chain.calls)
            .find(|call| call.name == name)
            .ok_or_else(|| anyhow::anyhow!("missing call {name}"))
    }

    #[test]
    fn recognizes_common_recipe_values() -> anyhow::Result<()> {
        let source = r"
            import gregtech.api.enums.ItemList;
            import gregtech.api.enums.Materials;
            import gregtech.api.enums.OrePrefixes;
            import gregtech.api.util.GTOreDictUnificator;
            import net.minecraft.init.Blocks;
            import net.minecraft.item.ItemStack;
            class R { void run() {
                builder.itemInputs(
                    GTOreDictUnificator.get(OrePrefixes.dust, Materials.Netherrack, 4),
                    ItemList.Cell_Water.get(1),
                    new ItemStack(Blocks.dirt, 2, 3));
                builder.fluidInputs(Materials.Water.getFluid(1_000), Materials.Ice.getSolid(250));
            }}
        ";
        let analysis = analyze_source("R.java", "gt5", None, source)?;
        let items = recipe_call(&analysis, "itemInputs")?;
        let recovered = recover_items(
            &items.arguments,
            &items.argument_expressions,
            &analysis.bindings,
        );
        assert_eq!(recovered.len(), 3);
        assert!(matches!(
            recovered[0].semantic,
            Some(ItemSemantic::OreDict { .. })
        ));
        assert!(matches!(
            recovered[1].semantic,
            Some(ItemSemantic::ItemList { .. })
        ));
        assert!(matches!(
            recovered[2].semantic,
            Some(ItemSemantic::ItemStack { .. })
        ));

        let fluids = recipe_call(&analysis, "fluidInputs")?;
        let recovered = recover_fluids(
            &fluids.arguments,
            &fluids.argument_expressions,
            &analysis.bindings,
        );
        assert_eq!(recovered.len(), 2);
        assert!(matches!(
            recovered[0].semantic,
            Some(FluidSemantic::FluidAccessor { .. })
        ));
        assert!(matches!(
            recovered[1].semantic.as_ref(),
            Some(FluidSemantic::FluidAccessor { accessor, .. }) if accessor == "getSolid"
        ));
        Ok(())
    }

    #[test]
    fn links_unique_local_item_factory_by_name_and_arity() -> anyhow::Result<()> {
        let source = r#"
            import net.minecraft.item.ItemStack;
            class R {
                static ItemStack get(String key, int amount) { return null; }
                void run() { builder.itemInputs(get("dustIron", 4)); }
            }
        "#;
        let analysis = analyze_source("R.java", "gt5", None, source)?;
        let mut project = minecraft_redox_java::JavaProjectIndex::default();
        project.add_analysis(&analysis);
        let items = recipe_call(&analysis, "itemInputs")?;
        let recovered = super::recover_items_with_project(
            &items.arguments,
            &items.argument_expressions,
            &analysis.bindings,
            &project,
            "R.java",
            items.byte_start,
        );
        let Some(ItemSemantic::LinkedMethod { method_id, .. }) = &recovered[0].semantic else {
            return Err(anyhow::anyhow!("local method was not linked"));
        };
        assert_eq!(method_id, "R#get(String,int)");
        Ok(())
    }

    #[test]
    fn qualifies_wildcard_static_material_fluid_receivers() -> anyhow::Result<()> {
        let material = analyze_source(
            "Materials.java",
            "gt5",
            None,
            r"
                package gregtech.api.enums;
                import net.minecraftforge.fluids.FluidStack;
                class Materials {
                    static final Materials Lubricant = new Materials();
                    FluidStack getFluid(int amount) { return null; }
                }
            ",
        )?;
        let source = r"
            import static gregtech.api.enums.Materials.*;
            import gregtech.api.enums.GTValues;
            import gregtech.api.enums.Materials;
            import net.minecraftforge.fluids.FluidStack;
            class Recipes {
                void concrete() {
                    GTValues.RA.stdBuilder()
                        .fluidInputs(Lubricant.getFluid(1000))
                        .duration(20).eut(2).addTo(gregtech.api.recipe.RecipeMaps.chemicalReactorRecipes);
                }
                void local(Materials Lubricant) {
                    GTValues.RA.stdBuilder()
                        .fluidInputs(Lubricant.getFluid(1000))
                        .duration(20).eut(2).addTo(gregtech.api.recipe.RecipeMaps.chemicalReactorRecipes);
                }
            }
        ";
        let analysis = analyze_source("Recipes.java", "gt5", None, source)?;
        let mut project = JavaProjectIndex::default();
        project.add_analysis(&material);
        project.add_analysis(&analysis);
        let recovery = crate::recover_with_project(&analysis, &project);
        assert_eq!(recovery.recipes.len(), 2);
        assert!(matches!(
            &recovery.recipes[0].fluid_input_semantics[0].semantic,
            Some(FluidSemantic::FluidAccessor { source_ref, .. })
                if source_ref == "gregtech.api.enums.Materials.Lubricant"
        ));
        assert!(matches!(
            &recovery.recipes[1].fluid_input_semantics[0].semantic,
            Some(FluidSemantic::FluidAccessor { source_ref, .. }) if source_ref == "Lubricant"
        ));
        Ok(())
    }

    #[test]
    fn inlines_effectively_final_local_item_and_fluid_initializers() -> anyhow::Result<()> {
        let source = r#"
            import static net.minecraftforge.fluids.FluidRegistry.getFluidStack;
            import gregtech.api.enums.GTValues;
            import gregtech.api.enums.Materials;
            import gregtech.api.objects.OreDictItemStack;
            import net.minecraftforge.fluids.FluidStack;
            class Recipes { void run() {
                var logWood16 = new OreDictItemStack("logWood", 16);
                FluidStack death = getFluidStack("fluiddeath", 30);
                GTValues.RA.stdBuilder()
                    .itemInputs(logWood16)
                    .itemOutputs(Materials.Charcoal.getGems(1))
                    .fluidInputs(death)
                    .duration(20).eut(2).addTo(mixerRecipes);
            }}
        "#;
        let analysis = analyze_source("Recipes.java", "gt5", None, source)?;
        let mut project = JavaProjectIndex::default();
        project.add_analysis(&analysis);
        let recovery = crate::recover_with_project(&analysis, &project);
        let recipe = recovery
            .recipes
            .first()
            .ok_or_else(|| anyhow::anyhow!("recipe missing"))?;
        assert!(matches!(
            recipe.item_input_semantics[0].semantic,
            Some(ItemSemantic::InlinedValue { .. })
        ));
        assert!(matches!(
            recipe.fluid_input_semantics[0].semantic,
            Some(FluidSemantic::InlinedValue { .. })
        ));
        Ok(())
    }

    #[test]
    fn recovers_finite_enhanced_for_item_alternatives() -> anyhow::Result<()> {
        let source = r"
            import gregtech.api.enums.GTValues;
            import gregtech.api.enums.Materials;
            import gregtech.api.enums.OrePrefixes;
            import gregtech.api.util.GTOreDictUnificator;
            import net.minecraft.item.ItemStack;
            class Recipes { void run() {
                ItemStack[] items = new ItemStack[] {
                    GTOreDictUnificator.get(OrePrefixes.dust, Materials.Talc, 1L),
                    GTOreDictUnificator.get(OrePrefixes.dust, Materials.Soapstone, 1L),
                    GTOreDictUnificator.get(OrePrefixes.dust, Materials.Redstone, 1L)
                };
                for (ItemStack item : items) {
                    GTValues.RA.stdBuilder().itemInputs(item).itemOutputs(Materials.Iron.getDust(1))
                        .duration(20).eut(4).addTo(gregtech.api.recipe.RecipeMaps.brewingRecipes);
                }
            }}
        ";
        let analysis = analyze_source("Recipes.java", "gt5", None, source)?;
        let mut project = JavaProjectIndex::default();
        project.add_analysis(&analysis);
        let recovery = crate::recover_with_project(&analysis, &project);
        let recipe = recovery.recipes.first().ok_or_else(|| anyhow::anyhow!("recipe missing"))?;
        assert!(matches!(
            &recipe.item_input_semantics[0].semantic,
            Some(ItemSemantic::ExplicitAlternatives { alternatives }) if alternatives.len() == 3
        ));
        Ok(())
    }

    #[test]
    fn inlines_single_return_local_item_helper() -> anyhow::Result<()> {
        let source = r"
            import gregtech.api.enums.ItemList;
            import gregtech.api.util.GTUtility;
            import net.minecraft.item.ItemStack;
            class R {
                static ItemStack oversized(ItemList item, int amount) {
                    return GTUtility.copyAmountUnsafe(amount, item.get(1));
                }
                void run() { builder.itemInputs(oversized(ItemList.Cell_Water, 48)); }
            }
        ";
        let analysis = analyze_source("R.java", "gt5", None, source)?;
        let mut project = minecraft_redox_java::JavaProjectIndex::default();
        project.add_analysis(&analysis);
        let items = recipe_call(&analysis, "itemInputs")?;
        let recovered = super::recover_items_with_project(
            &items.arguments,
            &items.argument_expressions,
            &analysis.bindings,
            &project,
            "R.java",
            items.byte_start,
        );
        let Some(ItemSemantic::InlinedMethod { result, .. }) = &recovered[0].semantic else {
            return Err(anyhow::anyhow!("local method body was not inlined"));
        };
        assert!(matches!(result.as_ref(), ItemSemantic::CopyAmount { .. }));
        Ok(())
    }

    #[test]
    fn links_method_on_typed_parameter_receiver() -> anyhow::Result<()> {
        let source = r"
            import net.minecraft.item.ItemStack;
            class Widget {
                ItemStack get(int amount) { return null; }
            }
            class R {
                void run(Widget widget) { builder.itemInputs(widget.get(4)); }
            }
        ";
        let analysis = analyze_source("R.java", "gt5", None, source)?;
        let mut project = minecraft_redox_java::JavaProjectIndex::default();
        project.add_analysis(&analysis);
        let items = recipe_call(&analysis, "itemInputs")?;
        let recovered = super::recover_items_with_project(
            &items.arguments,
            &items.argument_expressions,
            &analysis.bindings,
            &project,
            "R.java",
            items.byte_start,
        );
        let Some(ItemSemantic::LinkedMethod {
            receiver_type,
            method_id,
            ..
        }) = &recovered[0].semantic
        else {
            return Err(anyhow::anyhow!("typed receiver method was not linked"));
        };
        assert_eq!(receiver_type.as_deref(), Some("Widget"));
        assert_eq!(method_id, "Widget#get(int)");
        Ok(())
    }

    #[test]
    fn recognizes_copy_amount_and_ore_dict_overloads() -> anyhow::Result<()> {
        let source = r#"
            import gregtech.api.enums.Materials;
            import gregtech.api.enums.OrePrefixes;
            import gregtech.api.util.GTOreDictUnificator;
            import gregtech.api.util.GTUtility;
            class R { void run() {
                builder.itemInputs(
                    GTUtility.copyAmount(8, GTOreDictUnificator.get(OrePrefixes.dust, Materials.Iron, 1)),
                    GTOreDictUnificator.get("fuelCoke", 4));
            }}
        "#;
        let analysis = analyze_source("R.java", "gt5", None, source)?;
        let items = recipe_call(&analysis, "itemInputs")?;
        let recovered = recover_items(
            &items.arguments,
            &items.argument_expressions,
            &analysis.bindings,
        );
        assert!(matches!(
            recovered[0].semantic,
            Some(ItemSemantic::CopyAmount { .. })
        ));
        assert!(matches!(
            recovered[1].semantic,
            Some(ItemSemantic::OreDictQuery { .. })
        ));
        Ok(())
    }

    #[test]
    fn distinguishes_ore_dict_stack_from_item_stack() -> anyhow::Result<()> {
        let source = r#"
            import gregtech.api.objects.OreDictItemStack;
            class R { void run() { builder.itemInputs(new OreDictItemStack("dustIron", 1)); } }
        "#;
        let analysis = analyze_source("R.java", "gt5", None, source)?;
        let items = recipe_call(&analysis, "itemInputs")?;
        let recovered = recover_items(
            &items.arguments,
            &items.argument_expressions,
            &analysis.bindings,
        );
        assert!(matches!(
            recovered[0].semantic,
            Some(ItemSemantic::OreDictStack { .. })
        ));
        Ok(())
    }

    #[test]
    fn inlines_array_returning_input_helper_with_varargs_context() -> anyhow::Result<()> {
        let source = r#"
            import net.minecraft.init.Blocks;
            import net.minecraft.item.ItemStack;
            class R {
                static Object[] alt(int amount) { return new Object[] { "dustIron", amount }; }
                void run() {
                    builder.itemInputs(new ItemStack(Blocks.dirt, 1), alt(4));
                }
            }
        "#;
        let analysis = analyze_source("R.java", "gt5", None, source)?;
        let mut project = minecraft_redox_java::JavaProjectIndex::default();
        project.add_analysis(&analysis);
        let inputs = recipe_call(&analysis, "itemInputs")?;
        let recovered = recover_item_inputs_with_project(
            &inputs.arguments,
            &inputs.argument_expressions,
            &analysis.bindings,
            &project,
            "R.java",
            inputs.byte_start,
        );
        assert!(matches!(
            &recovered[1].semantic,
            Some(ItemSemantic::InlinedMethod { result, .. })
                if matches!(result.as_ref(), ItemSemantic::OreDictAlternativeInput { .. })
        ));
        Ok(())
    }

    #[test]
    fn links_typed_standard_collection_item_array() -> anyhow::Result<()> {
        let source = r"
            import java.util.List;
            import net.minecraft.item.ItemStack;
            class R {
                void run(List<ItemStack> items) {
                    builder.itemOutputs(items.toArray(new ItemStack[0]));
                }
            }
        ";
        let analysis = analyze_source("R.java", "gt5", None, source)?;
        let mut project = minecraft_redox_java::JavaProjectIndex::default();
        project.add_analysis(&analysis);
        let outputs = recipe_call(&analysis, "itemOutputs")?;
        let recovered = super::recover_items_with_project(
            &outputs.arguments,
            &outputs.argument_expressions,
            &analysis.bindings,
            &project,
            "R.java",
            outputs.byte_start,
        );
        assert!(matches!(
            &recovered[0].semantic,
            Some(ItemSemantic::TypedExpression { expression_type })
                if expression_type == "net.minecraft.item.ItemStack[]"
        ));
        Ok(())
    }

    #[test]
    fn links_parameterized_item_values_without_calling_them_concrete() -> anyhow::Result<()> {
        let source = r"
            import net.minecraft.item.ItemStack;
            class R {
                ItemStack member;
                void run(ItemStack parameter) {
                    ItemStack local = parameter;
                    builder.itemInputs(parameter, local, member);
                }
            }
        ";
        let analysis = analyze_source("R.java", "gt5", None, source)?;
        let mut project = minecraft_redox_java::JavaProjectIndex::default();
        project.add_analysis(&analysis);
        let inputs = recipe_call(&analysis, "itemInputs")?;
        let recovered = recover_item_inputs_with_project(
            &inputs.arguments,
            &inputs.argument_expressions,
            &analysis.bindings,
            &project,
            "R.java",
            inputs.byte_start,
        );
        let expected = [
            minecraft_redox_java::JavaValueKind::Parameter,
            minecraft_redox_java::JavaValueKind::Local,
            minecraft_redox_java::JavaValueKind::Field,
        ];
        for (value, expected_kind) in recovered.iter().zip(expected) {
            let Some(ItemSemantic::LinkedValue {
                value_kind,
                declared_type,
                ..
            }) = &value.semantic
            else {
                return Err(anyhow::anyhow!("recipe value was not linked"));
            };
            assert_eq!(*value_kind, expected_kind);
            assert_eq!(declared_type, "net.minecraft.item.ItemStack");
        }
        Ok(())
    }

    #[test]
    fn interprets_gt_recipe_input_arrays_only_in_input_context() -> anyhow::Result<()> {
        let source = r#"
            import net.minecraft.init.Blocks;
            import net.minecraft.item.ItemStack;
            class R { void run() {
                builder.itemInputs(
                    new Object[] { "circuitUV", 4 },
                    new ItemStack[] { new ItemStack(Blocks.dirt, 1) });
                builder.itemOutputs(new ItemStack[] { new ItemStack(Blocks.stone, 1) });
            }}
        "#;
        let analysis = analyze_source("R.java", "gt5", None, source)?;
        let inputs = recipe_call(&analysis, "itemInputs")?;
        let recovered = recover_item_inputs(
            &inputs.arguments,
            &inputs.argument_expressions,
            &analysis.bindings,
        );
        assert!(matches!(
            recovered[0].semantic,
            Some(ItemSemantic::OreDictAlternativeInput { .. })
        ));
        assert!(matches!(
            recovered[1].semantic,
            Some(ItemSemantic::ExplicitAlternatives { .. })
        ));

        let outputs = recipe_call(&analysis, "itemOutputs")?;
        let recovered = recover_items(
            &outputs.arguments,
            &outputs.argument_expressions,
            &analysis.bindings,
        );
        assert!(recovered[0].semantic.is_none());
        Ok(())
    }

    #[test]
    fn links_typed_fluid_field_chain() -> anyhow::Result<()> {
        let source = r"
            package demo;
            import net.minecraftforge.fluids.FluidStack;
            class Material { FluidStack getFluidStack(int amount) { return null; } }
            class Materials {
                public final Material WATER = new Material();
                public static Materials getInstance() { return null; }
            }
            class R {
                void run() {
                    builder.fluidInputs(Materials.getInstance().WATER.getFluidStack(1000));
                }
            }
        ";
        let analysis = analyze_source("R.java", "gt5", None, source)?;
        let mut project = minecraft_redox_java::JavaProjectIndex::default();
        project.add_analysis(&analysis);
        let fluids = recipe_call(&analysis, "fluidInputs")?;
        let recovered = recover_fluids_with_project(
            &fluids.arguments,
            &fluids.argument_expressions,
            &analysis.bindings,
            &project,
            "R.java",
            fluids.byte_start,
        );
        assert!(matches!(
            recovered[0].semantic,
            Some(FluidSemantic::LinkedMethod { .. })
        ));
        Ok(())
    }

    #[test]
    fn does_not_treat_volume_constants_as_time() -> anyhow::Result<()> {
        let source = r"
            import static gregtech.api.util.GTRecipeBuilder.INGOTS;
            class R { void run() { builder.duration(2 * INGOTS); } }
        ";
        let analysis = analyze_source("R.java", "gt5", None, source)?;
        let call = recipe_call(&analysis, "duration")?;
        let expression = call
            .argument_expressions
            .first()
            .ok_or_else(|| anyhow::anyhow!("missing duration expression"))?;
        assert_eq!(duration_ticks(expression, &analysis.bindings), None);
        Ok(())
    }

    #[test]
    fn evaluates_qualified_gt_duration_units() -> anyhow::Result<()> {
        let source = r"
            class R { void run() { builder.duration(60 * GTRecipeBuilder.SECONDS); } }
        ";
        let analysis = analyze_source("R.java", "gt5", None, source)?;
        let call = recipe_call(&analysis, "duration")?;
        let expression = call
            .argument_expressions
            .first()
            .ok_or_else(|| anyhow::anyhow!("missing duration expression"))?;
        assert_eq!(duration_ticks(expression, &analysis.bindings), Some(1200));
        Ok(())
    }

    #[test]
    fn parses_java_integer_literal_forms() {
        assert_eq!(parse_integer_literal("1_000L"), Some(1_000));
        assert_eq!(parse_integer_literal("0x10"), Some(16));
        assert_eq!(parse_integer_literal("0b1010"), Some(10));
        assert_eq!(parse_integer_literal("077"), Some(63));
    }

    #[test]
    fn evaluates_gt_duration_units_exactly() -> anyhow::Result<()> {
        let source = r"
            import static gregtech.api.util.GTRecipeBuilder.SECONDS;
            import static gregtech.api.util.GTRecipeBuilder.TICKS;
            class R { void run() { builder.duration(19 * SECONDS + 12 * TICKS); } }
        ";
        let analysis = analyze_source("R.java", "gt5", None, source)?;
        let call = recipe_call(&analysis, "duration")?;
        let expression: &JavaExpression = call
            .argument_expressions
            .first()
            .ok_or_else(|| anyhow::anyhow!("missing duration expression"))?;
        assert_eq!(duration_ticks(expression, &analysis.bindings), Some(392));
        Ok(())
    }
}
