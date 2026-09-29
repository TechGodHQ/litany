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

fn snapshot_artifacts(root: &Path) -> Vec<(bool, Option<Vec<u8>>)> {
    ARTIFACTS
        .iter()
        .map(|artifact| {
            let path = root.join(artifact);
            (
                path.exists(),
                path.exists().then(|| fs::read(path).unwrap()),
            )
        })
        .collect()
}

fn assert_artifacts_unchanged(root: &Path, before: &[(bool, Option<Vec<u8>>)]) {
    assert_eq!(
        before,
        snapshot_artifacts(root),
        "failed check must not create, remove, or rewrite artifacts"
    );
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
fn failed_checks_preserve_each_missing_or_stale_artifact() {
    for artifact in ARTIFACTS {
        let temp = fixture();
        fs::remove_file(temp.path().join(artifact)).unwrap();
        let before = snapshot_artifacts(temp.path());
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
        assert_artifacts_unchanged(temp.path(), &before);

        let temp = fixture();
        fs::write(temp.path().join(artifact), "stale\n").unwrap();
        let before = snapshot_artifacts(temp.path());
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
        assert_artifacts_unchanged(temp.path(), &before);
    }
}

#[test]
fn invalid_definition_and_config_fail_at_the_process_boundary() {
    let temp = fixture();
    fs::write(temp.path().join("api/operations.yaml"), "operations:\n  - name: broken\n    description: nope\n    method: GET\n    path: /broken\n    read: true\n    output_type: String\n    parameters:\n      - { name: x, description: x, type: string, required: true, location: nonsense }\n").unwrap();
    let before = snapshot_artifacts(temp.path());
    let output = run(temp.path(), "check");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("location"));
    assert_artifacts_unchanged(temp.path(), &before);

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
fn malformed_yaml_is_rejected_without_artifact_mutation() {
    for (source, malformed) in [
        ("api/operations.yaml", "operations: [\n"),
        ("hydra.yaml", "http_dispatch_fn: [\n"),
    ] {
        for command in ["check", "write"] {
            let temp = fixture();
            fs::write(temp.path().join(source), malformed).unwrap();
            let before = snapshot_artifacts(temp.path());
            let output = run(temp.path(), command);
            assert!(
                !output.status.success(),
                "malformed {source} unexpectedly passed {command}"
            );
            let diagnostic = String::from_utf8_lossy(&output.stderr).to_lowercase();
            assert!(
                diagnostic.contains("yaml") || diagnostic.contains("parse"),
                "diagnostic was not a useful YAML parse failure for {source} {command}: {diagnostic}"
            );
            assert_artifacts_unchanged(temp.path(), &before);
        }
    }
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
