//! [`Module`] implementation for older KW1281 controllers.

use opendiag_core::{Result, WriteMode};
use opendiag_proto::clock::Clock;
use opendiag_proto::kw1281::{Fault, Kw1281Client};
use opendiag_transport::Transport;

use crate::{ClearConfirmation, DtcReport, Ident, Module};

/// A KW1281 module with an open session. The session is ended when this is dropped.
pub struct Kw1281Module<T: Transport, C: Clock> {
    client: Kw1281Client<T, C>,
    ident: Vec<String>,
}

impl<T: Transport, C: Clock> Kw1281Module<T, C> {
    /// Run the 5-baud init against `address` and read the identification.
    pub fn connect(mut client: Kw1281Client<T, C>, address: u8) -> Result<Self> {
        let info = client.connect(address)?;
        Ok(Self {
            client,
            ident: info.ident,
        })
    }

    pub fn client(&self) -> &Kw1281Client<T, C> {
        &self.client
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

    fn clear_dtcs(&mut self, _confirmation: &ClearConfirmation) -> Result<Vec<DtcReport>> {
        let before = self.read_dtcs()?;
        let previous = self.client.write_mode();
        self.client.set_write_mode(WriteMode::Unlocked);
        let result = self.client.clear_fault_codes();
        self.client.set_write_mode(previous);
        result?;
        Ok(before)
    }
}

impl<T: Transport, C: Clock> Drop for Kw1281Module<T, C> {
    fn drop(&mut self) {
        // Best effort: the module times the session out by itself anyway.
        let _ = self.client.end();
    }
}
