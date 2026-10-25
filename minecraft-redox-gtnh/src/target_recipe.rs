use minecraft_redox_core::{
    SourceOrigin,
    content::{
        ContentId, ContentRecipe, FluidInputSpec, FluidSelector, FluidStackSpec, ItemInputSpec,
        ItemSelector, ItemStackIntegerPair, ItemStackSpec, RecipeBehavior, RecipeCondition,
        RecipeMetadataSpec, RecipeMetadataValue, RecipePolicySpec, RecipeSlotChance,
        RecipeSpecialSpec,
    },
};
use minecraft_redox_java::{
    JavaBindings, JavaExpression, JavaProjectIndex, JavaTypeKind, JavaValueKind,
};
use serde::Serialize;

use crate::{
    ExpressionSemantic, FluidSemantic, ItemSemantic, NumericSemantic, RecipeDefinition,
    RecipeMetadataDefinition, RecoveredExpression,
    fixed_decimal::fixed_decimal_value,
    recipe_expression::{literal_i64, recover_items_with_project, resolved_reference_name},
    scalar_expression::{scalar_i64, scalar_static_field_i64},
};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecipeLoweringBlockerKind {
    Fluids,
    Chances,
    Metadata,
    Modifiers,
    Special,
    PostAdd,
    MissingDuration,
    InvalidDuration,
    MissingEut,
    NonConstantEut,
    ItemInput,
    ItemOutput,
}

impl RecipeLoweringBlockerKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fluids => "fluids",
            Self::Chances => "chances",
            Self::Metadata => "metadata",
            Self::Modifiers => "modifiers",
            Self::Special => "special",
            Self::PostAdd => "post_add",
            Self::MissingDuration => "missing_duration",
            Self::InvalidDuration => "invalid_duration",
            Self::MissingEut => "missing_eut",
            Self::NonConstantEut => "non_constant_eut",
            Self::ItemInput => "item_input",
            Self::ItemOutput => "item_output",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RecipeLoweringBlocker {
    pub kind: RecipeLoweringBlockerKind,
    pub detail: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RecipeLoweringFailure {
    pub origin: SourceOrigin,
    pub blockers: Vec<RecipeLoweringBlocker>,
}

/// Lower one recovered GT recipe into source-language-free content IR.
///
/// # Errors
///
/// Returns every currently known blocker when source semantics cannot be
/// represented without loss by the deliberately small target recipe IR.
pub fn lower_recipe(
    recipe: &RecipeDefinition,
    bindings: &JavaBindings,
) -> Result<ContentRecipe, Box<RecipeLoweringFailure>> {
    lower_recipe_internal(recipe, bindings, None)
}

pub(crate) fn lower_recipe_with_project(
    recipe: &RecipeDefinition,
    bindings: &JavaBindings,
    project: &JavaProjectIndex,
) -> Result<ContentRecipe, Box<RecipeLoweringFailure>> {
    lower_recipe_internal(recipe, bindings, Some(project))
}

fn lower_recipe_internal(
    recipe: &RecipeDefinition,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
) -> Result<ContentRecipe, Box<RecipeLoweringFailure>> {
    let mut blockers = structural_blockers(recipe);

    let duration_ticks = match recipe.duration_ticks {
        None => {
            push_blocker(
                &mut blockers,
                RecipeLoweringBlockerKind::MissingDuration,
                "duration did not reduce to ticks",
            );
            None
        }
        Some(value) => {
            if let Ok(value) = u64::try_from(value) {
                Some(value)
            } else {
                push_blocker(
                    &mut blockers,
                    RecipeLoweringBlockerKind::InvalidDuration,
                    format!("duration ticks must be non-negative: {value}"),
                );
                None
            }
        }
    };

    let eut = match (recipe.eut_expression.as_ref(), recipe.eut_value) {
        (None, _) => {
            push_blocker(
                &mut blockers,
                RecipeLoweringBlockerKind::MissingEut,
                "recipe has no EU/t expression",
            );
            None
        }
        (Some(_), Some(value)) => Some(value),
        (Some(_), None) => {
            push_blocker(
                &mut blockers,
                RecipeLoweringBlockerKind::NonConstantEut,
                recipe
                    .eut
                    .clone()
                    .unwrap_or_else(|| String::from("<missing source>")),
            );
            None
        }
    };

    let extras = lower_recipe_extras(recipe, bindings, project, &mut blockers);
    let mut item_inputs = lower_inputs(recipe, bindings, &mut blockers);
    item_inputs.extend(lower_integrated_circuits(recipe, &mut blockers));
    let item_outputs = lower_outputs(recipe, bindings, &mut blockers);
    let item_input_consumption_chances = lower_slot_chances(
        &recipe.input_chance_semantics,
        recipe.item_input_semantics.len() + recipe.integrated_circuits.len(),
        "item input consumption",
        &mut blockers,
    );
    let item_output_production_chances = lower_slot_chances(
        &recipe.output_chance_semantics,
        recipe.item_output_semantics.len(),
        "item output production",
        &mut blockers,
    );
    let fluid_inputs = lower_fluid_inputs(
        &recipe.fluid_input_semantics,
        bindings,
        project,
        &mut blockers,
    );
    let fluid_outputs = lower_fluid_outputs(
        &recipe.fluid_output_semantics,
        bindings,
        project,
        &mut blockers,
    );

    if blockers.is_empty()
        && let (Some(duration_ticks), Some(eut)) = (duration_ticks, eut)
    {
        return Ok(ContentRecipe {
            origin: recipe.origin.clone(),
            recipe_map: ContentId::new(recipe.recipe_map.clone()),
            item_inputs,
            item_outputs,
            fluid_inputs,
            fluid_outputs,
            duration_ticks,
            eut,
            item_input_consumption_chances,
            item_output_production_chances,
            metadata: extras.metadata,
            category: extras.category,
            special: extras.special,
            special_value: extras.special_value,
            policy: extras.policy,
        });
    }

    Err(Box::new(RecipeLoweringFailure {
        origin: recipe.origin.clone(),
        blockers,
    }))
}

struct LoweredRecipeExtras {
    policy: RecipePolicySpec,
    metadata: Vec<RecipeMetadataSpec>,
    category: Option<ContentId>,
    special: Option<RecipeSpecialSpec>,
    special_value: Option<i64>,
}

fn lower_recipe_extras(
    recipe: &RecipeDefinition,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
    blockers: &mut Vec<RecipeLoweringBlocker>,
) -> LoweredRecipeExtras {
    LoweredRecipeExtras {
        policy: lower_recipe_policy(recipe, bindings, blockers),
        metadata: lower_recipe_metadata(recipe, bindings, project, blockers),
        category: lower_recipe_category(recipe, bindings, blockers),
        special: lower_recipe_special(recipe, bindings, project, blockers),
        special_value: lower_recipe_special_value(recipe, blockers),
    }
}

fn structural_blockers(recipe: &RecipeDefinition) -> Vec<RecipeLoweringBlocker> {
    let mut blockers = Vec::new();
    if !recipe.post_add_calls.is_empty() || !recipe.unhandled_calls.is_empty() {
        push_blocker(
            &mut blockers,
            RecipeLoweringBlockerKind::PostAdd,
            "post-add or unhandled builder behavior remains",
        );
    }
    blockers
}

fn lower_recipe_metadata(
    recipe: &RecipeDefinition,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
    blockers: &mut Vec<RecipeLoweringBlocker>,
) -> Vec<RecipeMetadataSpec> {
    recipe
        .metadata_semantics
        .iter()
        .filter_map(|metadata| {
            lower_metadata_entry(metadata, &recipe.origin.path, bindings, project).or_else(|| {
                push_blocker(
                    blockers,
                    RecipeLoweringBlockerKind::Metadata,
                    format!(
                        "unsupported metadata payload {}: {}",
                        metadata
                            .expected_value_type
                            .as_deref()
                            .unwrap_or("<unknown>"),
                        metadata.value_source
                    ),
                );
                None
            })
        })
        .collect()
}

fn lower_metadata_entry(
    metadata: &RecipeMetadataDefinition,
    source_path: &str,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
) -> Option<RecipeMetadataSpec> {
    let key = resolved_reference_name(&metadata.key_expression, bindings)
        .unwrap_or_else(|| bindings.resolve_reference(&metadata.key_source));
    let inferred_expected = infer_metadata_expected_type(metadata, bindings, project);
    let expected = metadata
        .expected_value_type
        .as_deref()
        .or(inferred_expected.as_deref())?;
    let value = if is_java_type(expected, "Integer") || is_java_type(expected, "Long") {
        RecipeMetadataValue::Integer {
            value: metadata_i64(&metadata.value_expression, bindings, project)
                .or_else(|| linked_metadata_field_i64(&metadata.value_semantic, project))?,
        }
    } else if is_java_type(expected, "Float") || is_java_type(expected, "Double") {
        RecipeMetadataValue::Decimal {
            value: fixed_decimal_value(&metadata.value_expression)?,
        }
    } else if is_decimal_triple_type(expected) {
        lower_decimal_triple_metadata(&metadata.value_expression)?
    } else if is_java_type(expected, "Boolean") {
        RecipeMetadataValue::Boolean {
            value: literal_boolean(&metadata.value_expression)?,
        }
    } else if is_java_type(expected, "String") {
        RecipeMetadataValue::Text {
            value: simple_string_literal(&metadata.value_expression)?,
        }
    } else if is_java_type(expected, "Materials") {
        RecipeMetadataValue::ContentRef {
            id: ContentId::new(resolved_reference_name(
                &metadata.value_expression,
                bindings,
            )?),
        }
    } else if is_java_type(expected, "ItemStack") {
        lower_item_stack_metadata(metadata, source_path, bindings, project)?
    } else if expected.ends_with("ItemStack[]") {
        lower_item_stack_array_metadata(metadata, source_path, bindings, project)?
    } else if is_item_stack_integer_pair_array(expected) {
        lower_item_stack_integer_pair_array_metadata(metadata, source_path, bindings, project)?
    } else if is_java_type(expected, "Scanning") {
        lower_scanning_metadata(&metadata.value_expression, bindings, project)?
    } else if is_java_type(expected, "Sievert") {
        lower_sievert_metadata(&metadata.value_expression, bindings, project)?
    } else if project.is_some_and(|project| {
        let owner = bindings.resolve_reference(expected);
        project.type_kind(&owner) == Some(JavaTypeKind::Enum)
    }) {
        RecipeMetadataValue::ContentRef {
            id: ContentId::new(resolved_reference_name(
                &metadata.value_expression,
                bindings,
            )?),
        }
    } else {
        return None;
    };
    Some(RecipeMetadataSpec {
        key: ContentId::new(key),
        value,
    })
}

fn lower_metadata_item_expression(
    source: &str,
    expression: &JavaExpression,
    source_path: &str,
    byte_start: usize,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
) -> Option<ItemStackSpec> {
    let sources = [source.to_owned()];
    let expressions = [expression.clone()];
    let recovered = if let Some(project) = project {
        recover_items_with_project(
            &sources,
            &expressions,
            bindings,
            project,
            source_path,
            byte_start,
        )
    } else {
        crate::recipe_expression::recover_items(&sources, &expressions, bindings)
    };
    lower_recovered_stack(recovered.first()?, bindings)
}

fn infer_metadata_expected_type(
    metadata: &RecipeMetadataDefinition,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
) -> Option<String> {
    let project = project?;
    let key = resolved_reference_name(&metadata.key_expression, bindings)
        .unwrap_or_else(|| bindings.resolve_reference(&metadata.key_source));
    let (owner, field_name) = key.rsplit_once('.')?;
    let field = project.resolve_field(owner, field_name, Some(true))?;
    if let Some(start) = field.type_name.find('<') {
        let end = field.type_name.rfind('>')?;
        let raw = field.type_name.get(..start)?.trim();
        if raw.ends_with("RecipeMetadataKey") {
            return Some(field.type_name.get(start + 1..end)?.trim().to_owned());
        }
    }
    project.generic_supertype_argument(&field.type_name, "RecipeMetadataKey", 0)
}

fn is_decimal_triple_type(expected: &str) -> bool {
    let compact: String = expected
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    compact.ends_with("Triple<Double,Double,Double>")
}

fn lower_decimal_triple_metadata(expression: &JavaExpression) -> Option<RecipeMetadataValue> {
    let JavaExpression::Call {
        method, arguments, ..
    } = expression
    else {
        return None;
    };
    if method != "of" || arguments.len() != 3 {
        return None;
    }
    Some(RecipeMetadataValue::DecimalTriple {
        first: fixed_decimal_value(&arguments[0])?,
        second: fixed_decimal_value(&arguments[1])?,
        third: fixed_decimal_value(&arguments[2])?,
    })
}

fn lower_item_stack_metadata(
    metadata: &RecipeMetadataDefinition,
    source_path: &str,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
) -> Option<RecipeMetadataValue> {
    Some(RecipeMetadataValue::ItemStack {
        value: lower_metadata_item_expression(
            &metadata.value_source,
            &metadata.value_expression,
            source_path,
            metadata.byte_start,
            bindings,
            project,
        )?,
    })
}

fn lower_item_stack_array_metadata(
    metadata: &RecipeMetadataDefinition,
    source_path: &str,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
) -> Option<RecipeMetadataValue> {
    let JavaExpression::Array { elements, .. } = &metadata.value_expression else {
        return None;
    };
    let values = elements
        .iter()
        .map(|expression| {
            lower_metadata_item_expression(
                &metadata.value_source,
                expression,
                source_path,
                metadata.byte_start,
                bindings,
                project,
            )
        })
        .collect::<Option<Vec<_>>>()?;
    Some(RecipeMetadataValue::ItemStacks { values })
}

fn is_item_stack_integer_pair_array(expected: &str) -> bool {
    let Some(element_type) = expected.strip_suffix("[]") else {
        return false;
    };
    element_type.contains("Pair<")
        && element_type.contains("ItemStack")
        && element_type.ends_with(", Integer>")
}

fn lower_item_stack_integer_pair_array_metadata(
    metadata: &RecipeMetadataDefinition,
    source_path: &str,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
) -> Option<RecipeMetadataValue> {
    let JavaExpression::Array { elements, .. } = &metadata.value_expression else {
        return None;
    };
    let values = elements
        .iter()
        .map(|element| {
            let JavaExpression::Call {
                method, arguments, ..
            } = element
            else {
                return None;
            };
            if method != "of" || arguments.len() != 2 {
                return None;
            }
            Some(ItemStackIntegerPair {
                item: lower_metadata_item_expression(
                    &metadata.value_source,
                    &arguments[0],
                    source_path,
                    metadata.byte_start,
                    bindings,
                    project,
                )?,
                value: metadata_i64(&arguments[1], bindings, project)?,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(RecipeMetadataValue::ItemStackIntegerPairs { values })
}

fn linked_metadata_field_i64(
    semantic: &ExpressionSemantic,
    project: Option<&JavaProjectIndex>,
) -> Option<i64> {
    let ExpressionSemantic::LinkedValue {
        value_id,
        value_kind: JavaValueKind::Field,
        ..
    } = semantic
    else {
        return None;
    };
    let (owner, name) = value_id.split_once("::field:")?;
    scalar_static_field_i64(owner, name, project?)
}

fn metadata_i64(
    expression: &JavaExpression,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
) -> Option<i64> {
    project.map_or_else(
        || literal_i64(expression),
        |project| scalar_i64(expression, bindings, project),
    )
}

fn lower_scanning_metadata(
    expression: &JavaExpression,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
) -> Option<RecipeMetadataValue> {
    let JavaExpression::New {
        type_name,
        arguments,
    } = expression
    else {
        return None;
    };
    if !is_java_type(type_name, "Scanning") || arguments.len() != 2 {
        return None;
    }
    Some(RecipeMetadataValue::Scanning {
        duration_ticks: u64::try_from(metadata_i64(&arguments[0], bindings, project)?).ok()?,
        voltage: metadata_i64(&arguments[1], bindings, project)?,
    })
}

fn lower_sievert_metadata(
    expression: &JavaExpression,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
) -> Option<RecipeMetadataValue> {
    let JavaExpression::New {
        type_name,
        arguments,
    } = expression
    else {
        return None;
    };
    if !is_java_type(type_name, "Sievert") || !(1..=2).contains(&arguments.len()) {
        return None;
    }
    let exact = arguments.get(1).map_or(Some(false), literal_boolean)?;
    Some(RecipeMetadataValue::Sievert {
        value: metadata_i64(&arguments[0], bindings, project)?,
        exact,
    })
}

fn lower_recipe_category(
    recipe: &RecipeDefinition,
    bindings: &JavaBindings,
    blockers: &mut Vec<RecipeLoweringBlocker>,
) -> Option<ContentId> {
    let category = recipe.recipe_categories.last()?;
    let Some(id) = resolved_reference_name(&category.expression, bindings) else {
        push_blocker(
            blockers,
            RecipeLoweringBlockerKind::Metadata,
            format!("unresolved recipe category: {}", category.source),
        );
        return None;
    };
    Some(ContentId::new(id))
}

fn is_java_type(type_name: &str, simple_name: &str) -> bool {
    type_name == simple_name || type_name.ends_with(&format!(".{simple_name}"))
}

fn literal_boolean(expression: &JavaExpression) -> Option<bool> {
    match expression {
        JavaExpression::Literal(value) if value == "true" => Some(true),
        JavaExpression::Literal(value) if value == "false" => Some(false),
        JavaExpression::Parenthesized(inner) | JavaExpression::Cast { value: inner, .. } => {
            literal_boolean(inner)
        }
        _ => None,
    }
}

fn lower_slot_chances(
    recovered: &[RecoveredExpression<NumericSemantic>],
    slot_count: usize,
    description: &str,
    blockers: &mut Vec<RecipeLoweringBlocker>,
) -> Vec<RecipeSlotChance> {
    if recovered.is_empty() {
        return Vec::new();
    }
    if recovered.len() != slot_count {
        push_blocker(
            blockers,
            RecipeLoweringBlockerKind::Chances,
            format!(
                "{description} chance count {} does not match recovered slot count {slot_count}",
                recovered.len()
            ),
        );
        return Vec::new();
    }

    recovered
        .iter()
        .enumerate()
        .filter_map(|(slot, recovered)| {
            let Some(NumericSemantic::ExactValue { value }) = recovered.semantic.as_ref() else {
                push_blocker(
                    blockers,
                    RecipeLoweringBlockerKind::Chances,
                    format!(
                        "dynamic {description} chance for slot {slot}: {}",
                        recovered.source
                    ),
                );
                return None;
            };
            let Ok(chance_10000) = u16::try_from(*value) else {
                push_blocker(
                    blockers,
                    RecipeLoweringBlockerKind::Chances,
                    format!("invalid {description} chance for slot {slot}: {value}"),
                );
                return None;
            };
            if chance_10000 > 10_000 {
                push_blocker(
                    blockers,
                    RecipeLoweringBlockerKind::Chances,
                    format!("invalid {description} chance for slot {slot}: {value}"),
                );
                return None;
            }
            let Ok(slot) = u32::try_from(slot) else {
                push_blocker(
                    blockers,
                    RecipeLoweringBlockerKind::Chances,
                    format!("{description} slot index does not fit u32: {slot}"),
                );
                return None;
            };
            Some(RecipeSlotChance { slot, chance_10000 })
        })
        .collect()
}

fn lower_recipe_special(
    recipe: &RecipeDefinition,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
    blockers: &mut Vec<RecipeLoweringBlocker>,
) -> Option<RecipeSpecialSpec> {
    let (Some(source), Some(expression)) = (&recipe.special, &recipe.special_expression) else {
        return None;
    };
    let sources = [source.clone()];
    let expressions = [expression.clone()];
    let recovered = if let Some(project) = project {
        recover_items_with_project(
            &sources,
            &expressions,
            bindings,
            project,
            &recipe.origin.path,
            recipe.special_byte_start.unwrap_or_default(),
        )
    } else {
        crate::recipe_expression::recover_items(&sources, &expressions, bindings)
    };
    if let Some(stack) = recovered
        .first()
        .and_then(|value| lower_recovered_stack(value, bindings))
    {
        return Some(RecipeSpecialSpec::ItemStack { value: stack });
    }
    push_blocker(
        blockers,
        RecipeLoweringBlockerKind::Special,
        format!("unsupported special payload: {source}"),
    );
    None
}

fn lower_recipe_special_value(
    recipe: &RecipeDefinition,
    blockers: &mut Vec<RecipeLoweringBlocker>,
) -> Option<i64> {
    recipe.special_value.as_ref()?;
    if let Some(value) = recipe.special_value_exact {
        return Some(value);
    }
    push_blocker(
        blockers,
        RecipeLoweringBlockerKind::Special,
        format!(
            "non-constant special value: {}",
            recipe
                .special_value
                .as_deref()
                .unwrap_or("<missing source>")
        ),
    );
    None
}

fn lower_recipe_policy(
    recipe: &RecipeDefinition,
    bindings: &JavaBindings,
    blockers: &mut Vec<RecipeLoweringBlocker>,
) -> RecipePolicySpec {
    let mut policy = RecipePolicySpec::default();
    for modifier in &recipe.modifiers {
        match modifier.name.as_str() {
            "requiresCleanRoom" if modifier.arguments.is_empty() => {
                policy.conditions.push(RecipeCondition::CleanRoom);
            }
            "requiresLowGravity" if modifier.arguments.is_empty() => {
                policy.conditions.push(RecipeCondition::LowGravity);
            }
            "requireMods" => {
                policy.required_mods.extend(
                    modifier
                        .arguments
                        .iter()
                        .map(|argument| ContentId::new(bindings.resolve_reference(argument))),
                );
            }
            "fake" if modifier.arguments.is_empty() => {
                policy.behaviors.push(RecipeBehavior::Fake);
            }
            "hidden" if modifier.arguments.is_empty() => {
                policy.behaviors.push(RecipeBehavior::Hidden);
            }
            "disabled" if modifier.arguments.is_empty() => {
                policy.behaviors.push(RecipeBehavior::Disabled);
            }
            "ignoreCollision" if modifier.arguments.is_empty() => {
                policy.behaviors.push(RecipeBehavior::IgnoreCollision);
            }
            "nbtSensitive" if modifier.arguments.is_empty() => {
                policy.behaviors.push(RecipeBehavior::NbtSensitive);
            }
            "noBuffer" if modifier.arguments.is_empty() => {
                policy.behaviors.push(RecipeBehavior::NoBuffer);
            }
            name => {
                push_blocker(
                    blockers,
                    RecipeLoweringBlockerKind::Modifiers,
                    format!("unsupported recipe modifier: {name}"),
                );
            }
        }
    }
    policy
}

fn lower_inputs(
    recipe: &RecipeDefinition,
    bindings: &JavaBindings,
    blockers: &mut Vec<RecipeLoweringBlocker>,
) -> Vec<ItemInputSpec> {
    recipe
        .item_input_semantics
        .iter()
        .enumerate()
        .filter_map(|(index, recovered)| {
            lower_recovered_stack(recovered, bindings)
                .map(ItemInputSpec::Stack)
                .or_else(|| {
                    push_blocker(
                        blockers,
                        RecipeLoweringBlockerKind::ItemInput,
                        format!("input {index}: {}", recovered.source),
                    );
                    None
                })
        })
        .collect()
}

fn lower_integrated_circuits(
    recipe: &RecipeDefinition,
    blockers: &mut Vec<RecipeLoweringBlocker>,
) -> Vec<ItemInputSpec> {
    recipe
        .integrated_circuits
        .iter()
        .filter_map(|recovered| {
            if let Some(NumericSemantic::ExactValue { value }) = recovered.semantic.as_ref() {
                Some(ItemInputSpec::Stack(ItemStackSpec {
                    selector: ItemSelector::Named {
                        id: ContentId::new("gregtech.api.enums.ItemList.Circuit_Integrated"),
                        variant: Some(*value),
                    },
                    amount: 0,
                }))
            } else {
                push_blocker(
                    blockers,
                    RecipeLoweringBlockerKind::ItemInput,
                    format!(
                        "dynamic integrated circuit configuration: {}",
                        recovered.source
                    ),
                );
                None
            }
        })
        .collect()
}

fn lower_outputs(
    recipe: &RecipeDefinition,
    bindings: &JavaBindings,
    blockers: &mut Vec<RecipeLoweringBlocker>,
) -> Vec<ItemStackSpec> {
    recipe
        .item_output_semantics
        .iter()
        .enumerate()
        .filter_map(|(index, recovered)| {
            lower_recovered_stack(recovered, bindings).or_else(|| {
                push_blocker(
                    blockers,
                    RecipeLoweringBlockerKind::ItemOutput,
                    format!("output {index}: {}", recovered.source),
                );
                None
            })
        })
        .collect()
}

fn lower_recovered_stack(
    recovered: &RecoveredExpression<ItemSemantic>,
    bindings: &JavaBindings,
) -> Option<ItemStackSpec> {
    lower_item_semantic(recovered.semantic.as_ref()?, bindings)
}

fn lower_item_semantic(semantic: &ItemSemantic, bindings: &JavaBindings) -> Option<ItemStackSpec> {
    match semantic {
        ItemSemantic::OreDict {
            prefix,
            material,
            amount,
        } => Some(ItemStackSpec {
            selector: ItemSelector::MaterialForm {
                material: ContentId::new(resolved_reference_name(material, bindings)?),
                form: ContentId::new(prefix.clone()),
            },
            amount: amount_u64(amount)?,
        }),
        ItemSemantic::OreDictQuery { key, amount } => Some(ItemStackSpec {
            selector: ItemSelector::OreKey {
                key: ContentId::new(simple_string_literal(key)?),
            },
            amount: amount_u64(amount)?,
        }),
        ItemSemantic::OreDictUnify {
            inner: Some(inner), ..
        }
        | ItemSemantic::InlinedMethod { result: inner, .. } => lower_item_semantic(inner, bindings),
        ItemSemantic::ItemList {
            item,
            accessor,
            arguments,
        } if accessor == "get" => Some(ItemStackSpec {
            selector: ItemSelector::Named {
                id: ContentId::new(item.clone()),
                variant: None,
            },
            amount: arguments.first().map_or(Some(1), amount_u64)?,
        }),
        ItemSemantic::ItemStack { item, amount, meta } => Some(ItemStackSpec {
            selector: ItemSelector::Named {
                id: ContentId::new(resolved_reference_name(item, bindings)?),
                variant: match meta.as_ref() {
                    None => None,
                    Some(meta) => Some(literal_i64(meta)?),
                },
            },
            amount: amount.as_ref().map_or(Some(1), amount_u64)?,
        }),
        ItemSemantic::CopyAmount {
            amount,
            meta: None,
            inner: Some(inner),
            ..
        } => {
            let mut stack = lower_item_semantic(inner, bindings)?;
            stack.amount = amount_u64(amount)?;
            Some(stack)
        }
        ItemSemantic::OreDictStack { arguments } => Some(ItemStackSpec {
            selector: ItemSelector::OreKey {
                key: ContentId::new(simple_string_literal(arguments.first()?)?),
            },
            amount: arguments.get(1).map_or(Some(1), amount_u64)?,
        }),
        _ => None,
    }
}

fn lower_fluid_inputs(
    recovered: &[RecoveredExpression<FluidSemantic>],
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
    blockers: &mut Vec<RecipeLoweringBlocker>,
) -> Vec<FluidInputSpec> {
    recovered
        .iter()
        .enumerate()
        .filter_map(|(index, recovered)| {
            lower_substitute_fluid_input(recovered, bindings, project)
                .or_else(|| {
                    recovered
                        .semantic
                        .as_ref()
                        .and_then(|semantic| lower_fluid_semantic(semantic, bindings, project))
                        .map(FluidInputSpec::Stack)
                })
                .or_else(|| {
                    push_blocker(
                        blockers,
                        RecipeLoweringBlockerKind::Fluids,
                        format!("fluid input {index}: {}", recovered.source),
                    );
                    None
                })
        })
        .collect()
}

fn lower_fluid_outputs(
    recovered: &[RecoveredExpression<FluidSemantic>],
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
    blockers: &mut Vec<RecipeLoweringBlocker>,
) -> Vec<FluidStackSpec> {
    recovered
        .iter()
        .enumerate()
        .filter_map(|(index, recovered)| {
            recovered
                .semantic
                .as_ref()
                .and_then(|semantic| lower_fluid_semantic(semantic, bindings, project))
                .or_else(|| {
                    push_blocker(
                        blockers,
                        RecipeLoweringBlockerKind::Fluids,
                        format!("fluid output {index}: {}", recovered.source),
                    );
                    None
                })
        })
        .collect()
}

fn lower_substitute_fluid_input(
    recovered: &RecoveredExpression<FluidSemantic>,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
) -> Option<FluidInputSpec> {
    let FluidSemantic::LinkedMethod {
        method_id,
        arguments,
        ..
    } = recovered.semantic.as_ref()?
    else {
        return None;
    };
    if method_id != "gregtech.api.objects.SubstituteFluidStack#soldering(long)"
        || arguments.len() != 1
    {
        return None;
    }
    let base = scalar_amount_u64(arguments.first()?, bindings, project)?;
    let choice = |material: &str, multiplier: u64| -> Option<FluidStackSpec> {
        Some(FluidStackSpec {
            selector: FluidSelector::MaterialForm {
                material: ContentId::new(format!("gregtech.api.enums.Materials.{material}")),
                form: ContentId::new("molten"),
            },
            amount: base.checked_mul(multiplier)?,
        })
    };
    Some(FluidInputSpec::Alternatives {
        choices: vec![
            choice("SolderingAlloy", 1)?,
            choice("Tin", 2)?,
            choice("Lead", 4)?,
        ],
    })
}

fn lower_fluid_semantic(
    semantic: &FluidSemantic,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
) -> Option<FluidStackSpec> {
    match semantic {
        FluidSemantic::FluidAccessor {
            source_ref,
            accessor,
            amount,
        } => Some(FluidStackSpec {
            selector: FluidSelector::MaterialForm {
                material: ContentId::new(source_ref.clone()),
                form: ContentId::new(fluid_form(accessor)?),
            },
            amount: scalar_amount_u64(amount, bindings, project)?,
        }),
        FluidSemantic::FluidStack { fluid, amount } => Some(FluidStackSpec {
            selector: fluid_stack_selector(fluid, bindings)?,
            amount: scalar_amount_u64(amount, bindings, project)?,
        }),
        FluidSemantic::RegistryFluid {
            provider,
            arguments,
        } if provider.ends_with("FluidRegistry.getFluidStack") && arguments.len() >= 2 => {
            Some(FluidStackSpec {
                selector: FluidSelector::Named {
                    id: ContentId::new(simple_string_literal(arguments.first()?)?),
                },
                amount: scalar_amount_u64(arguments.get(1)?, bindings, project)?,
            })
        }
        FluidSemantic::LinkedMethod {
            method_id,
            arguments,
            ..
        } => lower_gt_mod_handler_fluid(method_id, arguments, bindings, project),
        FluidSemantic::RegistryFluid { .. }
        | FluidSemantic::LinkedValue { .. }
        | FluidSemantic::TypedExpression { .. } => None,
    }
}

fn fluid_stack_selector(fluid: &JavaExpression, bindings: &JavaBindings) -> Option<FluidSelector> {
    if let Some(reference) = resolved_reference_name(fluid, bindings) {
        let id = match reference.as_str() {
            "net.minecraftforge.fluids.FluidRegistry.WATER" | "FluidRegistry.WATER" => "water",
            "net.minecraftforge.fluids.FluidRegistry.LAVA" | "FluidRegistry.LAVA" => "lava",
            _ => reference.as_str(),
        };
        return Some(FluidSelector::Named {
            id: ContentId::new(id),
        });
    }
    let JavaExpression::Call {
        receiver: Some(receiver),
        method,
        arguments,
    } = fluid
    else {
        return None;
    };
    if method != "getFluid" || arguments.len() != 1 {
        return None;
    }
    let owner = resolved_reference_name(receiver, bindings)?;
    if !owner.ends_with("FluidRegistry") {
        return None;
    }
    Some(FluidSelector::Named {
        id: ContentId::new(simple_string_literal(arguments.first()?)?),
    })
}

fn lower_gt_mod_handler_fluid(
    method_id: &str,
    arguments: &[JavaExpression],
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
) -> Option<FluidStackSpec> {
    let amount = scalar_amount_u64(arguments.first()?, bindings, project)?;
    let selector = match method_id {
        "gregtech.api.util.GTModHandler#getWater(long)" => FluidSelector::MaterialForm {
            material: ContentId::new("gregtech.api.enums.Materials.Water"),
            form: ContentId::new("fluid"),
        },
        "gregtech.api.util.GTModHandler#getLava(long)" => FluidSelector::MaterialForm {
            material: ContentId::new("gregtech.api.enums.Materials.Lava"),
            form: ContentId::new("fluid"),
        },
        "gregtech.api.util.GTModHandler#getSteam(long)" => FluidSelector::MaterialForm {
            material: ContentId::new("gregtech.api.enums.Materials.Steam"),
            form: ContentId::new("gas"),
        },
        "gregtech.api.util.GTModHandler#getMilk(long)" => FluidSelector::MaterialForm {
            material: ContentId::new("gregtech.api.enums.Materials.Milk"),
            form: ContentId::new("fluid"),
        },
        "gregtech.api.util.GTModHandler#getDistilledWater(long)" => FluidSelector::Named {
            id: ContentId::new("ic2distilledwater"),
        },
        "gregtech.api.util.GTModHandler#getIC2Coolant(long)" => FluidSelector::Named {
            id: ContentId::new("ic2coolant"),
        },
        "gregtech.api.util.GTModHandler#getLiquidDNA(long)" => FluidSelector::Named {
            id: ContentId::new("liquiddna"),
        },
        "gregtech.api.util.GTModHandler#getHotWater(int)" => FluidSelector::Named {
            id: ContentId::new("ic2hotwater"),
        },
        "gregtech.api.util.GTModHandler#getPahoehoeLava(int)" => FluidSelector::Named {
            id: ContentId::new("ic2pahoehoelava"),
        },
        "gregtech.api.util.GTModHandler#getSuperHeatedSteam(int)" => FluidSelector::Named {
            id: ContentId::new("ic2superheatedsteam"),
        },
        "gregtech.api.util.GTModHandler#getHotCoolant(int)" => FluidSelector::Named {
            id: ContentId::new("ic2hotcoolant"),
        },
        _ => return None,
    };
    Some(FluidStackSpec { selector, amount })
}

fn fluid_form(accessor: &str) -> Option<&'static str> {
    match accessor {
        "getFluid" | "getFluidStack" => Some("fluid"),
        "getMolten" => Some("molten"),
        "getGas" => Some("gas"),
        "getFluidOrGas" => Some("fluid_or_gas"),
        "getPlasma" => Some("plasma"),
        _ => None,
    }
}

fn scalar_amount_u64(
    expression: &JavaExpression,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
) -> Option<u64> {
    let value = project.map_or_else(
        || literal_i64(expression),
        |project| scalar_i64(expression, bindings, project),
    )?;
    u64::try_from(value).ok()
}

fn amount_u64(expression: &JavaExpression) -> Option<u64> {
    u64::try_from(literal_i64(expression)?).ok()
}

fn simple_string_literal(expression: &JavaExpression) -> Option<String> {
    let JavaExpression::Literal(value) = expression else {
        return None;
    };
    let value = value.strip_prefix('"')?.strip_suffix('"')?;
    (!value.contains('\\')).then(|| value.to_owned())
}

fn push_blocker(
    blockers: &mut Vec<RecipeLoweringBlocker>,
    kind: RecipeLoweringBlockerKind,
    detail: impl Into<String>,
) {
    blockers.push(RecipeLoweringBlocker {
        kind,
        detail: detail.into(),
    });
}

#[cfg(test)]
mod tests {
    use minecraft_redox_core::content::{
        FluidInputSpec, FluidSelector, ItemInputSpec, ItemSelector, RecipeBehavior,
        RecipeCondition, RecipeMetadataValue,
    };
    use minecraft_redox_java::{JavaProjectIndex, analyze_source};

    use super::{RecipeLoweringBlockerKind, lower_recipe};
    use crate::recover;

    #[test]
    fn lowers_primitive_metadata_and_recipe_category() -> anyhow::Result<()> {
        let source = r#"
            import static gregtech.api.recipe.RecipeMaps.compressorRecipes;
            import gregtech.api.enums.GTValues;
            import gregtech.api.enums.Materials;
            class RecipeMetadataKey<T> {}
            class RecipeCategory { static RecipeCategory SPECIAL; }
            class Keys {
                static RecipeMetadataKey<Integer> HEAT;
                static RecipeMetadataKey<Boolean> FLAG;
                static RecipeMetadataKey<String> NAME;
                static RecipeMetadataKey<Materials> MATERIAL;
            }
            class Recipes { static final int COAL = 7; void run() {
                GTValues.RA.stdBuilder()
                    .metadata(Keys.HEAT, 1700)
                    .metadata(Keys.FLAG, true)
                    .metadata(Keys.NAME, "demo")
                    .metadata(Keys.MATERIAL, Materials.Iron)
                    .metadata(Keys.HEAT, COAL)
                    .recipeCategory(RecipeCategory.SPECIAL)
                    .duration(20)
                    .eut(2)
                    .addTo(compressorRecipes);
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
        let lowered = super::lower_recipe_with_project(recipe, &analysis.bindings, &project)
            .map_err(|failure| anyhow::anyhow!("lowering failed: {failure:?}"))?;
        assert_eq!(lowered.metadata.len(), 5);
        assert!(matches!(
            lowered.metadata[0].value,
            RecipeMetadataValue::Integer { value: 1700 }
        ));
        assert!(matches!(
            lowered.metadata[1].value,
            RecipeMetadataValue::Boolean { value: true }
        ));
        assert!(matches!(
            &lowered.metadata[2].value,
            RecipeMetadataValue::Text { value } if value == "demo"
        ));
        assert!(matches!(
            &lowered.metadata[3].value,
            RecipeMetadataValue::ContentRef { id }
                if id.as_str() == "gregtech.api.enums.Materials.Iron"
        ));
        assert!(matches!(
            lowered.metadata[4].value,
            RecipeMetadataValue::Integer { value: 7 }
        ));
        assert_eq!(
            lowered
                .category
                .as_ref()
                .map(minecraft_redox_core::content::ContentId::as_str),
            Some("RecipeCategory.SPECIAL")
        );
        Ok(())
    }

    #[test]
    fn infers_metadata_payload_type_from_static_imported_key_field() -> anyhow::Result<()> {
        let constants = analyze_source(
            "GTRecipeConstants.java",
            "gt5",
            None,
            r"
                package gregtech.api.util;
                class RecipeMetadataKey<T> {}
                class GTRecipeConstants {
                    static final RecipeMetadataKey<Integer> COIL_HEAT = null;
                }
            ",
        )?;
        let analysis = analyze_source(
            "Recipes.java",
            "gt5",
            None,
            r"
                package demo;
                import static gregtech.api.util.GTRecipeConstants.COIL_HEAT;
                class GTValues { static final RA RA = new RA(); static class RA { Builder stdBuilder() { return null; } } }
                class Builder { Builder metadata(Object k, Object v) { return this; } Builder duration(int v) { return this; } Builder eut(int v) { return this; } void addTo(Object m) {} }
                class Recipes { void run() {
                    GTValues.RA.stdBuilder()
                        .metadata(COIL_HEAT, 4000)
                        .duration(20)
                        .eut(2)
                        .addTo(blastFurnaceRecipes);
                }}
            ",
        )?;
        let mut project = JavaProjectIndex::default();
        project.add_analysis(&constants);
        project.add_analysis(&analysis);
        let recovery = crate::recover_with_project(&analysis, &project);
        let recipe = recovery
            .recipes
            .first()
            .ok_or_else(|| anyhow::anyhow!("recipe missing"))?;
        let lowered = super::lower_recipe_with_project(recipe, &analysis.bindings, &project)
            .map_err(|failure| anyhow::anyhow!("lowering failed: {failure:?}"))?;
        assert!(matches!(
            lowered.metadata[0].value,
            RecipeMetadataValue::Integer { value: 4000 }
        ));
        Ok(())
    }

    #[test]
    fn lowers_item_stack_array_metadata() -> anyhow::Result<()> {
        let source = r"
            import static gregtech.api.recipe.RecipeMaps.compressorRecipes;
            import gregtech.api.enums.GTValues;
            import net.minecraft.item.ItemStack;
            class RecipeMetadataKey<T> {}
            class ItemList {
                static final Entry A = new Entry();
                static final Entry B = new Entry();
                static class Entry { ItemStack get(long amount) { return null; } }
            }
            class Keys { static RecipeMetadataKey<ItemStack[]> SHELL; }
            class Recipes { void run() {
                GTValues.RA.stdBuilder()
                    .metadata(Keys.SHELL, new ItemStack[] { ItemList.A.get(1), ItemList.B.get(2) })
                    .duration(20)
                    .eut(2)
                    .addTo(compressorRecipes);
            }}
        ";
        let analysis = analyze_source("Recipes.java", "gt5", None, source)?;
        let mut project = JavaProjectIndex::default();
        project.add_analysis(&analysis);
        let recovery = crate::recover_with_project(&analysis, &project);
        let recipe = recovery
            .recipes
            .first()
            .ok_or_else(|| anyhow::anyhow!("recipe missing"))?;
        let lowered = super::lower_recipe_with_project(recipe, &analysis.bindings, &project)
            .map_err(|failure| anyhow::anyhow!("lowering failed: {failure:?}"))?;
        let RecipeMetadataValue::ItemStacks { values } = &lowered.metadata[0].value else {
            return Err(anyhow::anyhow!("metadata was not an item stack array"));
        };
        assert_eq!(values.len(), 2);
        assert_eq!(values[0].amount, 1);
        assert_eq!(values[1].amount, 2);
        Ok(())
    }

    #[test]
    fn lowers_item_stack_integer_pair_array_metadata() -> anyhow::Result<()> {
        let source = r"
            import static gregtech.api.recipe.RecipeMaps.compressorRecipes;
            import gregtech.api.enums.GTValues;
            import net.minecraft.item.ItemStack;
            class RecipeMetadataKey<T> {}
            class Pair<A, B> { static <A, B> Pair<A, B> of(A a, B b) { return null; } }
            class ItemList {
                static final Entry A = new Entry();
                static class Entry { ItemStack get(long amount) { return null; } }
            }
            class Keys { static RecipeMetadataKey<Pair<ItemStack, Integer>[]> FUEL; }
            class Recipes { void run() {
                GTValues.RA.stdBuilder()
                    .metadata(Keys.FUEL, new Pair[] { Pair.of(ItemList.A.get(3), 7) })
                    .duration(20)
                    .eut(2)
                    .addTo(compressorRecipes);
            }}
        ";
        let analysis = analyze_source("Recipes.java", "gt5", None, source)?;
        let mut project = JavaProjectIndex::default();
        project.add_analysis(&analysis);
        let recovery = crate::recover_with_project(&analysis, &project);
        let recipe = recovery
            .recipes
            .first()
            .ok_or_else(|| anyhow::anyhow!("recipe missing"))?;
        let lowered = super::lower_recipe_with_project(recipe, &analysis.bindings, &project)
            .map_err(|failure| anyhow::anyhow!("lowering failed: {failure:?}"))?;
        let RecipeMetadataValue::ItemStackIntegerPairs { values } = &lowered.metadata[0].value
        else {
            return Err(anyhow::anyhow!(
                "metadata was not an item/integer pair array"
            ));
        };
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].item.amount, 3);
        assert_eq!(values[0].value, 7);
        Ok(())
    }

    #[test]
    fn lowers_decimal_and_decimal_triple_metadata() -> anyhow::Result<()> {
        let source = r"
            import static gregtech.api.recipe.RecipeMaps.compressorRecipes;
            import gregtech.api.enums.GTValues;
            class RecipeMetadataKey<T> {}
            class Triple<A, B, C> { static <A, B, C> Triple<A, B, C> of(A a, B b, C c) { return null; } }
            class Keys {
                static RecipeMetadataKey<Float> FACTOR;
                static RecipeMetadataKey<Triple<Double, Double, Double>> MODIFIERS;
            }
            class Recipes { void run() {
                GTValues.RA.stdBuilder()
                    .metadata(Keys.FACTOR, 0.0f)
                    .metadata(Keys.MODIFIERS, Triple.of(0.1d, 0.80d, 1.1d))
                    .duration(20)
                    .eut(2)
                    .addTo(compressorRecipes);
            }}
        ";
        let analysis = analyze_source("Recipes.java", "gt5", None, source)?;
        let mut project = JavaProjectIndex::default();
        project.add_analysis(&analysis);
        let recovery = crate::recover_with_project(&analysis, &project);
        let recipe = recovery
            .recipes
            .first()
            .ok_or_else(|| anyhow::anyhow!("recipe missing"))?;
        let lowered = super::lower_recipe_with_project(recipe, &analysis.bindings, &project)
            .map_err(|failure| anyhow::anyhow!("lowering failed: {failure:?}"))?;
        assert!(matches!(
            lowered.metadata[0].value,
            RecipeMetadataValue::Decimal {
                value: minecraft_redox_core::content::FixedDecimal {
                    coefficient: 0,
                    decimal_places: 0
                }
            }
        ));
        assert!(matches!(
            lowered.metadata[1].value,
            RecipeMetadataValue::DecimalTriple {
                first: minecraft_redox_core::content::FixedDecimal {
                    coefficient: 1,
                    decimal_places: 1
                },
                second: minecraft_redox_core::content::FixedDecimal {
                    coefficient: 8,
                    decimal_places: 1
                },
                third: minecraft_redox_core::content::FixedDecimal {
                    coefficient: 11,
                    decimal_places: 1
                }
            }
        ));
        Ok(())
    }

    #[test]
    fn lowers_scanning_and_sievert_metadata() -> anyhow::Result<()> {
        let source = r"
            import static gregtech.api.recipe.RecipeMaps.compressorRecipes;
            import gregtech.api.enums.GTValues;
            class RecipeMetadataKey<T> {}
            class Scanning { Scanning(int time, long voltage) {} }
            class Sievert { Sievert(int value, boolean exact) {} }
            class Keys {
                static RecipeMetadataKey<Scanning> SCAN;
                static RecipeMetadataKey<Sievert> SIEVERT;
            }
            class Recipes { void run() {
                GTValues.RA.stdBuilder()
                    .metadata(Keys.SCAN, new Scanning(2 * 20, 128L))
                    .metadata(Keys.SIEVERT, new Sievert(100, true))
                    .duration(20)
                    .eut(2)
                    .addTo(compressorRecipes);
            }}
        ";
        let analysis = analyze_source("Recipes.java", "gt5", None, source)?;
        let mut project = JavaProjectIndex::default();
        project.add_analysis(&analysis);
        let recovery = crate::recover_with_project(&analysis, &project);
        let recipe = recovery
            .recipes
            .first()
            .ok_or_else(|| anyhow::anyhow!("recipe missing"))?;
        let lowered = super::lower_recipe_with_project(recipe, &analysis.bindings, &project)
            .map_err(|failure| anyhow::anyhow!("lowering failed: {failure:?}"))?;
        assert!(matches!(
            lowered.metadata[0].value,
            RecipeMetadataValue::Scanning {
                duration_ticks: 40,
                voltage: 128
            }
        ));
        assert!(matches!(
            lowered.metadata[1].value,
            RecipeMetadataValue::Sievert {
                value: 100,
                exact: true
            }
        ));
        Ok(())
    }

    #[test]
    fn lowers_item_stack_metadata_through_item_semantics() -> anyhow::Result<()> {
        let source = r"
            import static gregtech.api.recipe.RecipeMaps.compressorRecipes;
            import gregtech.api.enums.GTValues;
            import net.minecraft.item.ItemStack;
            class RecipeMetadataKey<T> {}
            class ItemList {
                static ItemList Research;
                ItemStack get(long amount) { return null; }
            }
            class Keys { static RecipeMetadataKey<ItemStack> RESEARCH; }
            class Recipes { void run() {
                GTValues.RA.stdBuilder()
                    .metadata(Keys.RESEARCH, ItemList.Research.get(1L))
                    .duration(20)
                    .eut(2)
                    .addTo(compressorRecipes);
            }}
        ";
        let analysis = analyze_source("Recipes.java", "gt5", None, source)?;
        let mut project = JavaProjectIndex::default();
        project.add_analysis(&analysis);
        let recovery = crate::recover_with_project(&analysis, &project);
        let recipe = recovery
            .recipes
            .first()
            .ok_or_else(|| anyhow::anyhow!("recipe missing"))?;
        let lowered = super::lower_recipe_with_project(recipe, &analysis.bindings, &project)
            .map_err(|failure| anyhow::anyhow!("lowering failed: {failure:?}"))?;
        assert!(matches!(
            &lowered.metadata[0].value,
            RecipeMetadataValue::ItemStack { value }
                if value.amount == 1
                    && matches!(&value.selector, ItemSelector::Named { id, variant: None }
                        if id.as_str() == "ItemList.Research")
        ));
        Ok(())
    }

    #[test]
    fn lowers_item_special_and_exact_special_value() -> anyhow::Result<()> {
        let source = r"
            import static gregtech.api.recipe.RecipeMaps.compressorRecipes;
            import gregtech.api.enums.GTValues;
            import net.minecraft.item.ItemStack;
            class ItemList {
                static ItemList DataStick;
                ItemStack get(long amount) { return null; }
            }
            class Recipes { void run() {
                GTValues.RA.stdBuilder()
                    .special(ItemList.DataStick.get(1L))
                    .specialValue(42)
                    .duration(20)
                    .eut(2)
                    .addTo(compressorRecipes);
            }}
        ";
        let analysis = analyze_source("Recipes.java", "gt5", None, source)?;
        let mut project = JavaProjectIndex::default();
        project.add_analysis(&analysis);
        let recovery = crate::recover_with_project(&analysis, &project);
        let recipe = recovery
            .recipes
            .first()
            .ok_or_else(|| anyhow::anyhow!("recipe missing"))?;
        let lowered = super::lower_recipe_with_project(recipe, &analysis.bindings, &project)
            .map_err(|failure| anyhow::anyhow!("lowering failed: {failure:?}"))?;
        assert!(matches!(
            &lowered.special,
            Some(minecraft_redox_core::content::RecipeSpecialSpec::ItemStack { value })
                if value.amount == 1
                    && matches!(&value.selector, ItemSelector::Named { id, variant: None }
                        if id.as_str() == "ItemList.DataStick")
        ));
        assert_eq!(lowered.special_value, Some(42));
        Ok(())
    }

    #[test]
    fn blocks_custom_or_runtime_special_payloads() -> anyhow::Result<()> {
        let source = r"
            import static gregtech.api.recipe.RecipeMaps.compressorRecipes;
            import gregtech.api.enums.GTValues;
            class CustomSpecial {}
            class Recipes { void run(int value) {
                GTValues.RA.stdBuilder()
                    .special(new CustomSpecial())
                    .specialValue(value)
                    .duration(20)
                    .eut(2)
                    .addTo(compressorRecipes);
            }}
        ";
        let analysis = analyze_source("Recipes.java", "gt5", None, source)?;
        let mut project = JavaProjectIndex::default();
        project.add_analysis(&analysis);
        let recovery = crate::recover_with_project(&analysis, &project);
        let recipe = recovery
            .recipes
            .first()
            .ok_or_else(|| anyhow::anyhow!("recipe missing"))?;
        let Err(failure) = super::lower_recipe_with_project(recipe, &analysis.bindings, &project)
        else {
            return Err(anyhow::anyhow!("custom special unexpectedly lowered"));
        };
        assert!(
            failure
                .blockers
                .iter()
                .any(|blocker| blocker.kind == RecipeLoweringBlockerKind::Special)
        );
        Ok(())
    }

    #[test]
    fn blocks_runtime_numeric_metadata() -> anyhow::Result<()> {
        let source = r"
            import static gregtech.api.recipe.RecipeMaps.compressorRecipes;
            import gregtech.api.enums.GTValues;
            class RecipeMetadataKey<T> {}
            class Keys { static RecipeMetadataKey<Integer> HEAT; }
            class Recipes { void run(int heat) {
                GTValues.RA.stdBuilder()
                    .metadata(Keys.HEAT, heat)
                    .duration(20)
                    .eut(2)
                    .addTo(compressorRecipes);
            }}
        ";
        let analysis = analyze_source("Recipes.java", "gt5", None, source)?;
        let mut project = JavaProjectIndex::default();
        project.add_analysis(&analysis);
        let recovery = crate::recover_with_project(&analysis, &project);
        let recipe = recovery
            .recipes
            .first()
            .ok_or_else(|| anyhow::anyhow!("recipe missing"))?;
        let Err(failure) = super::lower_recipe_with_project(recipe, &analysis.bindings, &project)
        else {
            return Err(anyhow::anyhow!("runtime metadata unexpectedly lowered"));
        };
        assert!(
            failure
                .blockers
                .iter()
                .any(|blocker| blocker.kind == RecipeLoweringBlockerKind::Metadata)
        );
        Ok(())
    }

    #[test]
    fn lowers_literal_compressor_recipe() -> anyhow::Result<()> {
        let source = r"
            import static gregtech.api.recipe.RecipeMaps.compressorRecipes;
            import static gregtech.api.util.GTRecipeBuilder.SECONDS;
            import gregtech.api.enums.GTValues;
            import gregtech.api.enums.Materials;
            import gregtech.api.enums.OrePrefixes;
            import gregtech.api.util.GTOreDictUnificator;
            import net.minecraft.init.Blocks;
            import net.minecraft.item.ItemStack;
            class Recipes { void run() {
                GTValues.RA.stdBuilder()
                    .itemInputs(GTOreDictUnificator.get(OrePrefixes.dust, Materials.Netherrack, 4))
                    .itemOutputs(GTOreDictUnificator.get(new ItemStack(Blocks.netherrack, 3)))
                    .duration(5 * SECONDS)
                    .eut(2)
                    .addTo(compressorRecipes);
            }}
        ";
        let analysis = analyze_source("Recipes.java", "gt5", None, source)?;
        let recovery = recover(&analysis);
        let recipe = recovery
            .recipes
            .first()
            .ok_or_else(|| anyhow::anyhow!("recipe missing"))?;
        let lowered = lower_recipe(recipe, &analysis.bindings)
            .map_err(|failure| anyhow::anyhow!("lowering failed: {failure:?}"))?;

        assert_eq!(
            lowered.recipe_map.as_str(),
            "gregtech.api.recipe.RecipeMaps.compressorRecipes"
        );
        assert_eq!(lowered.duration_ticks, 100);
        assert_eq!(lowered.eut, 2);
        assert_eq!(lowered.item_inputs.len(), 1);
        let ItemInputSpec::Stack(input) = &lowered.item_inputs[0] else {
            return Err(anyhow::anyhow!("input was not a stack"));
        };
        assert_eq!(input.amount, 4);
        assert!(matches!(
            &input.selector,
            ItemSelector::MaterialForm { material, form }
                if material.as_str() == "gregtech.api.enums.Materials.Netherrack"
                    && form.as_str() == "gregtech.api.enums.OrePrefixes.dust"
        ));
        assert_eq!(lowered.item_outputs.len(), 1);
        assert_eq!(lowered.item_outputs[0].amount, 3);
        assert!(matches!(
            &lowered.item_outputs[0].selector,
            ItemSelector::Named { id, variant: None }
                if id.as_str() == "net.minecraft.init.Blocks.netherrack"
        ));
        Ok(())
    }

    #[test]
    fn lowers_item_input_and_output_chances() -> anyhow::Result<()> {
        let source = r"
            import static gregtech.api.recipe.RecipeMaps.compressorRecipes;
            import gregtech.api.enums.GTValues;
            import net.minecraft.init.Blocks;
            import net.minecraft.item.ItemStack;
            class Recipes { void run() {
                GTValues.RA.stdBuilder()
                    .itemInputs(new ItemStack(Blocks.dirt))
                    .inputChances(2_500)
                    .itemOutputs(new ItemStack(Blocks.stone))
                    .outputChances(10_000)
                    .duration(20)
                    .eut(2)
                    .addTo(compressorRecipes);
            }}
        ";
        let analysis = analyze_source("Recipes.java", "gt5", None, source)?;
        let recovery = recover(&analysis);
        let recipe = recovery
            .recipes
            .first()
            .ok_or_else(|| anyhow::anyhow!("recipe missing"))?;
        let lowered = lower_recipe(recipe, &analysis.bindings)
            .map_err(|failure| anyhow::anyhow!("lowering failed: {failure:?}"))?;

        assert_eq!(lowered.item_input_consumption_chances.len(), 1);
        assert_eq!(lowered.item_input_consumption_chances[0].slot, 0);
        assert_eq!(
            lowered.item_input_consumption_chances[0].chance_10000,
            2_500
        );
        assert_eq!(lowered.item_output_production_chances.len(), 1);
        assert_eq!(lowered.item_output_production_chances[0].slot, 0);
        assert_eq!(
            lowered.item_output_production_chances[0].chance_10000,
            10_000
        );
        Ok(())
    }

    #[test]
    fn blocks_dynamic_item_output_chance() -> anyhow::Result<()> {
        let source = r"
            import static gregtech.api.recipe.RecipeMaps.compressorRecipes;
            import gregtech.api.enums.GTValues;
            import net.minecraft.init.Blocks;
            import net.minecraft.item.ItemStack;
            class Recipes { void run(int chance) {
                GTValues.RA.stdBuilder()
                    .itemOutputs(new ItemStack(Blocks.stone))
                    .outputChances(chance)
                    .duration(20)
                    .eut(2)
                    .addTo(compressorRecipes);
            }}
        ";
        let analysis = analyze_source("Recipes.java", "gt5", None, source)?;
        let recovery = recover(&analysis);
        let recipe = recovery
            .recipes
            .first()
            .ok_or_else(|| anyhow::anyhow!("recipe missing"))?;
        let Err(failure) = lower_recipe(recipe, &analysis.bindings) else {
            return Err(anyhow::anyhow!("dynamic chance lowered as static content"));
        };
        assert!(failure.blockers.iter().any(|blocker| {
            blocker.kind == RecipeLoweringBlockerKind::Chances
                && blocker
                    .detail
                    .contains("dynamic item output production chance")
        }));
        Ok(())
    }

    #[test]
    fn lowers_recipe_policy_modifiers() -> anyhow::Result<()> {
        let source = r"
            import static gregtech.api.recipe.RecipeMaps.compressorRecipes;
            import gregtech.api.enums.GTValues;
            import gregtech.api.enums.Mods;
            class Recipes { void run() {
                GTValues.RA.stdBuilder()
                    .requiresCleanRoom()
                    .requiresLowGravity()
                    .requireMods(Mods.Forestry, Mods.Railcraft)
                    .fake()
                    .hidden()
                    .disabled()
                    .ignoreCollision()
                    .nbtSensitive()
                    .noBuffer()
                    .duration(20)
                    .eut(2)
                    .addTo(compressorRecipes);
            }}
        ";
        let analysis = analyze_source("Recipes.java", "gt5", None, source)?;
        let recovery = recover(&analysis);
        let recipe = recovery
            .recipes
            .first()
            .ok_or_else(|| anyhow::anyhow!("recipe missing"))?;
        let lowered = lower_recipe(recipe, &analysis.bindings)
            .map_err(|failure| anyhow::anyhow!("lowering failed: {failure:?}"))?;

        assert_eq!(
            lowered.policy.conditions,
            vec![RecipeCondition::CleanRoom, RecipeCondition::LowGravity]
        );
        assert_eq!(
            lowered
                .policy
                .required_mods
                .iter()
                .map(minecraft_redox_core::content::ContentId::as_str)
                .collect::<Vec<_>>(),
            vec![
                "gregtech.api.enums.Mods.Forestry",
                "gregtech.api.enums.Mods.Railcraft"
            ]
        );
        assert!(lowered.policy.behaviors.contains(&RecipeBehavior::Fake));
        assert!(lowered.policy.behaviors.contains(&RecipeBehavior::Hidden));
        assert!(lowered.policy.behaviors.contains(&RecipeBehavior::Disabled));
        assert!(
            lowered
                .policy
                .behaviors
                .contains(&RecipeBehavior::IgnoreCollision)
        );
        assert!(
            lowered
                .policy
                .behaviors
                .contains(&RecipeBehavior::NbtSensitive)
        );
        assert!(lowered.policy.behaviors.contains(&RecipeBehavior::NoBuffer));
        Ok(())
    }

    #[test]
    fn lowers_unsafe_oversized_item_input_without_legacy_modifier() -> anyhow::Result<()> {
        let source = r"
            import static gregtech.api.recipe.RecipeMaps.compressorRecipes;
            import gregtech.api.enums.GTValues;
            import net.minecraft.init.Blocks;
            import net.minecraft.item.ItemStack;
            class Recipes { void run() {
                GTValues.RA.stdBuilder()
                    .itemInputsUnsafe(new ItemStack(Blocks.dirt, 128))
                    .duration(20)
                    .eut(2)
                    .addTo(compressorRecipes);
            }}
        ";
        let analysis = analyze_source("Recipes.java", "gt5", None, source)?;
        let recovery = recover(&analysis);
        let recipe = recovery
            .recipes
            .first()
            .ok_or_else(|| anyhow::anyhow!("recipe missing"))?;
        assert!(
            recipe
                .modifiers
                .iter()
                .all(|modifier| modifier.name != "itemInputsUnsafe")
        );
        let lowered = lower_recipe(recipe, &analysis.bindings)
            .map_err(|failure| anyhow::anyhow!("lowering failed: {failure:?}"))?;
        let ItemInputSpec::Stack(input) = &lowered.item_inputs[0] else {
            return Err(anyhow::anyhow!("unsafe input was not a stack"));
        };
        assert_eq!(input.amount, 128);
        Ok(())
    }

    #[test]
    fn lowers_integrated_circuit_as_non_consumed_input() -> anyhow::Result<()> {
        let source = r"
            import static gregtech.api.recipe.RecipeMaps.compressorRecipes;
            import gregtech.api.enums.GTValues;
            class Recipes { void run() {
                GTValues.RA.stdBuilder()
                    .circuit(7)
                    .duration(20)
                    .eut(2)
                    .addTo(compressorRecipes);
            }}
        ";
        let analysis = analyze_source("Recipes.java", "gt5", None, source)?;
        let recovery = recover(&analysis);
        let recipe = recovery
            .recipes
            .first()
            .ok_or_else(|| anyhow::anyhow!("recipe missing"))?;
        assert_eq!(recipe.integrated_circuits.len(), 1);
        assert!(
            recipe
                .modifiers
                .iter()
                .all(|modifier| modifier.name != "circuit")
        );

        let lowered = lower_recipe(recipe, &analysis.bindings)
            .map_err(|failure| anyhow::anyhow!("lowering failed: {failure:?}"))?;
        assert_eq!(lowered.item_inputs.len(), 1);
        let ItemInputSpec::Stack(input) = &lowered.item_inputs[0] else {
            return Err(anyhow::anyhow!("circuit input was not a stack"));
        };
        assert_eq!(input.amount, 0);
        assert!(matches!(
            &input.selector,
            ItemSelector::Named { id, variant: Some(7) }
                if id.as_str() == "gregtech.api.enums.ItemList.Circuit_Integrated"
        ));
        Ok(())
    }

    #[test]
    fn blocks_dynamic_integrated_circuit_configuration() -> anyhow::Result<()> {
        let source = r"
            import static gregtech.api.recipe.RecipeMaps.compressorRecipes;
            import gregtech.api.enums.GTValues;
            class Recipes { void run(int configuration) {
                GTValues.RA.stdBuilder()
                    .circuit(configuration)
                    .duration(20)
                    .eut(2)
                    .addTo(compressorRecipes);
            }}
        ";
        let analysis = analyze_source("Recipes.java", "gt5", None, source)?;
        let recovery = recover(&analysis);
        let recipe = recovery
            .recipes
            .first()
            .ok_or_else(|| anyhow::anyhow!("recipe missing"))?;
        let Err(failure) = lower_recipe(recipe, &analysis.bindings) else {
            return Err(anyhow::anyhow!("dynamic circuit lowered as static content"));
        };
        assert!(failure.blockers.iter().any(|blocker| {
            blocker.kind == RecipeLoweringBlockerKind::ItemInput
                && blocker
                    .detail
                    .contains("dynamic integrated circuit configuration")
        }));
        Ok(())
    }

    #[test]
    fn lowers_soldering_substitute_as_one_fluid_alternative_slot() -> anyhow::Result<()> {
        let substitute = analyze_source(
            "SubstituteFluidStack.java",
            "gt5",
            None,
            r"
                package gregtech.api.objects;
                class SubstituteFluidStack {
                    static SubstituteFluidStack soldering(long amount) { return null; }
                }
            ",
        )?;
        let source = r"
            import static gregtech.api.recipe.RecipeMaps.compressorRecipes;
            import gregtech.api.enums.GTValues;
            import gregtech.api.objects.SubstituteFluidStack;
            class Recipes { void run() {
                GTValues.RA.stdBuilder()
                    .fluidInputs(SubstituteFluidStack.soldering(72))
                    .duration(20)
                    .eut(2)
                    .addTo(compressorRecipes);
            }}
        ";
        let analysis = analyze_source("Recipes.java", "gt5", None, source)?;
        let mut project = JavaProjectIndex::default();
        project.add_analysis(&substitute);
        project.add_analysis(&analysis);
        let recovery = crate::recover_with_project(&analysis, &project);
        let recipe = recovery
            .recipes
            .first()
            .ok_or_else(|| anyhow::anyhow!("recipe missing"))?;
        let lowered = super::lower_recipe_with_project(recipe, &analysis.bindings, &project)
            .map_err(|failure| anyhow::anyhow!("lowering failed: {failure:?}"))?;
        let FluidInputSpec::Alternatives { choices } = &lowered.fluid_inputs[0] else {
            return Err(anyhow::anyhow!("soldering input was not alternatives"));
        };
        assert_eq!(choices.len(), 3);
        assert_eq!(choices[0].amount, 72);
        assert_eq!(choices[1].amount, 144);
        assert_eq!(choices[2].amount, 288);
        assert!(matches!(
            &choices[0].selector,
            FluidSelector::MaterialForm { material, form }
                if material.as_str() == "gregtech.api.enums.Materials.SolderingAlloy"
                    && form.as_str() == "molten"
        ));
        Ok(())
    }

    #[test]
    fn lowers_material_and_registry_fluids_with_project_amounts() -> anyhow::Result<()> {
        let constants = analyze_source(
            "GTRecipeBuilder.java",
            "gt5",
            None,
            r"
                package gregtech.api.util;
                class GTRecipeBuilder { static final int INGOTS = 144; }
            ",
        )?;
        let source = r#"
            import static gregtech.api.recipe.RecipeMaps.compressorRecipes;
            import static gregtech.api.util.GTRecipeBuilder.INGOTS;
            import gregtech.api.enums.GTValues;
            import gregtech.api.enums.Materials;
            import net.minecraftforge.fluids.FluidRegistry;
            class Recipes { void run() {
                GTValues.RA.stdBuilder()
                    .fluidInputs(Materials.Iron.getMolten(2 * INGOTS))
                    .fluidOutputs(FluidRegistry.getFluidStack("steam", 1_000))
                    .duration(20)
                    .eut(2)
                    .addTo(compressorRecipes);
            }}
        "#;
        let analysis = analyze_source("Recipes.java", "gt5", None, source)?;
        let mut project = JavaProjectIndex::default();
        project.add_analysis(&constants);
        project.add_analysis(&analysis);
        let recovery = crate::recover_with_project(&analysis, &project);
        let recipe = recovery
            .recipes
            .first()
            .ok_or_else(|| anyhow::anyhow!("recipe missing"))?;
        let lowered = super::lower_recipe_with_project(recipe, &analysis.bindings, &project)
            .map_err(|failure| anyhow::anyhow!("lowering failed: {failure:?}"))?;
        assert_eq!(lowered.fluid_inputs.len(), 1);
        let FluidInputSpec::Stack(input) = &lowered.fluid_inputs[0] else {
            return Err(anyhow::anyhow!("fluid input was not a stack"));
        };
        assert_eq!(input.amount, 288);
        assert!(matches!(
            &input.selector,
            FluidSelector::MaterialForm { material, form }
                if material.as_str() == "gregtech.api.enums.Materials.Iron"
                    && form.as_str() == "molten"
        ));
        assert_eq!(lowered.fluid_outputs.len(), 1);
        assert_eq!(lowered.fluid_outputs[0].amount, 1_000);
        assert!(matches!(
            &lowered.fluid_outputs[0].selector,
            FluidSelector::Named { id } if id.as_str() == "steam"
        ));
        Ok(())
    }

    #[test]
    fn lowers_gt_mod_handler_fixed_fluids() -> anyhow::Result<()> {
        let mod_handler = analyze_source(
            "GTModHandler.java",
            "gt5",
            None,
            r"
                package gregtech.api.util;
                class FluidStack {}
                class GTModHandler {
                    static FluidStack getDistilledWater(long amount) { return null; }
                    static FluidStack getHotWater(int amount) { return null; }
                }
            ",
        )?;
        let source = r"
            import static gregtech.api.recipe.RecipeMaps.compressorRecipes;
            import gregtech.api.enums.GTValues;
            import gregtech.api.util.GTModHandler;
            class Recipes { void run() {
                GTValues.RA.stdBuilder()
                    .fluidInputs(GTModHandler.getDistilledWater(1_000))
                    .fluidOutputs(GTModHandler.getHotWater(250))
                    .duration(20)
                    .eut(2)
                    .addTo(compressorRecipes);
            }}
        ";
        let analysis = analyze_source("Recipes.java", "gt5", None, source)?;
        let mut project = JavaProjectIndex::default();
        project.add_analysis(&mod_handler);
        project.add_analysis(&analysis);
        let recovery = crate::recover_with_project(&analysis, &project);
        let recipe = recovery
            .recipes
            .first()
            .ok_or_else(|| anyhow::anyhow!("recipe missing"))?;
        let lowered = super::lower_recipe_with_project(recipe, &analysis.bindings, &project)
            .map_err(|failure| anyhow::anyhow!("lowering failed: {failure:?}"))?;
        let FluidInputSpec::Stack(input) = &lowered.fluid_inputs[0] else {
            return Err(anyhow::anyhow!("fluid input was not a stack"));
        };
        assert_eq!(input.amount, 1_000);
        assert!(matches!(
            &input.selector,
            FluidSelector::Named { id } if id.as_str() == "ic2distilledwater"
        ));
        assert_eq!(lowered.fluid_outputs[0].amount, 250);
        assert!(matches!(
            &lowered.fluid_outputs[0].selector,
            FluidSelector::Named { id } if id.as_str() == "ic2hotwater"
        ));
        Ok(())
    }

    #[test]
    fn lowers_fluid_registry_constructor_identities() -> anyhow::Result<()> {
        let source = r#"
            import static gregtech.api.recipe.RecipeMaps.compressorRecipes;
            import gregtech.api.enums.GTValues;
            import net.minecraftforge.fluids.FluidRegistry;
            import net.minecraftforge.fluids.FluidStack;
            class Recipes { void run() {
                GTValues.RA.stdBuilder()
                    .fluidInputs(new FluidStack(FluidRegistry.WATER, 750))
                    .fluidOutputs(new FluidStack(FluidRegistry.getFluid("potion.tea"), 750))
                    .duration(20)
                    .eut(2)
                    .addTo(compressorRecipes);
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
        let lowered = super::lower_recipe_with_project(recipe, &analysis.bindings, &project)
            .map_err(|failure| anyhow::anyhow!("lowering failed: {failure:?}"))?;
        let FluidInputSpec::Stack(input) = &lowered.fluid_inputs[0] else {
            return Err(anyhow::anyhow!("fluid input was not a stack"));
        };
        assert!(matches!(
            &input.selector,
            FluidSelector::Named { id } if id.as_str() == "water"
        ));
        assert!(matches!(
            &lowered.fluid_outputs[0].selector,
            FluidSelector::Named { id } if id.as_str() == "potion.tea"
        ));
        Ok(())
    }
}
