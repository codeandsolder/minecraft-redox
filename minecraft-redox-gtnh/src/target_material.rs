use minecraft_redox_core::{
    SourceOrigin,
    content::{
        ContentId, ContentMaterial, ContentTextSpec, FixedDecimal, MaterialArcSmeltingWithGasSpec,
        MaterialAspectSpec, MaterialAutoRecipePolicy, MaterialChemicalFormulaSpec,
        MaterialComponentSpec, MaterialDensitySpec, MaterialEnchantmentSpec, MaterialFormGroup,
        MaterialFormulaPart, MaterialFuelSpec, MaterialIconSetSpec, MaterialProcessingBehavior,
        MaterialProcessingSpec, MaterialToolSpec, MaterialTurbineSpec,
    },
};
use minecraft_redox_java::{JavaBindings, JavaExpression, JavaProjectIndex};
use serde::Serialize;

use crate::{
    BuilderCall, MaterialDefinition,
    fixed_decimal::fixed_decimal_value,
    recipe_expression::{literal_i64, resolved_reference_name},
    scalar_expression::scalar_i64,
};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MaterialLoweringBlockerKind {
    Name,
    DefaultLocalName,
    IconSet,
    Color,
    MeltingPoint,
    BlastFurnaceTemp,
    LegacyBridgeAdapter,
    UnsupportedProperty,
    InvalidProperty,
}

impl MaterialLoweringBlockerKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::DefaultLocalName => "default_local_name",
            Self::IconSet => "icon_set",
            Self::Color => "color",
            Self::MeltingPoint => "melting_point",
            Self::BlastFurnaceTemp => "blast_furnace_temp",
            Self::LegacyBridgeAdapter => "legacy_bridge_adapter",
            Self::UnsupportedProperty => "unsupported_property",
            Self::InvalidProperty => "invalid_property",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MaterialLoweringBlocker {
    pub kind: MaterialLoweringBlockerKind,
    pub detail: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MaterialLoweringFailure {
    pub origin: SourceOrigin,
    pub blockers: Vec<MaterialLoweringBlocker>,
}

#[derive(Default)]
struct MaterialAccumulator {
    form_groups: Vec<MaterialFormGroup>,
    added_forms: Vec<ContentId>,
    removed_forms: Vec<ContentId>,
    tags: Vec<ContentId>,
    components: Vec<MaterialComponentSpec>,
    aspects: Vec<MaterialAspectSpec>,
    ore_byproducts: Vec<ContentId>,
    element: Option<ContentId>,
    mining_level: Option<i64>,
    ore_multiplier: Option<i64>,
    blast_furnace_required: Option<bool>,
    glowing_ore: bool,
    unifiable: Option<bool>,
    argb: Option<u32>,
    molten_argb: Option<u32>,
    density: Option<MaterialDensitySpec>,
    fuel: Option<MaterialFuelSpec>,
    tool: Option<MaterialToolSpec>,
    turbine: Option<MaterialTurbineSpec>,
    heat_damage: Option<FixedDecimal>,
    chemical_formula: Option<MaterialChemicalFormulaSpec>,
    tool_enchantment: Option<MaterialEnchantmentSpec>,
    armor_enchantment: Option<MaterialEnchantmentSpec>,
    processing: MaterialProcessingSpec,
    direct_smelting: Option<ContentId>,
    smelting_into: Option<ContentId>,
    macerating_into: Option<ContentId>,
    arc_smelting_into: Option<ContentId>,
    arc_smelting_with_gas: Vec<MaterialArcSmeltingWithGasSpec>,
    flavor_text: Option<String>,
    alias_key: Option<String>,
}

#[derive(Default)]
struct MaterialBaseFields {
    name: Option<String>,
    default_local_name: Option<String>,
    icon_set: Option<MaterialIconSetSpec>,
    dye_color: Option<ContentId>,
    melting_point: Option<i64>,
    blast_furnace_temp: Option<i64>,
}

/// Lower one recovered material into source-language-free content IR.
///
/// # Errors
///
/// Returns every unsupported or malformed material behavior rather than silently
/// discarding builder semantics that the target material IR cannot yet express.
pub fn lower_material(
    material: &MaterialDefinition,
    bindings: &JavaBindings,
) -> Result<ContentMaterial, Box<MaterialLoweringFailure>> {
    lower_material_internal(material, bindings, None)
}

/// Lower a material using cross-file Java constant and type evidence.
///
/// # Errors
/// Reports unsupported or invalid material semantics.
pub fn lower_material_with_project(
    material: &MaterialDefinition,
    bindings: &JavaBindings,
    project: &JavaProjectIndex,
) -> Result<ContentMaterial, Box<MaterialLoweringFailure>> {
    lower_material_internal(material, bindings, Some(project))
}

fn lower_material_internal(
    material: &MaterialDefinition,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
) -> Result<ContentMaterial, Box<MaterialLoweringFailure>> {
    if let Some(bridge_type) = legacy_bridge_adapter_type(material, bindings, project) {
        return Err(Box::new(MaterialLoweringFailure {
            origin: material.origin.clone(),
            blockers: vec![MaterialLoweringBlocker {
                kind: MaterialLoweringBlockerKind::LegacyBridgeAdapter,
                detail: bridge_type,
            }],
        }));
    }
    let mut blockers = Vec::new();
    for call in &material.unhandled_calls {
        push_blocker(
            &mut blockers,
            MaterialLoweringBlockerKind::UnsupportedProperty,
            call.name.clone(),
        );
    }
    let mut material = material.clone();
    if let Some(project) = project {
        for call in material
            .definition_calls
            .iter_mut()
            .chain(&mut material.properties)
        {
            for expression in &mut call.argument_expressions {
                *expression = minecraft_redox_java::rewrite_references(expression, &|reference| {
                    project
                        .qualified_field_reference_at(
                            &material.origin.path,
                            call.byte_start,
                            reference,
                            bindings,
                        )
                        .map(JavaExpression::Reference)
                });
            }
        }
    }
    for call in &material.definition_calls {
        if call.argument_expressions.len() != 1 {
            record_property_result(call, None, &mut blockers);
        }
    }
    let base = lower_base_fields(&material, bindings, project, &mut blockers);
    let mut accumulated = MaterialAccumulator::default();
    for call in &material.properties {
        lower_property(call, bindings, project, &mut accumulated, &mut blockers);
    }
    if blockers.is_empty() {
        return finalize_material(&material, base, accumulated).ok_or_else(|| {
            Box::new(MaterialLoweringFailure {
                origin: material.origin.clone(),
                blockers: vec![MaterialLoweringBlocker {
                    kind: MaterialLoweringBlockerKind::InvalidProperty,
                    detail: String::from("material base fields were incomplete"),
                }],
            })
        });
    }
    Err(Box::new(MaterialLoweringFailure {
        origin: material.origin.clone(),
        blockers,
    }))
}

fn legacy_bridge_adapter_type(
    material: &MaterialDefinition,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
) -> Option<String> {
    let project = project?;
    let name = projected_receiver(material, "setName", "getDefaultName")?;
    let icon = projected_receiver(material, "setIconSet", "getTexSet")?;
    if name.receiver != icon.receiver {
        return None;
    }
    let receiver_type = project.expression_type_at(
        &material.origin.path,
        name.byte_position,
        &name.receiver,
        bindings,
    )?;
    receiver_type
        .ends_with("Werkstoff")
        .then_some(receiver_type)
}

struct ProjectedReceiver {
    receiver: JavaExpression,
    byte_position: usize,
}

fn projected_receiver(
    material: &MaterialDefinition,
    setter: &str,
    getter: &str,
) -> Option<ProjectedReceiver> {
    let call = material
        .definition_calls
        .iter()
        .rev()
        .find(|call| call.name == setter)?;
    let JavaExpression::Call {
        receiver: Some(receiver),
        method,
        arguments,
    } = call.argument_expressions.first()?
    else {
        return None;
    };
    if method != getter || !arguments.is_empty() {
        return None;
    }
    Some(ProjectedReceiver {
        receiver: receiver.as_ref().clone(),
        byte_position: call.byte_start,
    })
}

fn lower_base_fields(
    material: &MaterialDefinition,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
    blockers: &mut Vec<MaterialLoweringBlocker>,
) -> MaterialBaseFields {
    MaterialBaseFields {
        name: required_string(
            material.name.as_deref(),
            MaterialLoweringBlockerKind::Name,
            blockers,
        ),
        default_local_name: required_string(
            material.default_local_name.as_deref(),
            MaterialLoweringBlockerKind::DefaultLocalName,
            blockers,
        ),
        icon_set: optional_definition(
            material.icon_set.as_deref(),
            definition_expression(material, &["setIconSet"]),
            MaterialLoweringBlockerKind::IconSet,
            blockers,
            |value| icon_set_value(value, bindings),
        ),
        dye_color: optional_definition(
            material.color.as_deref(),
            definition_expression(material, &["setColor"]),
            MaterialLoweringBlockerKind::Color,
            blockers,
            |value| content_id(value, bindings),
        ),
        melting_point: optional_definition(
            material.melting_point.as_deref(),
            definition_expression(material, &["setMeltingPoint"]),
            MaterialLoweringBlockerKind::MeltingPoint,
            blockers,
            |value| scalar_value(value, bindings, project),
        ),
        blast_furnace_temp: optional_definition(
            material.blast_furnace_temp.as_deref(),
            definition_expression(material, &["setBlastFurnaceTemp"]),
            MaterialLoweringBlockerKind::BlastFurnaceTemp,
            blockers,
            |value| scalar_value(value, bindings, project),
        ),
    }
}

fn finalize_material(
    material: &MaterialDefinition,
    base: MaterialBaseFields,
    accumulated: MaterialAccumulator,
) -> Option<ContentMaterial> {
    let argb = accumulated.argb.unwrap_or(0x00ff_ffff);
    let molten_argb = accumulated.molten_argb.unwrap_or(argb);
    let density = accumulated.density.unwrap_or(MaterialDensitySpec {
        multiplier: 1,
        divider: 1,
    });
    let fuel = accumulated.fuel.unwrap_or_else(|| MaterialFuelSpec {
        fuel_type: ContentId::new("gregtech.api.enums.MaterialBuilder.FuelType.Diesel"),
        power: 0,
    });
    Some(ContentMaterial {
        origin: material.origin.clone(),
        name: base.name?,
        default_local_name: base.default_local_name?,
        icon_set: base.icon_set,
        dye_color: base.dye_color,
        argb,
        melting_point: base.melting_point,
        blast_furnace_temp: base.blast_furnace_temp,
        form_groups: accumulated.form_groups,
        added_forms: accumulated.added_forms,
        removed_forms: accumulated.removed_forms,
        tags: accumulated.tags,
        components: accumulated.components,
        aspects: accumulated.aspects,
        ore_byproducts: accumulated.ore_byproducts,
        element: accumulated.element,
        mining_level: accumulated.mining_level,
        ore_multiplier: accumulated.ore_multiplier,
        blast_furnace_required: accumulated.blast_furnace_required,
        glowing_ore: accumulated.glowing_ore,
        unifiable: accumulated.unifiable.unwrap_or(true),
        molten_argb,
        density,
        fuel,
        tool: accumulated.tool.unwrap_or_default(),
        turbine: accumulated.turbine.unwrap_or_default(),
        heat_damage: accumulated.heat_damage.unwrap_or(FixedDecimal::integer(0)),
        chemical_formula: accumulated.chemical_formula,
        tool_enchantment: accumulated.tool_enchantment,
        armor_enchantment: accumulated.armor_enchantment,
        processing: accumulated.processing,
        direct_smelting: accumulated.direct_smelting,
        smelting_into: accumulated.smelting_into,
        macerating_into: accumulated.macerating_into,
        arc_smelting_into: accumulated.arc_smelting_into,
        arc_smelting_with_gas: accumulated.arc_smelting_with_gas,
        flavor_text: accumulated.flavor_text,
        alias_key: accumulated.alias_key.unwrap_or_default(),
    })
}

#[derive(Clone, Copy)]
enum PropertyLoweringResult {
    Unsupported,
    Valid,
    Invalid,
}

fn lower_property(
    call: &BuilderCall,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
    material: &mut MaterialAccumulator,
    blockers: &mut Vec<MaterialLoweringBlocker>,
) {
    if let Some(group) = material_form_group(&call.name) {
        if !call.arguments.is_empty() {
            record_property_result(call, None, blockers);
            return;
        }
        material.form_groups.push(group);
        return;
    }

    if record_specialized_property_result(call, lower_processing_property(call, material), blockers)
    {
        return;
    }
    if record_specialized_property_result(
        call,
        lower_redirect_property(call, bindings, material),
        blockers,
    ) {
        return;
    }

    if property_arity(&call.name).is_some_and(|arity| call.argument_expressions.len() != arity) {
        record_property_result(call, None, blockers);
        return;
    }
    let result = match call.name.as_str() {
        "addOrePrefix" => {
            one_content_id(call, bindings).map(|value| material.added_forms.push(value))
        }
        "removeOrePrefix" => {
            one_content_id(call, bindings).map(|value| material.removed_forms.push(value))
        }
        "addSubTag" => one_content_id(call, bindings).map(|value| material.tags.push(value)),
        "addMaterial" => lower_counted_content(call, bindings, project).map(|(id, amount)| {
            material.components.push(MaterialComponentSpec {
                material: id,
                amount,
            });
        }),
        "addAspect" => lower_counted_content(call, bindings, project).map(|(id, amount)| {
            material
                .aspects
                .push(MaterialAspectSpec { aspect: id, amount });
        }),
        "addOreByproduct" => call
            .argument_expressions
            .first()
            .and_then(|value| supplier_content_id(value, bindings))
            .map(|value| material.ore_byproducts.push(value)),
        "setElement" => one_content_id(call, bindings).map(|value| material.element = Some(value)),
        "setMiningLevel" => {
            one_scalar(call, bindings, project).map(|value| material.mining_level = Some(value))
        }
        "setOreMultiplier" => {
            one_scalar(call, bindings, project).map(|value| material.ore_multiplier = Some(value))
        }
        "setBlastFurnaceRequired" => {
            one_bool(call).map(|value| material.blast_furnace_required = Some(value))
        }
        "setUnifiable" => one_bool(call).map(|value| material.unifiable = Some(value)),
        "setARGB" => one_u32(call, bindings, project).map(|value| {
            material.argb = Some(value);
            material.molten_argb = Some(value);
        }),
        "setMoltenARGB" => {
            one_u32(call, bindings, project).map(|value| material.molten_argb = Some(value))
        }
        "setDensity" => {
            lower_density(call, bindings, project).map(|value| material.density = Some(value))
        }
        "setFuel" => lower_fuel(call, bindings, project).map(|value| material.fuel = Some(value)),
        "setChemicalFormula" => lower_chemical_formula(call, bindings)
            .map(|value| material.chemical_formula = Some(value)),
        "setTool" => lower_tool(call, bindings, project).map(|value| material.tool = Some(value)),
        "setTurbine" => lower_turbine(call).map(|value| material.turbine = Some(value)),
        "setHeatDamage" => one_decimal(call).map(|value| material.heat_damage = Some(value)),
        "setToolEnchantment" => lower_enchantment(call, bindings, project)
            .map(|value| material.tool_enchantment = Some(value)),
        "setArmorEnchantment" => lower_enchantment(call, bindings, project)
            .map(|value| material.armor_enchantment = Some(value)),
        "setFlavorText" => one_string(call).map(|value| material.flavor_text = Some(value)),
        "setAliasKey" => one_string(call).map(|value| material.alias_key = Some(value)),
        "hasGlowingOre" if call.arguments.is_empty() => {
            material.glowing_ore = true;
            Some(())
        }
        _ => {
            push_blocker(
                blockers,
                MaterialLoweringBlockerKind::UnsupportedProperty,
                call.name.clone(),
            );
            return;
        }
    };

    record_property_result(call, result, blockers);
}

fn property_arity(name: &str) -> Option<usize> {
    match name {
        "addMaterial"
        | "addAspect"
        | "setDensity"
        | "setFuel"
        | "setToolEnchantment"
        | "setArmorEnchantment" => Some(2),
        "setTool" | "setTurbine" => Some(3),
        "setChemicalFormula" => None,
        "hasGlowingOre" => Some(0),
        _ => Some(1),
    }
}

fn record_specialized_property_result(
    call: &BuilderCall,
    result: PropertyLoweringResult,
    blockers: &mut Vec<MaterialLoweringBlocker>,
) -> bool {
    match result {
        PropertyLoweringResult::Unsupported => false,
        PropertyLoweringResult::Valid => true,
        PropertyLoweringResult::Invalid => {
            record_property_result(call, None, blockers);
            true
        }
    }
}

fn record_property_result(
    call: &BuilderCall,
    result: Option<()>,
    blockers: &mut Vec<MaterialLoweringBlocker>,
) {
    if result.is_none() {
        push_blocker(
            blockers,
            MaterialLoweringBlockerKind::InvalidProperty,
            format!("{}({})", call.name, call.arguments.join(", ")),
        );
    }
}

fn lower_processing_property(
    call: &BuilderCall,
    material: &mut MaterialAccumulator,
) -> PropertyLoweringResult {
    let result = match call.name.as_str() {
        "addElectrolyzerRecipe" if call.arguments.is_empty() => {
            material
                .processing
                .generated_behaviors
                .push(MaterialProcessingBehavior::ElectrolyzerRecipe);
            Some(())
        }
        "addCentrifugeRecipe" if call.arguments.is_empty() => {
            material
                .processing
                .generated_behaviors
                .push(MaterialProcessingBehavior::CentrifugeRecipe);
            Some(())
        }
        "addCrackingRecipes" if call.arguments.is_empty() => {
            material
                .processing
                .generated_behaviors
                .push(MaterialProcessingBehavior::CrackingRecipes);
            Some(())
        }
        "setAutoGenerateBlastFurnaceRecipes" => one_bool(call).map(|value| {
            material.processing.blast_furnace = auto_recipe_policy(value);
        }),
        "setAutoGeneratedVacuumFreezerRecipe" => one_bool(call).map(|value| {
            material.processing.vacuum_freezer = auto_recipe_policy(value);
        }),
        "setAutoGeneratedRecycleRecipes" => one_bool(call).map(|value| {
            material.processing.recycling = auto_recipe_policy(value);
        }),
        _ => return PropertyLoweringResult::Unsupported,
    };
    if result.is_some() {
        PropertyLoweringResult::Valid
    } else {
        PropertyLoweringResult::Invalid
    }
}

fn lower_redirect_property(
    call: &BuilderCall,
    bindings: &JavaBindings,
    material: &mut MaterialAccumulator,
) -> PropertyLoweringResult {
    let result = match call.name.as_str() {
        "setDirectSmelting" => one_supplier_content_id(call, bindings)
            .map(|value| material.direct_smelting = Some(value)),
        "setSmeltingInto" => one_supplier_content_id(call, bindings)
            .map(|value| material.smelting_into = Some(value)),
        "setMaceratingInto" => one_supplier_content_id(call, bindings)
            .map(|value| material.macerating_into = Some(value)),
        "setArcSmeltingInto" => one_supplier_content_id(call, bindings)
            .map(|value| material.arc_smelting_into = Some(value)),
        "setArcSmeltingIntoWithGas" => lower_arc_smelting_with_gas(call, bindings)
            .map(|value| material.arc_smelting_with_gas.push(value)),
        _ => return PropertyLoweringResult::Unsupported,
    };
    if result.is_some() {
        PropertyLoweringResult::Valid
    } else {
        PropertyLoweringResult::Invalid
    }
}

const fn auto_recipe_policy(enabled: bool) -> MaterialAutoRecipePolicy {
    if enabled {
        MaterialAutoRecipePolicy::Enabled
    } else {
        MaterialAutoRecipePolicy::Disabled
    }
}

fn one_u32(
    call: &BuilderCall,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
) -> Option<u32> {
    let value = one_scalar(call, bindings, project)?;
    u32::try_from(value).ok().or_else(|| {
        i32::try_from(value)
            .ok()
            .map(|signed| u32::from_ne_bytes(signed.to_ne_bytes()))
    })
}

fn lower_density(
    call: &BuilderCall,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
) -> Option<MaterialDensitySpec> {
    Some(MaterialDensitySpec {
        multiplier: scalar_value(call.argument_expressions.first()?, bindings, project)?,
        divider: scalar_value(call.argument_expressions.get(1)?, bindings, project)?,
    })
}

fn lower_fuel(
    call: &BuilderCall,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
) -> Option<MaterialFuelSpec> {
    Some(MaterialFuelSpec {
        fuel_type: content_id(call.argument_expressions.first()?, bindings)?,
        power: scalar_value(call.argument_expressions.get(1)?, bindings, project)?,
    })
}

fn lower_chemical_formula(
    call: &BuilderCall,
    bindings: &JavaBindings,
) -> Option<MaterialChemicalFormulaSpec> {
    if !(1..=2).contains(&call.argument_expressions.len()) {
        return None;
    }
    let mut parts = Vec::new();
    lower_formula_parts(call.argument_expressions.first()?, bindings, &mut parts)?;
    let localized = match call.argument_expressions.get(1) {
        Some(JavaExpression::Literal(value)) if value == "true" => true,
        Some(JavaExpression::Literal(value)) if value == "false" => false,
        Some(_) => return None,
        None => false,
    };
    Some(MaterialChemicalFormulaSpec { parts, localized })
}

fn lower_formula_parts(
    expression: &JavaExpression,
    bindings: &JavaBindings,
    parts: &mut Vec<MaterialFormulaPart>,
) -> Option<()> {
    match expression {
        JavaExpression::Literal(value) => {
            parts.push(MaterialFormulaPart::Text {
                value: decode_java_string_literal(value)?,
            });
            Some(())
        }
        JavaExpression::Binary {
            left,
            operator,
            right,
        } if operator == "+" => {
            lower_formula_parts(left, bindings, parts)?;
            lower_formula_parts(right, bindings, parts)
        }
        JavaExpression::Parenthesized(inner) => lower_formula_parts(inner, bindings, parts),
        JavaExpression::Call {
            receiver: Some(receiver),
            method,
            arguments,
        } if method == "getChemicalFormula" && arguments.is_empty() => {
            parts.push(MaterialFormulaPart::MaterialFormula {
                material: content_id(receiver, bindings)?,
            });
            Some(())
        }
        _ => lower_formula_symbol(expression, bindings, parts),
    }
}

fn lower_formula_symbol(
    expression: &JavaExpression,
    bindings: &JavaBindings,
    parts: &mut Vec<MaterialFormulaPart>,
) -> Option<()> {
    let reference = resolved_reference_name(expression, bindings)?;
    let (owner, name) = reference.rsplit_once('.')?;
    if owner.ends_with("CustomGlyphs") {
        parts.push(MaterialFormulaPart::Glyph {
            name: name.to_owned(),
        });
        return Some(());
    }
    if owner.ends_with("EnumChatFormatting") {
        parts.push(MaterialFormulaPart::Formatting {
            name: name.to_owned(),
        });
        return Some(());
    }
    None
}

fn decode_java_string_literal(source: &str) -> Option<String> {
    let source = source.trim();
    let body = source.strip_prefix('"')?.strip_suffix('"')?;
    let mut result = String::new();
    let mut chars = body.chars().peekable();
    while let Some(character) = chars.next() {
        if character != '\\' {
            result.push(character);
            continue;
        }
        match chars.next()? {
            'b' => result.push('\u{0008}'),
            't' => result.push('\t'),
            'n' => result.push('\n'),
            'f' => result.push('\u{000c}'),
            'r' => result.push('\r'),
            '"' => result.push('"'),
            '\'' => result.push('\''),
            '\\' => result.push('\\'),
            'u' => {
                while chars.peek() == Some(&'u') {
                    chars.next();
                }
                let digits: String = chars.by_ref().take(4).collect();
                if digits.len() != 4 {
                    return None;
                }
                let value = u32::from_str_radix(&digits, 16).ok()?;
                result.push(char::from_u32(value)?);
            }
            _ => return None,
        }
    }
    Some(result)
}

fn lower_tool(
    call: &BuilderCall,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
) -> Option<MaterialToolSpec> {
    Some(MaterialToolSpec {
        durability: scalar_value(call.argument_expressions.first()?, bindings, project)?,
        quality: scalar_value(call.argument_expressions.get(1)?, bindings, project)?,
        speed: fixed_decimal_value(call.argument_expressions.get(2)?)?,
    })
}

fn lower_turbine(call: &BuilderCall) -> Option<MaterialTurbineSpec> {
    Some(MaterialTurbineSpec {
        steam: fixed_decimal_value(call.argument_expressions.first()?)?,
        gas: fixed_decimal_value(call.argument_expressions.get(1)?)?,
        plasma: fixed_decimal_value(call.argument_expressions.get(2)?)?,
    })
}

fn one_decimal(call: &BuilderCall) -> Option<FixedDecimal> {
    (call.argument_expressions.len() == 1)
        .then(|| call.argument_expressions.first())
        .flatten()
        .and_then(fixed_decimal_value)
}

fn lower_enchantment(
    call: &BuilderCall,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
) -> Option<MaterialEnchantmentSpec> {
    Some(MaterialEnchantmentSpec {
        enchantment: supplier_content_id(call.argument_expressions.first()?, bindings)?,
        level: scalar_value(call.argument_expressions.get(1)?, bindings, project)?,
    })
}

fn one_supplier_content_id(call: &BuilderCall, bindings: &JavaBindings) -> Option<ContentId> {
    (call.argument_expressions.len() == 1)
        .then(|| call.argument_expressions.first())
        .flatten()
        .and_then(|value| supplier_content_id(value, bindings))
}

fn lower_arc_smelting_with_gas(
    call: &BuilderCall,
    bindings: &JavaBindings,
) -> Option<MaterialArcSmeltingWithGasSpec> {
    if call.argument_expressions.len() != 2 {
        return None;
    }
    Some(MaterialArcSmeltingWithGasSpec {
        gas: supplier_content_id(call.argument_expressions.first()?, bindings)?,
        material: supplier_content_id(call.argument_expressions.get(1)?, bindings)?,
    })
}

fn one_string(call: &BuilderCall) -> Option<String> {
    (call.arguments.len() == 1)
        .then(|| call.arguments.first())
        .flatten()
        .and_then(|value| decode_java_string_literal(value))
}

fn material_form_group(name: &str) -> Option<MaterialFormGroup> {
    match name {
        "addDustItems" => Some(MaterialFormGroup::Dust),
        "addMetalItems" => Some(MaterialFormGroup::Metal),
        "addGemItems" => Some(MaterialFormGroup::Gem),
        "addOreItems" => Some(MaterialFormGroup::Ore),
        "addCell" => Some(MaterialFormGroup::Cell),
        "addPlasma" => Some(MaterialFormGroup::Plasma),
        "addToolHeadItems" => Some(MaterialFormGroup::ToolHead),
        "addGearItems" => Some(MaterialFormGroup::Gear),
        "addEmpty" => Some(MaterialFormGroup::Empty),
        "addFluid" => Some(MaterialFormGroup::Fluid),
        "addGas" => Some(MaterialFormGroup::Gas),
        _ => None,
    }
}

fn lower_counted_content(
    call: &BuilderCall,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
) -> Option<(ContentId, u64)> {
    let [id, amount] = call.argument_expressions.as_slice() else {
        return None;
    };
    Some((
        content_id(id, bindings)?,
        u64::try_from(scalar_value(amount, bindings, project)?).ok()?,
    ))
}

fn one_content_id(call: &BuilderCall, bindings: &JavaBindings) -> Option<ContentId> {
    (call.argument_expressions.len() == 1)
        .then(|| call.argument_expressions.first())
        .flatten()
        .and_then(|value| content_id(value, bindings))
}

fn content_id(expression: &JavaExpression, bindings: &JavaBindings) -> Option<ContentId> {
    Some(ContentId::new(resolved_reference_name(
        expression, bindings,
    )?))
}

fn supplier_content_id(expression: &JavaExpression, bindings: &JavaBindings) -> Option<ContentId> {
    match expression {
        JavaExpression::Lambda { body } | JavaExpression::Parenthesized(body) => {
            supplier_content_id(body, bindings)
        }
        _ => content_id(expression, bindings),
    }
}

fn one_scalar(
    call: &BuilderCall,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
) -> Option<i64> {
    (call.argument_expressions.len() == 1)
        .then(|| call.argument_expressions.first())
        .flatten()
        .and_then(|value| scalar_value(value, bindings, project))
}

fn scalar_value(
    expression: &JavaExpression,
    bindings: &JavaBindings,
    project: Option<&JavaProjectIndex>,
) -> Option<i64> {
    project.map_or_else(
        || literal_i64(expression),
        |project| scalar_i64(expression, bindings, project),
    )
}

fn one_bool(call: &BuilderCall) -> Option<bool> {
    let [JavaExpression::Literal(value)] = call.argument_expressions.as_slice() else {
        return None;
    };
    match value.as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn required_string(
    source: Option<&str>,
    kind: MaterialLoweringBlockerKind,
    blockers: &mut Vec<MaterialLoweringBlocker>,
) -> Option<String> {
    let value = source.and_then(decode_java_string_literal);
    if value.is_none() {
        push_blocker(blockers, kind, source.unwrap_or("<missing>").to_owned());
    }
    value
}

fn definition_expression<'a>(
    material: &'a MaterialDefinition,
    names: &[&str],
) -> Option<&'a JavaExpression> {
    material
        .definition_calls
        .iter()
        .rev()
        .find(|call| names.contains(&call.name.as_str()))
        .and_then(|call| call.argument_expressions.first())
}

