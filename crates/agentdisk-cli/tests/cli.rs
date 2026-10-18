use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_agentdisk"))
}

#[test]
fn prints_version() {
    let out = bin().arg("--version").output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains(env!("CARGO_PKG_VERSION")), "got: {text}");
}

#[test]
fn prints_help() {
    let out = bin().arg("--help").output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("USAGE"));
}

#[test]
fn rejects_unknown_argument() {
    let out = bin().arg("--delete-everything").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
}
