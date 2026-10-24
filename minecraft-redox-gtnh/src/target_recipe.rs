use minecraft_redox_core::{
    SourceOrigin,
    content::{
        ContentId, ContentRecipe, FluidSelector, FluidStackSpec, ItemInputSpec, ItemSelector,
        ItemStackSpec, RecipeBehavior, RecipeCondition, RecipePolicySpec, RecipeSlotChance,
    },
};
use minecraft_redox_java::{JavaBindings, JavaExpression, JavaProjectIndex};
use serde::Serialize;

use crate::{
    FluidSemantic, ItemSemantic, NumericSemantic, RecipeDefinition, RecoveredExpression,
    recipe_expression::{literal_i64, resolved_reference_name},
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

    let policy = lower_recipe_policy(recipe, bindings, &mut blockers);
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
            duration_ticks,
            eut,
            item_input_consumption_chances,
            item_output_production_chances,
            policy,
        });
    }

    Err(Box::new(RecipeLoweringFailure {
        origin: recipe.origin.clone(),
        blockers,
    }))
}

fn structural_blockers(recipe: &RecipeDefinition) -> Vec<RecipeLoweringBlocker> {
    let mut blockers = Vec::new();
    if !recipe.metadata.is_empty() {
        push_blocker(
            &mut blockers,
            RecipeLoweringBlockerKind::Metadata,
            "recipe metadata is not in target recipe IR v1",
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
        FluidSemantic::RegistryFluid { .. }
        | FluidSemantic::LinkedMethod { .. }
        | FluidSemantic::LinkedValue { .. }
        | FluidSemantic::TypedExpression { .. } => None,
    }
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
        FluidSelector, ItemInputSpec, ItemSelector, RecipeBehavior, RecipeCondition,
    };
    use minecraft_redox_java::{JavaProjectIndex, analyze_source};

    use super::{RecipeLoweringBlockerKind, lower_recipe};
    use crate::recover;

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
        assert_eq!(lowered.fluid_inputs[0].amount, 288);
        assert!(matches!(
            &lowered.fluid_inputs[0].selector,
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
}
