//! One BMW ECU as a [`Module`] / [`FaultAccess`], so the existing guarded
//! clear flow (`plan_clear` / `apply_clear`: confirmation bound to the codes,
//! re-read, flushed snapshot, clear, re-read) works for BMW unchanged.
//!
//! DTCs are shown as the raw 3-byte hex code (e.g. `480A12`), not as SAE
//! letters: BMW codes are not SAE J2012 codes, so inventing `P`/`B`/`C`/`U`
//! text would be wrong.
//!
//! Reading uses ReadDTCInformation 0x19 02 with mask 0xFF and clearing uses
//! ClearDiagnosticInformation 0x14 FFFFFF, both generic ISO 14229. Whether an
//! F-series ECU wants a different session first is NOT VERIFIED.

use opendiag_core::{Result, WriteMode};
use opendiag_proto::link::MessageLink;
use opendiag_proto::uds::{UdsClient, UdsConfig};
use opendiag_services::{
    identify, status_flags, BlockAccess, DtcReport, FaultAccess, Ident, Module, WriteToken,
};

/// Raw hex form of a BMW DTC.
pub fn format_bmw_dtc(code: u32) -> String {
    format!("{:06X}", code & 0xFF_FFFF)
}

pub struct BmwEcu<L: MessageLink> {
    uds: UdsClient<L>,
    address: u8,
    /// Open the extended diagnostic session (0x10 03) before the first block access.
    extended_session: bool,
    session_open: bool,
}

impl<L: MessageLink> BmwEcu<L> {
    pub fn new(link: L, address: u8, cfg: UdsConfig) -> Self {
        Self {
            uds: UdsClient::new(link, cfg),
            address,
            extended_session: false,
            session_open: false,
        }
    }

    /// Open the extended diagnostic session (0x10 03) before reading or writing
    /// blocks. Whether an F-series ECU needs this is NOT VERIFIED, so it is opt-in.
    /// Never the programming session: that is refused at the link.
    pub fn with_extended_session(mut self) -> Self {
        self.extended_session = true;
        self
    }

    fn ensure_session(&mut self) -> Result<()> {
        if self.extended_session && !self.session_open {
            self.uds.session_control(0x03)?;
            self.session_open = true;
        }
        Ok(())
    }

    pub fn address(&self) -> u8 {
        self.address
    }

    /// The underlying UDS client, for DID reads and other read-only requests.
    pub fn uds(&mut self) -> &mut UdsClient<L> {
        &mut self.uds
    }
}

impl<L: MessageLink> FaultAccess for BmwEcu<L> {
    fn protocol(&self) -> &'static str {
        "hsfz-uds"
    }

    fn module_address(&self) -> u8 {
        self.address
    }

    fn describe(&self) -> String {
        match crate::ecus::name(self.address) {
            Some(n) => format!("BMW ECU {:#04X} ({n}) via HSFZ", self.address),
            None => format!("BMW ECU {:#04X} via HSFZ", self.address),
        }
    }

    fn read(&mut self) -> Result<Vec<DtcReport>> {
        Ok(self
            .uds
            .read_dtcs_by_status_mask(0xFF)?
            .into_iter()
            .map(|d| DtcReport {
                code: d.code,
                text: format_bmw_dtc(d.code),
                status: d.status,
                flags: status_flags(d.status),
            })
            .collect())
    }

    fn clear(&mut self, _token: &WriteToken) -> Result<()> {
        // Unlock only for this one request, and put the mode back whatever happens.
        let previous = self.uds.write_mode();
        self.uds.set_write_mode(WriteMode::Unlocked);
        let result = self.uds.clear_dtcs(0xFF_FFFF);
        self.uds.set_write_mode(previous);
        result
    }
}

impl<L: MessageLink> BlockAccess for BmwEcu<L> {
    fn protocol(&self) -> &'static str {
        "hsfz-uds"
    }

    fn module_address(&self) -> u8 {
        self.address
    }

    fn describe(&self) -> String {
        FaultAccess::describe(self)
    }

    fn read_block(&mut self, id: u16) -> Result<Vec<u8>> {
        self.ensure_session()?;
        self.uds.read_data_by_identifier(id)
    }

    fn writable(&self, id: u16) -> std::result::Result<(), String> {
        // 0xF100-0xF1FF are the ISO 14229 identification identifiers (VIN, part
        // numbers, ...). Identity data is never written.
        if (0xF100..=0xF1FF).contains(&id) {
            return Err(format!(
                "{id:#06X} is an identification identifier (VIN, part numbers, ...); OpenDiag never writes those"
            ));
        }
        Ok(())
    }

    fn write_block(&mut self, _token: &WriteToken, id: u16, data: &[u8]) -> Result<()> {
        self.ensure_session()?;
        let previous = self.uds.write_mode();
        self.uds.set_write_mode(WriteMode::Unlocked);
        let result = self.uds.write_data_by_identifier(id, data);
        self.uds.set_write_mode(previous);
        result
    }
}

impl<L: MessageLink> Module for BmwEcu<L> {
    fn identify(&mut self) -> Result<Ident> {
        identify(&mut self.uds)
    }

    fn read_dtcs(&mut self) -> Result<Vec<DtcReport>> {
        self.read()
    }

    fn faults(&mut self) -> &mut dyn FaultAccess {
        self
    }

    fn keep_alive(&mut self) -> Result<()> {
        self.uds.tester_present()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dtcs_are_six_hex_digits() {
        assert_eq!(format_bmw_dtc(0x480A12), "480A12");
        assert_eq!(format_bmw_dtc(0x000012), "000012");
    }
}
