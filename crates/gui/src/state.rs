use std::path::PathBuf;

/// Theme choice shown in the settings menu and saved through eframe storage.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum CameraPreset {
    Fit,
    Front,
    Top,
    BirdEye,
    #[default]
    Perspective,
    Orthographic,
    Reset,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum DisplayMode {
    #[default]
    Solid,
    Translucent,
    Wireframe,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum SliceMode {
    CurrentLayer,
    CurrentAndBelow,
    #[default]
    WholeStructure,
}

#[derive(Debug, Clone)]
pub(crate) enum ConversionDisplay {
    NotApplicable,
    Waiting,
    Running {
        stage: String,
        completed: usize,
        total: usize,
        elapsed_seconds: u64,
        logs: Vec<String>,
    },
    Done {
        output: PathBuf,
        blocks: usize,
        entities: usize,
        output_bytes: Option<u64>,
        warnings: Vec<String>,
        elapsed_seconds: u64,
        logs: Vec<String>,
    },
    Failed {
        stage: Option<String>,
        message: String,
        region: Option<String>,
        position: Option<[i32; 3]>,
        warnings: Vec<String>,
        elapsed_seconds: u64,
        logs: Vec<String>,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct WorkspaceRowView {
    pub path: PathBuf,
    pub name: String,
    pub extension: String,
    pub size: Option<u64>,
    pub preview_status: String,
    pub checked: bool,
    pub conversion: ConversionDisplay,
}

#[derive(Debug, Clone)]
pub(crate) struct BatchProgressView {
    pub completed_files: usize,
    pub total_files: usize,
    pub current_file: Option<String>,
    pub stage: Option<String>,
    pub stage_completed: usize,
    pub stage_total: usize,
    pub elapsed_seconds: u64,
    pub estimated_remaining_seconds: Option<u64>,
}

#[derive(Debug, Clone)]
pub(crate) struct WorkspaceView {
    pub rows: Vec<WorkspaceRowView>,
    pub selected: Option<usize>,
    pub checked: Vec<usize>,
    pub output_dir: Option<PathBuf>,
    pub overwrite: bool,
    pub queue_editable: bool,
    pub conversion_running: bool,
    pub progress: Option<BatchProgressView>,
    pub notice: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct PreviewRegionView {
    pub id: u32,
    pub name: String,
    pub size: [i32; 3],
    pub blocks: usize,
    pub entities: usize,
    pub visible: bool,
    pub position: [i32; 3],
    pub source_origin: [i64; 3],
}

#[derive(Debug, Clone)]
pub(crate) struct PreviewView {
    pub path: Option<PathBuf>,
    pub can_convert: bool,
    pub loading_text: Option<String>,
    pub error: Option<String>,
    pub size: [i32; 3],
    pub format: String,
    pub blocks: usize,
    pub entities: usize,
    pub data_version: Option<i32>,
    pub block_entities: usize,
    pub origin: [i64; 3],
    pub coordinate_min: [i64; 3],
    pub coordinate_max: [i64; 3],
    pub regions: Vec<PreviewRegionView>,
    pub current_layer: i32,
    pub max_layer: i32,
    pub slice_mode: SliceMode,
    pub camera_preset: CameraPreset,
    pub display_mode: DisplayMode,
    pub axes_visible: bool,
    pub grid_visible: bool,
    pub preview_mesh_blocks: usize,
    pub preview_mesh_faces: usize,
}

/// UI returns actions instead of mutating application state during egui rendering.
#[derive(Debug, Clone)]
pub(crate) enum AppAction {
    AddFiles,
    RemoveSelected,
    SelectRow(usize),
    ChooseOutputDirectory,
    SetOverwrite(bool),
    ToggleChecked(usize, bool),
    ToggleAllChecked(bool),
    ConvertSelected,
    ConvertBatch,
    ConvertPath(PathBuf),
    /// Continue only queued Waiting rows after a partial batch. Failed rows remain failed.
    ContinueWaiting,
    OpenPreview(PathBuf),
    BackToWorkspace,
    OpenFileLocation(PathBuf),
    SetTheme(ThemeMode),
    SetCamera(CameraPreset),
    SetDisplayMode(DisplayMode),
    SetAxesVisible(bool),
    SetGridVisible(bool),
    SetRegionVisible(u32, bool),
    SetLayer(i32),
    SetSliceMode(SliceMode),
}
