//! [`Module`] implementation for older KW1281 controllers.

use opendiag_core::{Result, WriteMode};
use opendiag_proto::clock::Clock;
use opendiag_proto::kw1281::{Fault, Kw1281Client, RawMeasurement};
use opendiag_transport::Transport;

use crate::{AdaptationAccess, DtcReport, FaultAccess, Ident, LiveData, Module, WriteToken};

/// A KW1281 module with an open session. The session is ended when this is dropped.
pub struct Kw1281Module<T: Transport, C: Clock> {
    client: Kw1281Client<T, C>,
    ident: Vec<String>,
    address: u8,
}

impl<T: Transport, C: Clock> Kw1281Module<T, C> {
    /// Run the 5-baud init against `address` and read the identification.
    pub fn connect(mut client: Kw1281Client<T, C>, address: u8) -> Result<Self> {
        let info = client.connect(address)?;
        Ok(Self {
            client,
            ident: info.ident,
            address,
        })
    }

    pub fn client(&self) -> &Kw1281Client<T, C> {
        &self.client
    }

    /// Run `f` with writes unlocked, always restoring the previous mode.
    fn with_unlocked<R>(
        &mut self,
        f: impl FnOnce(&mut Kw1281Client<T, C>) -> Result<R>,
    ) -> Result<R> {
        let previous = self.client.write_mode();
        self.client.set_write_mode(WriteMode::Unlocked);
        let result = f(&mut self.client);
        self.client.set_write_mode(previous);
        result
    }
}

fn report(f: &Fault) -> DtcReport {
    DtcReport {
        code: u32::from(f.code),
        text: f.text(),
        status: f.status,
        // The meaning of KW1281 status bytes is module-specific; show the raw byte only.
        flags: Vec::new(),
    }
}

impl<T: Transport, C: Clock> Module for Kw1281Module<T, C> {
    fn identify(&mut self) -> Result<Ident> {
        let lines: Vec<String> = self.ident.iter().map(|l| l.trim().to_string()).collect();
        Ok(Ident {
            // By convention the first line is the part number.
            part_number: lines.first().cloned(),
            lines,
            ..Ident::default()
        })
    }

    fn read_dtcs(&mut self) -> Result<Vec<DtcReport>> {
        Ok(self.client.read_fault_codes()?.iter().map(report).collect())
    }

    fn faults(&mut self) -> &mut dyn FaultAccess {
        self
    }

    fn keep_alive(&mut self) -> Result<()> {
        self.client.keep_alive()
    }

    fn live(&mut self) -> Option<&mut dyn LiveData> {
        Some(self)
    }

    fn adaptation(&mut self) -> Option<&mut dyn AdaptationAccess> {
        Some(self)
    }
}

impl<T: Transport, C: Clock> FaultAccess for Kw1281Module<T, C> {
    fn protocol(&self) -> &'static str {
        "kw1281"
    }

    fn module_address(&self) -> u8 {
        self.address
    }

    fn describe(&self) -> String {
        format!("KW1281 module {:#04X}", self.address)
    }

    fn read(&mut self) -> Result<Vec<DtcReport>> {
        Module::read_dtcs(self)
    }

    fn clear(&mut self, _token: &WriteToken) -> Result<()> {
        // The faults the module still reports afterwards are re-read by the caller.
        self.with_unlocked(|c| c.clear_fault_codes()).map(|_| ())
    }
}

impl<T: Transport, C: Clock> LiveData for Kw1281Module<T, C> {
    fn read_group(&mut self, group: u8) -> Result<Vec<RawMeasurement>> {
        self.client.read_group(group)
    }
}

impl<T: Transport, C: Clock> AdaptationAccess for Kw1281Module<T, C> {
    fn protocol(&self) -> &'static str {
        "kw1281"
    }

    fn module_address(&self) -> u8 {
        self.address
    }

    fn read(&mut self, channel: u8) -> Result<u16> {
        self.client.adaptation_read(channel)
    }

    fn test(&mut self, _token: &WriteToken, channel: u8, value: u16) -> Result<u16> {
        self.with_unlocked(|c| c.adaptation_test(channel, value))
    }

    fn save(
        &mut self,
        _token: &WriteToken,
        channel: u8,
        value: u16,
        workshop_code: u32,
    ) -> Result<()> {
        self.with_unlocked(|c| c.adaptation_save(channel, value, workshop_code))
    }
}

impl<T: Transport, C: Clock> Drop for Kw1281Module<T, C> {
    fn drop(&mut self) {
        // Best effort: the module times the session out by itself anyway.
        let _ = self.client.end();
    }
}
