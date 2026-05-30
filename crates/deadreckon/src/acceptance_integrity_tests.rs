use std::collections::BTreeMap;

use tempfile::TempDir;

use super::{AcceptanceDraft, write_project_acceptance};

fn suppressed_draft(command: &str) -> AcceptanceDraft {
    AcceptanceDraft {
        yaml: format!("name: suppressed\nchecks:\n  - kind: shell\n    command: \"{command}\"\n"),
        markdown: "# Done Criteria\n".to_string(),
        files: BTreeMap::new(),
    }
}

#[test]
fn def_done_compile_rejects_or_true_suppression() {
    let temp = TempDir::new().expect("tempdir");
    let draft = suppressed_draft("cargo test || true");

    let err = write_project_acceptance(temp.path(), &draft, false, false).expect_err("reject");

    assert!(err.to_string().contains("suppression pattern '|| true'"));
    assert!(
        !temp.path().join(".deadreckon/acceptance.yaml").exists(),
        "rejected done criteria must not be written"
    );
}

#[test]
fn def_done_compile_rejects_no_verify_and_exit_zero() {
    let temp = TempDir::new().expect("tempdir");
    let no_verify = suppressed_draft("git commit --no-verify");
    let exit_zero = suppressed_draft("pytest --exit-zero");

    let no_verify_err =
        write_project_acceptance(temp.path(), &no_verify, false, false).expect_err("reject");
    let exit_zero_err =
        write_project_acceptance(temp.path(), &exit_zero, false, false).expect_err("reject");

    assert!(
        no_verify_err
            .to_string()
            .contains("suppression pattern '--no-verify'")
    );
    assert!(
        exit_zero_err
            .to_string()
            .contains("suppression pattern '--exit-zero'")
    );
}
