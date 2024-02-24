use assert_cmd::prelude::*;
use assert_fs::prelude::*;
use predicates::prelude::*;
use std::process::Command;

#[test]
fn reorder() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("vcd-merger")?;

    let output = assert_fs::NamedTempFile::new("out.vcd")?;

    cmd.arg("tests/reorder.vcd").arg(output.path());

    cmd.assert().success();

    output.assert(predicate::path::exists());
    output.assert(predicate::path::eq_file("tests/reorder_expected.vcd"));

    Ok(())
}
