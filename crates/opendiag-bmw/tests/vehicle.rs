//! `BmwVehicle` through the brand-neutral `Vehicle` trait, over the in-memory loopback.

use opendiag_bmw::mock::{LoopbackStream, VirtualVehicle};
use opendiag_bmw::{BmwVehicle, HsfzGateway};
use opendiag_proto::clock::FakeClock;
use opendiag_services::{
    apply_clear, plan_clear, BackupStore, Brand, ClearConfirmation, Module, Vehicle, VehicleModule,
};

fn car() -> BmwVehicle<LoopbackStream, FakeClock> {
    let gw = HsfzGateway::new(
        LoopbackStream::new(VirtualVehicle::demo()),
        FakeClock::new(),
        0xF4,
    );
    BmwVehicle::new(gw, 0x10, "virtual BMW".into())
}

#[test]
fn identify_labels_the_unverified_items() {
    let mut v = car();
    assert_eq!(v.brand(), Brand::Bmw);
    let id = v.identify().unwrap();
    assert_eq!(id.vin.as_deref(), Some("WBASIMULATED00001"));
    let i_level = id.lines.iter().find(|l| l.label == "I-level").unwrap();
    assert_eq!(i_level.value, "F020-00-00-000");
    assert!(
        id.lines.iter().all(|l| !l.verified),
        "I-level and FA must be marked unverified: {:?}",
        id.lines
    );
}

#[test]
fn scan_lists_only_ecus_that_answer_with_their_code_counts() {
    let found = car().scan().unwrap();
    let ids: Vec<u8> = found.iter().map(|e| e.id).collect();
    assert_eq!(ids, vec![0x10, 0x12, 0x18, 0x60]);
    let dme = found.iter().find(|e| e.id == 0x12).unwrap();
    assert_eq!((dme.name.as_deref(), dme.dtc_count), (Some("DME"), Some(2)));
}

#[test]
fn each_ecu_is_a_module_and_the_vehicle_stays_alive() {
    let mut v = car();
    assert_eq!(v.default_ecu(), 0x10);
    assert_eq!(v.ecu(0x12).unwrap().read_dtcs().unwrap().len(), 2);
    assert_eq!(v.ecu(0x60).unwrap().read_dtcs().unwrap()[0].text, "930001");
    v.keep_alive().unwrap();
}

#[test]
fn the_guarded_clear_works_through_a_vehicle_module() {
    let store = BackupStore::new(
        std::env::temp_dir().join(format!("opendiag-vehicle-{}.jsonl", std::process::id())),
    );
    let _ = std::fs::remove_file(store.path());
    let mut m = VehicleModule::new(Box::new(car()), 0x12).unwrap();
    let shown = plan_clear(m.faults()).unwrap();
    assert_eq!(shown.len(), 2);
    let out = apply_clear(
        m.faults(),
        &store,
        &shown,
        &ClearConfirmation::for_codes(&shown),
    )
    .unwrap();
    assert!(out.remaining.is_empty());
    assert!(std::fs::read_to_string(store.path())
        .unwrap()
        .contains("\"kind\":\"dtc_clear\""));
    let _ = std::fs::remove_file(store.path());
}
