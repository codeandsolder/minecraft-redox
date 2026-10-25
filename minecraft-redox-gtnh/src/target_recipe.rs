use std::collections::BTreeMap;

use minecraft_redox_core::{
    SourceOrigin,
    content::{
        ContentId, ContentRecipe, FluidSelector, FluidStackSpec, ItemInputSpec, ItemInputStackSpec,
        ItemOutputSpec, ItemSelector, ItemStackSpec, Probability, RecipeRequirement, RecipeUnlock,
    },
};
use minecraft_redox_java::{JavaBindings, JavaExpression, JavaProjectIndex};
use serde::Serialize;

use crate::{
    FluidSemantic, ItemSemantic, RecipeDefinition, RecoveredExpression,
    recipe_expression::{
        literal_i64, recover_items, recover_items_with_project, resolved_reference_name,
    },
    scalar_expression::scalar_i64,
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
    let lowered_metadata = lower_metadata(recipe, bindings, project, &mut blockers);

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

    let (item_inputs, item_outputs) = lower_item_io(recipe, bindings, project, &mut blockers);
    let fluid_inputs = lower_fluids(
        &recipe.fluid_input_semantics,
        bindings,
        project,
        "input",
        &mut blockers,
    );
    let fluid_outputs = lower_fluids(
        &recipe.fluid_output_semantics,
        bindings,
        project,
        "output",
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
            requirements: lowered_metadata.requirements,
            unlock: lowered_metadata.unlock,
            duration_ticks,
            eut,
        });
    }

    Err(Box::new(RecipeLoweringFailure {
        origin: recipe.origin.clone(),
        blockers,
    }))
}

fn lower_item_io(
    recipe: &RecipeDefinition,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
    blockers: &mut Vec<RecipeLoweringBlocker>,
) -> (Vec<ItemInputSpec>, Vec<ItemOutputSpec>) {
    let input_probabilities = lower_chances(
        &recipe.input_chance_expressions,
        recipe.item_input_semantics.len(),
        bindings,
        project,
        "input",
        blockers,
    );
    let output_probabilities = lower_chances(
        &recipe.output_chance_expressions,
        recipe.item_output_semantics.len(),
        bindings,
        project,
        "output",
        blockers,
    );
    (
        lower_inputs(recipe, &input_probabilities, bindings, project, blockers),
        lower_outputs(recipe, &output_probabilities, bindings, project, blockers),
    )
}

fn structural_blockers(recipe: &RecipeDefinition) -> Vec<RecipeLoweringBlocker> {
    let mut blockers = Vec::new();
    if !recipe.modifiers.is_empty() {
        push_blocker(
            &mut blockers,
            RecipeLoweringBlockerKind::Modifiers,
            "recipe modifiers are not in target recipe IR v1",
        );
    }
    if recipe.special.is_some() || recipe.special_value.is_some() {
        push_blocker(
            &mut blockers,
            RecipeLoweringBlockerKind::Special,
            "special item/value is not in target recipe IR v1",
        );
    }
    if !recipe.post_add_calls.is_empty() || !recipe.unhandled_calls.is_empty() {
        push_blocker(
            &mut blockers,
            RecipeLoweringBlockerKind::PostAdd,
            "post-add or unhandled builder behavior remains",
        );
    }
    blockers
}

struct LoweredMetadata {
    requirements: Vec<RecipeRequirement>,
    unlock: Option<RecipeUnlock>,
}

fn lower_metadata(
    recipe: &RecipeDefinition,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
    blockers: &mut Vec<RecipeLoweringBlocker>,
) -> LoweredMetadata {
    let mut minimum_heat_kelvin = None;
    let mut minimum_capabilities = BTreeMap::new();
    let mut research_item = None;
    let mut scanning = None;

    for call in &recipe.metadata {
        if call.name != "metadata" {
            push_blocker(
                blockers,
                RecipeLoweringBlockerKind::Metadata,
                format!("unsupported recipe metadata call: {}", call.name),
            );
            continue;
        }

        let [key, value] = call.argument_expressions.as_slice() else {
            push_blocker(
                blockers,
                RecipeLoweringBlockerKind::Metadata,
                format!(
                    "metadata call does not have key/value arguments: {:?}",
                    call.arguments
                ),
            );
            continue;
        };

        let Some(key) = resolved_reference_name(key, bindings) else {
            push_blocker(
                blockers,
                RecipeLoweringBlockerKind::Metadata,
                format!(
                    "metadata key is not statically identifiable: {}",
                    call.arguments.join(", ")
                ),
            );
            continue;
        };

        if key == "gregtech.api.util.GTRecipeConstants.COIL_HEAT" {
            match lower_nonnegative_scalar(value, bindings, project) {
                Some(kelvin) => minimum_heat_kelvin = Some(kelvin),
                None => push_blocker(
                    blockers,
                    RecipeLoweringBlockerKind::Metadata,
                    format!(
                        "COIL_HEAT is not a static non-negative Kelvin value: {}",
                        call.arguments.join(", ")
                    ),
                ),
            }
        } else if let Some(capability) = casing_capability(&key) {
            match lower_nonnegative_scalar(value, bindings, project) {
                Some(level) => {
                    minimum_capabilities.insert(capability, level);
                }
                None => push_blocker(
                    blockers,
                    RecipeLoweringBlockerKind::Metadata,
                    format!(
                        "casing requirement is not a static non-negative capability level: {}",
                        call.arguments.join(", ")
                    ),
                ),
            }
        } else if matches!(
            key.as_str(),
            "gregtech.api.util.GTRecipeConstants.RESEARCH_ITEM"
                | "gregtech.api.util.GTRecipeConstants.SCANNING"
        ) {
            if recipe.recipe_map != "gregtech.api.util.GTRecipeConstants.AssemblyLine" {
                push_blocker(
                    blockers,
                    RecipeLoweringBlockerKind::Metadata,
                    format!(
                        "assembly-line research metadata used on recipe map {}",
                        recipe.recipe_map
                    ),
                );
                continue;
            }
            if key.ends_with(".RESEARCH_ITEM") {
                research_item = Some((call, value));
            } else {
                scanning = Some(value);
            }
        } else {
            push_blocker(
                blockers,
                RecipeLoweringBlockerKind::Metadata,
                format!("unsupported recipe metadata key: {key}"),
            );
        }
    }

    let requirements = lower_recipe_requirements(minimum_heat_kelvin, minimum_capabilities);
    let unlock =
        lower_assembly_line_unlock(recipe, research_item, scanning, bindings, project, blockers);
    LoweredMetadata {
        requirements,
        unlock,
    }
}

fn lower_recipe_requirements(
    minimum_heat_kelvin: Option<u64>,
    minimum_capabilities: BTreeMap<ContentId, u64>,
) -> Vec<RecipeRequirement> {
    let mut requirements = Vec::new();
    if let Some(kelvin) = minimum_heat_kelvin {
        requirements.push(RecipeRequirement::MinimumHeat { kelvin });
    }
    requirements.extend(
        minimum_capabilities
            .into_iter()
            .map(|(capability, level)| RecipeRequirement::MinimumCapability { capability, level }),
    );
    requirements
}

fn lower_assembly_line_unlock(
    recipe: &RecipeDefinition,
    research_item: Option<(&crate::BuilderCall, &JavaExpression)>,
    scanning: Option<&JavaExpression>,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
    blockers: &mut Vec<RecipeLoweringBlocker>,
) -> Option<RecipeUnlock> {
    if research_item.is_none() && scanning.is_none() {
        return None;
    }

    let Some((research_call, research_expression)) = research_item else {
        push_blocker(
            blockers,
            RecipeLoweringBlockerKind::Metadata,
            "AssemblyLine SCANNING has no RESEARCH_ITEM; the source recipe map drops this recipe",
        );
        return None;
    };
    let Some(scanning) = scanning else {
        push_blocker(
            blockers,
            RecipeLoweringBlockerKind::Metadata,
            "AssemblyLine RESEARCH_ITEM has no SCANNING data and cannot produce a usable scan",
        );
        return None;
    };

    let Some(sample) = lower_metadata_item(
        recipe,
        research_call,
        research_expression,
        bindings,
        project,
    ) else {
        push_blocker(
            blockers,
            RecipeLoweringBlockerKind::Metadata,
            format!(
                "AssemblyLine RESEARCH_ITEM did not lower to a concrete item: {}",
                research_call.arguments.join(", ")
            ),
        );
        return None;
    };

    let JavaExpression::New {
        type_name,
        arguments,
    } = scanning
    else {
        push_blocker(
            blockers,
            RecipeLoweringBlockerKind::Metadata,
            "AssemblyLine SCANNING is not a Scanning(time, voltage) constructor",
        );
        return None;
    };
    if !bindings.resolve_reference(type_name).ends_with(".Scanning") || arguments.len() != 2 {
        push_blocker(
            blockers,
            RecipeLoweringBlockerKind::Metadata,
            "AssemblyLine SCANNING is not the expected Scanning(time, voltage) value",
        );
        return None;
    }

    let Some(duration_ticks) = lower_nonnegative_scalar(&arguments[0], bindings, project)
        .filter(|duration| *duration > 0 && i32::try_from(*duration).is_ok())
    else {
        push_blocker(
            blockers,
            RecipeLoweringBlockerKind::Metadata,
            "AssemblyLine scan duration is not a positive source-int tick count",
        );
        return None;
    };
    let Some(eut) = scalar_i64_optional(&arguments[1], bindings, project)
        .filter(|eut| *eut > 0 && i32::try_from(*eut).is_ok())
    else {
        push_blocker(
            blockers,
            RecipeLoweringBlockerKind::Metadata,
            "Assembly Line scan EU/t is not a positive source-int value",
        );
        return None;
    };

    let sample = ItemInputStackSpec {
        selector: sample.selector,
        amount: sample.amount,
        consumption_probability: Probability::ONE,
    };
    Some(RecipeUnlock::Scan {
        sample,
        duration_ticks,
        eut,
    })
}

