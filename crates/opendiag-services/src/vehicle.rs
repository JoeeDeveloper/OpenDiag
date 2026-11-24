//! A connected vehicle, whatever the brand: what the VAG and BMW backends have
//! in common once connected.
//!
//! Extracted from the two working implementations, not designed ahead of them.
//! What turned out to be shared: identify, list ECUs, reach one ECU as a
//! [`Module`] (so fault codes, the guarded clear and every front end keep
//! working unchanged), and keep the session alive. What did not: *connecting*
//! (adapters, serial ports, TCP and discovery have nothing in common), so there
//! is deliberately no `connect` on the trait: each backend is constructed
//! already connected. Optional abilities (live data, adaptation, block access)
//! stay optional traits reached from the ECU, never promised by the vehicle.

use opendiag_core::{Error, Result};

use crate::{
    AdaptationAccess, DtcReport, FaultAccess, Ident, LiveData, Module, RawData, WriteToken,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Brand {
    Vag,
    Bmw,
}

/// Brand-specific address of an ECU. For VAG this is 0 (one module at a time);
/// for BMW it is the diagnostic address.
pub type EcuId = u8;

/// One ECU that answered a scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EcuEntry {
    pub id: EcuId,
    pub name: Option<String>,
    /// Stored fault codes, when they could be counted.
    pub dtc_count: Option<usize>,
}

/// One labelled line of identification. `verified` is false for items whose
/// meaning or source is not confirmed (for example BMW's I-level and FA), so a
/// front end can show that instead of presenting a guess as fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentLine {
    pub label: String,
    pub value: String,
    pub verified: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VehicleIdent {
    pub vin: Option<String>,
    pub lines: Vec<IdentLine>,
}

pub trait Vehicle {
    fn brand(&self) -> Brand;
    /// How it is connected, for display.
    fn describe(&self) -> String;
    fn identify(&mut self) -> Result<VehicleIdent>;
    /// ECUs that answer. VAG connections are one module, so this is one entry.
    fn scan(&mut self) -> Result<Vec<EcuEntry>>;
    /// The ECU to use when none is named.
    fn default_ecu(&self) -> EcuId;
    /// One ECU as a [`Module`].
    fn ecu(&mut self, id: EcuId) -> Result<&mut dyn Module>;
    fn keep_alive(&mut self) -> Result<()>;
}

/// A [`Vehicle`] that is a single already-connected module: how every VAG
/// connection works today.
pub struct SingleModule {
    module: Box<dyn Module>,
    description: String,
}

impl SingleModule {
    pub fn new(module: Box<dyn Module>, description: String) -> Self {
        Self {
            module,
            description,
        }
    }
}

impl Vehicle for SingleModule {
    fn brand(&self) -> Brand {
        Brand::Vag
    }

    fn describe(&self) -> String {
        self.description.clone()
    }

    fn identify(&mut self) -> Result<VehicleIdent> {
        let id: Ident = self.module.identify()?;
        let mut lines = Vec::new();
        let mut add = |label: &str, v: Option<String>| {
            if let Some(value) = v {
                lines.push(IdentLine {
                    label: label.into(),
                    value,
                    verified: true,
                });
            }
        };
        add("Part number", id.part_number);
        add("Software version", id.software_version);
        for l in id.lines {
            add("Ident", Some(l));
        }
        Ok(VehicleIdent { vin: id.vin, lines })
    }

    fn scan(&mut self) -> Result<Vec<EcuEntry>> {
        Ok(vec![EcuEntry {
            id: 0,
            name: Some(self.module.faults().describe()),
            dtc_count: self.module.read_dtcs().ok().map(|v| v.len()),
        }])
    }

    fn default_ecu(&self) -> EcuId {
        0
    }

    fn ecu(&mut self, id: EcuId) -> Result<&mut dyn Module> {
        if id != 0 {
            return Err(Error::Protocol(format!(
                "this connection has one module (id 0), not {id}"
            )));
        }
        Ok(self.module.as_mut())
    }

    fn keep_alive(&mut self) -> Result<()> {
        self.module.keep_alive()
    }
}

/// Stands in when the selected ECU cannot be reached: every call fails, none panics.
struct Unavailable(String);

impl FaultAccess for Unavailable {
    fn protocol(&self) -> &'static str {
        "unavailable"
    }
    fn module_address(&self) -> u8 {
        0
    }
    fn describe(&self) -> String {
        self.0.clone()
    }
    fn read(&mut self) -> Result<Vec<DtcReport>> {
        Err(Error::Protocol(self.0.clone()))
    }
    fn clear(&mut self, _token: &WriteToken) -> Result<()> {
        Err(Error::Protocol(self.0.clone()))
    }
}

/// One ECU of a vehicle, as a [`Module`]. Lets every front end that works on a
/// `Module` (the web server, the CLI) work on any brand's vehicle unchanged.
pub struct VehicleModule {
    vehicle: Box<dyn Vehicle>,
    id: EcuId,
    unavailable: Unavailable,
}

impl VehicleModule {
    /// Fails now, not later, if the ECU cannot be reached.
    pub fn new(mut vehicle: Box<dyn Vehicle>, id: EcuId) -> Result<Self> {
        vehicle.ecu(id)?;
        Ok(Self {
            vehicle,
            id,
            unavailable: Unavailable(format!("ECU {id:#04X} is not available")),
        })
    }
}

impl Module for VehicleModule {
    fn identify(&mut self) -> Result<Ident> {
        self.vehicle.ecu(self.id)?.identify()
    }

    fn read_dtcs(&mut self) -> Result<Vec<DtcReport>> {
        self.vehicle.ecu(self.id)?.read_dtcs()
    }

    fn faults(&mut self) -> &mut dyn FaultAccess {
        match self.vehicle.ecu(self.id) {
            Ok(m) => m.faults(),
            Err(_) => &mut self.unavailable,
        }
    }

    fn keep_alive(&mut self) -> Result<()> {
        self.vehicle.keep_alive()
    }

    fn live(&mut self) -> Option<&mut dyn LiveData> {
        self.vehicle.ecu(self.id).ok()?.live()
    }

    fn raw_data(&mut self) -> Option<&mut dyn RawData> {
        self.vehicle.ecu(self.id).ok()?.raw_data()
    }

    fn adaptation(&mut self) -> Option<&mut dyn AdaptationAccess> {
        self.vehicle.ecu(self.id).ok()?.adaptation()
    }
}