fn optional_definition<T>(
    source: Option<&str>,
    expression: Option<&JavaExpression>,
    kind: MaterialLoweringBlockerKind,
    blockers: &mut Vec<MaterialLoweringBlocker>,
    lower: impl FnOnce(&JavaExpression) -> Option<T>,
) -> Option<T> {
    let source = source?;
    let value = expression.and_then(lower);
    if value.is_none() {
        push_blocker(blockers, kind, source.to_owned());
    }
    value
}

fn icon_set_value(
    expression: &JavaExpression,
    bindings: &JavaBindings,
) -> Option<MaterialIconSetSpec> {
    if let Some(id) = content_id(expression, bindings) {
        return Some(MaterialIconSetSpec::Reference { id });
    }
    let JavaExpression::Call {
        receiver: Some(receiver),
        method,
        arguments,
    } = expression
    else {
        return None;
    };
    let base = Box::new(icon_set_value(receiver, bindings)?);
    let name = text_spec(arguments.first()?, bindings)?;
    match method.as_str() {
        "withCustomTextures" => {
            let textures = arguments
                .iter()
                .skip(1)
                .map(|value| content_id(value, bindings))
                .collect::<Option<Vec<_>>>()?;
            Some(MaterialIconSetSpec::CustomTextures {
                base,
                name,
                textures,
            })
        }
        "withStoneOreTextures" if arguments.len() == 1 => {
            Some(MaterialIconSetSpec::StoneOreTextures { base, name })
        }
        _ => None,
    }
}

