//! The data model. This is the single source of truth: the JSON Schema in
//! `data/schema/` is generated from these types.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Value of the `format` field in every data file.
pub const FORMAT: &str = "opendiag-data/0";

/// One YAML data file: a header, who it came from, and a list of entries of one kind.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct DataFile {
    /// Must be `opendiag-data/0`.
    pub format: String,
    pub provenance: Provenance,
    #[serde(flatten)]
    pub body: Body,
}

/// The entries, selected by the file's `kind`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "kind", content = "entries", rename_all = "snake_case")]
pub enum Body {
    Modules(Vec<ModuleDef>),
    Dtcs(Vec<DtcDef>),
    MeasuringBlocks(Vec<BlockDef>),
}

/// Where the data came from. Required on every file, so provenance can be audited.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    /// Who wrote or captured this data.
    pub author: String,
    pub source: Source,
    /// SPDX id of the data licence: `CC0-1.0`, `CC-BY-4.0` or `GPL-3.0-or-later`.
    pub license: String,
    /// Date of capture (`YYYY-MM-DD`). Required when `source` is `own_vehicle_capture`.
    pub captured_on: Option<String>,
    /// True only once the data has been checked against a real vehicle. Defaults to false.
    #[serde(default)]
    pub verified: bool,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// Written from scratch by the author.
    OriginalWork,
    /// Captured by the author from their own vehicle.
    OwnVehicleCapture,
    /// Facts taken from a public specification or standard, in the author's own words.
    PublicSpecification,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    Kw1281,
    Kwp2000,
    Uds,
    Tp20,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize, JsonSchema,
)]
pub enum CodeSpace {
    /// Generic powertrain/chassis/body/network codes like `P0171`.
    #[serde(rename = "sae_j2012")]
    SaeJ2012,
    /// Five-digit decimal codes used by older VAG modules, like `00525`.
    #[serde(rename = "vag_5digit")]
    Vag5Digit,
}

/// A control module, identified by its diagnostic address.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModuleDef {
    /// Diagnostic address, e.g. `0x01`.
    pub address: u8,
    pub name: String,
    pub description: Option<String>,
    #[serde(default)]
    pub protocols: Vec<Protocol>,
    /// Platforms this applies to (e.g. `PQ24`). Empty means "not stated".
    #[serde(default)]
    pub platforms: Vec<String>,
}

/// A trouble code and what it means, in the author's own words.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DtcDef {
    pub code_space: CodeSpace,
    /// `P0171` for `sae_j2012`, `00525` for `vag_5digit`.
    pub code: String,
    pub description: String,
}

/// Labels for one measuring-block group of one module.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BlockDef {
    pub protocol: Protocol,
    pub module_address: u8,
    /// Group (block) number, 0-255.
    pub group: u8,
    /// One entry per displayed value, in order. KW1281 groups have at most 4.
    pub cells: Vec<Cell>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Cell {
    pub label: String,
    pub unit: Option<String>,
}