fn lower_metadata_item(
    recipe: &RecipeDefinition,
    call: &crate::BuilderCall,
    expression: &JavaExpression,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
) -> Option<ItemStackSpec> {
    let source = call.arguments.get(1)?.clone();
    let sources = [source];
    let expressions = [expression.clone()];
    let recovered = project.map_or_else(
        || recover_items(&sources, &expressions, bindings),
        |project| {
            recover_items_with_project(
                &sources,
                &expressions,
                bindings,
                project,
                &recipe.origin.path,
                call.byte_start,
            )
        },
    );
    lower_item_semantic(recovered.first()?.semantic.as_ref()?, bindings, project)
}

fn casing_capability(key: &str) -> Option<ContentId> {
    let capability = match key {
        "gregtech.api.util.GTRecipeConstants.COAL_CASING_TIER" => {
            "machine_capability.component_assembly_line_casing"
        }
        "gregtech.api.util.GTRecipeConstants.CHEMPLANT_CASING_TIER" => {
            "machine_capability.chemical_plant_solid_casing"
        }
        "gregtech.api.util.GTRecipeConstants.PRECISE_ASSEMBLER_CASING_TIER" => {
            "machine_capability.precise_assembler_casing"
        }
        _ => return None,
    };
    Some(ContentId::new(capability))
}

