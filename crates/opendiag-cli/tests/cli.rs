use std::io::Write;
use std::process::{Command, Stdio};

fn run(args: &[&str], stdin: &str) -> (bool, String, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_opendiag"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn scan_prints_identification() {
    let (ok, out, _) = run(&["scan", "--simulate"], "");
    assert!(ok);
    assert!(out.contains("DEMO-0001-A"), "{out}");
    assert!(out.contains("DEMO00000000VIN01"), "{out}");
}

#[test]
fn dtc_read_lists_codes() {
    let (ok, out, _) = run(&["dtc", "read", "--simulate"], "");
    assert!(ok);
    assert!(
        out.contains("P0171-00") && out.contains("P0300-00"),
        "{out}"
    );
}

#[test]
fn ports_command_runs_without_hardware() {
    let (ok, _, err) = run(&["ports"], "");
    assert!(ok, "{err}");
}

#[test]
fn probe_reports_missing_port_and_rejects_bad_address() {
    let (ok, _, err) = run(&["probe", "--port", "NOPE-DOES-NOT-EXIST"], "");
    assert!(!ok);
    assert!(err.contains("cannot open"), "{err}");

    let (ok, _, err) = run(&["probe", "--port", "X", "--address", "zz"], "");
    assert!(!ok);
    assert!(err.contains("not a hex byte"), "{err}");
}

#[test]
fn kw1281_simulated_scan_and_dtc_read() {
    let (ok, out, err) = run(&["scan", "--simulate-kw1281"], "");
    assert!(ok, "{err}");
    assert!(
        out.contains("DEMO-KW-0001") && out.contains("VIRTUAL ENGINE ECU"),
        "{out}"
    );

    let (ok, out, err) = run(&["dtc", "read", "--simulate-kw1281"], "");
    assert!(ok, "{err}");
    assert!(out.contains("00525") && out.contains("00017"), "{out}");
}

#[test]
fn kw1281_simulated_clear_needs_confirmation() {
    let (ok, out, _) = run(&["dtc", "clear", "--simulate-kw1281"], "no\n");
    assert!(ok);
    assert!(
        out.contains("Cancelled") && !out.contains("Cleared"),
        "{out}"
    );

    let (ok, out, err) = run(&["dtc", "clear", "--simulate-kw1281", "--yes"], "");
    assert!(ok, "{err}");
    assert!(out.contains("Cleared 2 trouble code(s)"), "{out}");
}

#[test]
fn hardware_without_slow_init_explains_what_is_supported() {
    let (ok, _, err) = run(&["scan", "--port", "COM9"], "");
    assert!(!ok);
    assert!(err.contains("--slow-init"), "{err}");
}

#[test]
fn slow_init_scan_reports_missing_port() {
    let (ok, _, err) = run(
        &["scan", "--port", "NOPE-DOES-NOT-EXIST", "--slow-init"],
        "",
    );
    assert!(!ok);
    assert!(err.contains("cannot open"), "{err}");
}

fn data_dir() -> String {
    format!("{}/../../data", env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn data_validate_accepts_the_shipped_data() {
    let (ok, out, err) = run(&["data", "validate", &data_dir()], "");
    assert!(ok, "{err}");
    assert!(out.contains("OK:") && out.contains("14 module(s)"), "{out}");
}

#[test]
fn data_validate_reports_problems_with_file_names_and_fails() {
    let dir = std::env::temp_dir().join(format!("opendiag-cli-data-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("bad.yaml"),
        "format: opendiag-data/0\nkind: dtcs\n",
    )
    .unwrap();
    let (ok, _, err) = run(&["data", "validate", dir.to_str().unwrap()], "");
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(!ok);
    assert!(
        err.contains("bad.yaml") && err.contains("problem(s) found"),
        "{err}"
    );
}

#[test]
fn data_schema_prints_json_schema() {
    let (ok, out, _) = run(&["data", "schema"], "");
    assert!(ok);
    let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
    assert!(v.get("definitions").is_some(), "{out}");
}

#[test]
fn dtc_read_shows_descriptions_when_data_is_available() {
    let out = Command::new(env!("CARGO_BIN_EXE_opendiag"))
        .args(["dtc", "read", "--simulate"])
        .env("OPENDIAG_DATA", data_dir())
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&out.stdout);
    assert!(out.contains("Fuel mixture too lean on bank 1"), "{out}");
    assert!(out.contains("Random or multiple cylinder misfire"), "{out}");
}

#[test]
fn dtc_read_still_works_with_no_or_broken_data() {
    let out = Command::new(env!("CARGO_BIN_EXE_opendiag"))
        .args(["dtc", "read", "--simulate"])
        .env("OPENDIAG_DATA", "does-not-exist")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("P0171-00"));
}

#[test]
fn slow_init_probe_reports_missing_port() {
    let (ok, _, err) = run(
        &["probe", "--port", "NOPE-DOES-NOT-EXIST", "--slow-init"],
        "",
    );
    assert!(!ok);
    assert!(err.contains("cannot open"), "{err}");
}

#[test]
fn hardware_mode_is_refused_for_now() {
    let (ok, _, err) = run(&["scan"], "");
    assert!(!ok);
    assert!(err.contains("--simulate"), "{err}");
}

#[test]
fn clear_without_confirmation_changes_nothing() {
    let (ok, out, _) = run(&["dtc", "clear", "--simulate"], "no\n");
    assert!(ok);
    assert!(out.contains("Cancelled"), "{out}");
    assert!(!out.contains("Cleared"), "{out}");
}

#[test]
fn clear_with_confirmation_clears_and_reports() {
    let (ok, out, _) = run(&["dtc", "clear", "--simulate"], "YES\n");
    assert!(ok);
    assert!(out.contains("Cleared 2 trouble code(s)"), "{out}");
}

#[test]
fn clear_with_yes_flag_skips_prompt() {
    let (ok, out, _) = run(&["dtc", "clear", "--simulate", "--yes"], "");
    assert!(ok);
    assert!(out.contains("Cleared 2"), "{out}");
}
