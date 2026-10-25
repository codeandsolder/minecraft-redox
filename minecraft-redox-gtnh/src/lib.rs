//! GTNH-specific semantic recognizers built on Minecraft Redox Java structure.

mod recipe_expression;
mod scalar_expression;
mod target_recipe;

pub use recipe_expression::{FluidSemantic, ItemSemantic, RecoveredExpression};
pub use target_recipe::{
    RecipeLoweringBlocker, RecipeLoweringBlockerKind, RecipeLoweringFailure, lower_recipe,
    lower_recipe_with_project,
};

use minecraft_redox_core::SourceOrigin;
use minecraft_redox_java::{
    JavaAnalysis, JavaBindings, JavaCall, JavaCallChain, JavaExpression, JavaProjectIndex,
};
use recipe_expression::{
    duration_ticks, literal_i64, recover_fluids, recover_fluids_with_project, recover_item_inputs,
    recover_item_inputs_with_project, recover_items, recover_items_with_project,
};
use scalar_expression::scalar_i64;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BuilderCall {
    pub name: String,
    pub arguments: Vec<String>,
    pub argument_expressions: Vec<JavaExpression>,
    pub byte_start: usize,
    pub byte_end: usize,
}

impl From<&JavaCall> for BuilderCall {
    fn from(call: &JavaCall) -> Self {
        Self {
            name: call.name.clone(),
            arguments: call.arguments.clone(),
            argument_expressions: call.argument_expressions.clone(),
            byte_start: call.byte_start,
            byte_end: call.byte_end,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RecipeDefinition {
    pub origin: SourceOrigin,
    pub builder_root: String,
    pub recipe_map: String,
    pub item_inputs: Vec<String>,
    pub item_input_semantics: Vec<RecoveredExpression<ItemSemantic>>,
    pub item_outputs: Vec<String>,
    pub item_output_semantics: Vec<RecoveredExpression<ItemSemantic>>,
    pub fluid_inputs: Vec<String>,
    pub fluid_input_semantics: Vec<RecoveredExpression<FluidSemantic>>,
    pub fluid_outputs: Vec<String>,
    pub fluid_output_semantics: Vec<RecoveredExpression<FluidSemantic>>,
    pub input_chances: Vec<String>,
    pub input_chance_expressions: Vec<JavaExpression>,
    pub output_chances: Vec<String>,
    pub output_chance_expressions: Vec<JavaExpression>,
    pub duration: Option<String>,
    pub duration_ticks: Option<i64>,
    pub eut: Option<String>,
    pub eut_expression: Option<JavaExpression>,
    pub eut_value: Option<i64>,
    pub special_value: Option<String>,
    pub special: Option<String>,
    pub metadata: Vec<BuilderCall>,
    /// Explicit source presentation category, retained for archaeology/UI migration only.
    pub recipe_category: Option<BuilderCall>,
    /// Source UI-only decoration that does not change recipe execution.
    pub presentation_calls: Vec<BuilderCall>,
    /// Source registry policy that does not change the recipe once registered.
    pub registration_calls: Vec<BuilderCall>,
    /// Source runtime optimization hints with no process-semantic effect.
    pub execution_hints: Vec<BuilderCall>,
    /// Source shorthand calls that declare runtime environment requirements.
    pub environment_calls: Vec<BuilderCall>,
    /// Source execution-policy calls represented directly in target recipe semantics.
    pub execution_policy_calls: Vec<BuilderCall>,
    /// Source conditions controlling whether the recipe is registered at all.
    pub availability_calls: Vec<BuilderCall>,
    pub modifiers: Vec<BuilderCall>,
    pub post_add_calls: Vec<BuilderCall>,
    pub unhandled_calls: Vec<BuilderCall>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MaterialDefinition {
    pub origin: SourceOrigin,
    pub name: Option<String>,
    pub default_local_name: Option<String>,
    pub icon_set: Option<String>,
    pub color: Option<String>,
    pub melting_point: Option<String>,
    pub blast_furnace_temp: Option<String>,
    pub properties: Vec<BuilderCall>,
    pub unhandled_calls: Vec<BuilderCall>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GtnhRecovery {
    pub recipes: Vec<RecipeDefinition>,
    pub materials: Vec<MaterialDefinition>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct GtnhSummary {
    pub files: usize,
    pub parse_error_files: usize,
    pub parse_error_paths: Vec<String>,
    pub call_chains: usize,
    pub recipes: usize,
    pub recipes_builder_fully_normalized: usize,
    pub materials: usize,
    pub materials_builder_fully_normalized: usize,
    pub recipes_lowered: usize,
    pub recipe_lowering_blockers: BTreeMap<String, usize>,
    pub recipe_unhandled_methods: BTreeMap<String, usize>,
    pub material_unhandled_methods: BTreeMap<String, usize>,
    pub recipe_item_expressions: usize,
    pub recipe_item_semantics_resolved: usize,
    pub recipe_item_method_links: usize,
    pub recipe_item_value_links: usize,
    pub recipe_item_typed_expression_links: usize,
    pub recipe_fluid_expressions: usize,
    pub recipe_fluid_semantics_resolved: usize,
    pub recipe_fluid_method_links: usize,
    pub recipe_durations: usize,
    pub recipe_durations_resolved_to_ticks: usize,
}

impl GtnhSummary {
    #[must_use]
    pub fn merged(mut self, other: Self) -> Self {
        self.files += other.files;
        self.parse_error_files += other.parse_error_files;
        self.parse_error_paths.extend(other.parse_error_paths);
        self.call_chains += other.call_chains;
        self.recipes += other.recipes;
        self.recipes_builder_fully_normalized += other.recipes_builder_fully_normalized;
        self.materials += other.materials;
        self.materials_builder_fully_normalized += other.materials_builder_fully_normalized;
        self.recipes_lowered += other.recipes_lowered;
        merge_counts(
            &mut self.recipe_lowering_blockers,
            other.recipe_lowering_blockers,
        );
        self.recipe_item_expressions += other.recipe_item_expressions;
        self.recipe_item_semantics_resolved += other.recipe_item_semantics_resolved;
        self.recipe_item_method_links += other.recipe_item_method_links;
        self.recipe_item_value_links += other.recipe_item_value_links;
        self.recipe_item_typed_expression_links += other.recipe_item_typed_expression_links;
        self.recipe_fluid_expressions += other.recipe_fluid_expressions;
        self.recipe_fluid_semantics_resolved += other.recipe_fluid_semantics_resolved;
        self.recipe_fluid_method_links += other.recipe_fluid_method_links;
        self.recipe_durations += other.recipe_durations;
        self.recipe_durations_resolved_to_ticks += other.recipe_durations_resolved_to_ticks;
        merge_counts(
            &mut self.recipe_unhandled_methods,
            other.recipe_unhandled_methods,
        );
        merge_counts(
            &mut self.material_unhandled_methods,
            other.material_unhandled_methods,
        );
        self
    }

    pub fn absorb(&mut self, analysis: &JavaAnalysis, recovery: &GtnhRecovery) {
        self.absorb_internal(analysis, recovery, None);
    }

    pub fn absorb_with_project(
        &mut self,
        analysis: &JavaAnalysis,
        recovery: &GtnhRecovery,
        project: &JavaProjectIndex,
    ) {
        self.absorb_internal(analysis, recovery, Some(project));
    }

    fn absorb_recipe_lowering(
        &mut self,
        analysis: &JavaAnalysis,
        recovery: &GtnhRecovery,
        project: Option<&JavaProjectIndex>,
    ) {
        for recipe in &recovery.recipes {
            let lowered = project.map_or_else(
                || target_recipe::lower_recipe(recipe, &analysis.bindings),
                |project| {
                    target_recipe::lower_recipe_with_project(recipe, &analysis.bindings, project)
                },
            );
            match lowered {
                Ok(_) => self.recipes_lowered += 1,
                Err(failure) => {
                    let kinds: BTreeSet<_> = failure
                        .blockers
                        .iter()
                        .map(|blocker| blocker.kind)
                        .collect();
                    for kind in kinds {
                        *self
                            .recipe_lowering_blockers
                            .entry(kind.as_str().to_owned())
                            .or_default() += 1;
                    }
                }
            }
        }
    }

    fn absorb_internal(
        &mut self,
        analysis: &JavaAnalysis,
        recovery: &GtnhRecovery,
        project: Option<&JavaProjectIndex>,
    ) {
        self.files += 1;
        self.parse_error_files += usize::from(analysis.inventory.has_parse_errors);
        if analysis.inventory.has_parse_errors {
            self.parse_error_paths.push(analysis.inventory.path.clone());
        }
        self.call_chains += analysis.call_chains.len();
        self.recipes += recovery.recipes.len();
        self.materials += recovery.materials.len();
        self.recipes_builder_fully_normalized += recovery
            .recipes
            .iter()
            .filter(|recipe| recipe.unhandled_calls.is_empty())
            .count();
        self.materials_builder_fully_normalized += recovery
            .materials
            .iter()
            .filter(|material| material.unhandled_calls.is_empty())
            .count();
        self.absorb_recipe_lowering(analysis, recovery, project);
        for recipe in &recovery.recipes {
            self.recipe_item_expressions +=
                recipe.item_input_semantics.len() + recipe.item_output_semantics.len();
            for value in recipe
                .item_input_semantics
                .iter()
                .chain(&recipe.item_output_semantics)
            {
                match value.semantic {
                    Some(ItemSemantic::LinkedMethod { .. }) => {
                        self.recipe_item_method_links += 1;
                    }
                    Some(ItemSemantic::LinkedValue { .. }) => {
                        self.recipe_item_value_links += 1;
                    }
                    Some(ItemSemantic::TypedExpression { .. }) => {
                        self.recipe_item_typed_expression_links += 1;
                    }
                    Some(_) => self.recipe_item_semantics_resolved += 1,
                    None => {}
                }
            }
            self.recipe_fluid_expressions +=
                recipe.fluid_input_semantics.len() + recipe.fluid_output_semantics.len();
            for value in recipe
                .fluid_input_semantics
                .iter()
                .chain(&recipe.fluid_output_semantics)
            {
                match value.semantic {
                    Some(FluidSemantic::LinkedMethod { .. }) => {
                        self.recipe_fluid_method_links += 1;
                    }
                    Some(_) => self.recipe_fluid_semantics_resolved += 1,
                    None => {}
                }
            }
            self.recipe_durations += usize::from(recipe.duration.is_some());
            self.recipe_durations_resolved_to_ticks += usize::from(recipe.duration_ticks.is_some());
        }
        for call in recovery
            .recipes
            .iter()
            .flat_map(|recipe| recipe.unhandled_calls.iter())
        {
            *self
                .recipe_unhandled_methods
                .entry(call.name.clone())
                .or_default() += 1;
        }
        for call in recovery
            .materials
            .iter()
            .flat_map(|material| material.unhandled_calls.iter())
        {
            *self
                .material_unhandled_methods
                .entry(call.name.clone())
                .or_default() += 1;
        }
    }
}

fn merge_counts(target: &mut BTreeMap<String, usize>, source: BTreeMap<String, usize>) {
    for (key, count) in source {
        *target.entry(key).or_default() += count;
    }
}

#[must_use]
pub fn recover(analysis: &JavaAnalysis) -> GtnhRecovery {
    recover_internal(analysis, None)
}

#[must_use]
pub fn recover_with_project(analysis: &JavaAnalysis, project: &JavaProjectIndex) -> GtnhRecovery {
    recover_internal(analysis, Some(project))
}

fn recover_internal(analysis: &JavaAnalysis, project: Option<&JavaProjectIndex>) -> GtnhRecovery {
    GtnhRecovery {
        recipes: analysis
            .call_chains
            .iter()
            .filter_map(|chain| recognize_recipe(chain, &analysis.bindings, project))
            .collect(),
        materials: analysis
            .call_chains
            .iter()
            .filter_map(recognize_material)
            .collect(),
    }
}

fn empty_recipe_definition(
    chain: &JavaCallChain,
    bindings: &JavaBindings,
    recipe_map: String,
) -> RecipeDefinition {
    RecipeDefinition {
        origin: chain.origin.clone(),
        builder_root: bindings.resolve_reference(&chain.root),
        recipe_map,
        item_inputs: Vec::new(),
        item_input_semantics: Vec::new(),
        item_outputs: Vec::new(),
        item_output_semantics: Vec::new(),
        fluid_inputs: Vec::new(),
        fluid_input_semantics: Vec::new(),
        fluid_outputs: Vec::new(),
        fluid_output_semantics: Vec::new(),
        input_chances: Vec::new(),
        input_chance_expressions: Vec::new(),
        output_chances: Vec::new(),
        output_chance_expressions: Vec::new(),
        duration: None,
        duration_ticks: None,
        eut: None,
        eut_expression: None,
        eut_value: None,
        special_value: None,
        special: None,
        metadata: Vec::new(),
        recipe_category: None,
        presentation_calls: Vec::new(),
        registration_calls: Vec::new(),
        execution_hints: Vec::new(),
        environment_calls: Vec::new(),
        execution_policy_calls: Vec::new(),
        availability_calls: Vec::new(),
        modifiers: Vec::new(),
        post_add_calls: Vec::new(),
        unhandled_calls: Vec::new(),
    }
}

fn resolve_recipe_map(
    chain: &JavaCallChain,
    add_to: &minecraft_redox_java::JavaCall,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
) -> Option<String> {
    let raw = add_to.arguments.first()?.trim();
    let direct = bindings.resolve_reference(raw);
    let Some(project) = project else {
        return Some(direct);
    };
    if direct != raw || raw.contains('.') {
        return Some(direct);
    }
    let Some(JavaExpression::Reference(reference)) = add_to.argument_expressions.first() else {
        return Some(direct);
    };
    let Some(owner) = project
        .enclosing_method(&chain.origin.path, add_to.byte_start)
        .map(|method| method.owner.as_str())
    else {
        return Some(direct);
    };
    let Some(field) = project.resolve_field(owner, reference, Some(true)) else {
        return Some(direct);
    };
    Some(format!("{}.{}", field.owner, field.name))
}

fn recognize_recipe(
    chain: &JavaCallChain,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
) -> Option<RecipeDefinition> {
    if chain.calls.first()?.name != "stdBuilder" {
        return None;
    }
    let add_to = chain.calls.iter().rfind(|call| call.name == "addTo")?;
    let recipe_map = resolve_recipe_map(chain, add_to, bindings, project)?;
    let mut recipe = empty_recipe_definition(chain, bindings, recipe_map);

    let mut after_add = false;
    let mut integrated_circuit_inputs = Vec::new();
    for call in chain.calls.iter().skip(1) {
        if after_add {
            recipe.post_add_calls.push(call.into());
            continue;
        }
        match call.name.as_str() {
            "itemInputs" | "itemInputsUnsafe" => add_items(
                &mut recipe.item_inputs,
                &mut recipe.item_input_semantics,
                call,
                bindings,
                project,
                &chain.origin.path,
                true,
            ),
            "itemOutputs" => add_items(
                &mut recipe.item_outputs,
                &mut recipe.item_output_semantics,
                call,
                bindings,
                project,
                &chain.origin.path,
                false,
            ),
            "fluidInputs" => add_fluids(
                &mut recipe.fluid_inputs,
                &mut recipe.fluid_input_semantics,
                call,
                bindings,
                project,
                &chain.origin.path,
            ),
            "fluidOutputs" => add_fluids(
                &mut recipe.fluid_outputs,
                &mut recipe.fluid_output_semantics,
                call,
                bindings,
                project,
                &chain.origin.path,
            ),
            "inputChances" => {
                recipe.input_chances.extend(call.arguments.clone());
                recipe
                    .input_chance_expressions
                    .extend(chance_expressions(call));
            }
            "outputChances" => {
                recipe.output_chances.extend(call.arguments.clone());
                recipe
                    .output_chance_expressions
                    .extend(chance_expressions(call));
            }
            "duration" => set_duration(&mut recipe, call, bindings),
            "eut" => set_eut(&mut recipe, call, bindings, project),
            "specialValue" => recipe.special_value = one_argument(call),
            "special" => recipe.special = one_argument(call),
            "metadata" => recipe.metadata.push(call.into()),
            "recipeCategory" => recipe.recipe_category = Some(call.into()),
            "circuit" => {
                if let Some(input) = integrated_circuit_input(call) {
                    integrated_circuit_inputs.push(input);
                } else {
                    recipe.modifiers.push(call.into());
                }
            }
            "hidden" | "setNEIDesc" => recipe.presentation_calls.push(call.into()),
            "ignoreCollision" => recipe.registration_calls.push(call.into()),
            "noBuffer" => recipe.execution_hints.push(call.into()),
            "requiresCleanRoom" | "requiresLowGravity" => {
                recipe.environment_calls.push(call.into());
            }
            "fake" => recipe.execution_policy_calls.push(call.into()),
            "requireMods" => recipe.availability_calls.push(call.into()),
            "noOptimize" | "enabled" | "disabled" | "nbtSensitive" => {
                recipe.modifiers.push(call.into());
            }
            "addTo" => after_add = true,
            _ => recipe.unhandled_calls.push(call.into()),
        }
    }
    for input in integrated_circuit_inputs {
        recipe.item_inputs.push(input.source.clone());
        recipe.item_input_semantics.push(input);
    }
    Some(recipe)
}

fn chance_expressions(call: &JavaCall) -> Vec<JavaExpression> {
    if let [JavaExpression::Array { elements, .. }] = call.argument_expressions.as_slice() {
        return elements.clone();
    }
    call.argument_expressions.clone()
}

fn integrated_circuit_input(call: &JavaCall) -> Option<RecoveredExpression<ItemSemantic>> {
    let source_config = call.arguments.first()?;
    let config = call.argument_expressions.first()?.clone();
    let item = String::from("gregtech.api.enums.ItemList.Circuit_Integrated");
    let accessor = String::from("getWithDamage");
    let arguments = vec![JavaExpression::Literal(String::from("0")), config];

    Some(RecoveredExpression {
        source: format!("GTUtility.getIntegratedCircuit({source_config})"),
        expression: JavaExpression::Call {
            receiver: Some(Box::new(JavaExpression::Reference(item.clone()))),
            method: accessor.clone(),
            arguments: arguments.clone(),
        },
        semantic: Some(ItemSemantic::ItemList {
            item,
            accessor,
            arguments,
        }),
    })
}

fn set_duration(recipe: &mut RecipeDefinition, call: &JavaCall, bindings: &JavaBindings) {
    recipe.duration = one_argument(call);
    recipe.duration_ticks =
        one_expression(call).and_then(|expression| duration_ticks(expression, bindings));
}

fn set_eut(
    recipe: &mut RecipeDefinition,
    call: &JavaCall,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
) {
    recipe.eut = one_argument(call);
    recipe.eut_expression = one_expression(call).cloned();
    recipe.eut_value = one_expression(call).and_then(|expression| {
        project.map_or_else(
            || literal_i64(expression),
            |project| scalar_i64(expression, bindings, project),
        )
    });
}

fn recognize_material(chain: &JavaCallChain) -> Option<MaterialDefinition> {
    if !chain.root.starts_with("new MaterialBuilder")
        || chain.calls.last()?.name != "constructMaterial"
    {
        return None;
    }
    let mut material = MaterialDefinition {
        origin: chain.origin.clone(),
        name: None,
        default_local_name: None,
        icon_set: None,
        color: None,
        melting_point: None,
        blast_furnace_temp: None,
        properties: Vec::new(),
        unhandled_calls: Vec::new(),
    };

    for call in &chain.calls {
        match call.name.as_str() {
            "setName" => material.name = one_argument(call),
            "setDefaultLocalName" => material.default_local_name = one_argument(call),
            "setIconSet" => material.icon_set = one_argument(call),
            "setColor" | "setARGB" => material.color = one_argument(call),
            "setMeltingPoint" => material.melting_point = one_argument(call),
            "setBlastFurnaceTemp" => material.blast_furnace_temp = one_argument(call),
            "constructMaterial" => {}
            name if is_known_material_property(name) => material.properties.push(call.into()),
            _ => material.unhandled_calls.push(call.into()),
        }
    }
    Some(material)
}

fn add_items(
    raw: &mut Vec<String>,
    semantics: &mut Vec<RecoveredExpression<ItemSemantic>>,
    call: &JavaCall,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
    source_path: &str,
    input_context: bool,
) {
    raw.extend(call.arguments.clone());
    let recovered = match (project, input_context) {
        (Some(project), true) => recover_item_inputs_with_project(
            &call.arguments,
            &call.argument_expressions,
            bindings,
            project,
            source_path,
            call.byte_start,
        ),
        (Some(project), false) => recover_items_with_project(
            &call.arguments,
            &call.argument_expressions,
            bindings,
            project,
            source_path,
            call.byte_start,
        ),
        (None, true) => recover_item_inputs(&call.arguments, &call.argument_expressions, bindings),
        (None, false) => recover_items(&call.arguments, &call.argument_expressions, bindings),
    };
    semantics.extend(recovered);
}

fn add_fluids(
    raw: &mut Vec<String>,
    semantics: &mut Vec<RecoveredExpression<FluidSemantic>>,
    call: &JavaCall,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
    source_path: &str,
) {
    raw.extend(call.arguments.clone());
    let recovered = project.map_or_else(
        || recover_fluids(&call.arguments, &call.argument_expressions, bindings),
        |project| {
            recover_fluids_with_project(
                &call.arguments,
                &call.argument_expressions,
                bindings,
                project,
                source_path,
                call.byte_start,
            )
        },
    );
    semantics.extend(recovered);
}

fn one_expression(call: &JavaCall) -> Option<&JavaExpression> {
    if call.argument_expressions.len() == 1 {
        call.argument_expressions.first()
    } else {
        None
    }
}

fn one_argument(call: &JavaCall) -> Option<String> {
    if call.arguments.len() == 1 {
        call.arguments.first().cloned()
    } else {
        None
    }
}

fn is_known_material_property(name: &str) -> bool {
    matches!(
        name,
        "setTool"
            | "setBlastFurnaceRequired"
            | "setDensityMultiplier"
            | "setDensityDivider"
            | "setDensity"
            | "setDurability"
            | "setToolQuality"
            | "setToolSpeed"
            | "setHeatDamage"
            | "setElement"
            | "setFuel"
            | "setFuelType"
            | "setFuelPower"
            | "setOreMultiplier"
            | "setByProductMultiplier"
            | "setSmeltingMultiplier"
            | "setProcessingMaterialTierEU"
            | "setMiningLevel"
            | "setAutoGenerateBlastFurnaceRecipes"
            | "setAutoGeneratedVacuumFreezerRecipe"
            | "setAutoGeneratedRecycleRecipes"
            | "setChemicalFormula"
            | "setToolEnchantment"
            | "setArmorEnchantment"
            | "setDirectSmelting"
            | "setSmeltingInto"
            | "setMaceratingInto"
            | "setArcSmeltingInto"
            | "setArcSmeltingIntoWithGas"
            | "setFlavorText"
            | "setUnifiable"
            | "setTurbine"
            | "setAliasKey"
            | "setLiquidTemperature"
            | "setGasTemperature"
            | "setMoltenARGB"
            | "setTransparent"
            | "setRadioactive"
            | "hasGlowingOre"
            | "withCustomTextures"
            | "addDustItems"
            | "addMetalItems"
            | "addGemItems"
            | "addFluid"
            | "addGas"
            | "addMolten"
            | "addPlasma"
            | "addCell"
            | "addOre"
            | "addOreItems"
            | "addOrePrefix"
            | "removeOrePrefix"
            | "addOreByproduct"
            | "addOreByProducts"
            | "addToolHeadItems"
            | "addGearItems"
            | "addMaterial"
            | "addAspect"
            | "addSubTag"
            | "addElectrolyzerRecipe"
            | "addCentrifugeRecipe"
            | "addCrackingRecipes"
            | "addEmpty"
    )
}

#[cfg(test)]
mod tests {
    use super::{GtnhSummary, ItemSemantic, recover, recover_with_project};
    use minecraft_redox_java::{JavaExpression, JavaProjectIndex, analyze_source};

    #[test]
    fn recognizes_gt_recipe_builder() -> anyhow::Result<()> {
        let source = r"
            import static gregtech.api.recipe.RecipeMaps.maceratorRecipes;
            import static gregtech.api.util.GTRecipeBuilder.SECONDS;
            import gregtech.api.recipe.RecipeCategories;
            class Recipes { void run() {
                GTValues.RA.stdBuilder()
                    .circuit(7)
                    .itemInputs(ItemList.Cell_Water.get(1))
                    .itemOutputs(new ItemStack(Blocks.snow, 1))
                    .recipeCategory(RecipeCategories.maceratorRecycling)
                    .duration(15 * SECONDS)
                    .eut(2)
                    .addTo(maceratorRecipes);
            }}
        ";
        let analysis = analyze_source("Recipes.java", "gt5", None, source)?;
        let recovery = recover(&analysis);
        assert_eq!(recovery.recipes.len(), 1);
        let recipe = &recovery.recipes[0];
        assert_eq!(
            recipe.recipe_map,
            "gregtech.api.recipe.RecipeMaps.maceratorRecipes"
        );
        assert_eq!(recipe.duration.as_deref(), Some("15 * SECONDS"));
        assert_eq!(recipe.duration_ticks, Some(300));
        assert_eq!(recipe.item_input_semantics.len(), 2);
        assert!(recipe.item_input_semantics[0].semantic.is_some());
        assert!(matches!(
            recipe.item_input_semantics[1].semantic.as_ref(),
            Some(ItemSemantic::ItemList {
                item,
                accessor,
                arguments,
            }) if item == "gregtech.api.enums.ItemList.Circuit_Integrated"
                && accessor == "getWithDamage"
                && matches!(arguments.as_slice(), [
                    JavaExpression::Literal(amount),
                    JavaExpression::Literal(config),
                ] if amount == "0" && config == "7")
        ));
        assert_eq!(recipe.metadata.as_slice(), []);
        assert!(matches!(
            recipe.recipe_category.as_ref(),
            Some(call) if call.arguments == ["RecipeCategories.maceratorRecycling"]
        ));
        assert_eq!(recipe.modifiers.as_slice(), []);
        assert_eq!(recipe.eut.as_deref(), Some("2"));
        assert_eq!(recipe.unhandled_calls.len(), 0);
        let mut summary = GtnhSummary::default();
        summary.absorb(&analysis, &recovery);
        assert_eq!(summary.recipes_lowered, 1);
        assert!(summary.recipe_lowering_blockers.is_empty());
        Ok(())
    }

    #[test]
    fn resolves_same_class_static_recipe_map_identity() -> anyhow::Result<()> {
        let source = r"
            package kubatech.loaders;
            class DEFCRecipes {
                static final Object fusionCraftingRecipes = null;
                void run() {
                    GTValues.RA.stdBuilder()
                        .duration(20)
                        .eut(2)
                        .addTo(fusionCraftingRecipes);
                }
            }
        ";
        let analysis = analyze_source("DEFCRecipes.java", "gt5", None, source)?;
        let mut project = JavaProjectIndex::default();
        project.add_analysis(&analysis);
        let recovery = recover_with_project(&analysis, &project);
        assert_eq!(recovery.recipes.len(), 1);
        assert_eq!(
            recovery.recipes[0].recipe_map,
            "kubatech.loaders.DEFCRecipes.fusionCraftingRecipes"
        );
        Ok(())
    }

    #[test]
    fn recognizes_material_builder() -> anyhow::Result<()> {
        let source = r#"
            class MaterialsInit { void run() {
                Materials foo = new MaterialBuilder().setName("Foo")
                    .setIconSet(TextureSet.SET_METALLIC)
                    .setMeltingPoint(1000)
                    .setBlastFurnaceRequired(true)
                    .constructMaterial();
            }}
        "#;
        let analysis = analyze_source("MaterialsInit.java", "gt5", None, source)?;
        let recovery = recover(&analysis);
        assert_eq!(recovery.materials.len(), 1);
        let material = &recovery.materials[0];
        assert_eq!(material.name.as_deref(), Some("\"Foo\""));
        assert_eq!(material.unhandled_calls.len(), 0);
        Ok(())
    }
}