fn text_spec(expression: &JavaExpression, bindings: &JavaBindings) -> Option<ContentTextSpec> {
    string_expression(expression)
        .map(|value| ContentTextSpec::Literal { value })
        .or_else(|| content_id(expression, bindings).map(|id| ContentTextSpec::Symbol { id }))
}

fn string_expression(expression: &JavaExpression) -> Option<String> {
    let JavaExpression::Literal(source) = expression else {
        return None;
    };
    decode_java_string_literal(source)
}

fn push_blocker(
    blockers: &mut Vec<MaterialLoweringBlocker>,
    kind: MaterialLoweringBlockerKind,
    detail: impl Into<String>,
) {
    blockers.push(MaterialLoweringBlocker {
        kind,
        detail: detail.into(),
    });
}

#[cfg(test)]
mod tests {
    use minecraft_redox_core::content::{
        FixedDecimal, MaterialAutoRecipePolicy, MaterialFormGroup, MaterialFormulaPart,
        MaterialProcessingBehavior,
    };
    use minecraft_redox_java::{JavaProjectIndex, analyze_source};

    use super::{
        ContentMaterial, MaterialLoweringBlockerKind, MaterialLoweringFailure,
        lower_material_with_project,
    };

    fn lower_fixture(
        source: &str,
    ) -> anyhow::Result<Result<ContentMaterial, Box<MaterialLoweringFailure>>> {
        let analysis = analyze_source("MaterialsInit.java", "gt5", None, source)?;
        let mut project = JavaProjectIndex::default();
        project.add_analysis(&analysis);
        let recovery = crate::recover_with_project(&analysis, &project);
        let material = recovery
            .materials
            .first()
            .ok_or_else(|| anyhow::anyhow!("material was not recovered"))?;
        Ok(lower_material_with_project(
            material,
            &analysis.bindings,
            &project,
        ))
    }

