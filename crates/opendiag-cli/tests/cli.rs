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

#[test]
fn log_writes_csv_to_stdout_with_changing_values() {
    let (ok, out, err) = run(
        &[
            "log",
            "--simulate-kw1281",
            "--groups",
            "1,2",
            "--samples",
            "3",
            "--interval-ms",
            "10",
        ],
        "",
    );
    assert!(ok, "{err}");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "time_ms,group,cell,formula,a,b");
    assert_eq!(lines.len(), 1 + 3 * 3, "3 rounds x (2 + 1 cells): {out}");
    assert!(lines[1].starts_with("0,1,1,1,16,32"), "{}", lines[1]);
    assert!(
        out.contains(",1,1,1,16,40\n"),
        "value changes between rounds: {out}"
    );
    assert!(err.contains("Logged 3 round(s), 9 row(s)"), "{err}");
}

#[test]
fn log_writes_to_a_file_with_out() {
    let path = std::env::temp_dir().join(format!("opendiag-log-{}.csv", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let (ok, out, err) = run(
        &[
            "log",
            "--simulate-kw1281",
            "--groups",
            "2",
            "--samples",
            "2",
            "--interval-ms",
            "10",
            "--out",
            path.to_str().unwrap(),
        ],
        "",
    );
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    assert!(ok, "{err}");
    assert!(out.is_empty(), "CSV goes to the file, not stdout: {out}");
    assert_eq!(text.lines().count(), 1 + 2);
}

#[test]
fn log_refuses_modules_without_live_data_and_bad_input() {
    let (ok, _, err) = run(&["log", "--simulate", "--groups", "1"], "");
    assert!(!ok);
    assert!(err.contains("only available for KW1281"), "{err}");

    let (ok, _, err) = run(&["log", "--simulate-kw1281", "--groups", "x"], "");
    assert!(!ok && err.contains("invalid value"), "{err}");

    let (ok, _, _) = run(&["log", "--simulate-kw1281"], "");
    assert!(!ok, "--groups is required");

    let (ok, _, err) = run(
        &[
            "log",
            "--simulate-kw1281",
            "--groups",
            "1",
            "--interval-ms",
            "0",
        ],
        "",
    );
    assert!(!ok && err.contains("interval"), "{err}");

    let (ok, _, err) = run(
        &[
            "log",
            "--simulate-kw1281",
            "--groups",
            "99",
            "--samples",
            "1",
        ],
        "",
    );
    assert!(
        !ok && err.contains("NAK"),
        "unknown group is reported: {err}"
    );
}

fn temp_file(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("opendiag-cli-{}-{name}", std::process::id()))
}

#[test]
fn trace_records_a_simulated_kw1281_session() {
    let path = temp_file("kw.trace");
    let (ok, _, err) = run(
        &[
            "scan",
            "--simulate-kw1281",
            "--trace",
            path.to_str().unwrap(),
        ],
        "",
    );
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    assert!(ok, "{err}");
    assert!(
        err.contains("Trace:") && err.contains("event(s) written"),
        "{err}"
    );

    let events = opendiag_transport::parse_trace(&text).expect("the trace parses");
    assert!(matches!(
        events[0].event,
        opendiag_transport::TraceEvent::SlowInit(1)
    ));
    let sent = events
        .iter()
        .filter(|e| matches!(e.event, opendiag_transport::TraceEvent::Tx(_)))
        .count();
    let received = events
        .iter()
        .filter(|e| matches!(e.event, opendiag_transport::TraceEvent::Rx(_)))
        .count();
    assert!(
        sent > 10 && received > 10,
        "{sent} sent, {received} received"
    );
    // The goodbye block sent when the session closes is recorded too, and the
    // report is printed after it (so the count includes it).
    let last_tx = events.iter().rev().find_map(|e| match &e.event {
        opendiag_transport::TraceEvent::Tx(f) => Some(f.clone()),
        _ => None,
    });
    assert_eq!(
        last_tx,
        Some(opendiag_transport::Frame::Raw(vec![0x03])),
        "ends with the block end byte"
    );
    assert!(err.contains(&format!("{} event(s)", events.len())), "{err}");
}

#[test]
fn trace_records_can_frames_for_uds() {
    let path = temp_file("uds.trace");
    let (ok, _, err) = run(
        &[
            "dtc",
            "read",
            "--simulate",
            "--trace",
            path.to_str().unwrap(),
        ],
        "",
    );
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    assert!(ok, "{err}");
    assert!(
        text.contains("TX can 7E0#") && text.contains("RX can 7E8#"),
        "{text}"
    );
}

#[test]
fn trace_to_an_unwritable_path_is_a_clear_error() {
    let (ok, _, err) = run(
        &[
            "scan",
            "--simulate-kw1281",
            "--trace",
            "no-such-dir-xyz/out.trace",
        ],
        "",
    );
    assert!(!ok);
    assert!(err.contains("cannot create"), "{err}");
}

#[test]
fn capture_requires_exactly_one_source_and_reports_a_missing_device() {
    let out = temp_file("cap.trace");
    let out = out.to_str().unwrap();
    let (ok, _, err) = run(&["capture", "--out", out], "");
    assert!(!ok && err.contains("required"), "{err}");

    let (ok, _, err) = run(
        &["capture", "--port", "COM1", "--can", "can0", "--out", out],
        "",
    );
    assert!(!ok && err.contains("cannot be used with"), "{err}");

    let (ok, _, err) = run(
        &["capture", "--port", "NOPE-DOES-NOT-EXIST", "--out", out],
        "",
    );
    assert!(!ok && err.contains("cannot open"), "{err}");

    #[cfg(not(target_os = "linux"))]
    {
        let (ok, _, err) = run(&["capture", "--can", "can0", "--out", out], "");
        assert!(!ok && err.contains("only available on Linux"), "{err}");
    }
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
