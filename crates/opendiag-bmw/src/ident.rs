//! Read-only vehicle identification: VIN, vehicle order (FA) and I-level.
//!
//! - VIN: DID `0xF190`, the standard ISO 14229 identifier. VERIFIED.
//! - I-level and FA: BMW-specific DIDs [`DID_I_LEVEL`] and [`DID_FA`].
//!   **NOT VERIFIED.** They are recalled from community tooling and could not
//!   be confirmed from a public source. A wrong DID is harmless here (the ECU
//!   answers "request out of range" and the item is reported as unavailable),
//!   and `opendiag bmw did` lets you try others. Their contents are returned
//!   as raw bytes; no decoding is attempted until a real capture exists.

use opendiag_core::{Error, Result};
use opendiag_proto::link::MessageLink;
use opendiag_proto::uds::UdsClient;

pub const DID_VIN: u16 = 0xF190;
/// UNVERIFIED, see module docs.
pub const DID_I_LEVEL: u16 = 0x100B;
/// UNVERIFIED, see module docs.
pub const DID_FA: u16 = 0x3F06;

/// What the gateway reported about the vehicle.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BmwIdent {
    pub vin: Option<String>,
    /// Raw bytes of the I-level DID, if the ECU has it.
    pub i_level_raw: Option<Vec<u8>>,
    /// Raw bytes of the FA (vehicle order) DID, if the ECU has it.
    pub fa_raw: Option<Vec<u8>>,
}

impl BmwIdent {
    /// I-level as text when the bytes are printable ASCII, else `None`.
    pub fn i_level_text(&self) -> Option<String> {
        let raw = self.i_level_raw.as_ref()?;
        let t: Vec<u8> = raw.iter().copied().filter(|b| *b != 0).collect();
        (!t.is_empty() && t.iter().all(|b| (0x20..0x7F).contains(b)))
            .then(|| String::from_utf8_lossy(&t).into_owned())
    }
}

/// Read one DID; `None` if the ECU says the identifier does not exist.
pub fn read_did<L: MessageLink>(uds: &mut UdsClient<L>, did: u16) -> Result<Option<Vec<u8>>> {
    match uds.read_data_by_identifier(did) {
        Ok(d) => Ok(Some(d)),
        Err(Error::NegativeResponse { nrc: 0x31, .. }) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Read VIN, I-level and FA. Reads only.
pub fn read_ident<L: MessageLink>(uds: &mut UdsClient<L>) -> Result<BmwIdent> {
    let vin = read_did(uds, DID_VIN)?.map(|d| {
        String::from_utf8_lossy(&d)
            .trim_matches(|c: char| c == '\0' || c.is_whitespace())
            .to_string()
    });
    Ok(BmwIdent {
        vin,
        i_level_raw: read_did(uds, DID_I_LEVEL)?,
        fa_raw: read_did(uds, DID_FA)?,
    })
}
