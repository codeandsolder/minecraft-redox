use minecraft_redox_java::{JavaBindings, JavaExpression, JavaProjectIndex, substitute_references};
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
    LocalMethod {
        method_id: String,
        return_type: String,
        arguments: Vec<JavaExpression>,
    },
    InlinedLocalMethod {
        method_id: String,
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
pub fn recover_items_with_project(
    sources: &[String],
    expressions: &[JavaExpression],
    bindings: &JavaBindings,
    project: &JavaProjectIndex,
    source_path: &str,
) -> Vec<RecoveredExpression<ItemSemantic>> {
    recover_values(sources, expressions, |expression| {
        item_semantic(expression, bindings)
            .or_else(|| local_item_semantic(expression, bindings, project, source_path))
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

fn item_semantic(expression: &JavaExpression, bindings: &JavaBindings) -> Option<ItemSemantic> {
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

fn local_item_semantic(
    expression: &JavaExpression,
    bindings: &JavaBindings,
    project: &JavaProjectIndex,
    source_path: &str,
) -> Option<ItemSemantic> {
    let JavaExpression::Call {
        receiver: None,
        method,
        arguments,
    } = expression
    else {
        return None;
    };
    let resolved = project.resolve_local_method(source_path, method, arguments.len())?;
    if !resolved.return_type.ends_with("ItemStack") {
        return None;
    }
    let method_id = resolved.stable_id();
    if resolved.returns.len() == 1
        && let Some(replacements) = resolved.argument_bindings(arguments)
    {
        let substituted = substitute_references(&resolved.returns[0].expression, &replacements);
        if let Some(result) = item_semantic(&substituted, bindings) {
            return Some(ItemSemantic::InlinedLocalMethod {
                method_id,
                result: Box::new(result),
            });
        }
    }
    Some(ItemSemantic::LocalMethod {
        method_id,
        return_type: resolved.return_type.clone(),
        arguments: arguments.clone(),
    })
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

fn fluid_semantic(expression: &JavaExpression, bindings: &JavaBindings) -> Option<FluidSemantic> {
    match expression {
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
        "getFluid" | "getGas" | "getMolten" | "getPlasma" | "getFluidOrGas" | "getFluidStack"
    )
}

fn eval_i64(expression: &JavaExpression, bindings: &JavaBindings) -> Option<i64> {
    match expression {
        JavaExpression::Literal(value) => parse_integer_literal(value),
        JavaExpression::Reference(reference) => {
            known_integer_constant(&bindings.resolve_reference(reference))
        }
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
        recover_items,
    };
    use minecraft_redox_java::{JavaExpression, analyze_source};

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
                builder.fluidInputs(Materials.Water.getFluid(1_000));
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
        assert!(matches!(
            recovered[0].semantic,
            Some(FluidSemantic::FluidAccessor { .. })
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
        );
        let Some(ItemSemantic::LocalMethod { method_id, .. }) = &recovered[0].semantic else {
            return Err(anyhow::anyhow!("local method was not linked"));
        };
        assert_eq!(method_id, "R#get(String,int)");
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
        );
        let Some(ItemSemantic::InlinedLocalMethod { result, .. }) = &recovered[0].semantic else {
            return Err(anyhow::anyhow!("local method body was not inlined"));
        };
        assert!(matches!(result.as_ref(), ItemSemantic::CopyAmount { .. }));
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
