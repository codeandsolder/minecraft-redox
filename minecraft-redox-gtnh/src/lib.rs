//! GTNH-specific semantic recognizers built on Minecraft Redox Java structure.

mod recipe_expression;

pub use recipe_expression::{
    ExpressionSemantic, FluidSemantic, ItemSemantic, NumericSemantic, RecoveredExpression,
};

use minecraft_redox_core::SourceOrigin;
use minecraft_redox_java::{
    JavaAnalysis, JavaBindings, JavaCall, JavaCallChain, JavaExpression, JavaProjectIndex,
    generic_type_argument,
};
use recipe_expression::{
    recover_expression, recover_fluids, recover_fluids_with_project, recover_item_inputs,
    recover_item_inputs_with_project, recover_items, recover_items_with_project,
    recover_numeric_expression,
};
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
pub struct RecipeMetadataDefinition {
    pub key_source: String,
    pub key_expression: JavaExpression,
    pub key_type: Option<String>,
    pub expected_value_type: Option<String>,
    pub value_source: String,
    pub value_expression: JavaExpression,
    pub value_type: Option<String>,
    pub value_semantic: ExpressionSemantic,
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
    pub input_chance_semantics: Vec<RecoveredExpression<NumericSemantic>>,
    pub output_chances: Vec<String>,
    pub output_chance_semantics: Vec<RecoveredExpression<NumericSemantic>>,
    pub duration: Option<String>,
    pub duration_expression: Option<JavaExpression>,
    pub duration_semantic: Option<NumericSemantic>,
    pub duration_ticks: Option<i64>,
    pub eut: Option<String>,
    pub eut_expression: Option<JavaExpression>,
    pub eut_semantic: Option<NumericSemantic>,
    pub eut_value: Option<i64>,
    pub special_value: Option<String>,
    pub special_value_expression: Option<JavaExpression>,
    pub special_value_semantic: Option<NumericSemantic>,
    pub special_value_exact: Option<i64>,
    pub special: Option<String>,
    pub special_expression: Option<JavaExpression>,
    pub special_semantic: Option<ExpressionSemantic>,
    pub metadata: Vec<BuilderCall>,
    pub metadata_semantics: Vec<RecipeMetadataDefinition>,
    pub recipe_categories: Vec<RecoveredExpression<ExpressionSemantic>>,
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
    pub recipe_item_method_links: usize,
    pub recipe_item_value_links: usize,
    pub recipe_item_typed_expression_links: usize,
    pub recipe_fluid_expressions: usize,
    pub recipe_fluid_semantics_resolved: usize,
    pub recipe_fluid_method_links: usize,
    pub recipe_fluid_value_links: usize,
    pub recipe_fluid_typed_expression_links: usize,
    pub recipe_durations: usize,
    pub recipe_durations_resolved_to_ticks: usize,
    pub recipe_duration_method_links: usize,
    pub recipe_duration_value_links: usize,
    pub recipe_duration_typed_expressions: usize,
    pub recipe_duration_structured_expressions: usize,
    pub recipe_duration_opaque_expressions: usize,
    pub recipe_eut_expressions: usize,
    pub recipe_eut_resolved_to_value: usize,
    pub recipe_eut_method_links: usize,
    pub recipe_eut_value_links: usize,
    pub recipe_eut_typed_expressions: usize,
    pub recipe_eut_structured_expressions: usize,
    pub recipe_eut_opaque_expressions: usize,
    pub recipe_special_values: usize,
    pub recipe_special_values_exact: usize,
    pub recipe_special_value_method_links: usize,
    pub recipe_special_value_value_links: usize,
    pub recipe_special_value_typed_expressions: usize,
    pub recipe_special_value_structured_expressions: usize,
    pub recipe_special_value_opaque_expressions: usize,
    pub recipe_metadata_entries: usize,
    pub recipe_metadata_expected_types: usize,
    pub recipe_metadata_value_method_links: usize,
    pub recipe_metadata_value_links: usize,
    pub recipe_metadata_typed_values: usize,
    pub recipe_metadata_structured_values: usize,
    pub recipe_metadata_opaque_values: usize,
    pub recipe_categories: usize,
    pub recipe_specials: usize,
    pub recipe_special_method_links: usize,
    pub recipe_special_value_links: usize,
    pub recipe_special_typed_expressions: usize,
    pub recipe_special_structured_expressions: usize,
    pub recipe_special_opaque_expressions: usize,
    pub recipe_chance_expressions: usize,
    pub recipe_chances_exact: usize,
    pub recipe_chance_method_links: usize,
    pub recipe_chance_value_links: usize,
    pub recipe_chance_typed_expressions: usize,
    pub recipe_chance_structured_expressions: usize,
    pub recipe_chance_opaque_expressions: usize,
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
        self.recipe_item_method_links += other.recipe_item_method_links;
        self.recipe_item_value_links += other.recipe_item_value_links;
        self.recipe_item_typed_expression_links += other.recipe_item_typed_expression_links;
        self.recipe_fluid_expressions += other.recipe_fluid_expressions;
        self.recipe_fluid_semantics_resolved += other.recipe_fluid_semantics_resolved;
        self.recipe_fluid_method_links += other.recipe_fluid_method_links;
        self.recipe_fluid_value_links += other.recipe_fluid_value_links;
        self.recipe_fluid_typed_expression_links += other.recipe_fluid_typed_expression_links;
        self.recipe_durations += other.recipe_durations;
        self.recipe_durations_resolved_to_ticks += other.recipe_durations_resolved_to_ticks;
        self.recipe_duration_method_links += other.recipe_duration_method_links;
        self.recipe_duration_value_links += other.recipe_duration_value_links;
        self.recipe_duration_typed_expressions += other.recipe_duration_typed_expressions;
        self.recipe_duration_structured_expressions += other.recipe_duration_structured_expressions;
        self.recipe_duration_opaque_expressions += other.recipe_duration_opaque_expressions;
        self.recipe_eut_expressions += other.recipe_eut_expressions;
        self.recipe_eut_resolved_to_value += other.recipe_eut_resolved_to_value;
        self.recipe_eut_method_links += other.recipe_eut_method_links;
        self.recipe_eut_value_links += other.recipe_eut_value_links;
        self.recipe_eut_typed_expressions += other.recipe_eut_typed_expressions;
        self.recipe_eut_structured_expressions += other.recipe_eut_structured_expressions;
        self.recipe_eut_opaque_expressions += other.recipe_eut_opaque_expressions;
        self.recipe_special_values += other.recipe_special_values;
        self.recipe_special_values_exact += other.recipe_special_values_exact;
        self.recipe_special_value_method_links += other.recipe_special_value_method_links;
        self.recipe_special_value_value_links += other.recipe_special_value_value_links;
        self.recipe_special_value_typed_expressions += other.recipe_special_value_typed_expressions;
        self.recipe_special_value_structured_expressions +=
            other.recipe_special_value_structured_expressions;
        self.recipe_special_value_opaque_expressions +=
            other.recipe_special_value_opaque_expressions;
        self.recipe_metadata_entries += other.recipe_metadata_entries;
        self.recipe_metadata_expected_types += other.recipe_metadata_expected_types;
        self.recipe_metadata_value_method_links += other.recipe_metadata_value_method_links;
        self.recipe_metadata_value_links += other.recipe_metadata_value_links;
        self.recipe_metadata_typed_values += other.recipe_metadata_typed_values;
        self.recipe_metadata_structured_values += other.recipe_metadata_structured_values;
        self.recipe_metadata_opaque_values += other.recipe_metadata_opaque_values;
        self.recipe_categories += other.recipe_categories;
        self.recipe_specials += other.recipe_specials;
        self.recipe_special_method_links += other.recipe_special_method_links;
        self.recipe_special_value_links += other.recipe_special_value_links;
        self.recipe_special_typed_expressions += other.recipe_special_typed_expressions;
        self.recipe_special_structured_expressions += other.recipe_special_structured_expressions;
        self.recipe_special_opaque_expressions += other.recipe_special_opaque_expressions;
        self.recipe_chance_expressions += other.recipe_chance_expressions;
        self.recipe_chances_exact += other.recipe_chances_exact;
        self.recipe_chance_method_links += other.recipe_chance_method_links;
        self.recipe_chance_value_links += other.recipe_chance_value_links;
        self.recipe_chance_typed_expressions += other.recipe_chance_typed_expressions;
        self.recipe_chance_structured_expressions += other.recipe_chance_structured_expressions;
        self.recipe_chance_opaque_expressions += other.recipe_chance_opaque_expressions;
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
                    Some(FluidSemantic::LinkedValue { .. }) => {
                        self.recipe_fluid_value_links += 1;
                    }
                    Some(FluidSemantic::TypedExpression { .. }) => {
                        self.recipe_fluid_typed_expression_links += 1;
                    }
                    Some(_) => self.recipe_fluid_semantics_resolved += 1,
                    None => {}
                }
            }
            self.absorb_duration(recipe);
            self.absorb_eut(recipe);
            self.absorb_special_value(recipe);
            self.absorb_metadata(recipe);
            self.absorb_special(recipe);
            self.absorb_chances(recipe);
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

    fn absorb_duration(&mut self, recipe: &RecipeDefinition) {
        self.recipe_durations += usize::from(recipe.duration.is_some());
        let Some(duration) = &recipe.duration_semantic else {
            return;
        };
        match duration {
            NumericSemantic::ExactValue { .. } => self.recipe_durations_resolved_to_ticks += 1,
            NumericSemantic::LinkedMethod { .. } => self.recipe_duration_method_links += 1,
            NumericSemantic::LinkedValue { .. } => self.recipe_duration_value_links += 1,
            NumericSemantic::TypedExpression { .. } => {
                self.recipe_duration_typed_expressions += 1;
            }
            NumericSemantic::StructuredExpression { .. } => {
                self.recipe_duration_structured_expressions += 1;
            }
            NumericSemantic::OpaqueExpression { .. } => {
                self.recipe_duration_opaque_expressions += 1;
            }
        }
    }

    fn absorb_eut(&mut self, recipe: &RecipeDefinition) {
        self.recipe_eut_expressions += usize::from(recipe.eut.is_some());
        let Some(eut) = &recipe.eut_semantic else {
            return;
        };
        match eut {
            NumericSemantic::ExactValue { .. } => self.recipe_eut_resolved_to_value += 1,
            NumericSemantic::LinkedMethod { .. } => self.recipe_eut_method_links += 1,
            NumericSemantic::LinkedValue { .. } => self.recipe_eut_value_links += 1,
            NumericSemantic::TypedExpression { .. } => self.recipe_eut_typed_expressions += 1,
            NumericSemantic::StructuredExpression { .. } => {
                self.recipe_eut_structured_expressions += 1;
            }
            NumericSemantic::OpaqueExpression { .. } => self.recipe_eut_opaque_expressions += 1,
        }
    }

    fn absorb_special_value(&mut self, recipe: &RecipeDefinition) {
        self.recipe_special_values += usize::from(recipe.special_value.is_some());
        let Some(value) = &recipe.special_value_semantic else {
            return;
        };
        match value {
            NumericSemantic::ExactValue { .. } => self.recipe_special_values_exact += 1,
            NumericSemantic::LinkedMethod { .. } => self.recipe_special_value_method_links += 1,
            NumericSemantic::LinkedValue { .. } => self.recipe_special_value_value_links += 1,
            NumericSemantic::TypedExpression { .. } => {
                self.recipe_special_value_typed_expressions += 1;
            }
            NumericSemantic::StructuredExpression { .. } => {
                self.recipe_special_value_structured_expressions += 1;
            }
            NumericSemantic::OpaqueExpression { .. } => {
                self.recipe_special_value_opaque_expressions += 1;
            }
        }
    }

    fn absorb_metadata(&mut self, recipe: &RecipeDefinition) {
        self.recipe_metadata_entries += recipe.metadata_semantics.len();
        self.recipe_categories += recipe.recipe_categories.len();
        for metadata in &recipe.metadata_semantics {
            self.recipe_metadata_expected_types +=
                usize::from(metadata.expected_value_type.is_some());
            match metadata.value_semantic {
                ExpressionSemantic::LinkedMethod { .. } => {
                    self.recipe_metadata_value_method_links += 1;
                }
                ExpressionSemantic::LinkedValue { .. } => self.recipe_metadata_value_links += 1,
                ExpressionSemantic::TypedExpression { .. } => {
                    self.recipe_metadata_typed_values += 1;
                }
                ExpressionSemantic::StructuredExpression { .. } => {
                    self.recipe_metadata_structured_values += 1;
                }
                ExpressionSemantic::OpaqueExpression { .. } => {
                    self.recipe_metadata_opaque_values += 1;
                }
            }
        }
    }

    fn absorb_special(&mut self, recipe: &RecipeDefinition) {
        self.recipe_specials += usize::from(recipe.special.is_some());
        let Some(value) = &recipe.special_semantic else {
            return;
        };
        match value {
            ExpressionSemantic::LinkedMethod { .. } => self.recipe_special_method_links += 1,
            ExpressionSemantic::LinkedValue { .. } => self.recipe_special_value_links += 1,
            ExpressionSemantic::TypedExpression { .. } => {
                self.recipe_special_typed_expressions += 1;
            }
            ExpressionSemantic::StructuredExpression { .. } => {
                self.recipe_special_structured_expressions += 1;
            }
            ExpressionSemantic::OpaqueExpression { .. } => {
                self.recipe_special_opaque_expressions += 1;
            }
        }
    }

    fn absorb_chances(&mut self, recipe: &RecipeDefinition) {
        for chance in recipe
            .input_chance_semantics
            .iter()
            .chain(&recipe.output_chance_semantics)
        {
            self.recipe_chance_expressions += 1;
            let Some(value) = &chance.semantic else {
                continue;
            };
            match value {
                NumericSemantic::ExactValue { .. } => self.recipe_chances_exact += 1,
                NumericSemantic::LinkedMethod { .. } => self.recipe_chance_method_links += 1,
                NumericSemantic::LinkedValue { .. } => self.recipe_chance_value_links += 1,
                NumericSemantic::TypedExpression { .. } => {
                    self.recipe_chance_typed_expressions += 1;
                }
                NumericSemantic::StructuredExpression { .. } => {
                    self.recipe_chance_structured_expressions += 1;
                }
                NumericSemantic::OpaqueExpression { .. } => {
                    self.recipe_chance_opaque_expressions += 1;
                }
            }
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

fn new_recipe_definition(
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
        input_chance_semantics: Vec::new(),
        output_chances: Vec::new(),
        output_chance_semantics: Vec::new(),
        duration: None,
        duration_expression: None,
        duration_semantic: None,
        duration_ticks: None,
        eut: None,
        eut_expression: None,
        eut_semantic: None,
        eut_value: None,
        special_value: None,
        special_value_expression: None,
        special_value_semantic: None,
        special_value_exact: None,
        special: None,
        special_expression: None,
        special_semantic: None,
        metadata: Vec::new(),
        metadata_semantics: Vec::new(),
        recipe_categories: Vec::new(),
        modifiers: Vec::new(),
        post_add_calls: Vec::new(),
        unhandled_calls: Vec::new(),
    }
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
    let recipe_map = bindings.resolve_reference(add_to.arguments.first()?);
    let mut recipe = new_recipe_definition(chain, bindings, recipe_map);

    let mut after_add = false;
    for call in chain.calls.iter().skip(1) {
        if after_add {
            recipe.post_add_calls.push(call.into());
            continue;
        }
        match call.name.as_str() {
            "itemInputs" => add_recipe_items(
                &mut recipe,
                call,
                bindings,
                project,
                &chain.origin.path,
                true,
            ),
            "itemInputsUnsafe" => {
                add_unsafe_recipe_inputs(&mut recipe, call, bindings, project, &chain.origin.path);
            }
            "itemOutputs" => add_recipe_items(
                &mut recipe,
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
            "inputChances" | "outputChances" => {
                add_recipe_chance_call(&mut recipe, call, bindings, project, &chain.origin.path);
            }
            "duration" => set_duration(&mut recipe, call, bindings, project, &chain.origin.path),
            "eut" => set_eut(&mut recipe, call, bindings, project, &chain.origin.path),
            "specialValue" => {
                set_special_value(&mut recipe, call, bindings, project, &chain.origin.path);
            }
            "special" => set_special(&mut recipe, call, bindings, project, &chain.origin.path),
            "metadata" | "recipeCategory" => {
                add_recipe_metadata_call(&mut recipe, call, bindings, project, &chain.origin.path);
            }
            name if is_recipe_modifier(name) => recipe.modifiers.push(call.into()),
            "addTo" => after_add = true,
            _ => recipe.unhandled_calls.push(call.into()),
        }
    }
    Some(recipe)
}

fn set_special(
    recipe: &mut RecipeDefinition,
    call: &JavaCall,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
    source_path: &str,
) {
    let (Some(source), Some(expression)) = (one_argument(call), one_expression(call).cloned())
    else {
        return;
    };
    let semantic = recover_expression(
        &source,
        &expression,
        bindings,
        project,
        source_path,
        call.byte_start,
    );
    recipe.special = Some(source);
    recipe.special_expression = Some(expression);
    recipe.special_semantic = Some(semantic);
}

struct NumericCallRecovery {
    source: String,
    expression: JavaExpression,
    semantic: NumericSemantic,
    exact_value: Option<i64>,
}

fn recover_numeric_call(
    call: &JavaCall,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
    source_path: &str,
) -> Option<NumericCallRecovery> {
    let source = one_argument(call)?;
    let expression = one_expression(call)?.clone();
    let semantic = recover_numeric_expression(
        &source,
        &expression,
        bindings,
        project,
        source_path,
        call.byte_start,
    );
    let exact_value = match semantic {
        NumericSemantic::ExactValue { value } => Some(value),
        _ => None,
    };
    Some(NumericCallRecovery {
        source,
        expression,
        semantic,
        exact_value,
    })
}

fn add_recipe_metadata_call(
    recipe: &mut RecipeDefinition,
    call: &JavaCall,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
    source_path: &str,
) {
    match call.name.as_str() {
        "metadata" => add_recipe_metadata(recipe, call, bindings, project, source_path),
        "recipeCategory" => add_recipe_category(recipe, call, bindings, project, source_path),
        _ => unreachable!("metadata dispatcher received unexpected builder call"),
    }
}

fn add_recipe_metadata(
    recipe: &mut RecipeDefinition,
    call: &JavaCall,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
    source_path: &str,
) {
    recipe.metadata.push(call.into());
    let (Some(key_source), Some(value_source), Some(key_expression), Some(value_expression)) = (
        call.arguments.first(),
        call.arguments.get(1),
        call.argument_expressions.first(),
        call.argument_expressions.get(1),
    ) else {
        return;
    };
    let key_type = project.and_then(|project| {
        project.expression_type_at(source_path, call.byte_start, key_expression, bindings)
    });
    let expected_value_type = key_type.as_deref().and_then(|type_name| {
        generic_type_argument(type_name, 0)
            .map(str::to_owned)
            .or_else(|| {
                project.and_then(|project| {
                    project.generic_supertype_argument(type_name, "RecipeMetadataKey", 0)
                })
            })
    });
    let value_type = project.and_then(|project| {
        project.expression_type_at(source_path, call.byte_start, value_expression, bindings)
    });
    let value_semantic = recover_expression(
        value_source,
        value_expression,
        bindings,
        project,
        source_path,
        call.byte_start,
    );
    recipe.metadata_semantics.push(RecipeMetadataDefinition {
        key_source: key_source.clone(),
        key_expression: key_expression.clone(),
        key_type,
        expected_value_type,
        value_source: value_source.clone(),
        value_expression: value_expression.clone(),
        value_type,
        value_semantic,
    });
}

fn add_recipe_category(
    recipe: &mut RecipeDefinition,
    call: &JavaCall,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
    source_path: &str,
) {
    recipe.metadata.push(call.into());
    let (Some(source), Some(expression)) =
        (call.arguments.first(), call.argument_expressions.first())
    else {
        return;
    };
    recipe.recipe_categories.push(RecoveredExpression {
        source: source.clone(),
        expression: expression.clone(),
        semantic: Some(recover_expression(
            source,
            expression,
            bindings,
            project,
            source_path,
            call.byte_start,
        )),
    });
}

fn add_unsafe_recipe_inputs(
    recipe: &mut RecipeDefinition,
    call: &JavaCall,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
    source_path: &str,
) {
    add_recipe_items(recipe, call, bindings, project, source_path, true);
    recipe.modifiers.push(call.into());
}

fn is_recipe_modifier(name: &str) -> bool {
    matches!(
        name,
        "noOptimize"
            | "ignoreCollision"
            | "fake"
            | "enabled"
            | "disabled"
            | "hidden"
            | "requiresCleanRoom"
            | "requiresLowGravity"
            | "requireMods"
            | "nbtSensitive"
            | "noBuffer"
            | "setNEIDesc"
            | "circuit"
    )
}

fn set_duration(
    recipe: &mut RecipeDefinition,
    call: &JavaCall,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
    source_path: &str,
) {
    let Some(recovered) = recover_numeric_call(call, bindings, project, source_path) else {
        return;
    };
    recipe.duration = Some(recovered.source);
    recipe.duration_expression = Some(recovered.expression);
    recipe.duration_ticks = recovered.exact_value;
    recipe.duration_semantic = Some(recovered.semantic);
}

fn set_eut(
    recipe: &mut RecipeDefinition,
    call: &JavaCall,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
    source_path: &str,
) {
    let Some(recovered) = recover_numeric_call(call, bindings, project, source_path) else {
        return;
    };
    recipe.eut = Some(recovered.source);
    recipe.eut_expression = Some(recovered.expression);
    recipe.eut_value = recovered.exact_value;
    recipe.eut_semantic = Some(recovered.semantic);
}

fn set_special_value(
    recipe: &mut RecipeDefinition,
    call: &JavaCall,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
    source_path: &str,
) {
    let Some(recovered) = recover_numeric_call(call, bindings, project, source_path) else {
        return;
    };
    recipe.special_value = Some(recovered.source);
    recipe.special_value_expression = Some(recovered.expression);
    recipe.special_value_exact = recovered.exact_value;
    recipe.special_value_semantic = Some(recovered.semantic);
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

struct ChanceTarget<'a> {
    raw: &'a mut Vec<String>,
    semantics: &'a mut Vec<RecoveredExpression<NumericSemantic>>,
}

#[derive(Clone, Copy)]
struct RecoverySite<'a> {
    source_path: &'a str,
    byte_position: usize,
}

fn add_recipe_chance_call(
    recipe: &mut RecipeDefinition,
    call: &JavaCall,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
    source_path: &str,
) {
    let target = if call.name == "inputChances" {
        ChanceTarget {
            raw: &mut recipe.input_chances,
            semantics: &mut recipe.input_chance_semantics,
        }
    } else {
        ChanceTarget {
            raw: &mut recipe.output_chances,
            semantics: &mut recipe.output_chance_semantics,
        }
    };
    add_chances(target, call, bindings, project, source_path);
}

fn add_chances(
    mut target: ChanceTarget<'_>,
    call: &JavaCall,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
    source_path: &str,
) {
    let site = RecoverySite {
        source_path,
        byte_position: call.byte_start,
    };
    for (source, expression) in call.arguments.iter().zip(&call.argument_expressions) {
        add_chance_value(
            &mut target,
            source.clone(),
            expression.clone(),
            bindings,
            project,
            site,
        );
    }
}

fn add_chance_value(
    target: &mut ChanceTarget<'_>,
    source: String,
    expression: JavaExpression,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
    site: RecoverySite<'_>,
) {
    let semantic = recover_numeric_expression(
        &source,
        &expression,
        bindings,
        project,
        site.source_path,
        site.byte_position,
    );
    target.raw.push(source.clone());
    target.semantics.push(RecoveredExpression {
        source,
        expression,
        semantic: Some(semantic),
    });
}

fn add_recipe_items(
    recipe: &mut RecipeDefinition,
    call: &JavaCall,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
    source_path: &str,
    input_context: bool,
) {
    let mut item_call = call.clone();
    let chance_overload = project.is_some_and(|project| {
        is_item_chance_overload(call, bindings, project, source_path, input_context)
    });
    if chance_overload {
        let chance_source = item_call.arguments.pop();
        let chance_expression = item_call.argument_expressions.pop();
        if let (Some(source), Some(expression)) = (chance_source, chance_expression) {
            let mut target = if input_context {
                ChanceTarget {
                    raw: &mut recipe.input_chances,
                    semantics: &mut recipe.input_chance_semantics,
                }
            } else {
                ChanceTarget {
                    raw: &mut recipe.output_chances,
                    semantics: &mut recipe.output_chance_semantics,
                }
            };
            add_chance_value(
                &mut target,
                source,
                expression,
                bindings,
                project,
                RecoverySite {
                    source_path,
                    byte_position: call.byte_start,
                },
            );
        }
    }
    if input_context {
        add_items(
            &mut recipe.item_inputs,
            &mut recipe.item_input_semantics,
            &item_call,
            bindings,
            project,
            source_path,
            true,
        );
    } else {
        add_items(
            &mut recipe.item_outputs,
            &mut recipe.item_output_semantics,
            &item_call,
            bindings,
            project,
            source_path,
            false,
        );
    }
}

fn is_item_chance_overload(
    call: &JavaCall,
    bindings: &JavaBindings,
    project: &JavaProjectIndex,
    source_path: &str,
    input_context: bool,
) -> bool {
    if call.argument_expressions.len() != 2 {
        return false;
    }
    let Some(first) = call.argument_expressions.first() else {
        return false;
    };
    let Some(second) = call.argument_expressions.get(1) else {
        return false;
    };
    let Some(first_type) =
        project.expression_type_at(source_path, call.byte_start, first, bindings)
    else {
        return false;
    };
    let Some(second_type) =
        project.expression_type_at(source_path, call.byte_start, second, bindings)
    else {
        return false;
    };
    if second_type != "int[]" {
        return false;
    }
    first_type.ends_with("ItemStack[]")
        || (input_context && matches!(first_type.as_str(), "Object[]" | "java.lang.Object[]"))
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
    use super::{NumericSemantic, recover, recover_with_project};
    use minecraft_redox_java::{JavaProjectIndex, analyze_source};

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
    fn separates_builder_item_chance_overload() -> anyhow::Result<()> {
        let source = r"
            class ItemStack {}
            class Recipes {
                void run(ItemStack[] outputs, int[] chances) {
                    GTValues.RA.stdBuilder()
                        .itemOutputs(outputs, chances)
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
        assert_eq!(recipe.item_output_semantics.len(), 1);
        assert_eq!(recipe.output_chances, vec![String::from("chances")]);
        assert_eq!(recipe.output_chance_semantics.len(), 1);
        assert!(matches!(
            recipe.output_chance_semantics[0].semantic,
            Some(NumericSemantic::LinkedValue { .. })
        ));
        Ok(())
    }

    #[test]
    fn recovers_typed_recipe_metadata() -> anyhow::Result<()> {
        let source = r"
            import net.minecraft.item.ItemStack;
            class RecipeMetadataKey<T> {}
            class R {
                static RecipeMetadataKey<ItemStack> RESEARCH = new RecipeMetadataKey<>();
                void run(ItemStack item) {
                    GTValues.RA.stdBuilder()
                        .metadata(RESEARCH, item)
                        .duration(1)
                        .eut(1)
                        .addTo(assemblerRecipes);
                }
            }
        ";
        let analysis = analyze_source("R.java", "gt5", None, source)?;
        let mut project = JavaProjectIndex::default();
        project.add_analysis(&analysis);
        let recovery = super::recover_with_project(&analysis, &project);
        let metadata = recovery
            .recipes
            .first()
            .and_then(|recipe| recipe.metadata_semantics.first())
            .ok_or_else(|| anyhow::anyhow!("metadata was not recovered"))?;
        assert_eq!(
            metadata.expected_value_type.as_deref(),
            Some("net.minecraft.item.ItemStack")
        );
        assert!(matches!(
            metadata.value_semantic,
            super::ExpressionSemantic::LinkedValue { .. }
        ));
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
