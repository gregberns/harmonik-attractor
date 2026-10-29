//! The binary is `harmonik-attractor`, with `hka` as a second name (ticket
//! 25); help and version text name the program as typed.

use std::process::Command;

fn stdout_of(bin: &str, args: &[&str]) -> String {
    let output = Command::new(bin).args(args).output().unwrap();
    assert!(output.status.success(), "{bin} {args:?}: {output:?}");
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn hka_version_names_hka() {
    let version = stdout_of(env!("CARGO_BIN_EXE_hka"), &["--version"]);
    assert_eq!(
        version.trim(),
        format!("hka {}", env!("CARGO_PKG_VERSION")),
        "{version}"
    );
}

#[test]
fn harmonik_attractor_version_names_harmonik_attractor() {
    let version = stdout_of(env!("CARGO_BIN_EXE_harmonik-attractor"), &["--version"]);
    assert_eq!(
        version.trim(),
        format!("harmonik-attractor {}", env!("CARGO_PKG_VERSION")),
        "{version}"
    );
}

#[test]
fn help_names_the_program_as_typed() {
    for (bin, name) in [
        (env!("CARGO_BIN_EXE_hka"), "hka"),
        (
            env!("CARGO_BIN_EXE_harmonik-attractor"),
            "harmonik-attractor",
        ),
    ] {
        let help = stdout_of(bin, &["--help"]);
        assert!(help.contains(&format!("Usage: {name} ")), "{help}");
        let run_help = stdout_of(bin, &["run", "--help"]);
        assert!(
            run_help.contains(&format!("Usage: {name} run ")),
            "{run_help}"
        );
    }
}
