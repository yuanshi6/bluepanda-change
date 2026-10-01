use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

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
    Preview,
    Cancelled,
    Internal,
}

/// Cloneable cancellation signal shared by the preview worker and UI.
#[derive(Clone, Debug, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewFormat {
    Litematica,
    SpongeV1V2,
    SpongeV3,
    JavaStructure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewDocument {
    pub format: PreviewFormat,
    /// Minimum source coordinate represented by document-relative block positions.
    pub origin: [i64; 3],
    pub size: [i32; 3],
    /// Stable, deduplicated palette shared by all sparse preview blocks.
    pub palette: Vec<BlockState>,
    pub regions: Vec<PreviewRegion>,
    pub blocks: Vec<PreviewBlock>,
    pub entities_count: usize,
    pub warnings: Vec<ConversionWarning>,
}

/// Preview metadata and every region's sparse block candidates for interactive visibility.
/// `document.blocks` retains the merged, earlier-region-wins view used by existing callers;
/// `region_blocks` additionally contains overlapping candidates in source region order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewScene {
    pub document: PreviewDocument,
    pub data_version: Option<i32>,
    pub block_entities_count: usize,
    pub region_blocks: Vec<PreviewBlock>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewRegion {
    pub id: u32,
    pub name: String,
    pub source_origin: [i64; 3],
    /// Region origin relative to PreviewDocument::origin.
    pub position: [i32; 3],
    pub size: [i32; 3],
    pub block_count: usize,
    pub entities_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewBlock {
    /// Position relative to PreviewDocument::origin.
    pub position: [i32; 3],
    pub state: u32,
    pub region_id: u32,
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
