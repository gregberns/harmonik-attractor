//! The child environment: pure, computed once per invocation by `Agents`.

use std::collections::BTreeMap;

use crate::types::Record;

/// Removed from every child environment (design §2's default list), so an
/// agent bills the operator's subscription, never an API key it inherited.
pub const STRIPPED_ENV: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "OPENAI_API_KEY",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
];

/// The `PAS_*` variables set from the record. A parent value is never kept.
const PAS_VARS: &[&str] = &[
    "PAS_RUN_ID",
    "PAS_NODE_ID",
    "PAS_ATTEMPT",
    "PAS_INVOCATION_ID",
];

/// The parent environment minus [`STRIPPED_ENV`] and any `PAS_*` variable
/// above, plus `PAS_NODE_ID`, `PAS_ATTEMPT`, `PAS_INVOCATION_ID`, and
/// `PAS_RUN_ID` when the record has a run id.
pub fn child_env(parent: &BTreeMap<String, String>, record: &Record) -> BTreeMap<String, String> {
    let mut env: BTreeMap<String, String> = parent
        .iter()
        .filter(|(k, _)| !STRIPPED_ENV.contains(&k.as_str()) && !PAS_VARS.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    if let Some(run_id) = &record.run_id {
        env.insert("PAS_RUN_ID".into(), run_id.clone());
    }
    env.insert("PAS_NODE_ID".into(), record.node_id.clone());
    env.insert("PAS_ATTEMPT".into(), record.attempt.to_string());
    env.insert("PAS_INVOCATION_ID".into(), record.invocation_id.clone());
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(run_id: Option<&str>) -> Record {
        Record {
            run_id: run_id.map(String::from),
            node_id: "work".into(),
            attempt: 2,
            invocation_id: "inv-1".into(),
        }
    }

    fn parent(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn strips_every_listed_key_and_keeps_the_rest() {
        let mut pairs: Vec<(&str, &str)> = STRIPPED_ENV.iter().map(|k| (*k, "secret")).collect();
        pairs.push(("PATH", "/bin"));
        pairs.push(("HOME", "/home/x"));
        let env = child_env(&parent(&pairs), &record(Some("run-1")));
        for key in STRIPPED_ENV {
            assert!(!env.contains_key(*key), "{key} was passed through");
        }
        assert_eq!(env.get("PATH").map(String::as_str), Some("/bin"));
        assert_eq!(env.get("HOME").map(String::as_str), Some("/home/x"));
    }

    #[test]
    fn adds_the_four_pas_variables_from_the_record() {
        let env = child_env(&parent(&[("PAS_NODE_ID", "stale")]), &record(Some("run-1")));
        assert_eq!(env.get("PAS_RUN_ID").map(String::as_str), Some("run-1"));
        assert_eq!(env.get("PAS_NODE_ID").map(String::as_str), Some("work"));
        assert_eq!(env.get("PAS_ATTEMPT").map(String::as_str), Some("2"));
        assert_eq!(
            env.get("PAS_INVOCATION_ID").map(String::as_str),
            Some("inv-1")
        );
    }

    #[test]
    fn no_run_id_omits_pas_run_id_even_when_the_parent_has_one() {
        let env = child_env(&parent(&[("PAS_RUN_ID", "outer")]), &record(None));
        assert!(!env.contains_key("PAS_RUN_ID"));
        assert_eq!(env.get("PAS_NODE_ID").map(String::as_str), Some("work"));
    }
}
