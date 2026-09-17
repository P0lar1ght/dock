//! Slash-line arguments for launching a named workflow (`/deep-research …`).
//! Copied from Grok `named_workflow_args`, minus model-specific effort enums:
//! `--effort` is accepted and dropped so it does not leak into `args.query`.

use super::grok_tool::WorkflowToolInput;

const MAX_AGENT_BUDGET: u64 = WorkflowToolInput::MAX_AGENT_BUDGET;

pub(crate) struct NamedWorkflowArgs {
    pub args: serde_json::Value,
    pub agent_budget: Option<u64>,
}

#[derive(serde::Deserialize)]
struct KnownLaunchArgs {
    #[serde(default, deserialize_with = "deserialize_agent_budget")]
    agent_budget: Option<u64>,
}

fn deserialize_agent_budget<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = <serde_json::Value as serde::Deserialize>::deserialize(deserializer)?;
    let budget = value
        .as_u64()
        .ok_or_else(|| serde::de::Error::custom("`agent_budget` must be a positive integer"))?;
    parse_agent_budget(budget)
        .map(Some)
        .map_err(serde::de::Error::custom)
}

fn parse_agent_budget(value: u64) -> Result<u64, String> {
    if value == 0 {
        return Err("`agent_budget` must be a positive integer".into());
    }
    if value > MAX_AGENT_BUDGET {
        return Err(format!(
            "`agent_budget` must be at most {MAX_AGENT_BUDGET} agents"
        ));
    }
    Ok(value)
}

pub(crate) fn parse_named_workflow_args(input: &str) -> Result<NamedWorkflowArgs, String> {
    let input = input.trim();
    let (flag_budget, input) = parse_named_workflow_flags(input)?;
    if input.is_empty() {
        return Ok(NamedWorkflowArgs {
            args: serde_json::Value::Null,
            agent_budget: flag_budget,
        });
    }
    if let Ok(args @ serde_json::Value::Object(_)) =
        serde_json::from_str::<serde_json::Value>(input)
    {
        let known: KnownLaunchArgs =
            serde_json::from_value(args.clone()).map_err(|error| error.to_string())?;
        if flag_budget.is_some() && known.agent_budget.is_some() {
            return Err("set `agent_budget` once, using either the slash flag or JSON".into());
        }
        return Ok(NamedWorkflowArgs {
            args,
            agent_budget: flag_budget.or(known.agent_budget),
        });
    }
    Ok(NamedWorkflowArgs {
        args: serde_json::json!({ "query": input, "objective": input }),
        agent_budget: flag_budget,
    })
}

fn parse_named_workflow_flags(mut input: &str) -> Result<(Option<u64>, &str), String> {
    let mut agent_budget = None;
    loop {
        if let Some((value, remaining)) = parse_leading_arg(input, "agent-budget")? {
            if agent_budget.is_some() {
                return Err("set `--agent-budget` once".into());
            }
            let budget = value
                .parse::<u64>()
                .map_err(|_| "`--agent-budget` must be a positive integer".to_string())?;
            agent_budget = Some(parse_agent_budget(budget)?);
            input = remaining;
        } else if let Some((_, remaining)) = parse_leading_arg(input, "effort")? {
            input = remaining;
        } else {
            return Ok((agent_budget, input));
        }
    }
}

fn parse_leading_arg<'a>(input: &'a str, name: &str) -> Result<Option<(&'a str, &'a str)>, String> {
    let flag = format!("--{name}");
    let Some(rest) = input.strip_prefix(&flag) else {
        return Ok(None);
    };
    let value_input = if let Some(rest) = rest.strip_prefix('=') {
        rest
    } else if rest.is_empty() {
        return Err(format!("`{flag}` requires a value"));
    } else if rest.chars().next().is_some_and(char::is_whitespace) {
        rest.trim_start()
    } else {
        return Ok(None);
    };
    if value_input.is_empty() {
        return Err(format!("`{flag}` requires a value"));
    }
    let (value, remaining) = value_input
        .split_once(char::is_whitespace)
        .map_or((value_input, ""), |(value, input)| {
            (value, input.trim_start())
        });
    Ok(Some((value, remaining)))
}

/// JSON arguments for `workflow` with `source.type = name`.
pub fn workflow_slash_arguments(name: &str, typed: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("workflow name is required".into());
    }
    let parsed = parse_named_workflow_args(typed)?;
    let mut value = serde_json::json!({
        "source": { "type": "name", "name": name }
    });
    if !parsed.args.is_null() {
        value["args"] = parsed.args;
    }
    if let Some(budget) = parsed.agent_budget {
        value["agent_budget"] = serde_json::json!(budget);
    }
    Ok(value.to_string())
}

/// `/workflow <name> [args]` → (name, tool JSON).
pub fn workflow_command_arguments(args: &str) -> Result<(String, String), String> {
    let args = args.trim();
    if args.is_empty() {
        return Err("用法: /workflow <name> [参数]".into());
    }
    let (name, rest) = match args.split_once(char::is_whitespace) {
        Some((n, r)) => (n, r.trim()),
        None => (args, ""),
    };
    let json = workflow_slash_arguments(name, rest)?;
    Ok((name.to_string(), json))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_becomes_query_and_objective() {
        let parsed = parse_named_workflow_args("audit the release").unwrap();
        assert_eq!(
            parsed.args,
            serde_json::json!({
                "query": "audit the release",
                "objective": "audit the release",
            })
        );
        assert_eq!(parsed.agent_budget, None);
    }

    #[test]
    fn empty_args_are_null() {
        let parsed = parse_named_workflow_args("").unwrap();
        assert_eq!(parsed.args, serde_json::Value::Null);
        assert_eq!(parsed.agent_budget, None);
    }

    #[test]
    fn json_object_is_preserved() {
        let parsed =
            parse_named_workflow_args(r#"{"query":"review this","target":"main"}"#).unwrap();
        assert_eq!(
            parsed.args,
            serde_json::json!({ "query": "review this", "target": "main" })
        );
    }

    #[test]
    fn slash_flag_promotes_budget() {
        let parsed = parse_named_workflow_args("--agent-budget=32 audit").unwrap();
        assert_eq!(parsed.agent_budget, Some(32));
        assert_eq!(
            parsed.args,
            serde_json::json!({ "query": "audit", "objective": "audit" })
        );
    }

    #[test]
    fn effort_flag_is_stripped() {
        let parsed = parse_named_workflow_args("--effort medium audit the release").unwrap();
        assert_eq!(
            parsed.args,
            serde_json::json!({
                "query": "audit the release",
                "objective": "audit the release",
            })
        );
    }

    #[test]
    fn workflow_slash_arguments_omits_null_args() {
        let json = workflow_slash_arguments("deep-research", "").unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["source"]["type"], "name");
        assert_eq!(v["source"]["name"], "deep-research");
        assert!(v.get("args").is_none());
    }

    #[test]
    fn workflow_command_arguments_splits_name() {
        let (name, json) = workflow_command_arguments("deep-research rust async").unwrap();
        assert_eq!(name, "deep-research");
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["args"]["query"], "rust async");
    }

    #[test]
    fn invalid_budget_is_rejected() {
        assert!(parse_named_workflow_args("--agent-budget 0 x").is_err());
        assert!(parse_named_workflow_args(r#"{"agent_budget":1025}"#).is_err());
    }
}
