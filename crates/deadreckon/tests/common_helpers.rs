#![allow(clippy::expect_used)]

mod common;

#[test]
fn common_helpers_compile_and_are_reused() {
    let temp = common::repo_tempdir();
    let paths = deadreckon_core::DeadreckonPaths::from_home(temp.path().join("home"));
    let mut command = common::deadreckon(&paths);
    command.arg("--version");
    let output = command.output().expect("deadreckon --version");
    common::assert_success(&output);
    assert!(common::stdout(&output).contains("deadreckon"));
    assert!(common::stderr(&output).is_empty());
}
