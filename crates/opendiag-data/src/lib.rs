//! Open data format v0: module, DTC and measuring-block definitions as YAML,
//! validated in code and published as a generated JSON Schema.
//!
//! All data is community-authored. Nothing here may come from proprietary
//! label files or tools (see `docs/legal.md`); every file carries provenance.

mod model;
mod validate;

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

pub use model::{
    BlockDef, Body, Cell, CodeSpace, DataFile, DtcDef, ModuleDef, Protocol, Provenance, Source,
    FORMAT,
};
pub use validate::validate;

/// Largest data file accepted. Real files are a few kilobytes; the cap stops a
/// stray or hostile file from being read into memory.
const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// One problem in one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub file: String,
    pub message: String,
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.file, self.message)
    }
}

fn issue(file: &str, message: impl fmt::Display) -> Issue {
    Issue {
        file: file.to_string(),
        message: message.to_string(),
    }
}

/// The JSON Schema for data files, generated from the model so it cannot drift.
pub fn json_schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(DataFile)).expect("schema serialises")
}

/// The schema as indented JSON, ready to write to `data/schema/`.
pub fn json_schema_pretty() -> String {
    serde_json::to_string_pretty(&json_schema()).expect("schema serialises")
}

/// Parse and fully validate one file's text. `name` is used in messages.
pub fn parse_str(name: &str, text: &str) -> Result<DataFile, Vec<Issue>> {
    let value: serde_yaml::Value =
        serde_yaml::from_str(text).map_err(|e| vec![issue(name, format!("invalid YAML: {e}"))])?;
    let Some(map) = value.as_mapping() else {
        return Err(vec![issue(name, "top level must be a mapping")]);
    };
    // Unknown keys next to the flattened `kind`/`entries` would be ignored silently.
    let stray: Vec<Issue> = map
        .keys()
        .filter(|k| {
            !matches!(
                k.as_str(),
                Some("format" | "provenance" | "kind" | "entries")
            )
        })
        .map(|k| {
            issue(
                name,
                format!("unknown top-level key `{}`", k.as_str().unwrap_or("?")),
            )
        })
        .collect();
    if !stray.is_empty() {
        return Err(stray);
    }
    let file: DataFile = serde_yaml::from_str(text).map_err(|e| vec![issue(name, e)])?;
    let problems = validate(&file);
    if problems.is_empty() {
        Ok(file)
    } else {
        Err(problems.into_iter().map(|m| issue(name, m)).collect())
    }
}

/// Everything loaded from a directory, indexed for lookup.
#[derive(Debug, Default)]
pub struct DataSet {
    modules: BTreeMap<u8, ModuleDef>,
    dtcs: BTreeMap<(CodeSpace, String), DtcDef>,
    blocks: BTreeMap<(Protocol, u8, u8), BlockDef>,
    files: usize,
}

impl DataSet {
    pub fn module(&self, address: u8) -> Option<&ModuleDef> {
        self.modules.get(&address)
    }

    pub fn dtc(&self, space: CodeSpace, code: &str) -> Option<&DtcDef> {
        self.dtcs.get(&(space, code.to_string()))
    }

    pub fn block(&self, protocol: Protocol, module_address: u8, group: u8) -> Option<&BlockDef> {
        self.blocks.get(&(protocol, module_address, group))
    }

    pub fn file_count(&self) -> usize {
        self.files
    }

    pub fn module_count(&self) -> usize {
        self.modules.len()
    }

    pub fn dtc_count(&self) -> usize {
        self.dtcs.len()
    }

    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_files(&path, out)?;
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("yaml" | "yml")
        ) {
            out.push(path);
        }
    }
    Ok(())
}

/// Load and validate every `.yaml`/`.yml` file under `dir`. Returns all issues
/// found (not just the first), including duplicates across files.
pub fn load_dir(dir: &Path) -> Result<DataSet, Vec<Issue>> {
    let mut paths = Vec::new();
    collect_files(dir, &mut paths).map_err(|e| {
        vec![issue(
            &dir.display().to_string(),
            format!("cannot read directory: {e}"),
        )]
    })?;
    paths.sort();

    let mut set = DataSet::default();
    let mut issues = Vec::new();
    // Where each key was first defined, for cross-file duplicate messages.
    let mut origin: BTreeMap<String, String> = BTreeMap::new();

    for path in paths {
        let name = path
            .strip_prefix(dir)
            .unwrap_or(&path)
            .display()
            .to_string()
            .replace('\\', "/");
        if let Ok(meta) = fs::metadata(&path) {
            if meta.len() > MAX_FILE_BYTES {
                issues.push(issue(
                    &name,
                    format!(
                        "file is {} bytes; the limit is {MAX_FILE_BYTES}",
                        meta.len()
                    ),
                ));
                continue;
            }
        }
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                issues.push(issue(&name, format!("cannot read file: {e}")));
                continue;
            }
        };
        let file = match parse_str(&name, &text) {
            Ok(f) => f,
            Err(mut errs) => {
                issues.append(&mut errs);
                continue;
            }
        };
        set.files += 1;
        let mut claim = |key: String, issues: &mut Vec<Issue>| -> bool {
            if let Some(first) = origin.get(&key) {
                issues.push(issue(&name, format!("{key} is already defined in {first}")));
                false
            } else {
                origin.insert(key, name.clone());
                true
            }
        };
        match file.body {
            Body::Modules(entries) => {
                for m in entries {
                    if claim(format!("module {:#04X}", m.address), &mut issues) {
                        set.modules.insert(m.address, m);
                    }
                }
            }
            Body::Dtcs(entries) => {
                for d in entries {
                    if claim(format!("code {:?} {}", d.code_space, d.code), &mut issues) {
                        set.dtcs.insert((d.code_space, d.code.clone()), d);
                    }
                }
            }
            Body::MeasuringBlocks(entries) => {
                for b in entries {
                    let key = format!(
                        "measuring block {:?} {:#04X} group {}",
                        b.protocol, b.module_address, b.group
                    );
                    if claim(key, &mut issues) {
                        set.blocks
                            .insert((b.protocol, b.module_address, b.group), b);
                    }
                }
            }
        }
    }
    if issues.is_empty() {
        Ok(set)
    } else {
        Err(issues)
    }
}