    #[test]
    fn lowers_declarative_material_semantics() -> anyhow::Result<()> {
        let source = r#"
            import gregtech.api.enums.*;
            import gregtech.api.enums.Materials;
            class MaterialsInit {
                void run() {
                    Materials foo = new MaterialBuilder()
                        .setName("Foo")
                        .setDefaultLocalName("Foo Local")
                        .setIconSet(TextureSet.SET_METALLIC)
                        .setColor(Dyes.dyeOrange)
                        .setMoltenARGB(0x0011_2233)
                        .setARGB(0xf012_3456)
                        .setMeltingPoint(1_200)
                        .setBlastFurnaceTemp(2_400)
                        .addDustItems()
                        .addMaterial(Materials.Iron, 2)
                        .addSubTag(SubTag.METAL)
                        .addOreByproduct(() -> Materials.Nickel)
                        .setTool(1_120, 4, 3.50F)
                        .setFuel(FuelType.GasTurbine, 128)
                        .setChemicalFormula("Fe" + CustomGlyphs.SUBSCRIPT_2)
                        .addElectrolyzerRecipe()
                        .setAutoGeneratedRecycleRecipes(false)
                        .setDirectSmelting(() -> Materials.Steel)
                        .constructMaterial();
                }
            }
        "#;
        let lowered = lower_fixture(source)?
            .map_err(|failure| anyhow::anyhow!("material lowering failed: {failure:?}"))?;

        assert_eq!(lowered.name, "Foo");
        assert_eq!(
            lowered.dye_color.as_ref().map(|id| id.0.as_str()),
            Some("Dyes.dyeOrange")
        );
        assert_eq!(lowered.argb, 0xf012_3456);
        assert_eq!(lowered.molten_argb, lowered.argb);
        assert_eq!(lowered.default_local_name, "Foo Local");
        assert_eq!(lowered.melting_point, Some(1_200));
        assert_eq!(lowered.blast_furnace_temp, Some(2_400));
        assert_eq!(lowered.tool.durability, 1_120);
        assert_eq!(lowered.tool.quality, 4);
        assert_eq!(
            lowered.tool.speed,
            FixedDecimal {
                coefficient: 35,
                decimal_places: 1,
            }
        );
        assert!(lowered.form_groups.contains(&MaterialFormGroup::Dust));
        assert_eq!(lowered.components.len(), 1);
        assert_eq!(lowered.ore_byproducts.len(), 1);
        assert_eq!(lowered.fuel.power, 128);
        assert_eq!(
            lowered.direct_smelting.as_ref().map(|id| id.0.as_str()),
            Some("gregtech.api.enums.Materials.Steel")
        );
        assert!(
            lowered
                .processing
                .generated_behaviors
                .contains(&MaterialProcessingBehavior::ElectrolyzerRecipe)
        );
        assert_eq!(
            lowered.processing.recycling,
            MaterialAutoRecipePolicy::Disabled
        );
        let formula = lowered
            .chemical_formula
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("chemical formula missing"))?;
        assert!(matches!(
            formula.parts.as_slice(),
            [MaterialFormulaPart::Text { value }, MaterialFormulaPart::Glyph { name }]
                if value == "Fe" && name == "SUBSCRIPT_2"
        ));
        Ok(())
    }

    #[test]
    fn lowers_custom_texture_set_shape() -> anyhow::Result<()> {
        let source = r#"
            import gregtech.api.enums.*;
            class MaterialsInit {
                void run() {
                    Materials foo = new MaterialBuilder()
                        .setName("Foo")
                        .setDefaultLocalName("Foo")
                        .setIconSet(TextureSet.SET_DULL.withCustomTextures(
                            "foo", TextureType.DUST, TextureType.MOLTEN))
                        .constructMaterial();
                }
            }
        "#;
        let lowered = lower_fixture(source)?
            .map_err(|failure| anyhow::anyhow!("material lowering failed: {failure:?}"))?;
        assert!(matches!(
            lowered.icon_set,
            Some(minecraft_redox_core::content::MaterialIconSetSpec::CustomTextures {
                name: minecraft_redox_core::content::ContentTextSpec::Literal { ref value },
                ref textures,
                ..
            }) if value == "foo" && textures.len() == 2
        ));
        Ok(())
    }

    #[test]
    fn classifies_werkstoff_material_bridge_adapter() -> anyhow::Result<()> {
        let source = r#"
            import gregtech.api.enums.*;
            class TextureSet {}
            class Werkstoff {
                String getDefaultName() { return "dynamic"; }
                TextureSet getTexSet() { return null; }
            }
            class MaterialsInit {
                void run(Werkstoff werkstoff) {
                    Materials foo = new MaterialBuilder()
                        .setName(werkstoff.getDefaultName())
                        .setDefaultLocalName(werkstoff.getDefaultName())
                        .setIconSet(werkstoff.getTexSet())
                        .constructMaterial();
                }
            }
        "#;
        let Err(failure) = lower_fixture(source)? else {
            return Err(anyhow::anyhow!(
                "Werkstoff bridge lowered as static content"
            ));
        };
        assert_eq!(failure.blockers.len(), 1);
        assert_eq!(
            failure.blockers[0].kind,
            MaterialLoweringBlockerKind::LegacyBridgeAdapter
        );
        assert!(failure.blockers[0].detail.ends_with("Werkstoff"));
        Ok(())
    }

    #[test]
    fn refuses_runtime_derived_tool_stats() -> anyhow::Result<()> {
        let source = r#"
            import gregtech.api.enums.*;
            class MaterialsInit {
                void run(Werkstoff werkstoff) {
                    Materials foo = new MaterialBuilder()
                        .setName("Foo")
                        .setDefaultLocalName("Foo")
                        .setTool(werkstoff.getDurability(), werkstoff.getToolQuality(), werkstoff.getToolSpeed())
                        .constructMaterial();
                }
            }
        "#;
        let Err(failure) = lower_fixture(source)? else {
            return Err(anyhow::anyhow!(
                "runtime-derived tool stats lowered as static content"
            ));
        };
        assert!(failure.blockers.iter().any(|blocker| {
            blocker.kind == MaterialLoweringBlockerKind::InvalidProperty
                && blocker.detail.starts_with("setTool(")
        }));
        Ok(())
    }
    #[test]
    fn unsupported_builder_call_is_not_silently_discarded() -> anyhow::Result<()> {
        let source = r#"
            class MaterialsInit {
                void run() {
                    new MaterialBuilder().setName("Foo").setDefaultLocalName("Foo")
                        .unknownRuntimeProperty().constructMaterial();
                }
            }
        "#;
        let failure = lower_fixture(source)?
            .err()
            .ok_or_else(|| anyhow::anyhow!("unknown property was accepted"))?;
        assert!(failure.blockers.iter().any(|blocker| blocker.kind
            == MaterialLoweringBlockerKind::UnsupportedProperty
            && blocker.detail == "unknownRuntimeProperty"));
        Ok(())
    }

    #[test]
    fn molten_override_and_nested_texture_overlays_are_preserved() -> anyhow::Result<()> {
        let source = r#"
            class MaterialsInit {
                void run() {
                    new MaterialBuilder().setName("Foo").setDefaultLocalName("Foo")
                        .setARGB(0x0012_3456).setMoltenARGB(0x00ab_cdef)
                        .setIconSet(TextureSet.SET_DULL.withCustomTextures("foo", IconType.ORE).withStoneOreTextures("stone"))
                        .constructMaterial();
                }
            }
        "#;
        let material = lower_fixture(source)?.map_err(|failure| anyhow::anyhow!("{failure:?}"))?;
        assert_eq!(material.argb, 0x0012_3456);
        assert_eq!(material.molten_argb, 0x00ab_cdef);
        assert!(
            matches!(material.icon_set, Some(minecraft_redox_core::content::MaterialIconSetSpec::StoneOreTextures { base, .. }) if matches!(*base, minecraft_redox_core::content::MaterialIconSetSpec::CustomTextures { .. }))
        );
        Ok(())
    }
}
