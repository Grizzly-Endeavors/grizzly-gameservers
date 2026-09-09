use super::*;

#[test]
fn no_arguments_supervises() {
    let empty: [&str; 0] = [];
    assert_eq!(classify_args(empty), Invocation::Supervise);
}

#[test]
fn version_flag_reports_version() {
    assert_eq!(classify_args(["--version"]), Invocation::ReportVersion);
}

#[test]
fn version_flag_is_recognised_after_other_arguments() {
    assert_eq!(
        classify_args(["--unrelated", "--version"]),
        Invocation::ReportVersion
    );
}

#[test]
fn unrecognised_arguments_still_supervise() {
    assert_eq!(classify_args(["-v", "version"]), Invocation::Supervise);
}

#[test]
fn version_string_carries_the_crate_name() {
    assert!(VERSION.starts_with("grizzly-supervisor "));
}

#[test]
fn sandbox_marker_carries_its_command() {
    assert_eq!(
        classify_args([SANDBOX_ARG, "grep -c . /etc/services"]),
        Invocation::RunSandboxedShell("grep -c . /etc/services".to_owned())
    );
}

#[test]
fn sandbox_marker_is_only_recognised_first() {
    // A command that merely mentions the marker is a command, not a mode switch.
    assert_eq!(
        classify_args(["--version", SANDBOX_ARG]),
        Invocation::ReportVersion
    );
    assert_eq!(classify_args(["echo", SANDBOX_ARG]), Invocation::Supervise);
}

#[test]
fn sandbox_marker_without_a_command_runs_nothing() {
    assert_eq!(
        classify_args([SANDBOX_ARG]),
        Invocation::RunSandboxedShell(String::new())
    );
}
