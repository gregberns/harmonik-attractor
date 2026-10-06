//! `transcripts/<inv>.prompt.txt`: what an agent was started with, written
//! by `Agents::run` before the handler is called (design §1, §4).

use std::collections::BTreeMap;
use std::path::Path;

/// The prompt file's text: a version line, the invocation, the argv (an
/// argument equal to the prompt shown as `<prompt>`), the child
/// environment's variable names (sorted, never a value), then the prompt to
/// the end of the file.
pub fn prompt_file_text(
    invocation_id: &str,
    node_id: &str,
    attempt: u32,
    argv: &[String],
    env: &BTreeMap<String, String>,
    prompt: &str,
) -> String {
    let argv: Vec<&str> = argv
        .iter()
        .map(|arg| {
            if arg == prompt {
                "<prompt>"
            } else {
                arg.as_str()
            }
        })
        .collect();
    let names: Vec<&str> = env.keys().map(String::as_str).collect();
    format!(
        "pas prompt file v1\n\
         invocation: {invocation_id}\n\
         node: {node_id}\n\
         attempt: {attempt}\n\
         \n\
         argv:\n{argv}\n\
         \n\
         env (names only):\n{names}\n\
         \n\
         prompt:\n{prompt}",
        argv = argv.join("\n"),
        names = names.join("\n"),
    )
}

/// Write `text` to `path`, creating its folder. Observability only: the
/// caller logs a failure and runs the agent anyway.
pub(crate) fn write_prompt_file(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_text_has_the_argv_env_names_and_prompt_but_no_values() {
        let argv: Vec<String> = [
            "claude",
            "--model",
            "x",
            "-p",
            "line one\nline two",
            "--verbose",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        let env: BTreeMap<String, String> = [
            ("ZED", "z"),
            ("PAS_TEST_SECRET", "s3cr3t-value"),
            ("ALPHA", "a"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();

        let text = prompt_file_text("inv-1", "work", 2, &argv, &env, "line one\nline two");

        assert_eq!(
            text,
            "pas prompt file v1\n\
             invocation: inv-1\n\
             node: work\n\
             attempt: 2\n\
             \n\
             argv:\n\
             claude\n--model\nx\n-p\n<prompt>\n--verbose\n\
             \n\
             env (names only):\n\
             ALPHA\nPAS_TEST_SECRET\nZED\n\
             \n\
             prompt:\n\
             line one\nline two"
        );
        assert!(!text.contains("s3cr3t-value"));
    }

    #[test]
    fn write_creates_the_folder() {
        let dir = std::env::temp_dir().join(format!("pas-prompt-file-{}", std::process::id()));
        let path = dir.join("transcripts/inv.prompt.txt");
        write_prompt_file(&path, "hello").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
