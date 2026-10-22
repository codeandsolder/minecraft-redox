//! GTNH-specific semantic recognizers built on Minecraft Redox Java structure.

mod recipe_expression;

pub use recipe_expression::{FluidSemantic, ItemSemantic, RecoveredExpression};

use minecraft_redox_core::SourceOrigin;
use minecraft_redox_java::{JavaAnalysis, JavaBindings, JavaCall, JavaCallChain, JavaExpression};
use recipe_expression::{duration_ticks, recover_fluids, recover_items};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BuilderCall {
    pub name: String,
    pub arguments: Vec<String>,
}

impl From<&JavaCall> for BuilderCall {
    fn from(call: &JavaCall) -> Self {
        Self {
            name: call.name.clone(),
            arguments: call.arguments.clone(),
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
    pub output_chances: Vec<String>,
    pub duration: Option<String>,
    pub duration_ticks: Option<i64>,
    pub eut: Option<String>,
    pub special_value: Option<String>,
    pub special: Option<String>,
    pub metadata: Vec<BuilderCall>,
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
    pub recipe_unhandled_methods: BTreeMap<String, usize>,
    pub material_unhandled_methods: BTreeMap<String, usize>,
    pub recipe_item_expressions: usize,
    pub recipe_item_semantics_resolved: usize,
    pub recipe_fluid_expressions: usize,
    pub recipe_fluid_semantics_resolved: usize,
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
        self.recipe_item_expressions += other.recipe_item_expressions;
        self.recipe_item_semantics_resolved += other.recipe_item_semantics_resolved;
        self.recipe_fluid_expressions += other.recipe_fluid_expressions;
        self.recipe_fluid_semantics_resolved += other.recipe_fluid_semantics_resolved;
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
        for recipe in &recovery.recipes {
            self.recipe_item_expressions +=
                recipe.item_input_semantics.len() + recipe.item_output_semantics.len();
            self.recipe_item_semantics_resolved += recipe
                .item_input_semantics
                .iter()
                .chain(&recipe.item_output_semantics)
                .filter(|value| value.semantic.is_some())
                .count();
            self.recipe_fluid_expressions +=
                recipe.fluid_input_semantics.len() + recipe.fluid_output_semantics.len();
            self.recipe_fluid_semantics_resolved += recipe
                .fluid_input_semantics
                .iter()
                .chain(&recipe.fluid_output_semantics)
                .filter(|value| value.semantic.is_some())
                .count();
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
    GtnhRecovery {
        recipes: analysis
            .call_chains
            .iter()
            .filter_map(|chain| recognize_recipe(chain, &analysis.bindings))
            .collect(),
        materials: analysis
            .call_chains
            .iter()
            .filter_map(recognize_material)
            .collect(),
    }
}

fn recognize_recipe(chain: &JavaCallChain, bindings: &JavaBindings) -> Option<RecipeDefinition> {
    if chain.calls.first()?.name != "stdBuilder" {
        return None;
    }
    let add_to = chain.calls.iter().rfind(|call| call.name == "addTo")?;
    let recipe_map = bindings.resolve_reference(add_to.arguments.first()?);
    let mut recipe = RecipeDefinition {
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
        output_chances: Vec::new(),
        duration: None,
        duration_ticks: None,
        eut: None,
        special_value: None,
        special: None,
        metadata: Vec::new(),
        modifiers: Vec::new(),
        post_add_calls: Vec::new(),
        unhandled_calls: Vec::new(),
    };

    let mut after_add = false;
    for call in chain.calls.iter().skip(1) {
        if after_add {
            recipe.post_add_calls.push(call.into());
            continue;
        }
        match call.name.as_str() {
            "itemInputs" => add_items(
                &mut recipe.item_inputs,
                &mut recipe.item_input_semantics,
                call,
                bindings,
            ),
            "itemInputsUnsafe" => {
                add_items(
                    &mut recipe.item_inputs,
                    &mut recipe.item_input_semantics,
                    call,
                    bindings,
                );
                recipe.modifiers.push(call.into());
            }
            "itemOutputs" => add_items(
                &mut recipe.item_outputs,
                &mut recipe.item_output_semantics,
                call,
                bindings,
            ),
            "fluidInputs" => add_fluids(
                &mut recipe.fluid_inputs,
                &mut recipe.fluid_input_semantics,
                call,
                bindings,
            ),
            "fluidOutputs" => add_fluids(
                &mut recipe.fluid_outputs,
                &mut recipe.fluid_output_semantics,
                call,
                bindings,
            ),
            "inputChances" => recipe.input_chances.extend(call.arguments.clone()),
            "outputChances" => recipe.output_chances.extend(call.arguments.clone()),
            "duration" => {
                recipe.duration = one_argument(call);
                recipe.duration_ticks = one_expression(call)
                    .and_then(|expression| duration_ticks(expression, bindings));
            }
            "eut" => recipe.eut = one_argument(call),
            "specialValue" => recipe.special_value = one_argument(call),
            "special" => recipe.special = one_argument(call),
            "metadata" | "recipeCategory" => recipe.metadata.push(call.into()),
            "noOptimize" | "ignoreCollision" | "fake" | "enabled" | "disabled" | "hidden"
            | "requiresCleanRoom" | "requiresLowGravity" | "requireMods" | "nbtSensitive"
            | "noBuffer" | "setNEIDesc" | "circuit" => recipe.modifiers.push(call.into()),
            "addTo" => after_add = true,
            _ => recipe.unhandled_calls.push(call.into()),
        }
    }
    Some(recipe)
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
) {
    raw.extend(call.arguments.clone());
    semantics.extend(recover_items(
        &call.arguments,
        &call.argument_expressions,
        bindings,
    ));
}

fn add_fluids(
    raw: &mut Vec<String>,
    semantics: &mut Vec<RecoveredExpression<FluidSemantic>>,
    call: &JavaCall,
    bindings: &JavaBindings,
) {
    raw.extend(call.arguments.clone());
    semantics.extend(recover_fluids(
        &call.arguments,
        &call.argument_expressions,
        bindings,
    ));
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
    use super::recover;
    use minecraft_redox_java::analyze_source;

    #[test]
    fn recognizes_gt_recipe_builder() -> anyhow::Result<()> {
        let source = r"
            import static gregtech.api.util.GTRecipeBuilder.SECONDS;
            class Recipes { void run() {
                GTValues.RA.stdBuilder()
                    .itemInputs(ItemList.Cell_Water.get(1))
                    .itemOutputs(new ItemStack(Blocks.snow, 1))
                    .duration(15 * SECONDS)
                    .eut(2)
                    .addTo(compressorRecipes);
            }}
        ";
        let analysis = analyze_source("Recipes.java", "gt5", None, source)?;
        let recovery = recover(&analysis);
        assert_eq!(recovery.recipes.len(), 1);
        let recipe = &recovery.recipes[0];
        assert_eq!(recipe.recipe_map, "compressorRecipes");
        assert_eq!(recipe.duration.as_deref(), Some("15 * SECONDS"));
        assert_eq!(recipe.duration_ticks, Some(300));
        assert_eq!(recipe.item_input_semantics.len(), 1);
        assert!(recipe.item_input_semantics[0].semantic.is_some());
        assert_eq!(recipe.eut.as_deref(), Some("2"));
        assert_eq!(recipe.unhandled_calls.len(), 0);
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
