use std::collections::BTreeSet;

use minecraft_redox_java::{JavaBindings, JavaExpression, JavaField, JavaProjectIndex};

use crate::recipe_expression::{literal_i64, resolved_reference_name};

const MAX_DEPTH: usize = 32;

pub(crate) fn scalar_i64(
    expression: &JavaExpression,
    bindings: &JavaBindings,
    project: &JavaProjectIndex,
) -> Option<i64> {
    let mut seen = BTreeSet::new();
    eval_i64(expression, bindings, project, None, 0, &mut seen)
}

pub(crate) fn scalar_static_field_i64(
    owner: &str,
    name: &str,
    project: &JavaProjectIndex,
) -> Option<i64> {
    let field = project.resolve_field(owner, name, Some(true))?;
    if !field.is_final {
        return None;
    }
    let mut seen = BTreeSet::new();
    eval_field(field, project, 0, &mut seen)
}

fn eval_i64(
    expression: &JavaExpression,
    bindings: &JavaBindings,
    project: &JavaProjectIndex,
    current_owner: Option<&str>,
    depth: usize,
    seen: &mut BTreeSet<String>,
) -> Option<i64> {
    if depth >= MAX_DEPTH {
        return None;
    }
    if let Some(value) = literal_i64(expression) {
        return Some(value);
    }
    match expression {
        JavaExpression::Reference(_) | JavaExpression::FieldAccess { .. } => {
            let reference = resolved_reference_name(expression, bindings)?;
            if let Some(value) = standard_integer_constant(&reference) {
                return Some(value);
            }
            let field = resolve_static_field(project, bindings, current_owner, &reference)?;
            eval_field(field, project, depth + 1, seen)
        }
        JavaExpression::ArrayAccess { array, index } => {
            let index = eval_i64(index, bindings, project, current_owner, depth + 1, seen)?;
            let index = usize::try_from(index).ok()?;
            let reference = resolved_reference_name(array, bindings)?;
            let field = resolve_static_field(project, bindings, current_owner, &reference)?;
            eval_array_element(field, index, project, depth + 1, seen)
        }
        JavaExpression::Cast { type_name, value } => {
            let value = eval_i64(value, bindings, project, current_owner, depth + 1, seen)?;
            numeric_cast(type_name, value)
        }
        JavaExpression::Unary { operator, operand } => {
            let value = eval_i64(operand, bindings, project, current_owner, depth + 1, seen)?;
            match operator.as_str() {
                "+" => Some(value),
                "-" => value.checked_neg(),
                "~" => Some(!value),
                _ => None,
            }
        }
        JavaExpression::Ternary {
            consequence,
            alternative,
            ..
        } => {
            let consequence = eval_i64(
                consequence,
                bindings,
                project,
                current_owner,
                depth + 1,
                seen,
            )?;
            let alternative = eval_i64(
                alternative,
                bindings,
                project,
                current_owner,
                depth + 1,
                seen,
            )?;
            (consequence == alternative).then_some(consequence)
        }
        JavaExpression::Binary {
            left,
            operator,
            right,
        } => {
            let left = eval_i64(left, bindings, project, current_owner, depth + 1, seen)?;
            let right = eval_i64(right, bindings, project, current_owner, depth + 1, seen)?;
            eval_binary(left, operator, right)
        }
        JavaExpression::Parenthesized(inner) => {
            eval_i64(inner, bindings, project, current_owner, depth + 1, seen)
        }
        JavaExpression::Call { .. }
        | JavaExpression::New { .. }
        | JavaExpression::Array { .. }
        | JavaExpression::Literal(_)
        | JavaExpression::Lambda { .. }
        | JavaExpression::Raw(_) => None,
    }
}

fn eval_field(
    field: &JavaField,
    project: &JavaProjectIndex,
    depth: usize,
    seen: &mut BTreeSet<String>,
) -> Option<i64> {
    let key = format!("{}#{}", field.owner, field.name);
    if !seen.insert(key.clone()) {
        return None;
    }
    let result = (|| {
        let initializer = field.initializer.as_ref()?;
        let bindings = project.bindings_for_path(&field.origin.path)?;
        eval_i64(
            initializer,
            bindings,
            project,
            Some(&field.owner),
            depth,
            seen,
        )
    })();
    seen.remove(&key);
    result
}

fn eval_array_element(
    field: &JavaField,
    index: usize,
    project: &JavaProjectIndex,
    depth: usize,
    seen: &mut BTreeSet<String>,
) -> Option<i64> {
    if depth >= MAX_DEPTH {
        return None;
    }
    if let Some(JavaExpression::Array { elements, .. }) = field.initializer.as_ref() {
        let bindings = project.bindings_for_path(&field.origin.path)?;
        return eval_i64(
            elements.get(index)?,
            bindings,
            project,
            Some(&field.owner),
            depth + 1,
            seen,
        );
    }
    if verified_gt_voltage_practical(field) {
        let voltages = project.resolve_field(&field.owner, "V", Some(true))?;
        let voltage = eval_array_element(voltages, index, project, depth + 1, seen)?;
        return voltage.checked_mul(30)?.checked_div(32);
    }
    None
}

