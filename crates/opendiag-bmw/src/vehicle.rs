//! A BMW car behind its gateway, as an `opendiag_services::Vehicle`.
//!
//! One TCP connection, many ECUs: [`BmwVehicle::ecu`] makes a [`BmwEcu`] per
//! diagnostic address on first use and keeps it, so each ECU keeps its own
//! session state (for example an opened extended session).

use std::collections::BTreeMap;
use std::time::Duration;

use opendiag_core::Result;
use opendiag_proto::clock::Clock;
use opendiag_proto::uds::{UdsClient, UdsConfig};
use opendiag_services::{Brand, EcuEntry, EcuId, IdentLine, Module, Vehicle, VehicleIdent};

use crate::ecu::BmwEcu;
use crate::ecus;
use crate::gateway::{ByteStream, HsfzGateway, HsfzLink};
use crate::ident::read_ident;
use crate::scan::scan;

pub struct BmwVehicle<S: ByteStream, C: Clock> {
    gateway: HsfzGateway<S, C>,
    default_ecu: u8,
    description: String,
    probe_timeout: Duration,
    ecus: BTreeMap<u8, BmwEcu<HsfzLink<S, C>>>,
}

impl<S: ByteStream, C: Clock> BmwVehicle<S, C> {
    /// `default_ecu` is the gateway's diagnostic address (usually 0x10).
    pub fn new(gateway: HsfzGateway<S, C>, default_ecu: u8, description: String) -> Self {
        Self {
            gateway,
            default_ecu,
            description,
            probe_timeout: Duration::from_millis(300),
            ecus: BTreeMap::new(),
        }
    }

    /// How long a scan waits for each address.
    pub fn with_probe_timeout(mut self, t: Duration) -> Self {
        self.probe_timeout = t;
        self
    }

    pub fn gateway(&self) -> &HsfzGateway<S, C> {
        &self.gateway
    }
}

fn hex(b: &[u8]) -> String {
    b.iter()
        .map(|x| format!("{x:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

impl<S: ByteStream + 'static, C: Clock + 'static> Vehicle for BmwVehicle<S, C> {
    fn brand(&self) -> Brand {
        Brand::Bmw
    }

    fn describe(&self) -> String {
        self.description.clone()
    }

    fn identify(&mut self) -> Result<VehicleIdent> {
        let mut uds = UdsClient::new(self.gateway.link(self.default_ecu), UdsConfig::default());
        let id = read_ident(&mut uds)?;
        let mut lines = Vec::new();
        // Raw bytes, and marked unverified: the DIDs are recalled, not confirmed.
        if let Some(t) = id.i_level_text() {
            lines.push(IdentLine {
                label: "I-level".into(),
                value: t,
                verified: false,
            });
        } else if let Some(raw) = &id.i_level_raw {
            lines.push(IdentLine {
                label: "I-level (raw)".into(),
                value: hex(raw),
                verified: false,
            });
        }
        if let Some(raw) = &id.fa_raw {
            lines.push(IdentLine {
                label: "FA (raw, not decoded)".into(),
                value: hex(raw),
                verified: false,
            });
        }
        Ok(VehicleIdent { vin: id.vin, lines })
    }

    fn scan(&mut self) -> Result<Vec<EcuEntry>> {
        Ok(
            scan(&self.gateway, &ecus::known_addresses(), self.probe_timeout)?
                .into_iter()
                .filter(|e| e.present)
                .map(|e| EcuEntry {
                    id: e.address,
                    name: ecus::name(e.address).map(str::to_string),
                    dtc_count: e.dtc_count,
                })
                .collect(),
        )
    }

    fn default_ecu(&self) -> EcuId {
        self.default_ecu
    }

    fn ecu(&mut self, id: EcuId) -> Result<&mut dyn Module> {
        let gateway = &self.gateway;
        Ok(self
            .ecus
            .entry(id)
            .or_insert_with(|| BmwEcu::new(gateway.link(id), id, UdsConfig::default())))
    }

    fn keep_alive(&mut self) -> Result<()> {
        // TesterPresent to the gateway keeps the connection up.
        UdsClient::new(self.gateway.link(self.default_ecu), UdsConfig::default()).tester_present()
    }
}
