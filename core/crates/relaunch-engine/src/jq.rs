//! jq's value semantics, for the outputs ported from the bash engine's jq.

use std::cmp::Ordering;

use serde_json::Value;

/// `.key` on any value: `null` when absent or not an object (never panics).
#[must_use]
pub fn at<'a>(v: &'a Value, key: &str) -> &'a Value {
    v.get(key).unwrap_or(&Value::Null)
}

/// String interpolation (`"\(.x)"`): a string verbatim, anything else as JSON.
#[must_use]
pub fn interp(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// `//`: the value unless it is absent, `null` or `false`.
#[must_use]
pub fn or_default(v: Option<&Value>, default: Value) -> Value {
    match v {
        None | Some(Value::Null | Value::Bool(false)) => default,
        Some(v) => v.clone(),
    }
}

/// jq's total order: null < false < true < numbers < strings < arrays <
/// objects; numbers by value, strings by code point.
#[must_use]
pub fn cmp(a: &Value, b: &Value) -> Ordering {
    fn rank(v: &Value) -> u8 {
        match v {
            Value::Null => 0,
            Value::Bool(false) => 1,
            Value::Bool(true) => 2,
            Value::Number(_) => 3,
            Value::String(_) => 4,
            Value::Array(_) => 5,
            Value::Object(_) => 6,
        }
    }
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x
            .as_f64()
            .unwrap_or(0.0)
            .total_cmp(&y.as_f64().unwrap_or(0.0)),
        (Value::String(x), Value::String(y)) => x.cmp(y),
        _ => rank(a)
            .cmp(&rank(b))
            .then_with(|| a.to_string().cmp(&b.to_string())),
    }
}

/// jq's `==`: numbers compare by value.
#[must_use]
pub fn eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn jq_order_and_equality() {
        let mut v = vec![
            json!("b"),
            json!(2),
            json!(null),
            json!(true),
            json!("a"),
            json!(false),
            json!(1.5),
        ];
        v.sort_by(cmp);
        assert_eq!(
            v,
            vec![
                json!(null),
                json!(false),
                json!(true),
                json!(1.5),
                json!(2),
                json!("a"),
                json!("b")
            ]
        );
        assert!(eq(&json!(3), &json!(3.0)));
        assert_eq!(at(&json!([1]), "x"), &Value::Null);
        assert_eq!(or_default(Some(&json!(false)), json!("")), json!(""));
        assert_eq!(interp(&json!(null)), "null");
    }
}
