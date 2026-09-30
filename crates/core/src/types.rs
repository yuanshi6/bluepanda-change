use std::collections::BTreeMap;
use std::path::PathBuf;

/// Owned, typed NBT value. This preserves numeric and array tag kinds.
pub type NbtValue = fastnbt::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Read,
    Decompress,
    Parse,
    Validate,
    Decode,
    Normalize,
    Merge,
    Write,
    Verify,
    Commit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversionError {
    pub stage: Stage,
    pub message: String,
    pub region: Option<String>,
    pub position: Option<[i32; 3]>,
}

impl ConversionError {
    pub fn new(stage: Stage, message: impl Into<String>) -> Self {
        Self {
            stage,
            message: message.into(),
            region: None,
            position: None,
        }
    }

    pub fn in_region(mut self, region: impl Into<String>) -> Self {
        self.region = Some(region.into());
        self
    }

    pub fn at(mut self, position: [i32; 3]) -> Self {
        self.position = Some(position);
        self
    }
}

impl std::fmt::Display for ConversionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.stage, self.message)?;
        if let Some(region) = &self.region {
            write!(f, " (region {region})")?;
        }
        if let Some(pos) = self.position {
            write!(f, " at ({}, {}, {})", pos[0], pos[1], pos[2])?;
        }
        Ok(())
    }
}

impl std::error::Error for ConversionError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversionWarning {
    pub region: Option<String>,
    pub message: String,
}

impl ConversionWarning {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            region: None,
            message: message.into(),
        }
    }
    pub fn in_region(mut self, region: impl Into<String>) -> Self {
        self.region = Some(region.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressEvent {
    pub input: PathBuf,
    pub stage: Stage,
    pub completed: usize,
    pub total: usize,
}

#[derive(Debug, Clone, Default)]
pub struct ConversionOptions {
    pub overwrite: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversionResult {
    pub input: PathBuf,
    pub output: PathBuf,
    pub blocks: usize,
    pub entities: usize,
    pub warnings: Vec<ConversionWarning>,
}

#[derive(Debug, Clone)]
pub struct BatchFileResult {
    pub input: PathBuf,
    pub output: PathBuf,
    pub result: Result<ConversionResult, ConversionError>,
}

#[derive(Debug, Clone)]
pub struct BatchResult {
    pub results: Vec<BatchFileResult>,
}

impl BatchResult {
    pub fn all_succeeded(&self) -> bool {
        self.results.iter().all(|row| row.result.is_ok())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct BlockState {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(
        rename = "Properties",
        default,
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub properties: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct Region {
    pub name: String,
    pub position: [i32; 3],
    /// Signed extents from Litematica. Negative values indicate reversed axes.
    pub size: [i32; 3],
    pub palette: Vec<BlockState>,
    pub packed_states: Vec<i64>,
    pub block_entities: Vec<NbtValue>,
    pub entities: Vec<NbtValue>,
}

#[derive(Debug, Clone)]
pub struct ParsedSchematic {
    pub data_version: Option<i32>,
    pub regions: Vec<Region>,
    pub warnings: Vec<ConversionWarning>,
}