fn lower_nonnegative_scalar(
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

fn lower_chances(
    expressions: &[JavaExpression],
    expected_slots: usize,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
    label: &str,
    blockers: &mut Vec<RecipeLoweringBlocker>,
) -> Vec<Probability> {
    if expressions.is_empty() {
        return vec![Probability::ONE; expected_slots];
    }
    if expressions.len() > expected_slots {
        push_blocker(
            blockers,
            RecipeLoweringBlockerKind::Chances,
            format!(
                "{label} chance count {} exceeds {expected_slots} slots",
                expressions.len()
            ),
        );
        return vec![Probability::ONE; expected_slots];
    }

    let mut probabilities: Vec<_> = expressions
        .iter()
        .enumerate()
        .map(|(index, expression)| {
            lower_gt_probability(expression, bindings, project).unwrap_or_else(|| {
                push_blocker(
                    blockers,
                    RecipeLoweringBlockerKind::Chances,
                    format!("{label} chance {index} is not a static GT probability"),
                );
                Probability::ONE
            })
        })
        .collect();
    probabilities.resize(expected_slots, Probability::ONE);
    probabilities
}

fn lower_gt_probability(
    expression: &JavaExpression,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
) -> Option<Probability> {
    let value = project.map_or_else(
        || literal_i64(expression),
        |project| scalar_i64(expression, bindings, project),
    )?;
    let numerator = u64::try_from(value).ok()?;
    Probability::new(numerator, 10_000)
}

fn lower_inputs(
    recipe: &RecipeDefinition,
    probabilities: &[Probability],
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
    blockers: &mut Vec<RecipeLoweringBlocker>,
) -> Vec<ItemInputSpec> {
    recipe
        .item_input_semantics
        .iter()
        .zip(probabilities)
        .enumerate()
        .filter_map(|(index, (recovered, probability))| {
            lower_recovered_stack(recovered, bindings, project)
                .map(|stack| input_stack_spec(stack, *probability))
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

fn input_stack_spec(
    stack: ItemStackSpec,
    consumption_probability: Probability,
) -> ItemInputStackSpec {
    if stack.amount == 0 {
        ItemInputStackSpec {
            selector: stack.selector,
            amount: 1,
            consumption_probability: Probability::ZERO,
        }
    } else {
        ItemInputStackSpec {
            selector: stack.selector,
            amount: stack.amount,
            consumption_probability,
        }
    }
}

fn lower_outputs(
    recipe: &RecipeDefinition,
    probabilities: &[Probability],
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
    blockers: &mut Vec<RecipeLoweringBlocker>,
) -> Vec<ItemOutputSpec> {
    recipe
        .item_output_semantics
        .iter()
        .zip(probabilities)
        .enumerate()
        .filter_map(|(index, (recovered, probability))| {
            lower_recovered_stack(recovered, bindings, project)
                .map(|stack| ItemOutputSpec {
                    stack,
                    production_probability: *probability,
                })
                .or_else(|| {
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
    project: Option<&JavaProjectIndex>,
) -> Option<ItemStackSpec> {
    lower_item_semantic(recovered.semantic.as_ref()?, bindings, project)
}

fn lower_item_semantic(
    semantic: &ItemSemantic,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
) -> Option<ItemStackSpec> {
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
            amount: scalar_amount_u64(amount, bindings, project)?,
        }),
        ItemSemantic::OreDictQuery { key, amount } => Some(ItemStackSpec {
            selector: ItemSelector::OreKey {
                key: ContentId::new(simple_string_literal(key)?),
            },
            amount: scalar_amount_u64(amount, bindings, project)?,
        }),
        ItemSemantic::MaterialItem {
            material,
            accessor,
            arguments,
        } => lower_material_item(material, accessor, arguments, bindings, project),
        ItemSemantic::OreDictUnify {
            inner: Some(inner), ..
        }
        | ItemSemantic::InlinedMethod { result: inner, .. } => {
            lower_item_semantic(inner, bindings, project)
        }
        ItemSemantic::ItemList {
            item,
            accessor,
            arguments,
        } if accessor == "get" => Some(ItemStackSpec {
            selector: ItemSelector::Named {
                id: ContentId::new(item.clone()),
                variant: None,
            },
            amount: arguments.first().map_or(Some(1), |amount| {
                scalar_amount_u64(amount, bindings, project)
            })?,
        }),
        ItemSemantic::ItemList {
            item,
            accessor,
            arguments,
        } if accessor == "getWithDamage" && arguments.len() == 2 => Some(ItemStackSpec {
            selector: ItemSelector::Named {
                id: ContentId::new(item.clone()),
                variant: Some(scalar_i64_optional(arguments.get(1)?, bindings, project)?),
            },
            amount: scalar_amount_u64(arguments.first()?, bindings, project)?,
        }),
        ItemSemantic::ModItem {
            provider,
            arguments,
        } => lower_mod_item(provider, arguments, bindings, project),
        ItemSemantic::LinkedMethod {
            method_id,
            receiver,
            arguments,
            ..
        } => lower_linked_item_method(method_id, receiver.as_ref(), arguments, bindings, project),
        ItemSemantic::ItemStack { item, amount, meta } => Some(ItemStackSpec {
            selector: ItemSelector::Named {
                id: ContentId::new(resolved_reference_name(item, bindings)?),
                variant: match meta.as_ref() {
                    None => None,
                    Some(meta) => Some(scalar_i64_optional(meta, bindings, project)?),
                },
            },
            amount: amount.as_ref().map_or(Some(1), |amount| {
                scalar_amount_u64(amount, bindings, project)
            })?,
        }),
        ItemSemantic::CopyAmount {
            amount,
            meta: None,
            inner: Some(inner),
            ..
        } => {
            let mut stack = lower_item_semantic(inner, bindings, project)?;
            stack.amount = scalar_amount_u64(amount, bindings, project)?;
            Some(stack)
        }
        ItemSemantic::OreDictStack { arguments } => Some(ItemStackSpec {
            selector: ItemSelector::OreKey {
                key: ContentId::new(simple_string_literal(arguments.first()?)?),
            },
            amount: arguments.get(1).map_or(Some(1), |amount| {
                scalar_amount_u64(amount, bindings, project)
            })?,
        }),
        _ => None,
    }
}

fn lower_material_item(
    material: &str,
    accessor: &str,
    arguments: &[JavaExpression],
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
) -> Option<ItemStackSpec> {
    let (form, amount) = if matches!(accessor, "get" | "getPart" | "getComponentByPrefix") {
        let form = resolved_reference_name(arguments.first()?, bindings)?;
        if !form.contains("OrePrefixes.") {
            return None;
        }
        let amount = arguments.get(1).map_or(Some(1), |amount| {
            scalar_amount_u64(amount, bindings, project)
        })?;
        (form, amount)
    } else {
        let form = material_accessor_form(accessor)?;
        let amount = arguments.first().map_or(Some(1), |amount| {
            scalar_amount_u64(amount, bindings, project)
        })?;
        (format!("gregtech.api.enums.OrePrefixes.{form}"), amount)
    };
    Some(ItemStackSpec {
        selector: ItemSelector::MaterialForm {
            material: ContentId::new(material.to_owned()),
            form: ContentId::new(form),
        },
        amount,
    })
}

fn material_accessor_form(accessor: &str) -> Option<&'static str> {
    match accessor {
        "getDust" => Some("dust"),
        "getDustSmall" | "getSmallDust" => Some("dustSmall"),
        "getDustTiny" | "getTinyDust" => Some("dustTiny"),
        "getCell" | "getCells" => Some("cell"),
        "getPlate" | "getPlates" => Some("plate"),
        "getPlateDouble" => Some("plateDouble"),
        "getPlateDense" => Some("plateDense"),
        "getPlateSuperdense" => Some("plateSuperdense"),
        "getScrew" => Some("screw"),
        "getGear" => Some("gearGt"),
        "getGearSmall" => Some("gearGtSmall"),
        "getNanite" => Some("nanite"),
        "getFrameBox" => Some("frameGt"),
        "getBolt" => Some("bolt"),
        "getRod" => Some("stick"),
        "getLongRod" => Some("stickLong"),
        "getBlock" | "getBlocks" => Some("block"),
        "getGem" | "getGems" => Some("gem"),
        "getFineWire" => Some("wireFine"),
        "getRotor" => Some("rotor"),
        "getRing" => Some("ring"),
        "getIngot" | "getIngots" => Some("ingot"),
        "getOre" => Some("ore"),
        "getHotIngot" => Some("ingotHot"),
        "getNugget" | "getNuggets" => Some("nugget"),
        "getFoil" => Some("foil"),
        "getWire01" => Some("wireGt01"),
        "getWire02" => Some("wireGt02"),
        "getWire04" => Some("wireGt04"),
        "getWire08" => Some("wireGt08"),
        "getWire12" => Some("wireGt12"),
        "getWire16" => Some("wireGt16"),
        "getCable01" => Some("cableGt01"),
        "getCable02" => Some("cableGt02"),
        "getCable04" => Some("cableGt04"),
        "getCable08" => Some("cableGt08"),
        "getCable12" => Some("cableGt12"),
        "getCable16" => Some("cableGt16"),
        "getCrushed" => Some("crushed"),
        "getCrushedPurified" => Some("crushedPurified"),
        "getCrushedCentrifuged" => Some("crushedCentrifuged"),
        "getDustPurified" => Some("dustPure"),
        "getDustImpure" => Some("dustImpure"),
        "getMilled" => Some("milled"),
        "getRawOre" => Some("rawOre"),
        "getPlasmaCell" => Some("cellPlasma"),
        _ => None,
    }
}

fn lower_named_subtype_item_helper(
    method_id: &str,
    receiver: Option<&JavaExpression>,
    arguments: &[JavaExpression],
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
) -> Option<ItemStackSpec> {
    if matches!(
        method_id,
        "gregtech.common.items.ItemComb#getStackForType(CombType,int)"
            | "gregtech.common.items.ItemComb#getStackForType(CombType)"
    ) && matches!(arguments.len(), 1 | 2)
    {
        let id = qualified_reference_name(receiver?, bindings)?;
        let variant = qualified_reference_name(arguments.first()?, bindings)?;
        let amount = arguments.get(1).map_or(Some(1), |amount| {
            scalar_amount_u64(amount, bindings, project)
        })?;
        return Some(named_variant_stack(id, variant, amount));
    }

    if matches!(
        method_id,
        "gtPlusPlus.core.material.Particle#getBaseParticle(Particle)"
            | "gtPlusPlus.core.material.Particle#getBaseParticle(Particle,int)"
    ) && matches!(arguments.len(), 1 | 2)
    {
        let variant = qualified_reference_name(arguments.first()?, bindings)?;
        if !variant.starts_with("gtPlusPlus.core.material.Particle.") {
            return None;
        }
        let amount = arguments.get(1).map_or(Some(1), |amount| {
            scalar_amount_u64(amount, bindings, project)
        })?;
        return Some(named_variant_stack(
            "gtPlusPlus.core.item.ModItems.itemStandarParticleBase",
            variant,
            amount,
        ));
    }

    if method_id == "gtPlusPlus.xmod.forestry.bees.handler.GTPPCombType#getStackForType(int)"
        && arguments.len() == 1
    {
        let variant = qualified_reference_name(receiver?, bindings)?;
        if !variant.starts_with("gtPlusPlus.xmod.forestry.bees.handler.GTPPCombType.") {
            return None;
        }
        return Some(named_variant_stack(
            "gtPlusPlus.xmod.forestry.bees.registry.GTPP_Bees.combs",
            variant,
            scalar_amount_u64(arguments.first()?, bindings, project)?,
        ));
    }

    if method_id
        == "kekztech.common.items.MetaItemCraftingComponent#getStackOfAmountFromDamage(int,int)"
        && arguments.len() == 2
    {
        let JavaExpression::Call {
            receiver: Some(item),
            method,
            arguments: meta_arguments,
        } = arguments.first()?
        else {
            return None;
        };
        if method != "getMetaID" || !meta_arguments.is_empty() {
            return None;
        }
        let variant = qualified_reference_name(item, bindings)?;
        if !variant.starts_with("kekztech.Items.") {
            return None;
        }
        return Some(named_variant_stack(
            "kekztech.common.items.MetaItemCraftingComponent",
            variant,
            scalar_amount_u64(arguments.get(1)?, bindings, project)?,
        ));
    }

    None
}

fn named_variant_stack(
    id: impl Into<String>,
    variant: impl Into<String>,
    amount: u64,
) -> ItemStackSpec {
    ItemStackSpec {
        selector: ItemSelector::NamedVariant {
            id: ContentId::new(id),
            variant: ContentId::new(variant),
        },
        amount,
    }
}

fn lower_transparent_linked_item_helper(
    method_id: &str,
    arguments: &[JavaExpression],
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
) -> Option<ItemStackSpec> {
    if method_id == "tectech.loader.recipe.BaseRecipeLoader#getNHCoreModItem(String,int)"
        && arguments.len() == 2
    {
        let item = simple_string_literal(arguments.first()?)?;
        return Some(ItemStackSpec {
            selector: ItemSelector::Named {
                id: ContentId::new(format!("com.dreammaster.item.NHItemList.{item}")),
                variant: None,
            },
            amount: scalar_amount_u64(arguments.get(1)?, bindings, project)?,
        });
    }

    if method_id
        == "gtPlusPlus.core.util.minecraft.ItemUtils#getOrePrefixStack(gregtech.api.enums.OrePrefixes,gregtech.api.enums.Materials,int)"
        && arguments.len() == 3
    {
        let mut form = qualified_reference_name(arguments.first()?, bindings)?;
        if form == "gregtech.api.enums.OrePrefixes.rod" {
            form = String::from("gregtech.api.enums.OrePrefixes.stick");
        }
        if !form.contains("OrePrefixes.") {
            return None;
        }
        let material = qualified_reference_name(arguments.get(1)?, bindings)?;
        return Some(ItemStackSpec {
            selector: ItemSelector::MaterialForm {
                material: ContentId::new(material),
                form: ContentId::new(form),
            },
            amount: scalar_amount_u64(arguments.get(2)?, bindings, project)?,
        });
    }

    None
}

fn lower_linked_item_method(
    method_id: &str,
    receiver: Option<&JavaExpression>,
    arguments: &[JavaExpression],
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
) -> Option<ItemStackSpec> {
    if method_id
        == "goodgenerator.loader.ComponentAssemblyLineLoader#get(gregtech.api.enums.OrePrefixes,Object,int)"
        && arguments.len() == 3
    {
        let form = qualified_reference_name(arguments.first()?, bindings)?;
        if !form.contains("OrePrefixes.") {
            return None;
        }
        let material = qualified_reference_name(arguments.get(1)?, bindings)?;
        return Some(ItemStackSpec {
            selector: ItemSelector::MaterialForm {
                material: ContentId::new(material),
                form: ContentId::new(form),
            },
            amount: scalar_amount_u64(arguments.get(2)?, bindings, project)?,
        });
    }

    if matches!(
        method_id,
        "bartworks.system.material.Werkstoff#get(gregtech.api.enums.OrePrefixes,int)"
            | "bartworks.system.material.Werkstoff#get(gregtech.api.enums.OrePrefixes)"
    ) && matches!(arguments.len(), 1 | 2)
    {
        let material = qualified_reference_name(receiver?, bindings)?;
        let form = qualified_reference_name(arguments.first()?, bindings)?;
        if !form.contains("OrePrefixes.") {
            return None;
        }
        return Some(ItemStackSpec {
            selector: ItemSelector::MaterialForm {
                material: ContentId::new(material),
                form: ContentId::new(form),
            },
            amount: arguments.get(1).map_or(Some(1), |amount| {
                scalar_amount_u64(amount, bindings, project)
            })?,
        });
    }

    if let Some(accessor) = method_id
        .strip_prefix("gtPlusPlus.core.material.Material#")
        .and_then(|signature| signature.split_once('(').map(|(accessor, _)| accessor))
        .filter(|accessor| material_accessor_form(accessor).is_some())
        && arguments.len() == 1
    {
        let material = concrete_gtpp_material_receiver(receiver?, bindings)?;
        let form = material_accessor_form(accessor)?;
        return Some(ItemStackSpec {
            selector: ItemSelector::MaterialForm {
                material: ContentId::new(material),
                form: ContentId::new(format!("gregtech.api.enums.OrePrefixes.{form}")),
            },
            amount: scalar_amount_u64(arguments.first()?, bindings, project)?,
        });
    }

    if let Some(stack) =
        lower_transparent_linked_item_helper(method_id, arguments, bindings, project)
    {
        return Some(stack);
    }

    if let Some(stack) =
        lower_named_subtype_item_helper(method_id, receiver, arguments, bindings, project)
    {
        return Some(stack);
    }

    if method_id == "goodgenerator.util.ItemRefer#get(int)" && arguments.len() == 1 {
        let item = qualified_reference_name(receiver?, bindings)?;
        if !item.contains("ItemRefer.") {
            return None;
        }
        return Some(ItemStackSpec {
            selector: ItemSelector::Named {
                id: ContentId::new(item),
                variant: None,
            },
            amount: scalar_amount_u64(arguments.first()?, bindings, project)?,
        });
    }

    if method_id == "gregtech.api.enums.ItemList#get(long,Object...)" && !arguments.is_empty() {
        let item = qualified_reference_name(receiver?, bindings)?;
        if !item.contains("ItemList.") {
            return None;
        }
        return Some(ItemStackSpec {
            selector: ItemSelector::Named {
                id: ContentId::new(item),
                variant: None,
            },
            amount: scalar_amount_u64(arguments.first()?, bindings, project)?,
        });
    }

    None
}

fn concrete_gtpp_material_receiver(
    receiver: &JavaExpression,
    bindings: &JavaBindings,
) -> Option<String> {
    if let Some(name) = qualified_reference_name(receiver, bindings) {
        return Some(name);
    }

    let JavaExpression::FieldAccess { receiver, field } = receiver else {
        return None;
    };
    let JavaExpression::Call {
        receiver: Some(owner),
        method,
        arguments,
    } = receiver.as_ref()
    else {
        return None;
    };
    if method != "getInstance" || !arguments.is_empty() {
        return None;
    }
    let owner = qualified_reference_name(owner, bindings)?;
    (owner == "gtPlusPlus.core.material.MaterialsElements").then(|| format!("{owner}.{field}"))
}

fn qualified_reference_name(
    expression: &JavaExpression,
    bindings: &JavaBindings,
) -> Option<String> {
    let name = resolved_reference_name(expression, bindings)?;
    name.contains('.').then_some(name)
}

fn lower_mod_item(
    provider: &str,
    arguments: &[JavaExpression],
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
) -> Option<ItemStackSpec> {
    if provider.ends_with("GTModHandler.getModItem") {
        let mod_id = identity_component(arguments.first()?, bindings)?;
        let item = identity_component(arguments.get(1)?, bindings)?;
        let amount = arguments.get(2).map_or(Some(1), |amount| {
            scalar_amount_u64(amount, bindings, project)
        })?;
        let variant = match arguments.len() {
            2 | 3 => None,
            4 if is_java_null(arguments.get(3)?) => None,
            4 => Some(scalar_i64_optional(arguments.get(3)?, bindings, project)?),
            5 if is_java_null(arguments.get(4)?) => {
                Some(scalar_i64_optional(arguments.get(3)?, bindings, project)?)
            }
            _ => return None,
        };
        return Some(ItemStackSpec {
            selector: ItemSelector::Named {
                id: ContentId::new(format!("{mod_id}:{item}")),
                variant,
            },
            amount,
        });
    }

    if provider.ends_with("GTModHandler.getIC2Item") {
        let item = identity_component(arguments.first()?, bindings)?;
        let amount = scalar_amount_u64(arguments.get(1)?, bindings, project)?;
        let variant = match arguments.len() {
            2 => None,
            3 if is_java_null(arguments.get(2)?) => None,
            3 => Some(scalar_i64_optional(arguments.get(2)?, bindings, project)?),
            4 if is_java_null(arguments.get(3)?) => {
                Some(scalar_i64_optional(arguments.get(2)?, bindings, project)?)
            }
            _ => return None,
        };
        return Some(ItemStackSpec {
            selector: ItemSelector::Named {
                id: ContentId::new(format!("ic2:{item}")),
                variant,
            },
            amount,
        });
    }

    None
}

fn identity_component(expression: &JavaExpression, bindings: &JavaBindings) -> Option<String> {
    simple_string_literal(expression).or_else(|| resolved_reference_name(expression, bindings))
}

fn is_java_null(expression: &JavaExpression) -> bool {
    matches!(expression, JavaExpression::Literal(value) if value == "null")
}

fn scalar_i64_optional(
    expression: &JavaExpression,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
) -> Option<i64> {
    project.map_or_else(
        || literal_i64(expression),
        |project| scalar_i64(expression, bindings, project),
    )
}

fn lower_fluids(
    recovered: &[RecoveredExpression<FluidSemantic>],
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
    direction: &str,
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
                        format!("fluid {direction} {index}: {}", recovered.source),
                    );
                    None
                })
        })
        .collect()
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
            selector: FluidSelector::Named {
                id: ContentId::new(resolved_reference_name(fluid, bindings)?),
            },
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
        FluidSemantic::RegistryFluid { .. } | FluidSemantic::LinkedMethod { .. } => None,
    }
}

