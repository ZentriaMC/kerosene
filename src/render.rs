use std::collections::HashMap;

use eyre::{Context, eyre};
use serde_yaml::Value;
use tracing::trace;

const MAX_RESOLVE_DEPTH: usize = 20;

fn new_environment() -> minijinja::Environment<'static> {
    let mut env = minijinja::Environment::new();
    env.set_undefined_behavior(minijinja::UndefinedBehavior::Strict);
    env
}

fn has_template(s: &str) -> bool {
    s.contains("{{") || s.contains("{%")
}

/// Render a single string through minijinja if it contains template syntax.
fn render_string(
    env: &minijinja::Environment,
    s: &str,
    context: &minijinja::Value,
) -> eyre::Result<String> {
    if !has_template(s) {
        return Ok(s.to_owned());
    }

    env.render_str(s, context)
        .wrap_err_with(|| format!("failed to render template: {s}"))
}

/// Recursively walk a `serde_yaml::Value` tree and render any string values
/// through minijinja. Non-string values pass through unchanged.
pub fn render_value(value: &Value, vars: &HashMap<String, Value>) -> eyre::Result<Value> {
    let env = new_environment();
    let context = minijinja::Value::from_serialize(vars);
    render_value_inner(&env, value, &context)
}

fn render_value_inner(
    env: &minijinja::Environment,
    value: &Value,
    context: &minijinja::Value,
) -> eyre::Result<Value> {
    match value {
        Value::String(s) => {
            let rendered = render_string(env, s, context)?;
            Ok(Value::String(rendered))
        }
        Value::Sequence(seq) => {
            let rendered: eyre::Result<Vec<Value>> = seq
                .iter()
                .map(|v| render_value_inner(env, v, context))
                .collect();
            Ok(Value::Sequence(rendered?))
        }
        Value::Mapping(map) => {
            let mut rendered = serde_yaml::Mapping::new();
            for (k, v) in map {
                rendered.insert(k.clone(), render_value_inner(env, v, context)?);
            }
            Ok(Value::Mapping(rendered))
        }
        // Number, Bool, Null, Tagged — pass through
        other => Ok(other.clone()),
    }
}

/// Render a single string template with the given variables.
pub fn render_str(template: &str, vars: &HashMap<String, Value>) -> eyre::Result<String> {
    let env = new_environment();
    let context = minijinja::Value::from_serialize(vars);
    render_string(&env, template, &context)
}

/// Iteratively resolve variable values that reference other variables.
///
/// Each pass renders every value through minijinja using the current map
/// as context. Stops when output equals input (stable). Errors after
/// `MAX_RESOLVE_DEPTH` iterations to catch circular references.
pub fn resolve_vars(vars: &HashMap<String, Value>) -> eyre::Result<HashMap<String, Value>> {
    let env = new_environment();
    let mut current = vars.clone();

    for depth in 0..MAX_RESOLVE_DEPTH {
        let context = minijinja::Value::from_serialize(&current);
        let mut next = HashMap::with_capacity(current.len());
        let mut changed = false;

        for (key, value) in &current {
            let rendered = render_value_inner(&env, value, &context)?;
            if rendered != *value {
                changed = true;
            }
            next.insert(key.clone(), rendered);
        }

        if !changed {
            trace!(depth, "variables resolved");
            return Ok(next);
        }

        current = next;
    }

    Err(eyre!(
        "variable resolution did not stabilize after {MAX_RESOLVE_DEPTH} iterations (circular reference?)"
    ))
}
