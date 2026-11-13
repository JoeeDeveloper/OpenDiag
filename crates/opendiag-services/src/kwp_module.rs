//! [`Module`] implementation for KWP2000 controllers, over any carrier that
//! implements [`KwpService`] (K-line with fast init, or TP2.0 on CAN).
//!
//! Everything here follows the ISO 14230-3 structure, not captures of VAG
//! modules, so VAG-specific details are unverified: the diagnostic session mode,
//! the identification option, and how fault codes are shown (as 5-digit decimal
//! numbers with the raw status byte, like the older KW1281 modules).

use opendiag_core::{Error, Result, WriteMode};
use opendiag_proto::kwp::KwpService;

use crate::{DtcReport, FaultAccess, Ident, Module, WriteToken};

/// Diagnostic session mode requested after connecting. It is the mode that
/// appears in the public TP2.0 capture; not verified on K-line modules.
const DIAGNOSTIC_SESSION: u8 = 0x89;
/// ReadECUIdentification option used for the identification text.
const IDENT_OPTION: u8 = 0x9B;
/// All fault-code groups.
const ALL_GROUPS: u16 = 0xFF00;

/// A KWP2000 module with an open session. The session is ended on drop.
pub struct KwpModule<S: KwpService> {
    service: S,
    address: u8,
    protocol: &'static str,
    label: &'static str,
}

impl<S: KwpService> KwpModule<S> {
    /// Open a diagnostic session on a carrier that is already connected
    /// (K-line: after StartCommunication; TP2.0: after the channel is open).
    ///
    /// `protocol` names the carrier in backup records (`kwp2000` for K-line,
    /// `kwp2000-tp20` for CAN). A module that refuses the session request is
    /// still used: many answer the later services anyway.
    pub fn start(mut service: S, address: u8, protocol: &'static str) -> Result<Self> {
        match service.start_diagnostic_session(DIAGNOSTIC_SESSION) {
            Ok(()) | Err(Error::NegativeResponse { .. }) => {}
            Err(e) => return Err(e),
        }
        let label = if protocol == "kwp2000-tp20" {
            "KWP2000/TP2.0 module"
        } else {
            "KWP2000 module"
        };
        Ok(Self {
            service,
            address,
            protocol,
            label,
        })
    }

    pub fn service(&self) -> &S {
        &self.service
    }

    /// Run `f` with writes unlocked, always restoring the previous mode.
    fn with_unlocked<R>(&mut self, f: impl FnOnce(&mut S) -> Result<R>) -> Result<R> {
        let previous = self.service.write_mode();
        self.service.set_write_mode(WriteMode::Unlocked);
        let result = f(&mut self.service);
        self.service.set_write_mode(previous);
        result
    }
}

/// Printable ASCII is kept; anything else becomes `.` so raw identification
/// data is readable without being interpreted.
fn printable(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| {
            if (0x20..0x7F).contains(&b) {
                char::from(b)
            } else {
                '.'
            }
        })
        .collect()
}

impl<S: KwpService> Module for KwpModule<S> {
    fn identify(&mut self) -> Result<Ident> {
        let raw = match self.service.read_ecu_identification(IDENT_OPTION) {
            Ok(d) => d,
            // The module simply has no such identification block.
            Err(Error::NegativeResponse { .. }) => Vec::new(),
            Err(e) => return Err(e),
        };
        let text = printable(&raw);
        let text = text.trim_matches(|c: char| c == '.' || c.is_whitespace());
        // Raw, unparsed identification: the field layout is VAG-specific and not
        // known here, so no part number is claimed.
        Ok(Ident {
            lines: if text.is_empty() {
                Vec::new()
            } else {
                vec![text.to_string()]
            },
            ..Ident::default()
        })
    }

    fn read_dtcs(&mut self) -> Result<Vec<DtcReport>> {
        Ok(self
            .service
            .read_dtcs_by_status(0x00, ALL_GROUPS)?
            .into_iter()
            .map(|d| DtcReport {
                code: u32::from(d.code),
                text: format!("{:05}", d.code),
                status: d.status,
                // Status bit meanings are module-specific; show the raw byte only.
                flags: Vec::new(),
            })
            .collect())
    }

    fn faults(&mut self) -> &mut dyn FaultAccess {
        self
    }
}

impl<S: KwpService> FaultAccess for KwpModule<S> {
    fn protocol(&self) -> &'static str {
        self.protocol
    }

    fn module_address(&self) -> u8 {
        self.address
    }

    fn describe(&self) -> String {
        format!("{} {:#04X}", self.label, self.address)
    }

    fn read(&mut self) -> Result<Vec<DtcReport>> {
        Module::read_dtcs(self)
    }

    fn clear(&mut self, _token: &WriteToken) -> Result<()> {
        self.with_unlocked(|s| s.clear_dtcs(ALL_GROUPS))
    }
}

impl<S: KwpService> Drop for KwpModule<S> {
    fn drop(&mut self) {
        // Best effort: the module times the session out by itself anyway.
        self.service.close();
    }
}
