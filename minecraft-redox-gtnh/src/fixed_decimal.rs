use minecraft_redox_core::content::FixedDecimal;
use minecraft_redox_java::JavaExpression;

pub(crate) fn fixed_decimal_value(expression: &JavaExpression) -> Option<FixedDecimal> {
    match expression {
        JavaExpression::Literal(value) => parse_fixed_decimal_literal(value),
        JavaExpression::Parenthesized(inner) | JavaExpression::Cast { value: inner, .. } => {
            fixed_decimal_value(inner)
        }
        JavaExpression::Unary { operator, operand } if operator == "+" => {
            fixed_decimal_value(operand)
        }
        JavaExpression::Unary { operator, operand } if operator == "-" => {
            let mut value = fixed_decimal_value(operand)?;
            value.coefficient = value.coefficient.checked_neg()?;
            Some(value)
        }
        _ => None,
    }
}

pub(crate) fn parse_fixed_decimal_literal(source: &str) -> Option<FixedDecimal> {
    let mut text = source.trim().replace('_', "");
    if text.ends_with(['f', 'F', 'd', 'D']) {
        text.pop();
    }
    let negative = text.starts_with('-');
    if negative || text.starts_with('+') {
        text.remove(0);
    }
    let (base, exponent) = if let Some(index) = text.find(['e', 'E']) {
        (
            text.get(..index)?,
            text.get(index + 1..)?.parse::<i32>().ok()?,
        )
    } else {
        (text.as_str(), 0_i32)
    };
    let (integer, fraction) = base.split_once('.').unwrap_or((base, ""));
    if integer.is_empty() && fraction.is_empty() {
        return None;
    }
    let digits = format!("{integer}{fraction}");
    let mut coefficient = digits.parse::<i64>().ok()?;
    let mut scale = i32::try_from(fraction.len()).ok()?.checked_sub(exponent)?;
    if scale < 0 {
        coefficient = coefficient.checked_mul(10_i64.checked_pow(u32::try_from(-scale).ok()?)?)?;
        scale = 0;
    }
    if negative {
        coefficient = coefficient.checked_neg()?;
    }
    let mut decimal_places = u32::try_from(scale).ok()?;
    while decimal_places > 0 && coefficient % 10 == 0 {
        coefficient /= 10;
        decimal_places -= 1;
    }
    Some(FixedDecimal {
        coefficient,
        decimal_places,
    })
}

#[cfg(test)]
mod tests {
    use minecraft_redox_core::content::FixedDecimal;
    use minecraft_redox_java::JavaExpression;

    use super::{fixed_decimal_value, parse_fixed_decimal_literal};

    #[test]
    fn parses_decimal_literals_exactly() {
        assert_eq!(
            parse_fixed_decimal_literal("1_120.0f"),
            Some(FixedDecimal {
                coefficient: 1120,
                decimal_places: 0,
            })
        );
        assert_eq!(
            parse_fixed_decimal_literal("3.50F"),
            Some(FixedDecimal {
                coefficient: 35,
                decimal_places: 1,
            })
        );
        assert_eq!(
            parse_fixed_decimal_literal("-2.5e-2"),
            Some(FixedDecimal {
                coefficient: -25,
                decimal_places: 3,
            })
        );
    }

    #[test]
    fn unwraps_casts_parentheses_and_signs() {
        let expression = JavaExpression::Unary {
            operator: String::from("-"),
            operand: Box::new(JavaExpression::Cast {
                type_name: String::from("float"),
                value: Box::new(JavaExpression::Parenthesized(Box::new(
                    JavaExpression::Literal(String::from("1.25f")),
                ))),
            }),
        };
        assert_eq!(
            fixed_decimal_value(&expression),
            Some(FixedDecimal {
                coefficient: -125,
                decimal_places: 2,
            })
        );
    }
}