fn resolve_static_field<'a>(
    project: &'a JavaProjectIndex,
    bindings: &JavaBindings,
    current_owner: Option<&str>,
    reference: &str,
) -> Option<&'a JavaField> {
    if !reference.contains('.')
        && let Some(owner) = current_owner
        && let Some(field) = project.resolve_field(owner, reference, Some(true))
        && field.is_final
    {
        return Some(field);
    }

    let (owner, field_name) = reference.rsplit_once('.')?;
    let resolved_owner = bindings.resolve_reference(owner);
    if let Some(field) = project.resolve_field(&resolved_owner, field_name, Some(true))
        && field.is_final
    {
        return Some(field);
    }
    if let Some(package) = bindings.package.as_deref() {
        let owner = format!("{package}.{resolved_owner}");
        if let Some(field) = project.resolve_field(&owner, field_name, Some(true))
            && field.is_final
        {
            return Some(field);
        }
    }
    None
}

fn verified_gt_voltage_practical(field: &JavaField) -> bool {
    if field.owner != "gregtech.api.enums.GTValues" || field.name != "VP" {
        return false;
    }
    let Some(source) = field.initializer_source.as_deref() else {
        return false;
    };
    let compact: String = source
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    compact
        == "Arrays.stream(V).map(i->BigInteger.valueOf(i).multiply(BigInteger.valueOf(30)).divide(BigInteger.valueOf(32)).longValueExact()).toArray()"
}

fn standard_integer_constant(reference: &str) -> Option<i64> {
    match reference {
        "Integer.MAX_VALUE" | "java.lang.Integer.MAX_VALUE" => Some(i64::from(i32::MAX)),
        "Integer.MIN_VALUE" | "java.lang.Integer.MIN_VALUE" => Some(i64::from(i32::MIN)),
        "Long.MAX_VALUE" | "java.lang.Long.MAX_VALUE" => Some(i64::MAX),
        "Long.MIN_VALUE" | "java.lang.Long.MIN_VALUE" => Some(i64::MIN),
        _ => None,
    }
}

fn numeric_cast(type_name: &str, value: i64) -> Option<i64> {
    match type_name.trim() {
        "long" => Some(value),
        "int" => i32::try_from(value).ok().map(i64::from),
        "short" => i16::try_from(value).ok().map(i64::from),
        "byte" => i8::try_from(value).ok().map(i64::from),
        _ => None,
    }
}

fn eval_binary(left: i64, operator: &str, right: i64) -> Option<i64> {
    match operator {
        "+" => left.checked_add(right),
        "-" => left.checked_sub(right),
        "*" => left.checked_mul(right),
        "/" if right != 0 => left.checked_div(right),
        "%" if right != 0 => left.checked_rem(right),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use minecraft_redox_java::{JavaProjectIndex, analyze_source};

    use super::scalar_i64;

    #[test]
    fn evaluates_gt_recipe_tier_from_source_definitions() -> anyhow::Result<()> {
        let gt_values = analyze_source(
            "GTValues.java",
            "gt5",
            None,
            r"
                package gregtech.api.enums;
                import java.math.BigInteger;
                import java.util.Arrays;
                class GTValues {
                    static final long[] V = new long[] { 8L, 32L, 128L, 512L, 2048L };
                    static final long[] VP = Arrays.stream(V)
                        .map(i -> BigInteger.valueOf(i)
                            .multiply(BigInteger.valueOf(30))
                            .divide(BigInteger.valueOf(32))
                            .longValueExact())
                        .toArray();
                }
            ",
        )?;
        let voltage_index = analyze_source(
            "VoltageIndex.java",
            "gt5",
            None,
            r"
                package gregtech.api.enums;
                class VoltageIndex { static final int HV = 3; }
            ",
        )?;
        let tier_eu = analyze_source(
            "TierEU.java",
            "gt5",
            None,
            r"
                package gregtech.api.enums;
                class TierEU {
                    static final long RECIPE_HV = GTValues.VP[VoltageIndex.HV];
                }
            ",
        )?;
        let recipe = analyze_source(
            "Recipe.java",
            "gt5",
            None,
            r"
                package demo;
                import gregtech.api.enums.TierEU;
                class Recipe { void run() { builder.eut((int) TierEU.RECIPE_HV / 2); } }
            ",
        )?;
        let mut project = JavaProjectIndex::default();
        for analysis in [&gt_values, &voltage_index, &tier_eu, &recipe] {
            project.add_analysis(analysis);
        }
        let call = recipe
            .call_chains
            .iter()
            .flat_map(|chain| &chain.calls)
            .find(|call| call.name == "eut")
            .ok_or_else(|| anyhow::anyhow!("eut call missing"))?;
        let expression = call
            .argument_expressions
            .first()
            .ok_or_else(|| anyhow::anyhow!("eut expression missing"))?;
        assert_eq!(
            scalar_i64(expression, &recipe.bindings, &project,),
            Some(240)
        );
        Ok(())
    }
}
