use std::io::Write;
use std::process::{Command, Stdio};

/// Where commands that write a backup go unless a test says otherwise, so a test
/// that forgets `--backup-file` can never leave a file in the repository.
fn default_backup_file() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("opendiag-cli-default-{}.jsonl", std::process::id()))
}

fn run(args: &[&str], stdin: &str) -> (bool, String, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_opendiag"))
        .env("OPENDIAG_BACKUPS", default_backup_file())
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
fn kw1281_simulated_clear_snapshots_clears_and_reports_what_the_module_says() {
    let bk = temp_file("kw-clear.jsonl");
    let _ = std::fs::remove_file(&bk);
    let (ok, out, err) = run(
        &[
            "dtc",
            "clear",
            "--simulate-kw1281",
            "--backup-file",
            bk.to_str().unwrap(),
        ],
        "YES\n",
    );
    assert!(ok, "{err}");
    assert!(
        out.contains("cannot be undone") && out.contains("snapshot"),
        "{out}"
    );
    assert!(
        out.contains("Cleared 2 trouble code(s). Snapshot b0001"),
        "{out}"
    );
    assert!(
        out.contains("The module now reports no trouble codes"),
        "{out}"
    );

    let (ok, listing, _) = run(
        &["backup", "list", "--backup-file", bk.to_str().unwrap()],
        "",
    );
    std::fs::remove_file(&bk).unwrap();
    assert!(ok);
    assert!(
        listing.contains("b0001")
            && listing.contains("fault codes cleared (2 stored)")
            && listing.contains("was: 00525 status 0x20")
            && listing.contains("Verified"),
        "{listing}"
    );
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

#[test]
fn adaptation_read_shows_the_value_and_unknown_channels_are_refused() {
    let (ok, out, err) = run(
        &["adaptation", "read", "--simulate-kw1281", "--channel", "4"],
        "",
    );
    assert!(ok, "{err}");
    assert!(out.contains("Channel 4: 100"), "{out}");
    let (ok, _, err) = run(
        &["adaptation", "read", "--simulate-kw1281", "--channel", "99"],
        "",
    );
    assert!(!ok && err.contains("NAK"), "{err}");
    let (ok, _, err) = run(&["adaptation", "read", "--simulate", "--channel", "4"], "");
    assert!(!ok && err.contains("only available for KW1281"), "{err}");
}

#[test]
fn adaptation_set_backs_up_confirms_verifies_and_lists() {
    let store = temp_file("set.jsonl");
    let _ = std::fs::remove_file(&store);
    let s = store.to_str().unwrap();
    let args = [
        "adaptation",
        "set",
        "--simulate-kw1281",
        "--channel",
        "4",
        "--value",
        "200",
        "--workshop-code",
        "12345",
        "--backup-file",
        s,
    ];
    let (ok, out, err) = run(&args, "YES\n");
    assert!(ok, "{err}");
    assert!(out.contains("Adaptation channel 4: 100 -> 200"), "{out}");
    assert!(out.contains("WRITES to the module"), "{out}");
    assert!(
        out.contains("Done and verified") && out.contains("b0001"),
        "{out}"
    );

    let (ok, out, err) = run(&["backup", "list", "--backup-file", s], "");
    assert!(ok, "{err}");
    assert!(
        out.contains("b0001") && out.contains("channel 4: 100 -> 200") && out.contains("Verified"),
        "{out}"
    );
    std::fs::remove_file(&store).unwrap();
}

#[test]
fn adaptation_set_without_confirmation_changes_nothing_and_writes_no_backup() {
    let store = temp_file("cancel.jsonl");
    let _ = std::fs::remove_file(&store);
    let args = [
        "adaptation",
        "set",
        "--simulate-kw1281",
        "--channel",
        "4",
        "--value",
        "200",
        "--workshop-code",
        "0",
        "--backup-file",
        store.to_str().unwrap(),
    ];
    for answer in ["no\n", "\n", "yes\n", ""] {
        let (ok, out, _) = run(&args, answer);
        assert!(ok);
        assert!(out.contains("Cancelled"), "{answer:?}: {out}");
        assert!(
            !store.exists(),
            "a cancelled change must not create a backup ({answer:?})"
        );
    }
}

#[test]
fn adaptation_set_has_no_yes_flag_and_validates_the_workshop_code() {
    let base = [
        "adaptation",
        "set",
        "--simulate-kw1281",
        "--channel",
        "4",
        "--value",
        "200",
    ];
    let (ok, _, err) = run(
        &[&base[..], &["--workshop-code", "0", "--yes"]].concat(),
        "",
    );
    assert!(!ok && err.contains("--yes"), "no bypass flag: {err}");
    let (ok, _, err) = run(&base, "");
    assert!(!ok && err.contains("--workshop-code"), "required: {err}");
    let (ok, _, err) = run(&[&base[..], &["--workshop-code", "16777216"]].concat(), "");
    assert!(!ok && err.contains("not in 0..=16777215"), "{err}");
    let (ok, _, err) = run(&[&base[..], &["--workshop-code", "x"]].concat(), "");
    assert!(!ok && err.contains("invalid value"), "{err}");
}

#[test]
fn setting_the_value_it_already_has_is_a_no_op() {
    let store = temp_file("noop.jsonl");
    let _ = std::fs::remove_file(&store);
    let (ok, out, err) = run(
        &[
            "adaptation",
            "set",
            "--simulate-kw1281",
            "--channel",
            "4",
            "--value",
            "100",
            "--workshop-code",
            "0",
            "--backup-file",
            store.to_str().unwrap(),
        ],
        "YES\n",
    );
    assert!(ok, "{err}");
    assert!(
        out.contains("nothing to change") && !store.exists(),
        "{out}"
    );
}

#[test]
fn backup_restore_puts_a_value_back_and_records_the_restore() {
    use opendiag_services::backup::NewBackup;
    let path = temp_file("restore.jsonl");
    let _ = std::fs::remove_file(&path);
    // A past change on module 0x01, channel 4: 50 -> 100 (the virtual module now holds 100).
    let store = opendiag_services::BackupStore::new(&path);
    store
        .plan(NewBackup {
            protocol: "kw1281".into(),
            module_address: 1,
            kind: "adaptation".into(),
            channel: 4,
            old_value: 50,
            new_value: 100,
            restores: None,
        })
        .unwrap();
    let s = path.to_str().unwrap();
    let args = [
        "backup",
        "restore",
        "b0001",
        "--simulate-kw1281",
        "--workshop-code",
        "0",
        "--backup-file",
        s,
    ];

    let (ok, out, _) = run(&args, "no\n");
    assert!(
        ok && out.contains("currently 100, will be set back to 50") && out.contains("Cancelled"),
        "{out}"
    );
    assert_eq!(
        store.records().unwrap().len(),
        1,
        "cancelling records nothing"
    );

    let (ok, out, err) = run(&args, "YES\n");
    assert!(ok, "{err}");
    assert!(
        out.contains("Restored and verified") && out.contains("b0002"),
        "{out}"
    );
    let (_, listing, _) = run(&["backup", "list", "--backup-file", s], "");
    assert!(
        listing.contains("Restored") && listing.contains("(restores b0001)"),
        "{listing}"
    );
    std::fs::remove_file(&path).unwrap();

    let (ok, out, _) = run(&["backup", "list", "--backup-file", s], "");
    assert!(ok && out.contains("No backups"), "{out}");
    let (ok, _, err) = run(
        &[
            "backup",
            "restore",
            "b0001",
            "--simulate-kw1281",
            "--workshop-code",
            "0",
            "--backup-file",
            s,
        ],
        "YES\n",
    );
    assert!(!ok && err.contains("no backup record"), "{err}");
}

#[test]
fn unix_times_in_the_backup_list_are_formatted_as_utc_dates() {
    use opendiag_services::backup::NewBackup;
    // Write a record with a known timestamp by hand.
    let path = temp_file("dates.jsonl");
    let _ = std::fs::remove_file(&path);
    let store = opendiag_services::BackupStore::new(&path);
    let rec = store
        .plan(NewBackup {
            protocol: "kw1281".into(),
            module_address: 1,
            kind: "adaptation".into(),
            channel: 4,
            old_value: 1,
            new_value: 2,
            restores: None,
        })
        .unwrap();
    // 2026-11-13 17:06:23 UTC = 1_794_589_583
    let line = std::fs::read_to_string(&path).unwrap().replace(
        &format!("\"created_unix\":{}", rec.created_unix),
        "\"created_unix\":1794589583",
    );
    std::fs::write(&path, line).unwrap();
    let (ok, out, _) = run(
        &["backup", "list", "--backup-file", path.to_str().unwrap()],
        "",
    );
    std::fs::remove_file(&path).unwrap();
    assert!(ok && out.contains("2026-11-13 17:06:23 UTC"), "{out}");
}

#[test]
fn kwp2000_simulated_scan_and_dtc_read() {
    let (ok, out, err) = run(&["scan", "--simulate-kwp"], "");
    assert!(ok, "{err}");
    assert!(
        out.contains("Ident            : VIRTUAL KWP2000 ECU"),
        "{out}"
    );

    let (ok, out, err) = run(&["dtc", "read", "--simulate-kwp"], "");
    assert!(ok, "{err}");
    assert!(
        out.contains("12345  status 0x20") && out.contains("54321  status 0x01"),
        "{out}"
    );
}

#[test]
fn kwp2000_guarded_clear_really_empties_the_module_and_lists_the_snapshot() {
    let bk = temp_file("kwp-clear.jsonl");
    let _ = std::fs::remove_file(&bk);
    let b = bk.to_str().unwrap();
    let (ok, out, err) = run(
        &["dtc", "clear", "--simulate-kwp", "--backup-file", b],
        "YES\n",
    );
    assert!(ok, "{err}");
    assert!(
        out.contains("Cleared 2 trouble code(s). Snapshot b0001"),
        "{out}"
    );
    assert!(
        out.contains("The module now reports no trouble codes"),
        "{out}"
    );

    let (_, listing, _) = run(&["backup", "list", "--backup-file", b], "");
    std::fs::remove_file(&bk).unwrap();
    assert!(
        listing.contains("KWP2000 module 0x01 fault codes cleared (2 stored)")
            && listing.contains("was: 12345 status 0x20")
            && listing.contains("Verified"),
        "{listing}"
    );
}

#[test]
fn kwp2000_trace_starts_with_the_fast_init_wakeup() {
    let path = temp_file("kwp.trace");
    let (ok, _, err) = run(
        &["scan", "--simulate-kwp", "--trace", path.to_str().unwrap()],
        "",
    );
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    assert!(ok, "{err}");
    let events = opendiag_transport::parse_trace(&text).unwrap();
    assert!(
        matches!(events[0].event, opendiag_transport::TraceEvent::FastInit),
        "{:?}",
        events[0]
    );
    // StartCommunication (0x81) follows, then the diagnostic session request.
    assert!(text.contains("TX raw 81 01 F1 81"), "{text}");
}

#[test]
fn kwp2000_has_no_measuring_block_groups_or_adaptation() {
    let (ok, _, err) = run(&["log", "--simulate-kwp", "--groups", "1"], "");
    assert!(!ok && err.contains("only available for KW1281"), "{err}");
    let (ok, _, err) = run(
        &["adaptation", "read", "--simulate-kwp", "--channel", "1"],
        "",
    );
    assert!(!ok && err.contains("only available for KW1281"), "{err}");
}

#[test]
fn hardware_flag_combinations_are_validated_with_helpful_messages() {
    let (ok, _, err) = run(
        &["scan", "--port", "COM9", "--slow-init", "--fast-init"],
        "",
    );
    assert!(!ok && err.contains("cannot be used with"), "{err}");

    let (ok, _, err) = run(
        &["scan", "--port", "NOPE-DOES-NOT-EXIST", "--fast-init"],
        "",
    );
    assert!(!ok && err.contains("cannot open"), "{err}");

    let (ok, _, err) = run(&["scan", "--port", "COM9", "--can", "can0"], "");
    assert!(!ok && err.contains("not both"), "{err}");

    let (ok, _, err) = run(&["scan"], "");
    assert!(
        !ok && err.contains("--fast-init") && err.contains("--can"),
        "{err}"
    );

    let (ok, _, err) = run(&["scan", "--can", "opendiag-no-such-if"], "");
    assert!(
        !ok && (err.contains("only available on Linux") || err.contains("cannot open")),
        "{err}"
    );
}

#[test]
fn report_writes_a_labelled_simulated_html_file() {
    let out = temp_file("report.html");
    let _ = std::fs::remove_file(&out);
    for (flag, marker) in [
        ("--simulate-kw1281", "VIRTUAL ENGINE ECU"),
        ("--simulate-kwp", "VIRTUAL KWP2000 ECU"),
        ("--simulate", "P0171-00"),
    ] {
        let (ok, stdout, err) = run(&["report", flag, "--out", out.to_str().unwrap()], "");
        assert!(ok, "{flag}: {err}");
        assert!(stdout.is_empty(), "the HTML goes to the file, not stdout");
        assert!(
            err.contains("Report written to") && err.contains("review it before sharing"),
            "{err}"
        );
        let html = std::fs::read_to_string(&out).unwrap();
        assert!(html.starts_with("<!doctype html>"), "{flag}");
        assert!(
            html.contains("SIMULATED DATA"),
            "{flag}: simulated reports must say so"
        );
        assert!(html.contains(marker), "{flag}: {marker}");
    }
    let _ = std::fs::remove_file(&out);
}

#[test]
fn report_goes_to_stdout_by_default_and_is_read_only() {
    let (ok, out, err) = run(&["report", "--simulate-kw1281"], "");
    assert!(ok, "{err}");
    assert!(
        out.starts_with("<!doctype html>") && out.contains("00525") && out.contains("00017"),
        "{out}"
    );
    // Read-only: no backup file is created by generating a report.
    assert!(!default_backup_file().exists());
}

#[test]
fn report_uses_the_data_set_for_descriptions_and_escapes_them() {
    let dir = std::env::temp_dir().join(format!("opendiag-report-data-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // A valid data file whose description tries to inject markup.
    std::fs::write(
        dir.join("dtc.yaml"),
        "format: opendiag-data/0\nkind: dtcs\nprovenance:\n  author: T\n  source: original_work\n  license: CC0-1.0\nentries:\n  - { code_space: sae_j2012, code: P0171, description: \"<b onmouseover=x>lean</b>\" }\n",
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_opendiag"))
        .args(["report", "--simulate"])
        .env("OPENDIAG_DATA", &dir)
        .output()
        .unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    let html = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success());
    assert!(
        html.contains("&lt;b onmouseover=x&gt;lean&lt;/b&gt;"),
        "{html}"
    );
    assert!(!html.contains("<b onmouseover"), "{html}");

    // With the shipped data set, a real description appears.
    let shipped = Command::new(env!("CARGO_BIN_EXE_opendiag"))
        .args(["report", "--simulate"])
        .env("OPENDIAG_DATA", data_dir())
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&shipped.stdout).contains("Fuel mixture too lean on bank 1"));
}

#[test]
fn report_errors_are_clear() {
    let (ok, _, err) = run(
        &[
            "report",
            "--simulate-kw1281",
            "--out",
            "no-such-dir-xyz/r.html",
        ],
        "",
    );
    assert!(!ok && err.contains("cannot write"), "{err}");
    let (ok, _, err) = run(&["report"], "");
    assert!(!ok && err.contains("no connection chosen"), "{err}");
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
fn clear_without_confirmation_changes_nothing_and_writes_no_snapshot() {
    let bk = temp_file("cancel-clear.jsonl");
    let _ = std::fs::remove_file(&bk);
    for answer in ["no\n", "\n", "yes\n", "y\n", ""] {
        for sim in ["--simulate", "--simulate-kw1281", "--simulate-kwp"] {
            let (ok, out, _) = run(
                &["dtc", "clear", sim, "--backup-file", bk.to_str().unwrap()],
                answer,
            );
            assert!(ok);
            assert!(
                out.contains("Cancelled") && !out.contains("Cleared"),
                "{sim} {answer:?}: {out}"
            );
            assert!(
                !bk.exists(),
                "a cancelled clear must not write a snapshot ({sim} {answer:?})"
            );
        }
    }
}

#[test]
fn clear_on_a_module_whose_fault_is_active_says_so_instead_of_claiming_success() {
    // The virtual UDS ECU keeps reporting its codes after a clear, like an active fault.
    let bk = temp_file("uds-clear.jsonl");
    let _ = std::fs::remove_file(&bk);
    let (ok, out, err) = run(
        &[
            "dtc",
            "clear",
            "--simulate",
            "--backup-file",
            bk.to_str().unwrap(),
        ],
        "YES\n",
    );
    let (_, listing, _) = run(
        &["backup", "list", "--backup-file", bk.to_str().unwrap()],
        "",
    );
    std::fs::remove_file(&bk).unwrap();
    assert!(ok, "{err}");
    assert!(out.contains("Cleared 2 trouble code(s)"), "{out}");
    assert!(
        out.contains("still reported") && !out.contains("now reports no trouble codes"),
        "{out}"
    );
    assert!(
        listing.contains("VerificationFailed") && listing.contains("still reported after clearing"),
        "{listing}"
    );
}

#[test]
fn clear_has_no_yes_flag() {
    let (ok, _, err) = run(&["dtc", "clear", "--simulate", "--yes"], "");
    assert!(!ok && err.contains("--yes"), "{err}");
}

#[test]
fn a_clear_snapshot_cannot_be_restored() {
    let bk = temp_file("norestore.jsonl");
    let _ = std::fs::remove_file(&bk);
    let b = bk.to_str().unwrap();
    run(
        &["dtc", "clear", "--simulate-kw1281", "--backup-file", b],
        "YES\n",
    );
    let (ok, _, err) = run(
        &[
            "backup",
            "restore",
            "b0001",
            "--simulate-kw1281",
            "--workshop-code",
            "0",
            "--backup-file",
            b,
        ],
        "YES\n",
    );
    std::fs::remove_file(&bk).unwrap();
    assert!(!ok && err.contains("cannot be written back"), "{err}");
}

// ---- opendiag serve (read-only local web interface) ----

struct Server {
    child: std::process::Child,
    port: u16,
    token: String,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start_server(args: &[&str]) -> Server {
    use std::io::{BufRead, BufReader};
    let mut child = Command::new(env!("CARGO_BIN_EXE_opendiag"))
        .arg("serve")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn opendiag serve");
    let mut line = String::new();
    BufReader::new(child.stdout.take().expect("stdout"))
        .read_line(&mut line)
        .expect("read url");
    let url = line.trim();
    let rest = url
        .strip_prefix("http://127.0.0.1:")
        .unwrap_or_else(|| panic!("unexpected first line: {url:?}"));
    let (port, token) = rest.split_once("/?t=").expect("url shape");
    Server {
        port: port.parse().expect("port"),
        token: token.to_string(),
        child,
    }
}

/// Minimal HTTP/1.1 client: returns (status, lowercase header block, body).
fn http(
    port: u16,
    method: &str,
    path: &str,
    host: &str,
    token: Option<&str>,
) -> (u16, String, String) {
    use std::io::Read;
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .unwrap();
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
    if let Some(t) = token {
        req.push_str(&format!("X-OpenDiag-Token: {t}\r\n"));
    }
    req.push_str("\r\n");
    s.write_all(req.as_bytes()).unwrap();
    let mut raw = String::new();
    s.read_to_string(&mut raw).expect("read response");
    let (head, body) = raw.split_once("\r\n\r\n").expect("response shape");
    let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
    (status, head.to_ascii_lowercase(), body.to_string())
}

#[test]
fn serve_answers_over_real_http_with_security_headers() {
    let srv = start_server(&["--simulate-kw1281"]);
    let host = format!("127.0.0.1:{}", srv.port);

    let (status, head, body) = http(srv.port, "GET", &format!("/?t={}", srv.token), &host, None);
    assert_eq!(status, 200);
    assert!(body.contains("<title>OpenDiag</title>"));
    assert!(
        head.contains("content-security-policy: default-src 'none'"),
        "{head}"
    );
    assert!(head.contains("x-content-type-options: nosniff"));
    assert!(head.contains("cache-control: no-store"));

    let (status, _, body) = http(srv.port, "GET", "/api/ident", &host, Some(&srv.token));
    assert_eq!(status, 200);
    assert!(body.contains("VIRTUAL ENGINE ECU"), "{body}");
    let (status, _, body) = http(srv.port, "GET", "/api/info", &host, Some(&srv.token));
    assert_eq!(status, 200);
    assert!(body.contains("\"simulated\":true"));
    let (status, _, body) = http(
        srv.port,
        "GET",
        "/api/live?groups=1",
        &host,
        Some(&srv.token),
    );
    assert_eq!(status, 200, "{body}");
}

#[test]
fn serve_refuses_bad_token_host_and_methods_over_real_http() {
    let srv = start_server(&["--simulate-kw1281"]);
    let host = format!("127.0.0.1:{}", srv.port);

    assert_eq!(http(srv.port, "GET", "/api/dtcs", &host, None).0, 401);
    assert_eq!(
        http(srv.port, "GET", "/api/dtcs", &host, Some("wrong")).0,
        401
    );
    assert_eq!(http(srv.port, "GET", "/", &host, None).0, 401);
    // DNS rebinding: right token, wrong Host.
    let (status, _, _) = http(
        srv.port,
        "GET",
        "/api/dtcs",
        "evil.example",
        Some(&srv.token),
    );
    assert_eq!(status, 403);
    // Read-only: no method other than GET, whatever the token.
    for m in ["POST", "PUT", "DELETE"] {
        assert_eq!(
            http(srv.port, m, "/api/dtcs", &host, Some(&srv.token)).0,
            405,
            "{m}"
        );
    }
    // Refusals still carry the security headers.
    let (_, head, _) = http(srv.port, "GET", "/api/dtcs", &host, None);
    assert!(head.contains("content-security-policy"));
}

#[test]
fn serve_uses_a_fresh_token_each_run() {
    let a = start_server(&["--simulate-kw1281"]);
    let b = start_server(&["--simulate-kw1281"]);
    assert_ne!(a.token, b.token);
    assert_eq!(a.token.len(), 32);
}

#[test]
fn data_lookup_finds_known_codes_and_rejects_unknown_ones() {
    let ok = Command::new(env!("CARGO_BIN_EXE_opendiag"))
        .args(["data", "lookup", "P0171-00", "--path", "../../data"])
        .output()
        .unwrap();
    assert!(
        ok.status.success(),
        "{}",
        String::from_utf8_lossy(&ok.stderr)
    );
    assert!(String::from_utf8_lossy(&ok.stdout).contains("too lean"));
    let bad = Command::new(env!("CARGO_BIN_EXE_opendiag"))
        .args(["data", "lookup", "P9999", "--path", "../../data"])
        .output()
        .unwrap();
    assert!(!bad.status.success());
}

// ---- ELM327 adapter path (software adapter in front of the virtual UDS ECU) ----

#[test]
fn elm327_simulation_gives_the_same_codes_as_the_direct_simulation() {
    let (ok_direct, direct, _) = run(&["dtc", "read", "--simulate"], "");
    let (ok_elm, elm, err) = run(&["dtc", "read", "--simulate-elm327"], "");
    assert!(ok_direct && ok_elm, "{err}");
    assert_eq!(direct, elm);
    assert!(
        err.contains("[simulated]") && err.contains("ELM327"),
        "{err}"
    );
}

#[test]
fn elm327_simulation_scan_and_report_are_marked_simulated() {
    let (ok, out, err) = run(&["scan", "--simulate-elm327"], "");
    assert!(ok, "{err}");
    assert!(!out.is_empty());
    let (ok, html, err) = run(&["report", "--simulate-elm327"], "");
    assert!(ok, "{err}");
    assert!(
        html.contains("SIMULATED"),
        "report must carry the simulated banner"
    );
}

#[test]
fn elm327_options_conflict_with_other_connections() {
    let (ok, _, err) = run(&["scan", "--simulate-elm327", "--port", "COM1"], "");
    assert!(!ok);
    assert!(err.contains("choose one of"), "{err}");
}

#[test]
fn doctor_prints_a_summary_and_writes_it_to_a_file_without_a_vin() {
    let out = std::env::temp_dir().join(format!("opendiag-doctor-{}.txt", std::process::id()));
    let (ok, stdout, err) = run(&["doctor", "--out", out.to_str().unwrap()], "");
    assert!(ok, "{err}");
    for want in [
        "version:",
        "platform:",
        "self-check: KW1281 virtual module OK",
        "self-check: UDS virtual module OK",
        "self-check: KWP2000 virtual module OK",
        "says nothing about your cable",
    ] {
        assert!(stdout.contains(want), "missing {want:?} in {stdout}");
    }
    let file = std::fs::read_to_string(&out).unwrap();
    std::fs::remove_file(&out).ok();
    assert_eq!(file, stdout);
    // The summary is for sharing: it must not contain identification from modules.
    assert!(!stdout.contains("DEMO-KW-0001") && !stdout.contains("VIRTUAL ENGINE ECU"));
}

#[test]
fn bmw_simulated_discovery_lists_the_virtual_car() {
    let (ok, out, err) = run(&["bmw", "discover", "--simulate", "--wait-ms", "400"], "");
    assert!(ok, "{err}");
    assert!(out.contains("WBASIMULATED00001"), "{out}");
    assert!(out.contains("0x10"), "{out}");
}

#[test]
fn bmw_simulated_ident_reads_vin_and_flags_unverified_items() {
    let (ok, out, err) = run(&["bmw", "ident", "--simulate"], "");
    assert!(ok, "{err}");
    assert!(out.contains("WBASIMULATED00001"), "{out}");
    assert!(out.contains("F020-00-00-000"), "{out}");
    assert!(out.contains("unverified"), "{out}");
}

#[test]
fn bmw_simulated_did_read_and_missing_did() {
    let (ok, out, _) = run(
        &["bmw", "did", "--simulate", "--target", "0x12", "0xF190"],
        "",
    );
    assert!(ok);
    assert!(out.contains("57 42 41"), "{out}");
    let (ok, out, _) = run(&["bmw", "did", "--simulate", "0xABCD"], "");
    assert!(ok);
    assert!(out.contains("does not have DID"), "{out}");
}

#[test]
fn bmw_simulated_scan_lists_the_ecus_with_code_counts() {
    let (ok, out, err) = run(&["bmw", "scan", "--simulate"], "");
    assert!(ok, "{err}");
    assert!(out.contains("4 ECU(s) answered"), "{out}");
    assert!(
        out.contains("0x12") && out.contains("2 fault code(s)"),
        "{out}"
    );
    assert!(
        out.contains("0x60") && out.contains("1 fault code(s)"),
        "{out}"
    );
    assert!(
        !out.contains("0x29"),
        "an ECU the car lacks must not be listed: {out}"
    );
    let (ok, out, err) = run(&["bmw", "scan", "--simulate", "--all"], "");
    assert!(ok, "{err}");
    assert!(out.contains("4 ECU(s) answered"), "{out}");
}

#[test]
fn bmw_simulated_dtc_read_shows_raw_hex_codes() {
    let (ok, out, err) = run(
        &["bmw", "dtc", "read", "--simulate", "--target", "0x12"],
        "",
    );
    assert!(ok, "{err}");
    assert!(out.contains("480A12") && out.contains("480B01"), "{out}");
    assert!(out.contains("confirmed"), "{out}");
}

#[test]
fn bmw_simulated_clear_needs_yes_snapshots_and_verifies() {
    let backups = temp_file("bmw-clear.jsonl");
    let _ = std::fs::remove_file(&backups);
    let b = backups.to_str().unwrap();
    let args = [
        "bmw",
        "dtc",
        "clear",
        "--simulate",
        "--target",
        "0x12",
        "--backup-file",
        b,
    ];
    // Anything but YES: nothing changes and no snapshot is written.
    let (ok, out, _) = run(&args, "no\n");
    assert!(ok);
    assert!(out.contains("Cancelled"), "{out}");
    assert!(!backups.exists());
    // YES: snapshot first, then the clear, then the re-read.
    let (ok, out, err) = run(&args, "YES\n");
    assert!(ok, "{err}");
    assert!(out.contains("Cleared 2 fault code(s)"), "{out}");
    assert!(out.contains("now reports no fault codes"), "{out}");
    let text = std::fs::read_to_string(&backups).unwrap();
    assert!(
        text.contains("480A12") && text.contains("\"status\":\"verified\""),
        "{text}"
    );
    let _ = std::fs::remove_file(&backups);
}

#[test]
fn bmw_clear_has_no_yes_flag() {
    let (ok, _, err) = run(&["bmw", "dtc", "clear", "--simulate", "--yes"], "");
    assert!(!ok);
    assert!(err.contains("--yes"), "{err}");
}

#[test]
fn bmw_simulated_live_logs_raw_csv() {
    let (ok, out, err) = run(
        &[
            "bmw",
            "live",
            "--simulate",
            "--target",
            "0x12",
            "--did",
            "0x4002,0xBEEF",
            "--interval-ms",
            "10",
            "--samples",
            "2",
        ],
        "",
    );
    assert!(ok, "{err}");
    assert!(out.starts_with("time_ms,did,status,hex"), "{out}");
    assert!(out.contains("0x4002,ok,0BB8"), "{out}");
    assert!(out.contains("0xBEEF,absent,"), "{out}");
    assert_eq!(out.lines().count(), 5, "{out}");
}

fn bmw_block_args<'a>(sub: &'a [&'a str], backups: &'a str) -> Vec<&'a str> {
    let mut v = vec!["bmw", "block"];
    v.extend_from_slice(sub);
    v.extend_from_slice(&["--simulate", "--target", "0x12", "--backup-file", backups]);
    v
}

#[test]
fn bmw_block_read_and_guarded_write_in_the_simulator() {
    let backups = temp_file("bmw-block.jsonl");
    let _ = std::fs::remove_file(&backups);
    let b = backups.to_str().unwrap();
    let (ok, out, err) = run(
        &[
            "bmw",
            "block",
            "read",
            "--simulate",
            "--target",
            "0x12",
            "--did",
            "0x2000",
        ],
        "",
    );
    assert!(ok, "{err}");
    assert!(out.contains("0102030405060708"), "{out}");

    let args = bmw_block_args(
        &["write", "--did", "0x2000", "--hex", "010203AA05060708"],
        b,
    );
    // Not YES: nothing written, no backup.
    let (ok, out, _) = run(&args, "no\n");
    assert!(ok);
    assert!(out.contains("Cancelled"), "{out}");
    assert!(out.contains("offset(s) [3]"), "{out}");
    assert!(!backups.exists());
    // YES: backed up, written, verified.
    let (ok, out, err) = run(&args, "YES\n");
    assert!(ok, "{err}");
    assert!(out.contains("verified by reading it back"), "{out}");
    let text = std::fs::read_to_string(&backups).unwrap();
    assert!(text.contains("\"old_hex\":\"0102030405060708\""), "{text}");
    // The backup list shows it with the undo hint.
    let (ok, out, _) = run(&["backup", "list", "--backup-file", b], "");
    assert!(ok);
    assert!(
        out.contains("0x2000") && out.contains("bmw block restore"),
        "{out}"
    );
    let _ = std::fs::remove_file(&backups);
}

#[test]
fn bmw_block_write_refuses_identity_data_wrong_length_and_has_no_yes_flag() {
    let backups = temp_file("bmw-block-refused.jsonl");
    let _ = std::fs::remove_file(&backups);
    let b = backups.to_str().unwrap();
    let (ok, _, err) = run(
        &bmw_block_args(&["write", "--did", "0xF190", "--hex", "00"], b),
        "YES\n",
    );
    assert!(!ok);
    assert!(err.contains("identification"), "{err}");
    let (ok, _, err) = run(
        &bmw_block_args(&["write", "--did", "0x2000", "--hex", "0102"], b),
        "YES\n",
    );
    assert!(!ok);
    assert!(err.contains("length must not change"), "{err}");
    let (ok, _, err) = run(
        &bmw_block_args(&["write", "--did", "0x2000", "--hex", "zz"], b),
        "YES\n",
    );
    assert!(!ok && err.contains("hex"), "{err}");
    let (ok, _, err) = run(
        &bmw_block_args(&["write", "--did", "0x2000", "--hex", "00", "--yes"], b),
        "",
    );
    assert!(!ok && err.contains("--yes"), "{err}");
    assert!(!backups.exists());
}

#[test]
fn serve_brand_bmw_serves_the_simulated_bmw_read_only() {
    let srv = start_server(&["--brand", "bmw", "--simulate"]);
    let host = format!("127.0.0.1:{}", srv.port);
    let (status, _, body) = http(srv.port, "GET", "/api/info", &host, Some(&srv.token));
    assert_eq!(status, 200);
    assert!(
        body.contains("\"simulated\":true") && body.contains("BMW"),
        "{body}"
    );
    let (status, _, body) = http(srv.port, "GET", "/api/ident", &host, Some(&srv.token));
    assert_eq!(status, 200);
    assert!(body.contains("WBASIMULATED00001"), "{body}");
    let (status, _, _) = http(srv.port, "POST", "/api/dtc", &host, Some(&srv.token));
    assert_ne!(status, 200, "serve must stay read-only");
}

#[test]
fn bmw_serve_command_works_directly_and_brand_flag_does_not_change_vag() {
    // `--brand vag` leaves a VAG command working as before.
    let (ok, out, err) = run(&["--brand", "vag", "scan", "--simulate"], "");
    assert!(ok, "{err}");
    assert!(!out.is_empty());
    let (ok, _, err) = run(&["--brand", "bmw", "scan", "--simulate"], "");
    assert!(!ok);
    assert!(err.contains("opendiag bmw scan"), "{err}");
}

fn coding_example() -> String {
    format!(
        "{}/../../docs/examples/bmw-coding-synthetic.json",
        env!("CARGO_MANIFEST_DIR")
    )
}

#[test]
fn bmw_coding_show_names_the_fields() {
    let f = coding_example();
    let (ok, out, err) = run(
        &[
            "bmw",
            "coding",
            "show",
            "--simulate",
            "--target",
            "0x12",
            "--did",
            "0x2000",
            "--coding-file",
            &f,
        ],
        "",
    );
    assert!(ok, "{err}");
    assert!(
        err.contains("Synthetic example"),
        "provenance must be shown: {err}"
    );
    assert!(out.contains("demo_mode") && out.contains("eco"), "{out}");
    assert!(
        out.contains("demo_limit") && out.contains("no label"),
        "{out}"
    );
}

#[test]
fn bmw_coding_set_goes_through_the_guarded_flow() {
    let f = coding_example();
    let backups = temp_file("bmw-coding.jsonl");
    let _ = std::fs::remove_file(&backups);
    let b = backups.to_str().unwrap();
    let args = [
        "bmw",
        "coding",
        "set",
        "--simulate",
        "--target",
        "0x12",
        "--did",
        "0x2000",
        "--coding-file",
        &f,
        "--field",
        "demo_mode",
        "--value",
        "sport",
        "--backup-file",
        b,
    ];
    let (ok, out, _) = run(&args, "no\n");
    assert!(ok);
    assert!(out.contains("Field demo_mode: eco -> sport"), "{out}");
    assert!(out.contains("Cancelled") && !backups.exists(), "{out}");
    let (ok, out, err) = run(&args, "YES\n");
    assert!(ok, "{err}");
    assert!(out.contains("verified by reading it back"), "{out}");
    let text = std::fs::read_to_string(&backups).unwrap();
    assert!(
        text.contains("\"old_hex\":\"0102030405060708\"")
            && text.contains("\"new_hex\":\"0202030405060708\""),
        "{text}"
    );
    let _ = std::fs::remove_file(&backups);
}

#[test]
fn bmw_coding_refuses_unknown_values_fields_and_blocks_without_writing() {
    let f = coding_example();
    let backups = temp_file("bmw-coding-bad.jsonl");
    let _ = std::fs::remove_file(&backups);
    let b = backups.to_str().unwrap();
    let base = |field: &'static str, value: &'static str, did: &'static str| {
        run(
            &[
                "bmw",
                "coding",
                "set",
                "--simulate",
                "--target",
                "0x12",
                "--did",
                did,
                "--coding-file",
                &f,
                "--field",
                field,
                "--value",
                value,
                "--backup-file",
                b,
            ],
            "YES\n",
        )
    };
    assert!(!base("demo_mode", "turbo", "0x2000").0);
    assert!(!base("demo_mode", "3", "0x2000").0);
    assert!(!base("nope", "1", "0x2000").0);
    let (ok, _, err) = base("demo_mode", "eco", "0x2001");
    assert!(!ok && err.contains("no definition"), "{err}");
    assert!(!backups.exists(), "nothing may be backed up or written");
}

// ---- replay: a recorded session standing in for the vehicle (no hardware) ----

fn trace_path(name: &str) -> String {
    std::env::temp_dir()
        .join(format!(
            "opendiag-replay-{}-{name}.trace",
            std::process::id()
        ))
        .to_string_lossy()
        .into_owned()
}

/// Record `cmd` against the simulator, then run it again from the recording; the output must match.
fn assert_replay_matches(cmd: &[&str], sim_flag: &str, replay_flags: &[&str], name: &str) {
    let trace = trace_path(name);
    let mut rec: Vec<&str> = cmd.to_vec();
    rec.extend([sim_flag, "--trace", &trace]);
    let (ok, live, err) = run(&rec, "");
    assert!(ok, "record failed: {err}");

    let mut rep: Vec<&str> = cmd.to_vec();
    rep.extend(["--replay", &trace]);
    rep.extend(replay_flags);
    let (ok, replayed, err) = run(&rep, "");
    let _ = std::fs::remove_file(&trace);
    assert!(ok, "replay failed: {err}");
    assert!(err.contains("[replay]"), "{err}");
    // A log's time_ms column is the real clock, which differs run to run on a busy machine.
    assert_eq!(without_time_column(&live), without_time_column(&replayed));
}

fn without_time_column(out: &str) -> String {
    if !out.starts_with("time_ms,") {
        return out.to_string();
    }
    out.lines()
        .map(|l| l.split_once(',').map_or(l, |(_, rest)| rest))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn replay_reproduces_scan_and_dtc_read_for_every_protocol() {
    assert_replay_matches(&["scan"], "--simulate", &[], "uds-scan");
    assert_replay_matches(&["dtc", "read"], "--simulate", &[], "uds-dtc");
    assert_replay_matches(&["scan"], "--simulate-kw1281", &["--slow-init"], "kw-scan");
    assert_replay_matches(
        &["dtc", "read"],
        "--simulate-kw1281",
        &["--slow-init"],
        "kw-dtc",
    );
    assert_replay_matches(&["scan"], "--simulate-kwp", &["--fast-init"], "kwp-scan");
    assert_replay_matches(
        &["dtc", "read"],
        "--simulate-kwp",
        &["--fast-init"],
        "kwp-dtc",
    );
}

#[test]
fn replay_reproduces_a_live_data_log() {
    assert_replay_matches(
        &[
            "log",
            "--groups",
            "1",
            "--samples",
            "2",
            "--interval-ms",
            "10",
        ],
        "--simulate-kw1281",
        &["--slow-init"],
        "kw-log",
    );
}

#[test]
fn replay_can_be_asked_the_same_thing_twice() {
    let trace = trace_path("twice");
    let (ok, _, err) = run(&["dtc", "read", "--simulate-kw1281", "--trace", &trace], "");
    assert!(ok, "{err}");
    for _ in 0..2 {
        let (ok, out, err) = run(&["dtc", "read", "--replay", &trace, "--slow-init"], "");
        assert!(ok, "{err}");
        assert!(out.contains("00525"), "{out}");
    }
    let _ = std::fs::remove_file(&trace);
}

#[test]
fn replay_names_a_request_that_was_never_recorded() {
    let trace = trace_path("missing");
    let (ok, _, err) = run(&["scan", "--simulate", "--trace", &trace], "");
    assert!(ok, "{err}");
    let (ok, _, err) = run(&["dtc", "read", "--replay", &trace], "");
    let _ = std::fs::remove_file(&trace);
    assert!(!ok);
    assert!(err.contains("not in recording"), "{err}");
    assert!(err.contains("TX can 7E0#"), "{err}");
}

#[test]
fn replay_refuses_every_write_before_anything_is_planned() {
    let trace = trace_path("writes");
    let (ok, _, err) = run(&["dtc", "read", "--simulate-kw1281", "--trace", &trace], "");
    assert!(ok, "{err}");
    let backup = trace_path("writes-backup");
    let writes: [&[&str]; 2] = [
        &["dtc", "clear", "--backup-file", &backup],
        &[
            "adaptation",
            "set",
            "--channel",
            "4",
            "--value",
            "1",
            "--workshop-code",
            "1",
            "--backup-file",
            &backup,
        ],
    ];
    for w in writes {
        let mut args: Vec<&str> = w.to_vec();
        args.extend(["--replay", &trace, "--slow-init"]);
        let (ok, out, err) = run(&args, "y\ny\n");
        assert!(!ok, "{args:?} must fail, got {out}");
        assert!(err.contains("read-only"), "{args:?}: {err}");
    }
    assert!(
        !std::path::Path::new(&backup).exists(),
        "a refused write must not even take a backup"
    );
    let _ = std::fs::remove_file(&trace);
}

#[test]
fn replay_cannot_be_combined_with_a_live_connection() {
    let (ok, _, err) = run(&["scan", "--replay", "x.trace", "--simulate"], "");
    assert!(!ok);
    assert!(err.contains("cannot be used with"), "{err}");
}

// ---- serve --replay: the dashboard fed from a recording ----

fn json_get(srv: &Server, path: &str) -> serde_json::Value {
    let host = format!("127.0.0.1:{}", srv.port);
    let (status, _, body) = http(srv.port, "GET", path, &host, Some(&srv.token));
    assert_eq!(status, 200, "{path}: {body}");
    serde_json::from_str(&body).expect("json")
}

/// The first cell's A*256+B for each of the logged rounds, from the CSV `log` writes.
fn recorded_first_cells(csv: &str) -> Vec<u32> {
    csv.lines()
        .skip(1)
        .filter_map(|l| {
            let f: Vec<&str> = l.split(',').collect();
            (f[2] == "1").then(|| f[4].parse::<u32>().unwrap() * 256 + f[5].parse::<u32>().unwrap())
        })
        .collect()
}

#[test]
fn serve_replay_labels_itself_and_steps_through_the_recorded_readings() {
    let trace = trace_path("serve-live");
    let (ok, csv, err) = run(
        &[
            "log",
            "--groups",
            "1",
            "--samples",
            "3",
            "--interval-ms",
            "10",
            "--simulate-kw1281",
            "--trace",
            &trace,
        ],
        "",
    );
    assert!(ok, "{err}");
    let recorded = recorded_first_cells(&csv);
    assert_eq!(recorded.len(), 3);
    assert!(
        recorded[0] != recorded[1],
        "the simulator's values must change"
    );

    let srv = start_server(&["--replay", &trace, "--slow-init"]);
    let info = json_get(&srv, "/api/info");
    assert_eq!(info["replay"], true);
    assert_eq!(info["simulated"], false);
    assert!(
        info["connection"].as_str().unwrap().starts_with("Replay"),
        "{info}"
    );

    let mut seen = Vec::new();
    for _ in 0..3 {
        let live = json_get(&srv, "/api/live?groups=1");
        let c = &live["groups"][0]["cells"][0];
        seen.push(c["a"].as_u64().unwrap() as u32 * 256 + c["b"].as_u64().unwrap() as u32);
    }
    let _ = std::fs::remove_file(&trace);
    assert_eq!(
        seen, recorded,
        "replay must play the readings back in recorded order"
    );
}

#[test]
fn serve_replay_stays_alive_while_idle_and_exposes_no_write() {
    let trace = trace_path("serve-idle");
    let (ok, _, err) = run(&["dtc", "read", "--simulate-kw1281", "--trace", &trace], "");
    assert!(ok, "{err}");
    let srv = start_server(&["--replay", &trace, "--slow-init"]);
    // Longer than the server's 2 s idle keep-alive interval.
    std::thread::sleep(std::time::Duration::from_millis(2600));
    let dtcs = json_get(&srv, "/api/dtcs");
    assert!(!dtcs.as_array().unwrap().is_empty(), "{dtcs}");
    let host = format!("127.0.0.1:{}", srv.port);
    let (status, _, _) = http(srv.port, "POST", "/api/dtcs", &host, Some(&srv.token));
    let _ = std::fs::remove_file(&trace);
    assert_eq!(status, 405);
}

#[test]
fn a_replayed_report_says_so() {
    let trace = trace_path("report");
    let (ok, _, err) = run(&["report", "--simulate-kw1281", "--trace", &trace], "");
    assert!(ok, "{err}");
    let (ok, html, err) = run(&["report", "--replay", &trace, "--slow-init"], "");
    let _ = std::fs::remove_file(&trace);
    assert!(ok, "{err}");
    assert!(html.contains("REPLAYED DATA"), "{html}");
    assert!(!html.contains("SIMULATED DATA"), "{html}");
}

// ---- log --ids: raw bytes by identifier over KWP2000 and UDS ----

#[test]
fn log_ids_writes_raw_hex_for_kwp2000_and_uds() {
    let (ok, out, err) = run(
        &[
            "log",
            "--simulate-kwp",
            "--ids",
            "0x01,0x02",
            "--samples",
            "2",
            "--interval-ms",
            "10",
        ],
        "",
    );
    assert!(ok, "{err}");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "time_ms,id,length,hex");
    assert_eq!(lines.len(), 5, "{out}");
    assert!(lines[1].starts_with("0,0x0001,4,1020300"), "{out}");

    let (ok, out, err) = run(
        &["log", "--simulate", "--ids", "0xF190", "--samples", "1"],
        "",
    );
    assert!(ok, "{err}");
    assert!(out.contains("0,0xF190,17,44454D4F"), "{out}");
}

#[test]
fn log_ids_is_refused_for_kw1281_and_needs_groups_or_ids_but_not_both() {
    let (ok, _, err) = run(&["log", "--simulate-kw1281", "--ids", "0x01"], "");
    assert!(!ok && err.contains("KW1281 modules use --groups"), "{err}");
    let (ok, _, err) = run(&["log", "--simulate-kwp"], "");
    assert!(!ok && err.contains("--groups"), "{err}");
    let (ok, _, err) = run(
        &["log", "--simulate-kwp", "--groups", "1", "--ids", "1"],
        "",
    );
    assert!(!ok && err.contains("cannot be used with"), "{err}");
    let (ok, _, err) = run(&["log", "--simulate-kwp", "--ids", "0xZZ"], "");
    assert!(!ok && err.contains("not a hex identifier"), "{err}");
}

#[test]
fn log_ids_can_be_replayed_from_a_recording() {
    assert_replay_matches(
        &[
            "log",
            "--ids",
            "0x01,0x02",
            "--samples",
            "3",
            "--interval-ms",
            "10",
        ],
        "--simulate-kwp",
        &["--fast-init"],
        "kwp-raw-log",
    );
    assert_replay_matches(
        &[
            "log",
            "--ids",
            "0xF190",
            "--samples",
            "2",
            "--interval-ms",
            "10",
        ],
        "--simulate",
        &[],
        "uds-raw-log",
    );
}
