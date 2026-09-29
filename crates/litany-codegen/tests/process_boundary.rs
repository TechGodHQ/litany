use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use tempfile::TempDir;

const ARTIFACTS: [&str; 4] = [
    "generated/cli.rs",
    "generated/http.rs",
    "generated/mcp.json",
    "generated/ts-client/index.ts",
];

fn fixture() -> TempDir {
    let temp = TempDir::new().expect("temporary fixture");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    copy(
        root.join("api/operations.yaml"),
        temp.path().join("api/operations.yaml"),
    );
    copy(root.join("hydra.yaml"), temp.path().join("hydra.yaml"));
    for artifact in ARTIFACTS {
        copy(root.join(artifact), temp.path().join(artifact));
    }
    temp
}

fn copy(from: PathBuf, to: PathBuf) {
    fs::create_dir_all(to.parent().expect("parent")).expect("create fixture parent");
    fs::copy(from, to).expect("copy fixture file");
}

fn run(root: &Path, command: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_litany-codegen"))
        .arg(command)
        .current_dir(root)
        .output()
        .expect("run litany-codegen")
}

#[test]
fn check_accepts_clean_fixture_without_rewriting_artifacts() {
    let temp = fixture();
    let before: Vec<_> = ARTIFACTS
        .iter()
        .map(|p| fs::read(temp.path().join(p)).unwrap())
        .collect();
    let output = run(temp.path(), "check");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let after: Vec<_> = ARTIFACTS
        .iter()
        .map(|p| fs::read(temp.path().join(p)).unwrap())
        .collect();
    assert_eq!(before, after, "check must not rewrite committed artifacts");
}

#[test]
fn check_reports_each_missing_or_stale_artifact() {
    for artifact in ARTIFACTS {
        let temp = fixture();
        fs::remove_file(temp.path().join(artifact)).unwrap();
        let output = run(temp.path(), "check");
        assert!(
            !output.status.success(),
            "missing {artifact} unexpectedly passed"
        );
        let diagnostic = String::from_utf8_lossy(&output.stderr);
        assert!(
            diagnostic.contains(artifact),
            "diagnostic did not name {artifact}: {diagnostic}"
        );

        let temp = fixture();
        fs::write(temp.path().join(artifact), "stale\n").unwrap();
        let output = run(temp.path(), "check");
        assert!(
            !output.status.success(),
            "stale {artifact} unexpectedly passed"
        );
        let diagnostic = String::from_utf8_lossy(&output.stderr);
        assert!(
            diagnostic.contains(artifact),
            "diagnostic did not name {artifact}: {diagnostic}"
        );
    }
}

#[test]
fn invalid_definition_and_config_fail_at_the_process_boundary() {
    let temp = fixture();
    fs::write(temp.path().join("api/operations.yaml"), "operations:\n  - name: broken\n    description: nope\n    method: GET\n    path: /broken\n    read: true\n    output_type: String\n    parameters:\n      - { name: x, description: x, type: string, required: true, location: nonsense }\n").unwrap();
    let output = run(temp.path(), "check");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("location"));

    let temp = fixture();
    fs::write(
        temp.path().join("hydra.yaml"),
        "http_dispatch_fn: crate::dispatch\nhttp_state_type: crate::State\nts_client_name: class\n",
    )
    .unwrap();
    let output = run(temp.path(), "write");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("ts_client_name"));

    let temp = fixture();
    fs::write(
        temp.path().join("hydra.yaml"),
        "http_dispatch_fn: crate::dispatch\nhttp_state_type: crate::State\nts_client_nam: LitanyClient\n",
    )
    .unwrap();
    let output = run(temp.path(), "write");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unknown hydra.yaml key"));
}

#[test]
fn two_writes_are_byte_identical() {
    let temp = fixture();
    assert!(run(temp.path(), "write").status.success());
    let first: Vec<_> = ARTIFACTS
        .iter()
        .map(|p| fs::read(temp.path().join(p)).unwrap())
        .collect();
    assert!(run(temp.path(), "write").status.success());
    let second: Vec<_> = ARTIFACTS
        .iter()
        .map(|p| fs::read(temp.path().join(p)).unwrap())
        .collect();
    assert_eq!(first, second);
}
