//! The child environment: pure, computed once per invocation by `Agents`.

use std::collections::BTreeMap;

use crate::profile::ProfileEnv;
use crate::types::Record;

/// The `PAS_*` variables set from the record. A parent value is never kept.
const PAS_VARS: &[&str] = &[
    "PAS_RUN_ID",
    "PAS_NODE_ID",
    "PAS_ATTEMPT",
    "PAS_INVOCATION_ID",
    "PAS_SESSION_ID",
];

/// The parent environment minus the profile's `env.remove` and any `PAS_*`
/// variable above, plus the profile's `env.set`, then `PAS_NODE_ID`,
/// `PAS_ATTEMPT`, `PAS_INVOCATION_ID`, and `PAS_RUN_ID` when the record has
/// a run id. So `env.set` beats `env.remove`, and `PAS_*` beat both.
pub fn child_env(
    parent: &BTreeMap<String, String>,
    profile_env: &ProfileEnv,
    record: &Record,
    session_id: &str,
) -> BTreeMap<String, String> {
    let removed = |key: &str| profile_env.remove.iter().any(|r| r == key);
    let mut env: BTreeMap<String, String> = parent
        .iter()
        .filter(|(k, _)| !removed(k) && !PAS_VARS.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    env.extend(profile_env.set.iter().map(|(k, v)| (k.clone(), v.clone())));
    if let Some(run_id) = &record.run_id {
        env.insert("PAS_RUN_ID".into(), run_id.clone());
    }
    env.insert("PAS_NODE_ID".into(), record.node_id.clone());
    env.insert("PAS_ATTEMPT".into(), record.attempt.to_string());
    env.insert("PAS_INVOCATION_ID".into(), record.invocation_id.clone());
    env.insert("PAS_SESSION_ID".into(), session_id.to_string());
    env
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin_profiles;

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

    fn default_env() -> ProfileEnv {
        builtin_profiles().unwrap().remove(0).env
    }

    fn env(remove: &[&str], set: &[(&str, &str)]) -> ProfileEnv {
        ProfileEnv {
            remove: remove.iter().map(|k| k.to_string()).collect(),
            set: parent(set),
        }
    }

    #[test]
    fn strips_every_default_key_and_keeps_the_rest() {
        let defaults = default_env();
        let mut pairs: Vec<(&str, &str)> = defaults
            .remove
            .iter()
            .map(|k| (k.as_str(), "secret"))
            .collect();
        pairs.push(("PATH", "/bin"));
        pairs.push(("HOME", "/home/x"));
        let env = child_env(&parent(&pairs), &defaults, &record(Some("run-1")), "sess-1");
        for key in &defaults.remove {
            assert!(!env.contains_key(key), "{key} was passed through");
        }
        assert_eq!(env.get("PATH").map(String::as_str), Some("/bin"));
        assert_eq!(env.get("HOME").map(String::as_str), Some("/home/x"));
    }

    #[test]
    fn a_profile_remove_list_replaces_the_default_one() {
        // Design decision 8: a profile that sets `env.remove` replaces the
        // default strip list, so ANTHROPIC_API_KEY then passes through.
        let env = child_env(
            &parent(&[("ANTHROPIC_API_KEY", "k"), ("X", "x")]),
            &env(&["X"], &[]),
            &record(None),
            "sess-1",
        );
        assert_eq!(env.get("ANTHROPIC_API_KEY").map(String::as_str), Some("k"));
        assert!(!env.contains_key("X"));
    }

    #[test]
    fn set_beats_remove_and_pas_variables_beat_set() {
        let env = child_env(
            &parent(&[("K", "parent")]),
            &env(&["K"], &[("K", "profile"), ("PAS_NODE_ID", "mine")]),
            &record(None),
            "sess-1",
        );
        assert_eq!(env.get("K").map(String::as_str), Some("profile"));
        assert_eq!(env.get("PAS_NODE_ID").map(String::as_str), Some("work"));
    }

    #[test]
    fn adds_the_four_pas_variables_from_the_record() {
        let env = child_env(
            &parent(&[("PAS_NODE_ID", "stale")]),
            &default_env(),
            &record(Some("run-1")),
            "sess-1",
        );
        assert_eq!(env.get("PAS_RUN_ID").map(String::as_str), Some("run-1"));
        assert_eq!(env.get("PAS_NODE_ID").map(String::as_str), Some("work"));
        assert_eq!(env.get("PAS_ATTEMPT").map(String::as_str), Some("2"));
        assert_eq!(
            env.get("PAS_INVOCATION_ID").map(String::as_str),
            Some("inv-1")
        );
        assert_eq!(
            env.get("PAS_SESSION_ID").map(String::as_str),
            Some("sess-1")
        );
    }

    #[test]
    fn no_run_id_omits_pas_run_id_even_when_the_parent_has_one() {
        let env = child_env(
            &parent(&[("PAS_RUN_ID", "outer")]),
            &default_env(),
            &record(None),
            "sess-1",
        );
        assert!(!env.contains_key("PAS_RUN_ID"));
        assert_eq!(env.get("PAS_NODE_ID").map(String::as_str), Some("work"));
    }
}
