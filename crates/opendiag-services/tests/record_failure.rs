//! When a write has already reached the module and only the bookkeeping fails
//! afterwards, the error must say what happened to the module, not just that a
//! file could not be written.

use std::path::{Path, PathBuf};

use opendiag_proto::clock::FakeClock;
use opendiag_proto::kw1281::sim::Kw1281Sim;
use opendiag_proto::kw1281::{Kw1281Client, Kw1281Config};
use opendiag_services::{
    apply_change, apply_clear, plan_change, plan_clear, BackupStore, ClearConfirmation,
    Kw1281Module, Module, WriteConfirmation,
};

fn temp_store(name: &str) -> BackupStore {
    let path: PathBuf =
        std::env::temp_dir().join(format!("opendiag-rf-{}-{name}.jsonl", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(&path);
    BackupStore::new(path)
}

/// Replace the backup file with a directory, so reading or appending it fails from now on.
fn sabotage(path: &Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::create_dir(path);
}

fn cleanup(store: &BackupStore) {
    let _ = std::fs::remove_dir(store.path());
    let _ = std::fs::remove_file(store.path());
}

fn module(sim: Kw1281Sim) -> Kw1281Module<Kw1281Sim, FakeClock> {
    let client = Kw1281Client::new(sim, FakeClock::new(), Kw1281Config::default());
    Kw1281Module::connect(client, 1).unwrap()
}

#[test]
fn a_cleared_module_with_an_unrecordable_status_says_the_codes_were_cleared() {
    let store = temp_store("clear-ok");
    let p = store.path().to_path_buf();
    let sim = Kw1281Sim::new()
        .ident_line("X")
        .fault(525, 0x20)
        .on_write(move |what| {
            if what == "clear" {
                sabotage(&p); // the snapshot existed when the write arrived; now it breaks
            }
        });
    let mut m = module(sim);
    let shown = plan_clear(m.faults()).unwrap();
    let err = apply_clear(
        m.faults(),
        &store,
        &shown,
        &ClearConfirmation::for_codes(&shown),
    )
    .unwrap_err()
    .to_string();

    assert!(
        err.contains("WERE cleared"),
        "must not hide that the module changed: {err}"
    );
    assert!(
        err.contains("b0001") && err.contains("still says `planned`"),
        "{err}"
    );
    assert!(
        m.client().transport().cleared(),
        "the module really was cleared"
    );
    cleanup(&store);
}

#[test]
fn an_applied_adaptation_with_an_unrecordable_status_says_it_was_applied() {
    let store = temp_store("adapt-ok");
    let p = store.path().to_path_buf();
    let sim = Kw1281Sim::new()
        .ident_line("X")
        .adaptation(4, 100)
        .on_write(move |what| {
            if what == "adaptation save" {
                sabotage(&p);
            }
        });
    let mut m = module(sim);
    let change = plan_change(m.adaptation().unwrap(), 4, 200).unwrap();
    let confirmation = WriteConfirmation::for_change(&change);
    let err = apply_change(m.adaptation().unwrap(), &store, &change, 0, &confirmation)
        .unwrap_err()
        .to_string();

    assert!(err.contains("WAS applied and verified"), "{err}");
    assert!(
        err.contains("reads 200") && err.contains("old value 100"),
        "{err}"
    );
    assert_eq!(m.client().transport().adaptation_value(4), Some(200));
    cleanup(&store);
}

#[test]
fn a_refused_save_still_reports_the_refusal_when_the_record_cannot_be_updated() {
    let store = temp_store("adapt-refused");
    let p = store.path().to_path_buf();
    let sim = Kw1281Sim::new()
        .ident_line("X")
        .adaptation(4, 100)
        .refuse_adaptation_save()
        .on_write(move |what| {
            if what == "adaptation save" {
                sabotage(&p);
            }
        });
    let mut m = module(sim);
    let change = plan_change(m.adaptation().unwrap(), 4, 200).unwrap();
    let confirmation = WriteConfirmation::for_change(&change);
    let err = apply_change(m.adaptation().unwrap(), &store, &change, 0, &confirmation)
        .unwrap_err()
        .to_string();

    assert!(
        err.contains("the save failed"),
        "the real failure comes first: {err}"
    );
    assert!(
        err.contains("In addition, backup record b0001 could not be updated"),
        "and the bookkeeping failure is appended, not substituted: {err}"
    );
    assert_eq!(m.client().transport().adaptation_value(4), Some(100));
    cleanup(&store);
}