fn fluid_form(accessor: &str) -> Option<&'static str> {
    match accessor {
        "getFluid" | "getFluidStack" => Some("fluid"),
        "getSolid" => Some("solid"),
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
        FluidSelector, ItemInputSpec, ItemSelector, Probability, RecipeRequirement, RecipeUnlock,
    };
    use minecraft_redox_java::{JavaExpression, JavaProjectIndex, analyze_source};

    use super::{
        RecipeLoweringBlockerKind, lower_named_subtype_item_helper, lower_recipe,
        lower_recipe_with_project,
    };
    use crate::{recover, recover_with_project};

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
        assert_eq!(input.consumption_probability, Probability::ONE);
        assert!(matches!(
            &input.selector,
            ItemSelector::MaterialForm { material, form }
                if material.as_str() == "gregtech.api.enums.Materials.Netherrack"
                    && form.as_str() == "gregtech.api.enums.OrePrefixes.dust"
        ));
        assert_eq!(lowered.item_outputs.len(), 1);
        assert_eq!(lowered.item_outputs[0].stack.amount, 3);
        assert!(matches!(
            &lowered.item_outputs[0].stack.selector,
            ItemSelector::Named { id, variant: None }
                if id.as_str() == "net.minecraft.init.Blocks.netherrack"
        ));
        Ok(())
    }

    #[test]
    fn lowers_material_item_accessors_to_ore_forms() -> anyhow::Result<()> {
        let source = r"
            import static gregtech.api.recipe.RecipeMaps.compressorRecipes;
            import gregtech.api.enums.GTValues;
            import gregtech.api.enums.Materials;
            import gregtech.api.enums.OrePrefixes;
            import bartworks.system.material.WerkstoffLoader;
            import gtPlusPlus.core.material.MaterialsAlloy;
            class Recipes { void run() {
                GTValues.RA.stdBuilder()
                    .itemInputs(Materials.Iron.getDust(2))
                    .itemOutputs(
                        WerkstoffLoader.Example.get(OrePrefixes.plateDense, 3),
                        MaterialsAlloy.INCONEL_792.getComponentByPrefix(OrePrefixes.wireGt04, 5))
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
        let ItemInputSpec::Stack(input) = &lowered.item_inputs[0] else {
            return Err(anyhow::anyhow!("material input was not a stack"));
        };
        assert_eq!(input.amount, 2);
        assert_eq!(input.consumption_probability, Probability::ONE);
        assert!(matches!(
            &input.selector,
            ItemSelector::MaterialForm { material, form }
                if material.as_str() == "gregtech.api.enums.Materials.Iron"
                    && form.as_str() == "gregtech.api.enums.OrePrefixes.dust"
        ));
        assert_eq!(lowered.item_outputs[0].stack.amount, 3);
        assert!(matches!(
            &lowered.item_outputs[0].stack.selector,
            ItemSelector::MaterialForm { material, form }
                if material.as_str() == "bartworks.system.material.WerkstoffLoader.Example"
                    && form.as_str() == "gregtech.api.enums.OrePrefixes.plateDense"
        ));
        assert_eq!(lowered.item_outputs[1].stack.amount, 5);
        assert!(matches!(
            &lowered.item_outputs[1].stack.selector,
            ItemSelector::MaterialForm { material, form }
                if material.as_str() == "gtPlusPlus.core.material.MaterialsAlloy.INCONEL_792"
                    && form.as_str() == "gregtech.api.enums.OrePrefixes.wireGt04"
        ));
        Ok(())
    }

    #[test]
    fn lowers_circuit_as_required_non_consumed_input() -> anyhow::Result<()> {
        let source = r"
            import static gregtech.api.recipe.RecipeMaps.compressorRecipes;
            import gregtech.api.enums.GTValues;
            import gregtech.api.enums.Materials;
            class Recipes { void run() {
                GTValues.RA.stdBuilder()
                    .circuit(7)
                    .itemInputsUnsafe(Materials.Iron.getDust(128))
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
        assert_eq!(lowered.item_inputs.len(), 2);

        let ItemInputSpec::Stack(material) = &lowered.item_inputs[0] else {
            return Err(anyhow::anyhow!("material input was not a stack"));
        };
        assert_eq!(material.amount, 128);
        assert_eq!(material.consumption_probability, Probability::ONE);

        let ItemInputSpec::Stack(circuit) = &lowered.item_inputs[1] else {
            return Err(anyhow::anyhow!("circuit input was not a stack"));
        };
        assert_eq!(circuit.amount, 1);
        assert_eq!(circuit.consumption_probability, Probability::ZERO);
        assert!(matches!(
            &circuit.selector,
            ItemSelector::Named {
                id,
                variant: Some(7),
            } if id.as_str() == "gregtech.api.enums.ItemList.Circuit_Integrated"
        ));
        Ok(())
    }

    #[test]
    fn lowers_source_backed_linked_item_helpers() -> anyhow::Result<()> {
        let source = r"
            package goodgenerator.loader;
            import static gregtech.api.enums.Materials.Iron;
            import static gregtech.api.enums.OrePrefixes.plateDense;
            import static gregtech.api.recipe.RecipeMaps.compressorRecipes;
            import goodgenerator.util.ItemRefer;
            import gregtech.api.enums.GTValues;
            import gregtech.api.enums.OrePrefixes;
            import gregtech.api.util.GTUtility;
            import net.minecraft.item.ItemStack;
            class ComponentAssemblyLineLoader {
                private static ItemStack get(OrePrefixes prefix, Object material, int amount) {
                    return null;
                }
                void run() {
                    GTValues.RA.stdBuilder()
                        .itemInputs(get(plateDense, Iron, 48))
                        .itemOutputs(GTUtility.copyAmount(3, ItemRefer.Radiation_Protection_Plate.get(2)))
                        .duration(20)
                        .eut(2)
                        .addTo(compressorRecipes);
                }
            }
        ";
        let item_refer = r"
            package goodgenerator.util;
            import net.minecraft.item.ItemStack;
            class ItemRefer {
                static final ItemRefer Radiation_Protection_Plate = new ItemRefer();
                ItemStack get(int amount) { return null; }
            }
        ";
        let helper = analyze_source("ComponentAssemblyLineLoader.java", "gt5", None, source)?;
        let item_refer = analyze_source("ItemRefer.java", "gt5", None, item_refer)?;
        let mut project = JavaProjectIndex::default();
        project.add_analysis(&helper);
        project.add_analysis(&item_refer);
        let recovery = crate::recover_with_project(&helper, &project);
        let recipe = recovery
            .recipes
            .first()
            .ok_or_else(|| anyhow::anyhow!("recipe missing"))?;
        let lowered = super::lower_recipe_with_project(recipe, &helper.bindings, &project)
            .map_err(|failure| anyhow::anyhow!("lowering failed: {failure:?}"))?;

        let ItemInputSpec::Stack(input) = &lowered.item_inputs[0] else {
            return Err(anyhow::anyhow!("helper input was not a stack"));
        };
        assert_eq!(input.amount, 48);
        assert!(matches!(
            &input.selector,
            ItemSelector::MaterialForm { material, form }
                if material.as_str() == "gregtech.api.enums.Materials.Iron"
                    && form.as_str() == "gregtech.api.enums.OrePrefixes.plateDense"
        ));
        assert_eq!(lowered.item_outputs[0].stack.amount, 3);
        assert!(matches!(
            &lowered.item_outputs[0].stack.selector,
            ItemSelector::Named { id, variant: None }
                if id.as_str() == "goodgenerator.util.ItemRefer.Radiation_Protection_Plate"
        ));
        Ok(())
    }

    #[test]
    fn lowers_concrete_werkstoff_receivers_to_material_forms() -> anyhow::Result<()> {
        let werkstoff = r"
            package bartworks.system.material;
            import gregtech.api.enums.OrePrefixes;
            import net.minecraft.item.ItemStack;
            class Werkstoff {
                ItemStack get(OrePrefixes prefix) { return null; }
                ItemStack get(OrePrefixes prefix, int amount) { return null; }
            }
        ";
        let loader = r"
            package bartworks.system.material;
            class WerkstoffLoader {
                static final Werkstoff CalciumChloride = new Werkstoff();
            }
        ";
        let recipe_source = r"
            package demo;
            import static gregtech.api.recipe.RecipeMaps.compressorRecipes;
            import bartworks.system.material.WerkstoffLoader;
            import gregtech.api.enums.GTValues;
            import gregtech.api.enums.OrePrefixes;
            class Recipes { void run() {
                GTValues.RA.stdBuilder()
                    .itemInputs(WerkstoffLoader.CalciumChloride.get(OrePrefixes.dust, 3))
                    .itemOutputs(WerkstoffLoader.CalciumChloride.get(OrePrefixes.plate))
                    .duration(20)
                    .eut(2)
                    .addTo(compressorRecipes);
            }}
        ";
        let werkstoff = analyze_source("Werkstoff.java", "gt5", None, werkstoff)?;
        let loader = analyze_source("WerkstoffLoader.java", "gt5", None, loader)?;
        let recipe = analyze_source("Recipes.java", "gt5", None, recipe_source)?;
        let mut project = JavaProjectIndex::default();
        for analysis in [&werkstoff, &loader, &recipe] {
            project.add_analysis(analysis);
        }
        let recovery = crate::recover_with_project(&recipe, &project);
        let recovered = recovery
            .recipes
            .first()
            .ok_or_else(|| anyhow::anyhow!("recipe missing"))?;
        let lowered = super::lower_recipe_with_project(recovered, &recipe.bindings, &project)
            .map_err(|failure| anyhow::anyhow!("lowering failed: {failure:?}"))?;

        let ItemInputSpec::Stack(input) = &lowered.item_inputs[0] else {
            return Err(anyhow::anyhow!("Werkstoff input was not a stack"));
        };
        assert_eq!(input.amount, 3);
        assert!(matches!(
            &input.selector,
            ItemSelector::MaterialForm { material, form }
                if material.as_str() == "bartworks.system.material.WerkstoffLoader.CalciumChloride"
                    && form.as_str() == "gregtech.api.enums.OrePrefixes.dust"
        ));
        assert_eq!(lowered.item_outputs[0].stack.amount, 1);
        assert!(matches!(
            &lowered.item_outputs[0].stack.selector,
            ItemSelector::MaterialForm { material, form }
                if material.as_str() == "bartworks.system.material.WerkstoffLoader.CalciumChloride"
                    && form.as_str() == "gregtech.api.enums.OrePrefixes.plate"
        ));
        Ok(())
    }

    #[test]
    fn lowers_concrete_gtpp_material_receivers_to_material_forms() -> anyhow::Result<()> {
        let material = r"
            package gtPlusPlus.core.material;
            import net.minecraft.item.ItemStack;
            class Material {
                ItemStack getDust(int amount) { return null; }
                ItemStack getRawOre(int amount) { return null; }
            }
        ";
        let elements = r"
            package gtPlusPlus.core.material;
            class MaterialsElements {
                private static final MaterialsElements INSTANCE = new MaterialsElements();
                final Material THORIUM232 = new Material();
                static MaterialsElements getInstance() { return INSTANCE; }
            }
        ";
        let fluorides = r"
            package gtPlusPlus.core.material.nuclear;
            import gtPlusPlus.core.material.Material;
            class MaterialsFluorides {
                static final Material LITHIUM_FLUORIDE = new Material();
            }
        ";
        let recipe_source = r"
            package demo;
            import static gregtech.api.recipe.RecipeMaps.compressorRecipes;
            import gregtech.api.enums.GTValues;
            import gtPlusPlus.core.material.MaterialsElements;
            import gtPlusPlus.core.material.nuclear.MaterialsFluorides;
            class Recipes { void run() {
                GTValues.RA.stdBuilder()
                    .itemInputs(MaterialsElements.getInstance().THORIUM232.getDust(2))
                    .itemOutputs(MaterialsFluorides.LITHIUM_FLUORIDE.getRawOre(3))
                    .duration(20).eut(2).addTo(compressorRecipes);
            }}
        ";
        let material = analyze_source("Material.java", "gt5", None, material)?;
        let elements = analyze_source("MaterialsElements.java", "gt5", None, elements)?;
        let fluorides = analyze_source("MaterialsFluorides.java", "gt5", None, fluorides)?;
        let recipe = analyze_source("Recipes.java", "gt5", None, recipe_source)?;
        let mut project = JavaProjectIndex::default();
        for analysis in [&material, &elements, &fluorides, &recipe] {
            project.add_analysis(analysis);
        }
        let recovery = crate::recover_with_project(&recipe, &project);
        let recovered = recovery
            .recipes
            .first()
            .ok_or_else(|| anyhow::anyhow!("recipe missing"))?;
        let lowered = super::lower_recipe_with_project(recovered, &recipe.bindings, &project)
            .map_err(|failure| anyhow::anyhow!("lowering failed: {failure:?}"))?;

        let ItemInputSpec::Stack(input) = &lowered.item_inputs[0] else {
            return Err(anyhow::anyhow!("GT++ material input was not a stack"));
        };
        assert_eq!(input.amount, 2);
        assert!(matches!(
            &input.selector,
            ItemSelector::MaterialForm { material, form }
                if material.as_str() == "gtPlusPlus.core.material.MaterialsElements.THORIUM232"
                    && form.as_str() == "gregtech.api.enums.OrePrefixes.dust"
        ));
        assert_eq!(lowered.item_outputs[0].stack.amount, 3);
        assert!(matches!(
            &lowered.item_outputs[0].stack.selector,
            ItemSelector::MaterialForm { material, form }
                if material.as_str() == "gtPlusPlus.core.material.nuclear.MaterialsFluorides.LITHIUM_FLUORIDE"
                    && form.as_str() == "gregtech.api.enums.OrePrefixes.rawOre"
        ));
        Ok(())
    }

    #[test]
    fn lowers_semantic_named_item_variants_without_legacy_meta_ids() -> anyhow::Result<()> {
        let analysis = analyze_source(
            "Variants.java",
            "gt5",
            None,
            r"
                package demo;
                import gregtech.common.items.CombType;
                import gregtech.common.items.GTBees;
                import gtPlusPlus.core.material.Particle;
                import kekztech.Items;
                class Variants {}
            ",
        )?;
        let bindings = &analysis.bindings;
        let field = |owner: &str, name: &str| JavaExpression::FieldAccess {
            receiver: Box::new(JavaExpression::Reference(owner.to_owned())),
            field: name.to_owned(),
        };

        let comb = lower_named_subtype_item_helper(
            "gregtech.common.items.ItemComb#getStackForType(CombType,int)",
            Some(&field("GTBees", "combs")),
            &[
                field("CombType", "MANASTEEL"),
                JavaExpression::Literal("4".into()),
            ],
            bindings,
            None,
        )
        .ok_or_else(|| anyhow::anyhow!("comb subtype did not lower"))?;
        assert_eq!(comb.amount, 4);
        assert!(matches!(
            comb.selector,
            ItemSelector::NamedVariant { id, variant }
                if id.as_str() == "gregtech.common.items.GTBees.combs"
                    && variant.as_str() == "gregtech.common.items.CombType.MANASTEEL"
        ));

        let particle = lower_named_subtype_item_helper(
            "gtPlusPlus.core.material.Particle#getBaseParticle(Particle)",
            Some(&JavaExpression::Reference("Particle".into())),
            &[field("Particle", "PROTON")],
            bindings,
            None,
        )
        .ok_or_else(|| anyhow::anyhow!("particle subtype did not lower"))?;
        assert!(matches!(
            particle.selector,
            ItemSelector::NamedVariant { id, variant }
                if id.as_str() == "gtPlusPlus.core.item.ModItems.itemStandarParticleBase"
                    && variant.as_str() == "gtPlusPlus.core.material.Particle.PROTON"
        ));

        let kekz_variant = JavaExpression::Call {
            receiver: Some(Box::new(field("Items", "YSZCeramicDust"))),
            method: "getMetaID".into(),
            arguments: Vec::new(),
        };
        let kekz = lower_named_subtype_item_helper(
            "kekztech.common.items.MetaItemCraftingComponent#getStackOfAmountFromDamage(int,int)",
            None,
            &[kekz_variant, JavaExpression::Literal("6".into())],
            bindings,
            None,
        )
        .ok_or_else(|| anyhow::anyhow!("Kekz subtype did not lower"))?;
        assert_eq!(kekz.amount, 6);
        assert!(matches!(
            kekz.selector,
            ItemSelector::NamedVariant { id, variant }
                if id.as_str() == "kekztech.common.items.MetaItemCraftingComponent"
                    && variant.as_str() == "kekztech.Items.YSZCeramicDust"
        ));

        assert!(
            lower_named_subtype_item_helper(
                "gregtech.common.items.ItemComb#getStackForType(CombType,int)",
                Some(&JavaExpression::Reference("dynamicComb".into())),
                &[
                    field("CombType", "MANASTEEL"),
                    JavaExpression::Literal("1".into())
                ],
                bindings,
                None,
            )
            .is_none()
        );
        Ok(())
    }

    #[test]
    fn lowers_transparent_registry_and_ore_helpers() -> anyhow::Result<()> {
        let base_loader = analyze_source(
            "BaseRecipeLoader.java",
            "gt5",
            None,
            r"
                package tectech.loader.recipe;
                import net.minecraft.item.ItemStack;
                class BaseRecipeLoader {
                    static ItemStack getNHCoreModItem(String name, int count) { return null; }
                }
            ",
        )?;
        let item_utils = analyze_source(
            "ItemUtils.java",
            "gt5",
            None,
            r"
                package gtPlusPlus.core.util.minecraft;
                import gregtech.api.enums.Materials;
                import gregtech.api.enums.OrePrefixes;
                import net.minecraft.item.ItemStack;
                class ItemUtils {
                    static ItemStack getOrePrefixStack(OrePrefixes prefix, Materials material, int amount) {
                        return null;
                    }
                }
            ",
        )?;
        let recipe = analyze_source(
            "Recipes.java",
            "gt5",
            None,
            r#"
                package demo;
                import static gregtech.api.recipe.RecipeMaps.compressorRecipes;
                import gregtech.api.enums.GTValues;
                import gregtech.api.enums.Materials;
                import gregtech.api.enums.OrePrefixes;
                import gtPlusPlus.core.util.minecraft.ItemUtils;
                import tectech.loader.recipe.BaseRecipeLoader;
                class Recipes { void run() {
                    GTValues.RA.stdBuilder()
                        .itemInputs(BaseRecipeLoader.getNHCoreModItem("Display", 4))
                        .itemOutputs(ItemUtils.getOrePrefixStack(OrePrefixes.rod, Materials.Iron, 3))
                        .duration(20).eut(2).addTo(compressorRecipes);
                }}
            "#,
        )?;
        let mut project = JavaProjectIndex::default();
        for analysis in [&base_loader, &item_utils, &recipe] {
            project.add_analysis(analysis);
        }
        let recovery = crate::recover_with_project(&recipe, &project);
        let recovered = recovery
            .recipes
            .first()
            .ok_or_else(|| anyhow::anyhow!("recipe missing"))?;
        let lowered = super::lower_recipe_with_project(recovered, &recipe.bindings, &project)
            .map_err(|failure| anyhow::anyhow!("lowering failed: {failure:?}"))?;

        let ItemInputSpec::Stack(input) = &lowered.item_inputs[0] else {
            return Err(anyhow::anyhow!("NHCore item was not a stack"));
        };
        assert_eq!(input.amount, 4);
        assert!(matches!(
            &input.selector,
            ItemSelector::Named { id, variant: None }
                if id.as_str() == "com.dreammaster.item.NHItemList.Display"
        ));
        assert_eq!(lowered.item_outputs[0].stack.amount, 3);
        assert!(matches!(
            &lowered.item_outputs[0].stack.selector,
            ItemSelector::MaterialForm { material, form }
                if material.as_str() == "gregtech.api.enums.Materials.Iron"
                    && form.as_str() == "gregtech.api.enums.OrePrefixes.stick"
        ));
        Ok(())
    }

    #[test]
    fn lowers_mod_registry_items_without_hiding_fallback_semantics() -> anyhow::Result<()> {
        let source = r#"
            import static gregtech.api.recipe.RecipeMaps.compressorRecipes;
            import gregtech.api.enums.GTValues;
            import gregtech.api.util.GTModHandler;
            class Recipes { void run() {
                GTValues.RA.stdBuilder()
                    .itemInputs(GTModHandler.getModItem("dreamcraft", "MysteriousCrystalLens", 0, 7))
                    .itemOutputs(GTModHandler.getIC2Item("reactorVentDiamond", 2, 1, null))
                    .duration(20)
                    .eut(2)
                    .addTo(compressorRecipes);
            }}
        "#;
        let analysis = analyze_source("Recipes.java", "gt5", None, source)?;
        let recovery = recover(&analysis);
        let recipe = recovery
            .recipes
            .first()
            .ok_or_else(|| anyhow::anyhow!("recipe missing"))?;
        let lowered = lower_recipe(recipe, &analysis.bindings)
            .map_err(|failure| anyhow::anyhow!("lowering failed: {failure:?}"))?;

        let ItemInputSpec::Stack(input) = &lowered.item_inputs[0] else {
            return Err(anyhow::anyhow!("mod input was not a stack"));
        };
        assert_eq!(input.amount, 1);
        assert_eq!(input.consumption_probability, Probability::ZERO);
        assert!(matches!(
            &input.selector,
            ItemSelector::Named {
                id,
                variant: Some(7),
            } if id.as_str() == "dreamcraft:MysteriousCrystalLens"
        ));

        assert_eq!(lowered.item_outputs[0].stack.amount, 2);
        assert!(matches!(
            &lowered.item_outputs[0].stack.selector,
            ItemSelector::Named {
                id,
                variant: Some(1),
            } if id.as_str() == "ic2:reactorVentDiamond"
        ));
        Ok(())
    }

    #[test]
    fn blocks_mod_item_with_non_null_fallback() -> anyhow::Result<()> {
        let source = r#"
            import static gregtech.api.recipe.RecipeMaps.compressorRecipes;
            import gregtech.api.enums.GTValues;
            import gregtech.api.util.GTModHandler;
            import net.minecraft.init.Blocks;
            import net.minecraft.item.ItemStack;
            class Recipes { void run() {
                GTValues.RA.stdBuilder()
                    .itemInputs(GTModHandler.getModItem(
                        "example", "missing", 1, new ItemStack(Blocks.dirt, 1)))
                    .duration(20)
                    .eut(2)
                    .addTo(compressorRecipes);
            }}
        "#;
        let analysis = analyze_source("Recipes.java", "gt5", None, source)?;
        let recovery = recover(&analysis);
        let recipe = recovery
            .recipes
            .first()
            .ok_or_else(|| anyhow::anyhow!("recipe missing"))?;
        let failure = lower_recipe(recipe, &analysis.bindings)
            .err()
            .ok_or_else(|| anyhow::anyhow!("fallback recipe unexpectedly lowered"))?;
        assert!(
            failure
                .blockers
                .iter()
                .any(|blocker| blocker.kind == RecipeLoweringBlockerKind::ItemInput)
        );
        Ok(())
    }

    #[test]
    fn lowers_assembly_line_research_to_consumed_scan_unlock() -> anyhow::Result<()> {
        let item_refer = analyze_source(
            "ItemRefer.java",
            "gt5",
            None,
            r"
                package goodgenerator.util;
                import net.minecraft.item.ItemStack;
                class ItemRefer {
                    static final ItemRefer Radiation_Protection_Plate = new ItemRefer();
                    ItemStack get(int amount) { return null; }
                }
            ",
        )?;
        let recipe = analyze_source(
            "Recipes.java",
            "gt5",
            None,
            r"
                package demo;
                import static gregtech.api.util.GTRecipeConstants.AssemblyLine;
                import static gregtech.api.util.GTRecipeConstants.RESEARCH_ITEM;
                import static gregtech.api.util.GTRecipeConstants.SCANNING;
                import goodgenerator.util.ItemRefer;
                import gregtech.api.enums.GTValues;
                import gregtech.api.enums.Materials;
                import gregtech.api.util.recipe.Scanning;
                class Recipes { void run() {
                    GTValues.RA.stdBuilder()
                        .itemInputs(
                            Materials.Iron.getDust(1),
                            Materials.Copper.getDust(1),
                            Materials.Tin.getDust(1),
                            Materials.Gold.getDust(1))
                        .fluidInputs(Materials.Water.getFluid(1000))
                        .itemOutputs(Materials.Steel.getDust(1))
                        .metadata(RESEARCH_ITEM, ItemRefer.Radiation_Protection_Plate.get(2))
                        .metadata(SCANNING, new Scanning(1200, 2048))
                        .duration(20)
                        .eut(2)
                        .addTo(AssemblyLine);
                }}
            ",
        )?;
        let mut project = JavaProjectIndex::default();
        project.add_analysis(&item_refer);
        project.add_analysis(&recipe);
        let recovery = recover_with_project(&recipe, &project);
        let recovered = recovery
            .recipes
            .first()
            .ok_or_else(|| anyhow::anyhow!("recipe missing"))?;
        let lowered = lower_recipe_with_project(recovered, &recipe.bindings, &project)
            .map_err(|failure| anyhow::anyhow!("lowering failed: {failure:?}"))?;

        let Some(RecipeUnlock::Scan {
            sample,
            duration_ticks,
            eut,
        }) = &lowered.unlock
        else {
            return Err(anyhow::anyhow!("assembly-line scan unlock missing"));
        };
        assert_eq!(*duration_ticks, 1200);
        assert_eq!(*eut, 2048);
        assert_eq!(sample.amount, 2);
        assert_eq!(sample.consumption_probability, Probability::ONE);
        assert!(matches!(
            &sample.selector,
            ItemSelector::Named { id, variant: None }
                if id.as_str() == "goodgenerator.util.ItemRefer.Radiation_Protection_Plate"
        ));
        Ok(())
    }

    #[test]
    fn assembly_line_scanning_without_research_item_stays_blocked() -> anyhow::Result<()> {
        let source = r"
            import static gregtech.api.util.GTRecipeConstants.AssemblyLine;
            import static gregtech.api.util.GTRecipeConstants.SCANNING;
            import gregtech.api.enums.GTValues;
            import gregtech.api.enums.Materials;
            import gregtech.api.util.recipe.Scanning;
            class Recipes { void run() {
                GTValues.RA.stdBuilder()
                    .itemInputs(
                        Materials.Iron.getDust(1),
                        Materials.Copper.getDust(1),
                        Materials.Tin.getDust(1),
                        Materials.Gold.getDust(1))
                    .fluidInputs(Materials.Water.getFluid(1000))
                    .itemOutputs(Materials.Steel.getDust(1))
                    .metadata(SCANNING, new Scanning(1200, 2048))
                    .duration(20)
                    .eut(2)
                    .addTo(AssemblyLine);
            }}
        ";
        let analysis = analyze_source("Recipes.java", "gt5", None, source)?;
        let recovery = recover(&analysis);
        let recipe = recovery
            .recipes
            .first()
            .ok_or_else(|| anyhow::anyhow!("recipe missing"))?;
        let failure = lower_recipe(recipe, &analysis.bindings)
            .err()
            .ok_or_else(|| {
                anyhow::anyhow!("source-dropped AssemblyLine recipe unexpectedly lowered")
            })?;
        assert!(failure.blockers.iter().any(|blocker| {
            blocker.kind == RecipeLoweringBlockerKind::Metadata
                && blocker.detail.contains("has no RESEARCH_ITEM")
        }));
        Ok(())
    }

    #[test]
    fn lowers_coil_heat_as_minimum_process_heat_kelvin() -> anyhow::Result<()> {
        let source = r"
            import static gregtech.api.recipe.RecipeMaps.compressorRecipes;
            import static gregtech.api.util.GTRecipeConstants.COIL_HEAT;
            import gregtech.api.enums.GTValues;
            import gregtech.api.enums.Materials;
            class Recipes {
                static final int BASE_HEAT = 1800;
                void run() {
                    GTValues.RA.stdBuilder()
                        .itemInputs(Materials.Iron.getDust(1))
                        .itemOutputs(Materials.Steel.getDust(1))
                        .metadata(COIL_HEAT, Recipes.BASE_HEAT + 100)
                        .duration(20)
                        .eut(2)
                        .addTo(compressorRecipes);
                }
            }
        ";
        let analysis = analyze_source("Recipes.java", "gt5", None, source)?;
        let mut project = JavaProjectIndex::default();
        project.add_analysis(&analysis);
        let recovery = recover_with_project(&analysis, &project);
        let recipe = recovery
            .recipes
            .first()
            .ok_or_else(|| anyhow::anyhow!("recipe missing"))?;
        let lowered = lower_recipe_with_project(recipe, &analysis.bindings, &project)
            .map_err(|failure| anyhow::anyhow!("lowering failed: {failure:?}"))?;
        assert_eq!(
            lowered.requirements,
            [RecipeRequirement::MinimumHeat { kelvin: 1900 }]
        );
        Ok(())
    }

    #[test]
    fn coil_heat_metadata_is_last_write_wins() -> anyhow::Result<()> {
        let source = r"
            import static gregtech.api.recipe.RecipeMaps.compressorRecipes;
            import static gregtech.api.util.GTRecipeConstants.COIL_HEAT;
            import gregtech.api.enums.GTValues;
            import gregtech.api.enums.Materials;
            class Recipes { void run() {
                GTValues.RA.stdBuilder()
                    .itemInputs(Materials.Iron.getDust(1))
                    .itemOutputs(Materials.Steel.getDust(1))
                    .metadata(COIL_HEAT, 1200)
                    .metadata(COIL_HEAT, 2400)
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
            lowered.requirements,
            [RecipeRequirement::MinimumHeat { kelvin: 2400 }]
        );
        Ok(())
    }

    #[test]
    fn lowers_casing_metadata_to_named_machine_capabilities() -> anyhow::Result<()> {
        let source = r"
            import static gregtech.api.recipe.RecipeMaps.compressorRecipes;
            import static gregtech.api.util.GTRecipeConstants.CHEMPLANT_CASING_TIER;
            import static gregtech.api.util.GTRecipeConstants.COAL_CASING_TIER;
            import static gregtech.api.util.GTRecipeConstants.PRECISE_ASSEMBLER_CASING_TIER;
            import gregtech.api.enums.GTValues;
            import gregtech.api.enums.Materials;
            class Recipes {
                void component() {
                    GTValues.RA.stdBuilder()
                        .itemInputs(Materials.Iron.getDust(1))
                        .itemOutputs(Materials.Steel.getDust(1))
                        .metadata(COAL_CASING_TIER, 6)
                        .duration(20).eut(2).addTo(compressorRecipes);
                }
                void chemical() {
                    GTValues.RA.stdBuilder()
                        .itemInputs(Materials.Iron.getDust(1))
                        .itemOutputs(Materials.Steel.getDust(1))
                        .metadata(CHEMPLANT_CASING_TIER, 3)
                        .duration(20).eut(2).addTo(compressorRecipes);
                }
                void precise() {
                    GTValues.RA.stdBuilder()
                        .itemInputs(Materials.Iron.getDust(1))
                        .itemOutputs(Materials.Steel.getDust(1))
                        .metadata(PRECISE_ASSEMBLER_CASING_TIER, 2)
                        .duration(20).eut(2).addTo(compressorRecipes);
                }
            }
        ";
        let analysis = analyze_source("Recipes.java", "gt5", None, source)?;
        let recovery = recover(&analysis);
        let expected = [
            ("machine_capability.component_assembly_line_casing", 6),
            ("machine_capability.chemical_plant_solid_casing", 3),
            ("machine_capability.precise_assembler_casing", 2),
        ];
        assert_eq!(recovery.recipes.len(), expected.len());
        for (recipe, (capability, level)) in recovery.recipes.iter().zip(expected) {
            let lowered = lower_recipe(recipe, &analysis.bindings)
                .map_err(|failure| anyhow::anyhow!("lowering failed: {failure:?}"))?;
            assert_eq!(
                lowered.requirements,
                [RecipeRequirement::MinimumCapability {
                    capability: minecraft_redox_core::content::ContentId::new(capability),
                    level,
                }]
            );
        }
        Ok(())
    }

    #[test]
    fn algae_pond_tier_is_not_a_minimum_machine_capability() -> anyhow::Result<()> {
        let source = r"
            import static gregtech.api.recipe.RecipeMaps.compressorRecipes;
            import static gregtech.api.util.GTRecipeConstants.ALGAE_POND_TIER;
            import gregtech.api.enums.GTValues;
            import gregtech.api.enums.Materials;
            class Recipes { void run() {
                GTValues.RA.stdBuilder()
                    .itemInputs(Materials.Iron.getDust(1))
                    .itemOutputs(Materials.Steel.getDust(1))
                    .metadata(ALGAE_POND_TIER, 4)
                    .duration(20).eut(2).addTo(compressorRecipes);
            }}
        ";
        let analysis = analyze_source("Recipes.java", "gt5", None, source)?;
        let recovery = recover(&analysis);
        let recipe = recovery
            .recipes
            .first()
            .ok_or_else(|| anyhow::anyhow!("recipe missing"))?;
        let Err(failure) = lower_recipe(recipe, &analysis.bindings) else {
            return Err(anyhow::anyhow!(
                "algae selector unexpectedly lowered as a requirement"
            ));
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
    fn unsupported_metadata_remains_fail_closed() -> anyhow::Result<()> {
        let source = r"
            import static gregtech.api.recipe.RecipeMaps.compressorRecipes;
            import static gregtech.api.util.GTRecipeConstants.FUEL_VALUE;
            import gregtech.api.enums.GTValues;
            import gregtech.api.enums.Materials;
            class Recipes { void run() {
                GTValues.RA.stdBuilder()
                    .itemInputs(Materials.Iron.getDust(1))
                    .itemOutputs(Materials.Steel.getDust(1))
                    .metadata(FUEL_VALUE, 100)
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
            return Err(anyhow::anyhow!("unsupported metadata unexpectedly lowered"));
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
    fn lowers_exact_item_consumption_and_production_probabilities() -> anyhow::Result<()> {
        let source = r"
            import static gregtech.api.recipe.RecipeMaps.compressorRecipes;
            import gregtech.api.enums.GTValues;
            import gregtech.api.enums.Materials;
            class Recipes { void run() {
                GTValues.RA.stdBuilder()
                    .circuit(7)
                    .itemInputs(Materials.Iron.getDust(2))
                    .inputChances(25 * 100, 75 * 100)
                    .itemOutputs(Materials.Steel.getDust(3))
                    .outputChances(12_50)
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

        let ItemInputSpec::Stack(dust) = &lowered.item_inputs[0] else {
            return Err(anyhow::anyhow!("dust input was not a stack"));
        };
        assert_eq!(
            dust.consumption_probability,
            Probability::new(1, 4).ok_or_else(|| anyhow::anyhow!("invalid probability"))?
        );

        let ItemInputSpec::Stack(circuit) = &lowered.item_inputs[1] else {
            return Err(anyhow::anyhow!("circuit input was not a stack"));
        };
        assert_eq!(circuit.consumption_probability, Probability::ZERO);

        let output = lowered
            .item_outputs
            .first()
            .ok_or_else(|| anyhow::anyhow!("output missing"))?;
        assert_eq!(
            output.production_probability,
            Probability::new(1, 8).ok_or_else(|| anyhow::anyhow!("invalid probability"))?
        );
        assert_eq!(output.stack.amount, 3);
        Ok(())
    }

    #[test]
    fn blocks_dynamic_probabilities_and_defaults_missing_trailing_entries() -> anyhow::Result<()> {
        let source = r"
            import static gregtech.api.recipe.RecipeMaps.compressorRecipes;
            import gregtech.api.enums.GTValues;
            import gregtech.api.enums.Materials;
            class Recipes {
                void dynamic(int chance) {
                    GTValues.RA.stdBuilder()
                        .itemInputs(Materials.Iron.getDust(1))
                        .itemOutputs(Materials.Steel.getDust(1))
                        .outputChances(chance)
                        .duration(20)
                        .eut(2)
                        .addTo(compressorRecipes);
                }
                void shortArray() {
                    GTValues.RA.stdBuilder()
                        .itemInputs(Materials.Iron.getDust(1))
                        .itemOutputs(Materials.Steel.getDust(1), Materials.Copper.getDust(1))
                        .outputChances(5000)
                        .duration(20)
                        .eut(2)
                        .addTo(compressorRecipes);
                }
            }
        ";
        let analysis = analyze_source("Recipes.java", "gt5", None, source)?;
        let recovery = recover(&analysis);
        assert_eq!(recovery.recipes.len(), 2);

        let Err(failure) = lower_recipe(&recovery.recipes[0], &analysis.bindings) else {
            return Err(anyhow::anyhow!("dynamic probability unexpectedly lowered"));
        };
        assert!(
            failure
                .blockers
                .iter()
                .any(|blocker| blocker.kind == RecipeLoweringBlockerKind::Chances)
        );

        let lowered = lower_recipe(&recovery.recipes[1], &analysis.bindings)
            .map_err(|failure| anyhow::anyhow!("short chance array failed: {failure:?}"))?;
        assert_eq!(
            lowered.item_outputs[0].production_probability,
            Probability::new(1, 2).ok_or_else(|| anyhow::anyhow!("invalid probability"))?
        );
        assert_eq!(
            lowered.item_outputs[1].production_probability,
            Probability::ONE
        );
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
                    .fluidOutputs(
                        FluidRegistry.getFluidStack("steam", 1_000),
                        Materials.Ice.getSolid(2 * INGOTS))
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
        assert_eq!(lowered.fluid_inputs[0].amount, 288);
        assert!(matches!(
            &lowered.fluid_inputs[0].selector,
            FluidSelector::MaterialForm { material, form }
                if material.as_str() == "gregtech.api.enums.Materials.Iron"
                    && form.as_str() == "molten"
        ));
        assert_eq!(lowered.fluid_outputs.len(), 2);
        assert_eq!(lowered.fluid_outputs[0].amount, 1_000);
        assert!(matches!(
            &lowered.fluid_outputs[0].selector,
            FluidSelector::Named { id } if id.as_str() == "steam"
        ));
        assert_eq!(lowered.fluid_outputs[1].amount, 288);
        assert!(matches!(
            &lowered.fluid_outputs[1].selector,
            FluidSelector::MaterialForm { material, form }
                if material.as_str() == "gregtech.api.enums.Materials.Ice"
                    && form.as_str() == "solid"
        ));
        Ok(())
    }
}
