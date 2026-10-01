use crate::preview::{self, BlockInput, GlowRenderer, LayerView, build_mesh_with_progress};
use crate::state::{AppAction, CameraPreset, DisplayMode, SliceMode, ThemeMode};
use crate::ui::{self, IconGlyph};
use bluepanda_core::{
    CancellationToken, ConversionError, ConversionEvent, ConversionOptions, ConversionResult,
    PreviewDocument, PreviewScene, ProgressEvent, Stage, convert_batch_with_events,
    load_preview_scene,
};
use eframe::egui;
use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

const INPUT_EXTENSIONS: &[&str] = &["litematic", "schem", "nbt"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppPage {
    Workspace,
    Preview,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum FileKind {
    Schematic,
    JavaStructure,
    Unsupported,
}

fn file_kind(path: &Path) -> FileKind {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("litematic" | "schem") => FileKind::Schematic,
        Some("nbt") => FileKind::JavaStructure,
        _ => FileKind::Unsupported,
    }
}

#[derive(Clone, Debug)]
enum PreviewState {
    NotRequested,
    Loading {
        stage: String,
        completed: usize,
        total: usize,
    },
    Building {
        completed: usize,
        total: usize,
    },
    Ready,
    Failed(String),
}

#[derive(Clone, Debug)]
enum ConversionState {
    NotApplicable,
    Waiting,
    Running(String),
    Done(ConversionResult),
    Failed(String),
}

struct Row {
    path: PathBuf,
    preview: PreviewState,
    conversion: ConversionState,
}

impl Row {
    fn new(path: PathBuf) -> Self {
        let conversion = if file_kind(&path) == FileKind::JavaStructure {
            ConversionState::NotApplicable
        } else {
            ConversionState::Waiting
        };
        Self {
            path,
            preview: PreviewState::NotRequested,
            conversion,
        }
    }
}

enum PreviewCommand {
    Load {
        generation: u64,
        mesh_generation: u64,
        path: PathBuf,
        token: CancellationToken,
    },
    Build {
        generation: u64,
        mesh_generation: u64,
        blocks: Arc<Vec<BlockInput>>,
        layer: LayerView,
        visible_regions: std::collections::BTreeSet<u32>,
        token: CancellationToken,
    },
    Stop,
}

impl PreviewCommand {
    fn cancel(&self) {
        match self {
            Self::Load { token, .. } | Self::Build { token, .. } => token.cancel(),
            Self::Stop => {}
        }
    }
}

enum PreviewMessage {
    Progress {
        generation: u64,
        event: ProgressEvent,
    },
    Loaded {
        generation: u64,
        scene: PreviewScene,
        blocks: Arc<Vec<BlockInput>>,
    },
    MeshProgress {
        generation: u64,
        mesh_generation: u64,
        completed: usize,
        total: usize,
    },
    MeshReady {
        generation: u64,
        mesh_generation: u64,
        layer: LayerView,
        mesh: preview::CpuMesh,
    },
    Failed {
        generation: u64,
        mesh_generation: u64,
        message: String,
        cancelled: bool,
    },
}

enum ConversionMessage {
    Event(ConversionEvent),
    Done,
}

struct BluePandaApp {
    rows: Vec<Row>,
    selected: Option<usize>,
    output_dir: Option<PathBuf>,
    overwrite: bool,
    conversion_running: bool,
    conversion_rx: Receiver<ConversionMessage>,
    conversion_tx: Sender<ConversionMessage>,
    preview_tx: Sender<PreviewCommand>,
    preview_rx: Receiver<PreviewMessage>,
    preview_generation: u64,
    mesh_generation: u64,
    preview_token: Option<CancellationToken>,
    mesh_token: Option<CancellationToken>,
    preview_path: Option<PathBuf>,
    preview_source_row: Option<PathBuf>,
    preview_state: PreviewState,
    document: Option<PreviewDocument>,
    blocks: Arc<Vec<BlockInput>>,
    layer: LayerView,
    layer_change_at: Option<Instant>,
    pending_layer: Option<LayerView>,
    renderer: GlowRenderer,
    mesh_stats: Option<(usize, usize)>,
    queue_notice: Option<String>,
    page: AppPage,
    theme_mode: ThemeMode,
    settings_open: bool,
    checked_paths: HashSet<PathBuf>,
    conversion_logs: HashMap<PathBuf, Vec<String>>,
    conversion_elapsed: HashMap<PathBuf, u64>,
    conversion_progress: HashMap<PathBuf, ProgressEvent>,
    conversion_warnings: HashMap<PathBuf, Vec<String>>,
    conversion_errors: HashMap<PathBuf, ConversionError>,
    conversion_started: Option<Instant>,
    conversion_file_started: HashMap<PathBuf, Instant>,
    follow_current_conversion: bool,
    conversion_batch_elapsed: u64,
    batch_inputs: Vec<PathBuf>,
    current_y: i32,
    display_mode: DisplayMode,
    axes_visible: bool,
    grid_visible: bool,
    visible_regions: HashSet<u32>,
    slice_mode: SliceMode,
    camera_preset: CameraPreset,
    preview_data_version: Option<i32>,
    preview_block_entities: usize,
    preview_origin: [i64; 3],
    capture_dir: PathBuf,
    capture_scenario: Option<String>,
    capture_scenario_started: bool,
    capture_taken: bool,
    capture_exit_when_done: bool,
    capture_inner_size: Option<[f32; 2]>,
    capture_size_requested: bool,
    frame_count: u32,
}

impl BluePandaApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let args = std::env::args_os().collect::<Vec<_>>();
        let capture_dir = arg_value(&args, "--capture-dir")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../../design/implementation-captures")
            });
        let capture_scenario =
            arg_value(&args, "--capture-scenario").and_then(|value| value.into_string().ok());
        let capture_inner_size = capture_scenario.as_ref().map(|_| {
            match arg_value(&args, "--capture-size").as_deref() {
                Some(value) if value == OsStr::new("small") => [900.0, 600.0],
                _ => [1200.0, 820.0],
            }
        });
        let stored_theme = cc
            .storage
            .and_then(|storage| eframe::get_value::<ThemeMode>(storage, "theme_mode"))
            .unwrap_or_default();
        let theme_mode = arg_value(&args, "--capture-theme")
            .and_then(|value| value.into_string().ok())
            .and_then(|value| match value.as_str() {
                "system" => Some(ThemeMode::System),
                "light" => Some(ThemeMode::Light),
                "dark" => Some(ThemeMode::Dark),
                _ => None,
            })
            .unwrap_or(stored_theme);
        crate::theme::apply(&cc.egui_ctx, theme_mode);
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "noto_sc_regular".into(),
            egui::FontData::from_static(include_bytes!("../assets/NotoSansSC-Regular.ttf")).into(),
        );
        fonts.font_data.insert(
            "noto_sc_semibold".into(),
            egui::FontData::from_static(include_bytes!("../assets/NotoSansSC-SemiBold.ttf")).into(),
        );
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "noto_sc_regular".into());
        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .push("noto_sc_regular".into());
        fonts.families.insert(
            egui::FontFamily::Name("noto_sc_semibold".into()),
            vec!["noto_sc_semibold".into()],
        );
        cc.egui_ctx.set_fonts(fonts);

        let (conversion_tx, conversion_rx) = mpsc::channel();
        let (preview_tx, preview_commands) = mpsc::channel();
        let (preview_messages, preview_rx) = mpsc::channel();
        thread::Builder::new()
            .name("bluepanda-preview".into())
            .spawn(move || preview_worker(preview_commands, preview_messages))
            .expect("preview worker thread starts");

        let mut app = Self {
            rows: Vec::new(),
            selected: None,
            output_dir: None,
            overwrite: false,
            conversion_running: false,
            conversion_rx,
            conversion_tx,
            preview_tx,
            preview_rx,
            preview_generation: 0,
            mesh_generation: 0,
            preview_token: None,
            mesh_token: None,
            preview_path: None,
            preview_source_row: None,
            preview_state: PreviewState::NotRequested,
            document: None,
            blocks: Arc::new(Vec::new()),
            layer: LayerView::All,
            layer_change_at: None,
            pending_layer: None,
            renderer: GlowRenderer::default(),
            mesh_stats: None,
            queue_notice: None,
            page: AppPage::Workspace,
            theme_mode,
            settings_open: false,
            checked_paths: HashSet::new(),
            conversion_logs: HashMap::new(),
            conversion_elapsed: HashMap::new(),
            conversion_progress: HashMap::new(),
            conversion_warnings: HashMap::new(),
            conversion_errors: HashMap::new(),
            conversion_started: None,
            conversion_file_started: HashMap::new(),
            follow_current_conversion: false,
            conversion_batch_elapsed: 0,
            batch_inputs: Vec::new(),
            current_y: 0,
            display_mode: DisplayMode::Solid,
            axes_visible: true,
            grid_visible: true,
            visible_regions: HashSet::new(),
            slice_mode: SliceMode::WholeStructure,
            camera_preset: CameraPreset::Perspective,
            preview_data_version: None,
            preview_block_entities: 0,
            preview_origin: [0; 3],
            capture_dir,
            capture_scenario,
            capture_scenario_started: false,
            capture_taken: false,
            capture_exit_when_done: args.iter().any(|arg| arg == OsStr::new("--capture-exit")),
            capture_inner_size,
            capture_size_requested: false,
            frame_count: 0,
        };
        app.add_paths(preview_paths_from_args(args.iter().cloned()));
        if let Some(output) = arg_value(&args, "--capture-output") {
            app.output_dir = Some(PathBuf::from(output));
        }
        if app.capture_scenario.is_some() && app.output_dir.is_none() {
            app.output_dir = Some(
                std::env::temp_dir()
                    .join(format!("bluepanda-capture-output-{}", std::process::id())),
            );
        }
        if app.capture_scenario.as_deref() == Some("waiting") {
            app.selected = None;
        }
        if app
            .capture_scenario
            .as_deref()
            .is_some_and(|scenario| scenario.starts_with("preview"))
        {
            app.page = AppPage::Preview;
            if let Some(path) = app.selected_path() {
                app.request_preview(path.clone(), Some(path));
            }
        }
        app
    }

    fn workspace_view(&self) -> crate::state::WorkspaceView {
        use crate::state::{ConversionDisplay, WorkspaceRowView};
        let rows = self
            .rows
            .iter()
            .map(|row| {
                let metadata = std::fs::metadata(&row.path).ok();
                let conversion = match &row.conversion {
                    ConversionState::NotApplicable => ConversionDisplay::NotApplicable,
                    ConversionState::Waiting => ConversionDisplay::Waiting,
                    ConversionState::Running(stage) => ConversionDisplay::Running {
                        stage: stage.clone(),
                        completed: self
                            .conversion_progress
                            .get(&row.path)
                            .map(|event| event.completed)
                            .unwrap_or(0),
                        total: self
                            .conversion_progress
                            .get(&row.path)
                            .map(|event| event.total)
                            .unwrap_or(1),
                        elapsed_seconds: self
                            .conversion_file_started
                            .get(&row.path)
                            .map(|started| started.elapsed().as_secs())
                            .or_else(|| self.conversion_elapsed.get(&row.path).copied())
                            .unwrap_or(0),
                        logs: self
                            .conversion_logs
                            .get(&row.path)
                            .cloned()
                            .unwrap_or_default(),
                    },
                    ConversionState::Done(result) => ConversionDisplay::Done {
                        output: result.output.clone(),
                        blocks: result.blocks,
                        entities: result.entities,
                        output_bytes: std::fs::metadata(&result.output)
                            .ok()
                            .map(|metadata| metadata.len()),
                        warnings: result
                            .warnings
                            .iter()
                            .map(|warning| warning.message.clone())
                            .collect(),
                        elapsed_seconds: self
                            .conversion_elapsed
                            .get(&row.path)
                            .copied()
                            .unwrap_or(0),
                        logs: self
                            .conversion_logs
                            .get(&row.path)
                            .cloned()
                            .unwrap_or_default(),
                    },
                    ConversionState::Failed(message) => ConversionDisplay::Failed {
                        stage: self.conversion_errors.get(&row.path).and_then(|error| {
                            (error.stage != Stage::Internal).then(|| format!("{:?}", error.stage))
                        }),
                        message: message.clone(),
                        region: self
                            .conversion_errors
                            .get(&row.path)
                            .and_then(|error| error.region.clone()),
                        position: self
                            .conversion_errors
                            .get(&row.path)
                            .and_then(|error| error.position),
                        warnings: self
                            .conversion_warnings
                            .get(&row.path)
                            .cloned()
                            .unwrap_or_default(),
                        elapsed_seconds: self
                            .conversion_elapsed
                            .get(&row.path)
                            .copied()
                            .unwrap_or(0),
                        logs: self
                            .conversion_logs
                            .get(&row.path)
                            .cloned()
                            .unwrap_or_default(),
                    },
                };
                WorkspaceRowView {
                    path: row.path.clone(),
                    name: row
                        .path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                    extension: row
                        .path
                        .extension()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                    size: metadata.map(|metadata| metadata.len()),
                    preview_status: Self::preview_status_text(&row.preview),
                    checked: self.checked_paths.contains(&row.path),
                    conversion,
                }
            })
            .collect();
        let checked = self
            .rows
            .iter()
            .enumerate()
            .filter_map(|(index, row)| self.checked_paths.contains(&row.path).then_some(index))
            .collect();
        let running = self.rows.iter().find_map(|row| match &row.conversion {
            ConversionState::Running(stage) if self.batch_inputs.contains(&row.path) => {
                Some((row.path.clone(), stage.clone()))
            }
            _ => None,
        });
        let completed_files = self
            .rows
            .iter()
            .filter(|row| {
                self.batch_inputs.contains(&row.path)
                    && matches!(
                        row.conversion,
                        ConversionState::Done(_) | ConversionState::Failed(_)
                    )
            })
            .count();
        let progress = (!self.batch_inputs.is_empty()).then(|| {
            let (current_path, stage) = running
                .map(|(path, stage)| (Some(path), Some(stage)))
                .unwrap_or((None, None));
            let stage_progress = current_path
                .as_ref()
                .and_then(|path| self.conversion_progress.get(path));
            crate::state::BatchProgressView {
                completed_files,
                total_files: self.batch_inputs.len(),
                current_file: current_path
                    .as_ref()
                    .and_then(|path| path.file_name())
                    .map(|name| name.to_string_lossy().into_owned()),
                stage,
                stage_completed: stage_progress.map(|event| event.completed).unwrap_or(0),
                stage_total: stage_progress.map(|event| event.total).unwrap_or(1),
                elapsed_seconds: self
                    .conversion_started
                    .map(|started| started.elapsed().as_secs())
                    .unwrap_or(self.conversion_batch_elapsed),
                estimated_remaining_seconds: self.conversion_started.and_then(|started| {
                    (completed_files > 0).then(|| {
                        started.elapsed().as_secs().saturating_mul(
                            self.batch_inputs.len().saturating_sub(completed_files) as u64,
                        ) / completed_files as u64
                    })
                }),
            }
        });
        crate::state::WorkspaceView {
            rows,
            selected: self.selected,
            checked,
            output_dir: self.output_dir.clone(),
            overwrite: self.overwrite,
            queue_editable: self.can_edit_queue(),
            conversion_running: self.conversion_running,
            progress,
            notice: self.queue_notice.clone(),
        }
    }

    fn preview_view(&self) -> crate::state::PreviewView {
        use crate::state::{PreviewRegionView, PreviewView};
        let document = self.document.as_ref();
        let size = document.map(|document| document.size).unwrap_or([0; 3]);
        let coordinate_min = self.preview_origin;
        let coordinate_max = std::array::from_fn(|axis| {
            coordinate_min[axis] + i64::from(size[axis].saturating_sub(1).max(0))
        });
        PreviewView {
            path: self.preview_path.clone(),
            can_convert: self
                .preview_path
                .as_ref()
                .is_some_and(|path| file_kind(path) == FileKind::Schematic)
                && self.output_dir.is_some()
                && !self.conversion_running,
            loading_text: matches!(
                self.preview_state,
                PreviewState::Loading { .. } | PreviewState::Building { .. }
            )
            .then(|| Self::preview_status_text(&self.preview_state)),
            error: if let PreviewState::Failed(error) = &self.preview_state {
                Some(error.clone())
            } else {
                None
            },
            size,
            format: document
                .map(|document| match document.format {
                    bluepanda_core::PreviewFormat::Litematica => {
                        "Litematica (.litematic)".to_string()
                    }
                    bluepanda_core::PreviewFormat::SpongeV1V2 => {
                        "Sponge Schematic v1/v2 (.schem)".to_string()
                    }
                    bluepanda_core::PreviewFormat::SpongeV3 => {
                        "Sponge Schematic v3 (.schem)".to_string()
                    }
                    bluepanda_core::PreviewFormat::JavaStructure => {
                        "Java Structure (.nbt)".to_string()
                    }
                })
                .unwrap_or_default(),
            blocks: document.map(|document| document.blocks.len()).unwrap_or(0),
            entities: document
                .map(|document| document.entities_count)
                .unwrap_or(0),
            data_version: self.preview_data_version,
            block_entities: self.preview_block_entities,
            origin: self.preview_origin,
            coordinate_min,
            coordinate_max,
            regions: document
                .map(|document| {
                    document
                        .regions
                        .iter()
                        .map(|region| PreviewRegionView {
                            id: region.id,
                            name: region.name.clone(),
                            size: region.size,
                            blocks: region.block_count,
                            entities: region.entities_count,
                            visible: self.visible_regions.contains(&region.id),
                            position: region.position,
                            source_origin: region.source_origin,
                        })
                        .collect()
                })
                .unwrap_or_default(),
            current_layer: self.current_y,
            max_layer: size[1].saturating_sub(1).max(0),
            slice_mode: self.slice_mode,
            camera_preset: self.camera_preset,
            display_mode: self.display_mode,
            axes_visible: self.axes_visible,
            grid_visible: self.grid_visible,
            preview_mesh_blocks: self.mesh_stats.map(|stats| stats.0).unwrap_or(0),
            preview_mesh_faces: self.mesh_stats.map(|stats| stats.1).unwrap_or(0),
        }
    }

    fn render_titlebar(&mut self, ctx: &egui::Context) -> Vec<AppAction> {
        let colors = crate::theme::tokens(ctx);
        let mut actions = Vec::new();
        egui::TopBottomPanel::top("custom_titlebar")
            .exact_height(54.0)
            .frame(
                egui::Frame::new()
                    .fill(colors.surface)
                    .inner_margin(egui::Margin::symmetric(12, 2))
                    .stroke(egui::Stroke::new(1.0, colors.border)),
            )
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    self.paint_brand_cube(ui, colors.blue, colors.surface);
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new("BluePandaChange")
                                .font(egui::FontId::new(
                                    17.0,
                                    egui::FontFamily::Name("noto_sc_semibold".into()),
                                ))
                                .color(colors.text),
                        );
                        ui.label(
                            egui::RichText::new("批量转换 Minecraft 蓝图为 Structure")
                                .size(12.0)
                                .color(colors.muted),
                        );
                    });
                    let reserved = 262.0;
                    let drag_width = (ui.available_width() - reserved).max(40.0);
                    let (_drag_rect, drag) = ui.allocate_exact_size(
                        egui::vec2(drag_width, 38.0),
                        egui::Sense::click_and_drag(),
                    );
                    if drag.drag_started() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                    }
                    if drag.double_clicked() {
                        let maximized =
                            ctx.input(|input| input.viewport().maximized.unwrap_or(false));
                        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
                    }

                    if ui::icon_button(ui, IconGlyph::Settings, colors.muted, 34.0).clicked() {
                        self.settings_open = !self.settings_open;
                    }
                    egui::ComboBox::from_id_salt("theme_mode")
                        .selected_text(match self.theme_mode {
                            ThemeMode::System => "跟随系统",
                            ThemeMode::Light => "浅色",
                            ThemeMode::Dark => "深色",
                        })
                        .width(112.0)
                        .show_ui(ui, |ui| {
                            for (mode, label) in [
                                (ThemeMode::System, "跟随系统"),
                                (ThemeMode::Light, "浅色"),
                                (ThemeMode::Dark, "深色"),
                            ] {
                                if ui
                                    .selectable_value(&mut self.theme_mode, mode, label)
                                    .clicked()
                                {
                                    actions.push(AppAction::SetTheme(mode));
                                }
                            }
                        });
                    let maximized = ctx.input(|input| input.viewport().maximized.unwrap_or(false));
                    if ui::icon_button(ui, IconGlyph::Minimize, colors.text, 30.0).clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                    }
                    if ui::icon_button(ui, IconGlyph::Maximize, colors.text, 30.0).clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
                    }
                    if ui::icon_button(ui, IconGlyph::Close, colors.text, 30.0).clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
            });
        if self.settings_open {
            egui::Window::new("设置")
                .id(egui::Id::new("settings_window"))
                .open(&mut self.settings_open)
                .collapsible(false)
                .resizable(false)
                .default_width(300.0)
                .show(ctx, |ui| {
                    ui.heading("BluePandaChange 设置");
                    ui.separator();
                    ui.label("外观");
                    for (mode, label) in [
                        (ThemeMode::System, "跟随系统"),
                        (ThemeMode::Light, "浅色"),
                        (ThemeMode::Dark, "深色"),
                    ] {
                        if ui.radio_value(&mut self.theme_mode, mode, label).clicked() {
                            actions.push(AppAction::SetTheme(mode));
                        }
                    }
                    ui.separator();
                    ui.label("预览支持 .litematic、.schem 与 Java Structure .nbt 文件。");
                });
        }
        actions
    }

    fn maybe_start_capture_scenario(&mut self) {
        self.frame_count = self.frame_count.saturating_add(1);
        if self.capture_scenario_started {
            return;
        }
        match self.capture_scenario.as_deref() {
            Some("running" | "complete" | "failure") => {
                self.capture_scenario_started = true;
                if self.output_dir.is_none() {
                    self.output_dir = Some(
                        std::env::temp_dir()
                            .join(format!("bluepanda-capture-output-{}", std::process::id())),
                    );
                }
                self.overwrite = true;
                if self.capture_scenario.as_deref() == Some("failure") {
                    let input_dir = std::env::temp_dir().join(format!(
                        "bluepanda-capture-malformed-{}",
                        std::process::id()
                    ));
                    let _ = std::fs::create_dir_all(&input_dir);
                    let malformed = input_dir.join("malformed_fixture.schem");
                    let _ = std::fs::write(&malformed, b"not a gzip or schematic document");
                    self.add_paths([malformed]);
                }
                let mut inputs = self.convertible_paths();
                if self.capture_scenario.as_deref() == Some("failure") && inputs.len() > 1 {
                    let failing = inputs.pop().expect("length checked");
                    inputs.insert(1, failing);
                }
                self.start_conversion(inputs);
            }
            Some(scenario) if scenario.starts_with("preview") => {
                self.capture_scenario_started = true;
                self.page = AppPage::Preview;
            }
            Some("workspace" | "waiting" | "empty") => self.capture_scenario_started = true,
            _ => {}
        }
    }

    fn capture_debug_screenshot(&mut self, ctx: &egui::Context) {
        let scenario_ready = match self.capture_scenario.as_deref() {
            Some("empty" | "workspace" | "waiting") => self.frame_count > 1,
            Some(scenario) if scenario.starts_with("preview") => matches!(
                self.preview_state,
                PreviewState::Ready | PreviewState::Failed(_)
            ),
            Some("running") => {
                self.conversion_running
                    && self.batch_inputs.iter().any(|path| {
                        self.conversion_progress
                            .get(path)
                            .is_some_and(|progress| progress.total > 1 && progress.completed > 0)
                    })
            }
            Some("complete") => {
                !self.conversion_running
                    && !self.batch_inputs.is_empty()
                    && self
                        .rows
                        .iter()
                        .filter(|row| self.batch_inputs.contains(&row.path))
                        .all(|row| {
                            matches!(
                                row.conversion,
                                ConversionState::Done(_) | ConversionState::Failed(_)
                            )
                        })
            }
            Some("failure") => {
                !self.conversion_running
                    && !self.batch_inputs.is_empty()
                    && self.rows.iter().any(|row| {
                        self.batch_inputs.contains(&row.path)
                            && matches!(row.conversion, ConversionState::Failed(_))
                    })
            }
            _ => false,
        };
        if !self.capture_taken && scenario_ready {
            let name = self
                .capture_scenario
                .clone()
                .unwrap_or_else(|| match self.page {
                    AppPage::Workspace => "workspace".into(),
                    AppPage::Preview => "preview".into(),
                });
            let _ = std::fs::create_dir_all(&self.capture_dir);
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(name)));
            self.capture_taken = true;
        }
        if ctx.input(|input| input.key_pressed(egui::Key::F12)) {
            let name = match self.page {
                AppPage::Workspace => "workspace-manual",
                AppPage::Preview => "preview-manual",
            };
            let _ = std::fs::create_dir_all(&self.capture_dir);
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(
                name.to_string(),
            )));
        }
        let events = ctx.input(|input| input.raw.events.clone());
        for event in events {
            if let egui::Event::Screenshot {
                user_data, image, ..
            } = event
            {
                let name = user_data
                    .data
                    .as_ref()
                    .and_then(|data| data.downcast_ref::<String>())
                    .map(String::as_str)
                    .unwrap_or("capture");
                let filename = format!("{name}.png");
                let path = self.capture_dir.join(filename);
                let rgba = image
                    .pixels
                    .iter()
                    .flat_map(|pixel| pixel.to_array())
                    .collect::<Vec<_>>();
                if let Err(error) = image::save_buffer(
                    &path,
                    &rgba,
                    image.size[0] as u32,
                    image.size[1] as u32,
                    image::ColorType::Rgba8,
                ) {
                    eprintln!("screenshot save failed: {error}");
                } else {
                    eprintln!("screenshot saved: {}", path.display());
                    if self.capture_exit_when_done {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
            }
        }
    }

    fn paint_brand_cube(&self, ui: &mut egui::Ui, blue: egui::Color32, surface: egui::Color32) {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(30.0, 34.0), egui::Sense::hover());
        let painter = ui.painter();
        let c = rect.center();
        let top = [
            c + egui::vec2(0.0, -13.0),
            c + egui::vec2(12.0, -6.0),
            c,
            c + egui::vec2(-12.0, -6.0),
        ];
        let left = [
            top[3],
            c,
            c + egui::vec2(0.0, 13.0),
            c + egui::vec2(-12.0, 6.0),
        ];
        let right = [
            c,
            top[1],
            c + egui::vec2(12.0, 6.0),
            c + egui::vec2(0.0, 13.0),
        ];
        painter.add(egui::Shape::convex_polygon(
            top.to_vec(),
            blue,
            egui::Stroke::new(1.0, surface),
        ));
        painter.add(egui::Shape::convex_polygon(
            left.to_vec(),
            egui::Color32::from_rgb(12, 86, 205),
            egui::Stroke::new(1.0, surface),
        ));
        painter.add(egui::Shape::convex_polygon(
            right.to_vec(),
            egui::Color32::from_rgb(61, 145, 255),
            egui::Stroke::new(1.0, surface),
        ));
    }

    fn apply_action(&mut self, ctx: &egui::Context, action: AppAction) {
        match action {
            AppAction::AddFiles => {
                if self.can_edit_queue() {
                    if let Some(paths) = rfd::FileDialog::new()
                        .add_filter("结构文件", INPUT_EXTENSIONS)
                        .pick_files()
                    {
                        self.add_paths(paths);
                    }
                }
            }
            AppAction::RemoveSelected => {
                self.remove_checked_or_selected();
            }
            AppAction::SelectRow(index) => {
                self.follow_current_conversion = false;
                if index < self.rows.len() {
                    self.selected = Some(index);
                    let path = self.rows[index].path.clone();
                    self.request_preview(path.clone(), Some(path));
                }
            }
            AppAction::ChooseOutputDirectory => {
                if self.can_edit_queue() {
                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                        self.output_dir = Some(path);
                    }
                }
            }
            AppAction::SetOverwrite(overwrite) => {
                if self.can_edit_queue() {
                    self.overwrite = overwrite;
                }
            }
            AppAction::ToggleChecked(index, checked) => {
                if let Some(row) = self.rows.get(index) {
                    if checked {
                        self.checked_paths.insert(row.path.clone());
                    } else {
                        self.checked_paths.remove(&row.path);
                    }
                }
            }
            AppAction::ToggleAllChecked(checked) => {
                self.checked_paths.clear();
                if checked {
                    self.checked_paths
                        .extend(self.rows.iter().map(|row| row.path.clone()));
                }
            }
            AppAction::ConvertSelected => {
                if let Some(path) = self
                    .selected_path()
                    .filter(|path| file_kind(path) == FileKind::Schematic)
                {
                    self.start_conversion(vec![path]);
                }
            }
            AppAction::ConvertBatch => self.start_conversion(self.convertible_paths()),
            AppAction::ConvertPath(path) => {
                if file_kind(&path) == FileKind::Schematic {
                    self.start_conversion(vec![path]);
                }
            }
            AppAction::ContinueWaiting => {
                self.page = AppPage::Workspace;
                if self.conversion_running {
                    self.follow_current_conversion = true;
                    self.selected = self
                        .rows
                        .iter()
                        .position(|row| {
                            self.batch_inputs.contains(&row.path)
                                && matches!(row.conversion, ConversionState::Running(_))
                        })
                        .or_else(|| {
                            self.rows.iter().position(|row| {
                                self.batch_inputs.contains(&row.path)
                                    && matches!(row.conversion, ConversionState::Waiting)
                            })
                        });
                } else {
                    let waiting = self
                        .rows
                        .iter()
                        .filter(|row| {
                            matches!(row.conversion, ConversionState::Waiting)
                                && file_kind(&row.path) == FileKind::Schematic
                        })
                        .map(|row| row.path.clone())
                        .collect::<Vec<_>>();
                    if !waiting.is_empty() {
                        self.start_conversion(waiting);
                    }
                }
            }
            AppAction::OpenPreview(path) => {
                self.page = AppPage::Preview;
                let source = self
                    .rows
                    .iter()
                    .any(|row| row.path == path)
                    .then(|| path.clone());
                self.request_preview(path, source);
            }
            AppAction::BackToWorkspace => self.page = AppPage::Workspace,
            AppAction::OpenFileLocation(path) => Self::reveal_file(&path),
            AppAction::SetTheme(mode) => {
                self.theme_mode = mode;
                crate::theme::apply(ctx, mode);
            }
            AppAction::SetCamera(preset) => {
                self.camera_preset = preset;
                self.renderer.set_camera_preset(preset);
            }
            AppAction::SetDisplayMode(mode) => {
                self.display_mode = mode;
                self.renderer.set_display_mode(mode);
            }
            AppAction::SetAxesVisible(visible) => {
                self.axes_visible = visible;
                self.renderer.set_axes_visible(visible);
            }
            AppAction::SetGridVisible(visible) => {
                self.grid_visible = visible;
                self.renderer.set_grid_visible(visible);
            }
            AppAction::SetRegionVisible(id, visible) => {
                if visible {
                    self.visible_regions.insert(id);
                } else {
                    self.visible_regions.remove(&id);
                }
                let layer = match self.slice_mode {
                    SliceMode::WholeStructure => LayerView::All,
                    SliceMode::CurrentLayer => LayerView::Layer(self.current_y),
                    SliceMode::CurrentAndBelow => LayerView::Through(self.current_y),
                };
                self.request_layer_mesh(layer);
            }
            AppAction::SetLayer(y) => {
                self.current_y = y;
                if self.slice_mode == SliceMode::WholeStructure {
                    self.slice_mode = SliceMode::CurrentLayer;
                }
                let layer = match self.slice_mode {
                    SliceMode::WholeStructure => LayerView::All,
                    SliceMode::CurrentLayer => LayerView::Layer(y),
                    SliceMode::CurrentAndBelow => LayerView::Through(y),
                };
                self.request_layer_mesh(layer);
            }
            AppAction::SetSliceMode(mode) => {
                self.slice_mode = mode;
                let layer = match mode {
                    SliceMode::WholeStructure => LayerView::All,
                    SliceMode::CurrentLayer => LayerView::Layer(self.current_y),
                    SliceMode::CurrentAndBelow => LayerView::Through(self.current_y),
                };
                self.request_layer_mesh(layer);
            }
        }
    }

    fn reveal_file(path: &std::path::Path) {
        #[cfg(target_os = "macos")]
        let mut command = {
            let mut command = std::process::Command::new("open");
            command.arg("-R").arg(path);
            command
        };
        #[cfg(target_os = "windows")]
        let mut command = {
            let mut command = std::process::Command::new("explorer");
            command.arg(format!("/select,{}", path.display()));
            command
        };
        #[cfg(all(unix, not(target_os = "macos")))]
        let mut command = {
            let mut command = std::process::Command::new("xdg-open");
            command.arg(path.parent().unwrap_or(path));
            command
        };
        let _ = command.spawn();
    }

    fn can_edit_queue(&self) -> bool {
        !self.conversion_running
    }

    fn selected_path(&self) -> Option<PathBuf> {
        self.selected
            .and_then(|index| self.rows.get(index))
            .map(|row| row.path.clone())
    }

    fn can_convert_selected(&self) -> bool {
        self.can_edit_queue()
            && self.output_dir.is_some()
            && self
                .selected_path()
                .is_some_and(|path| file_kind(&path) == FileKind::Schematic)
    }

    fn convertible_paths(&self) -> Vec<PathBuf> {
        self.rows
            .iter()
            .filter(|row| file_kind(&row.path) == FileKind::Schematic)
            .map(|row| row.path.clone())
            .collect()
    }

    fn add_paths(&mut self, paths: impl IntoIterator<Item = PathBuf>) -> bool {
        if !self.can_edit_queue() {
            return false;
        }
        let mut last = None;
        let mut rejected = 0usize;
        for path in paths {
            if file_kind(&path) == FileKind::Unsupported {
                rejected += 1;
                continue;
            }
            let index = self
                .rows
                .iter()
                .position(|row| row.path == path)
                .unwrap_or_else(|| {
                    self.rows.push(Row::new(path.clone()));
                    self.rows.len() - 1
                });
            last = Some((index, path));
        }
        if let Some((index, path)) = last {
            self.selected = Some(index);
            self.request_preview(path.clone(), Some(path));
        }
        self.queue_notice = (rejected > 0)
            .then(|| format!("已忽略 {rejected} 个不支持的文件；支持 .litematic、.schem、.nbt"));
        true
    }

    fn remove_selected(&mut self) -> bool {
        if !self.can_edit_queue() {
            return false;
        }
        let Some(index) = self.selected.take() else {
            return false;
        };
        if index >= self.rows.len() {
            return false;
        }
        let removed = self.rows.remove(index);
        if self.preview_path.as_ref() == Some(&removed.path) {
            self.cancel_preview();
        }
        self.selected = if self.rows.is_empty() {
            None
        } else {
            Some(index.min(self.rows.len() - 1))
        };
        if let Some(path) = self.selected_path() {
            self.request_preview(path.clone(), Some(path));
        }
        true
    }

    fn remove_checked_or_selected(&mut self) {
        if !self.can_edit_queue() {
            return;
        }
        let selected_path = self.selected_path();
        let removed = if self.checked_paths.is_empty() {
            selected_path.iter().cloned().collect::<HashSet<_>>()
        } else {
            self.checked_paths.clone()
        };
        if removed.is_empty() {
            return;
        }
        self.rows.retain(|row| !removed.contains(&row.path));
        self.checked_paths.retain(|path| !removed.contains(path));
        if self
            .preview_path
            .as_ref()
            .is_some_and(|path| removed.contains(path))
        {
            self.cancel_preview();
        }
        self.selected = selected_path
            .and_then(|path| self.rows.iter().position(|row| row.path == path))
            .or_else(|| (!self.rows.is_empty()).then_some(0));
        if let Some(path) = self.selected_path() {
            self.request_preview(path.clone(), Some(path));
        }
    }

    fn cancel_preview(&mut self) {
        if let Some(token) = self.preview_token.take() {
            token.cancel();
        }
        if let Some(token) = self.mesh_token.take() {
            token.cancel();
        }
        self.preview_generation = self.preview_generation.wrapping_add(1);
        self.mesh_generation = self.mesh_generation.wrapping_add(1);
        self.preview_path = None;
        self.preview_source_row = None;
        self.document = None;
        self.blocks = Arc::new(Vec::new());
        self.visible_regions.clear();
        self.preview_data_version = None;
        self.preview_block_entities = 0;
        self.preview_origin = [0; 3];
        self.preview_state = PreviewState::NotRequested;
        self.layer = LayerView::All;
        self.pending_layer = None;
        self.renderer.clear();
        self.mesh_stats = None;
    }

    fn request_preview(&mut self, path: PathBuf, source_row: Option<PathBuf>) {
        if let Some(token) = self.preview_token.take() {
            token.cancel();
        }
        if let Some(token) = self.mesh_token.take() {
            token.cancel();
        }
        self.preview_generation = self.preview_generation.wrapping_add(1);
        self.mesh_generation = self.mesh_generation.wrapping_add(1);
        self.preview_path = Some(path.clone());
        self.preview_source_row = source_row.clone();
        self.document = None;
        self.blocks = Arc::new(Vec::new());
        self.visible_regions.clear();
        self.preview_data_version = None;
        self.preview_block_entities = 0;
        self.preview_origin = [0; 3];
        self.layer = LayerView::All;
        self.pending_layer = None;
        self.layer_change_at = None;
        self.preview_state = PreviewState::Loading {
            stage: "启动预览".into(),
            completed: 0,
            total: 1,
        };
        self.renderer.clear();
        self.mesh_stats = None;
        if let Some(row) =
            source_row.and_then(|source| self.rows.iter_mut().find(|row| row.path == source))
        {
            row.preview = self.preview_state.clone();
        }
        let token = CancellationToken::default();
        self.preview_token = Some(token.clone());
        let command = PreviewCommand::Load {
            generation: self.preview_generation,
            mesh_generation: self.mesh_generation,
            path,
            token,
        };
        if self.preview_tx.send(command).is_err() {
            self.set_preview_failure("预览任务队列已关闭".into());
        }
    }

    fn request_layer_mesh(&mut self, layer: LayerView) {
        if self.document.is_none() {
            return;
        }
        if let Some(token) = self.mesh_token.take() {
            token.cancel();
        }
        self.mesh_generation = self.mesh_generation.wrapping_add(1);
        self.layer = layer;
        let token = CancellationToken::default();
        self.mesh_token = Some(token.clone());
        self.preview_state = PreviewState::Building {
            completed: 0,
            total: self.blocks.len(),
        };
        self.renderer.clear();
        self.mesh_stats = None;
        if let Some(row) = self
            .preview_source_row
            .as_ref()
            .and_then(|source| self.rows.iter_mut().find(|row| &row.path == source))
        {
            row.preview = self.preview_state.clone();
        }
        let command = PreviewCommand::Build {
            generation: self.preview_generation,
            mesh_generation: self.mesh_generation,
            blocks: self.blocks.clone(),
            layer,
            visible_regions: self.visible_regions.iter().copied().collect(),
            token,
        };
        let _ = self.preview_tx.send(command);
    }

    fn configure_capture_preview(&mut self) -> bool {
        let Some(scenario) = self.capture_scenario.as_deref() else {
            return false;
        };
        let max_y = self
            .document
            .as_ref()
            .map(|document| document.size[1].saturating_sub(1).max(0))
            .unwrap_or(0);
        let mut rebuild_mesh = false;
        match scenario {
            "preview-wireframe" => {
                self.display_mode = DisplayMode::Wireframe;
                self.renderer.set_display_mode(self.display_mode);
            }
            "preview-translucent" => {
                self.display_mode = DisplayMode::Translucent;
                self.renderer.set_display_mode(self.display_mode);
            }
            "preview-orthographic" => {
                self.camera_preset = CameraPreset::Orthographic;
                self.renderer.set_camera_preset(self.camera_preset);
            }
            "preview-top" => {
                self.camera_preset = CameraPreset::Top;
                self.renderer.set_camera_preset(self.camera_preset);
            }
            "preview-layer-zero" => {
                self.slice_mode = SliceMode::CurrentLayer;
                self.current_y = 0;
                rebuild_mesh = true;
            }
            "preview-through" => {
                self.slice_mode = SliceMode::CurrentAndBelow;
                self.current_y = (max_y / 2).max(1).min(max_y);
                rebuild_mesh = true;
            }
            "preview-orthographic-through-hidden" => {
                self.slice_mode = SliceMode::CurrentAndBelow;
                self.current_y = (max_y / 2).max(1).min(max_y);
                self.camera_preset = CameraPreset::Orthographic;
                self.renderer.set_camera_preset(self.camera_preset);
                if let Some(region) = self
                    .document
                    .as_ref()
                    .and_then(|document| document.regions.first())
                    .map(|region| region.id)
                {
                    self.visible_regions.remove(&region);
                }
                rebuild_mesh = true;
            }
            "preview-regions-hidden" => {
                if let Some(region) = self.visible_regions.iter().next().copied() {
                    self.visible_regions.remove(&region);
                }
                rebuild_mesh = true;
            }
            _ => {}
        }
        rebuild_mesh
    }

    fn set_preview_failure(&mut self, message: String) {
        self.preview_state = PreviewState::Failed(message.clone());
        if let Some(row) = self
            .preview_source_row
            .as_ref()
            .and_then(|source| self.rows.iter_mut().find(|row| &row.path == source))
        {
            row.preview = PreviewState::Failed(message);
        }
        self.renderer.clear();
    }

    fn start_conversion(&mut self, inputs: Vec<PathBuf>) {
        if self.conversion_running || self.output_dir.is_none() || inputs.is_empty() {
            return;
        }
        let output_dir = self.output_dir.clone().expect("checked above");
        let inputs: Vec<_> = inputs
            .into_iter()
            .filter(|path| file_kind(path) == FileKind::Schematic)
            .collect();
        if inputs.is_empty() {
            return;
        }
        for row in &mut self.rows {
            if inputs.contains(&row.path) {
                row.conversion = ConversionState::Waiting;
            }
        }
        self.batch_inputs = inputs.clone();
        self.follow_current_conversion = true;
        self.selected = inputs
            .first()
            .and_then(|path| self.rows.iter().position(|row| &row.path == path));
        self.conversion_started = Some(Instant::now());
        for path in &inputs {
            self.conversion_logs.insert(path.clone(), Vec::new());
            self.conversion_warnings.insert(path.clone(), Vec::new());
            self.conversion_errors.remove(path);
            self.conversion_progress.remove(path);
            self.conversion_elapsed.insert(path.clone(), 0);
        }
        self.conversion_running = true;
        let tx = self.conversion_tx.clone();
        let options = ConversionOptions {
            overwrite: self.overwrite,
        };
        let capture_progress = self.capture_scenario.as_deref() == Some("running");
        thread::Builder::new()
            .name("bluepanda-convert".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    convert_batch_with_events(&inputs, &output_dir, &options, |event| {
                        let _ = tx.send(ConversionMessage::Event(event));
                        if capture_progress {
                            thread::sleep(Duration::from_millis(24));
                        }
                    })
                }));
                if result.is_err() {
                    for path in inputs {
                        let error =
                            ConversionError::new(Stage::Internal, "后台转换任务发生内部异常");
                        let _ = tx.send(ConversionMessage::Event(ConversionEvent::FileFinished(
                            bluepanda_core::FileFinished {
                                input: path.clone(),
                                output: output_dir.join(path.file_name().unwrap_or_default()),
                                result: Err(error),
                                warnings: Vec::new(),
                                elapsed: Duration::ZERO,
                            },
                        )));
                    }
                }
                let _ = tx.send(ConversionMessage::Done);
            })
            .expect("conversion worker thread starts");
    }

    fn drain_preview(&mut self) {
        while let Ok(message) = self.preview_rx.try_recv() {
            match message {
                PreviewMessage::Progress { generation, event }
                    if generation == self.preview_generation =>
                {
                    self.preview_state = PreviewState::Loading {
                        stage: format!("{:?}", event.stage),
                        completed: event.completed,
                        total: event.total.max(1),
                    };
                    if let Some(row) = self
                        .preview_source_row
                        .as_ref()
                        .and_then(|source| self.rows.iter_mut().find(|row| &row.path == source))
                    {
                        row.preview = self.preview_state.clone();
                    }
                }
                PreviewMessage::Loaded {
                    generation,
                    scene,
                    blocks,
                } if generation == self.preview_generation => {
                    self.preview_data_version = scene.data_version;
                    self.preview_block_entities = scene.block_entities_count;
                    self.preview_origin = scene.document.origin;
                    self.visible_regions = scene
                        .document
                        .regions
                        .iter()
                        .map(|region| region.id)
                        .collect();
                    self.renderer
                        .fit(scene.document.size, scene.document.origin);
                    self.document = Some(scene.document);
                    self.blocks = blocks;
                    self.mesh_token = self.preview_token.clone();
                    self.preview_state = PreviewState::Building {
                        completed: 0,
                        total: self.blocks.len(),
                    };
                    if self.configure_capture_preview() {
                        let layer = match self.slice_mode {
                            SliceMode::WholeStructure => LayerView::All,
                            SliceMode::CurrentLayer => LayerView::Layer(self.current_y),
                            SliceMode::CurrentAndBelow => LayerView::Through(self.current_y),
                        };
                        self.request_layer_mesh(layer);
                    }
                }
                PreviewMessage::MeshProgress {
                    generation,
                    mesh_generation,
                    completed,
                    total,
                } if generation == self.preview_generation
                    && mesh_generation == self.mesh_generation =>
                {
                    self.preview_state = PreviewState::Building { completed, total };
                }
                PreviewMessage::MeshReady {
                    generation,
                    mesh_generation,
                    layer,
                    mesh,
                } if generation == self.preview_generation
                    && mesh_generation == self.mesh_generation =>
                {
                    self.mesh_stats = Some((mesh.block_count, mesh.face_count));
                    self.renderer.set_mesh(mesh);
                    self.layer = layer;
                    self.preview_state = PreviewState::Ready;
                    self.mesh_token = None;
                    if let Some(row) = self
                        .preview_source_row
                        .as_ref()
                        .and_then(|source| self.rows.iter_mut().find(|row| &row.path == source))
                    {
                        row.preview = PreviewState::Ready;
                    }
                }
                PreviewMessage::Failed {
                    generation,
                    mesh_generation,
                    message,
                    cancelled,
                } if generation == self.preview_generation
                    && mesh_generation == self.mesh_generation
                    && !cancelled =>
                {
                    self.mesh_token = None;
                    self.set_preview_failure(message);
                }
                _ => {}
            }
        }
    }

    fn drain_conversion(&mut self) {
        while let Ok(message) = self.conversion_rx.try_recv() {
            match message {
                ConversionMessage::Event(ConversionEvent::Progress(event)) => {
                    if self.follow_current_conversion {
                        self.selected = self.rows.iter().position(|row| row.path == event.input);
                    }
                    if let Some(row) = self.rows.iter_mut().find(|row| row.path == event.input) {
                        row.conversion = ConversionState::Running(format!("{:?}", event.stage));
                        self.conversion_progress
                            .insert(event.input.clone(), event.clone());
                        self.conversion_file_started
                            .entry(event.input.clone())
                            .or_insert_with(Instant::now);
                        let logs = self.conversion_logs.entry(event.input).or_default();
                        let label = format!(
                            "{:?}: {}/{}",
                            event.stage,
                            event.completed,
                            event.total.max(1)
                        );
                        if logs.last() != Some(&label) {
                            logs.push(label);
                        }
                    }
                }
                ConversionMessage::Event(ConversionEvent::Warning { input, warning }) => {
                    self.conversion_warnings
                        .entry(input.clone())
                        .or_default()
                        .push(warning.message.clone());
                    self.conversion_logs
                        .entry(input)
                        .or_default()
                        .push(format!("警告：{}", warning.message));
                }
                ConversionMessage::Event(ConversionEvent::FileFinished(finished)) => {
                    let path = finished.input.clone();
                    self.conversion_elapsed
                        .insert(path.clone(), finished.elapsed.as_secs());
                    self.conversion_file_started.remove(&path);
                    let warnings = finished
                        .warnings
                        .iter()
                        .map(|warning| warning.message.clone())
                        .collect::<Vec<_>>();
                    self.conversion_warnings
                        .insert(path.clone(), warnings.clone());
                    let state = match finished.result {
                        Ok(result) => ConversionState::Done(result),
                        Err(error) => {
                            self.conversion_errors.insert(path.clone(), error.clone());
                            self.selected = self.rows.iter().position(|row| row.path == path);
                            self.follow_current_conversion = false;
                            ConversionState::Failed(error.to_string())
                        }
                    };
                    if let Some(row) = self.rows.iter_mut().find(|row| row.path == path) {
                        row.conversion = state;
                    }
                    let logs = self.conversion_logs.entry(path).or_default();
                    logs.push(format!("处理完成，用时 {} 秒", finished.elapsed.as_secs()));
                    for warning in warnings {
                        if !logs.iter().any(|line| line == &format!("警告：{warning}")) {
                            logs.push(format!("警告：{warning}"));
                        }
                    }
                }
                ConversionMessage::Event(ConversionEvent::BatchFinished { .. }) => {}
                ConversionMessage::Done => {
                    self.conversion_batch_elapsed = self
                        .conversion_started
                        .map(|started| started.elapsed().as_secs())
                        .unwrap_or(self.conversion_batch_elapsed);
                    self.conversion_running = false;
                    self.conversion_started = None;
                    self.follow_current_conversion = false;
                    if self.capture_scenario.as_deref() == Some("complete") {
                        self.selected = self
                            .batch_inputs
                            .first()
                            .and_then(|path| self.rows.iter().position(|row| &row.path == path));
                    }
                }
            }
        }
    }

    fn preview_status_text(state: &PreviewState) -> String {
        match state {
            PreviewState::NotRequested => "未预览".into(),
            PreviewState::Loading {
                stage,
                completed,
                total,
            } => format!("读取中：{stage} {completed}/{total}"),
            PreviewState::Building { completed, total } => format!("生成网格：{completed}/{total}"),
            PreviewState::Ready => "预览就绪".into(),
            PreviewState::Failed(error) => format!("预览失败：{error}"),
        }
    }

    fn conversion_status_text(state: &ConversionState) -> String {
        match state {
            ConversionState::NotApplicable => "Java Structure（仅预览）".into(),
            ConversionState::Waiting => "等待转换".into(),
            ConversionState::Running(stage) => format!("转换中：{stage}"),
            ConversionState::Done(result) => {
                format!("完成：{} 方块，{} 实体", result.blocks, result.entities)
            }
            ConversionState::Failed(error) => format!("转换失败：{error}"),
        }
    }
}

fn preview_worker(commands: Receiver<PreviewCommand>, messages: Sender<PreviewMessage>) {
    while let Ok(mut command) = commands.recv() {
        while let Ok(newer) = commands.try_recv() {
            command.cancel();
            command = newer;
        }
        match command {
            PreviewCommand::Stop => break,
            PreviewCommand::Load {
                generation,
                mesh_generation,
                path,
                token,
            } => {
                let tx = messages.clone();
                let loaded = load_preview_scene(&path, &token, |event| {
                    let _ = tx.send(PreviewMessage::Progress { generation, event });
                });
                match loaded {
                    Ok(scene) => {
                        if token.is_cancelled() {
                            continue;
                        }
                        let blocks = match document_to_blocks(&scene) {
                            Ok(blocks) => blocks,
                            Err(message) => {
                                let _ = messages.send(PreviewMessage::Failed {
                                    generation,
                                    mesh_generation,
                                    message,
                                    cancelled: false,
                                });
                                continue;
                            }
                        };
                        let blocks = Arc::new(blocks);
                        let visible_regions = scene
                            .document
                            .regions
                            .iter()
                            .map(|region| region.id)
                            .collect::<std::collections::BTreeSet<_>>();
                        let _ = messages.send(PreviewMessage::Loaded {
                            generation,
                            scene,
                            blocks: blocks.clone(),
                        });
                        let _ = build_and_send_mesh(
                            generation,
                            mesh_generation,
                            blocks,
                            LayerView::All,
                            visible_regions,
                            token,
                            &messages,
                        );
                    }
                    Err(error) => {
                        let _ = messages.send(PreviewMessage::Failed {
                            generation,
                            mesh_generation,
                            message: error.to_string(),
                            cancelled: error.stage == Stage::Cancelled,
                        });
                    }
                }
            }
            PreviewCommand::Build {
                generation,
                mesh_generation,
                blocks,
                layer,
                visible_regions,
                token,
            } => {
                let _ = build_and_send_mesh(
                    generation,
                    mesh_generation,
                    blocks,
                    layer,
                    visible_regions,
                    token,
                    &messages,
                );
            }
        }
    }
}

fn document_to_blocks(scene: &PreviewScene) -> Result<Vec<BlockInput>, String> {
    scene
        .region_blocks
        .iter()
        .map(|block| {
            let state = scene
                .document
                .palette
                .get(block.state as usize)
                .ok_or_else(|| format!("方块状态索引 {} 超出调色板", block.state))?;
            Ok(BlockInput {
                position: block.position,
                name: state.name.clone(),
                properties: state.properties.clone().into_iter().collect(),
                region_id: block.region_id,
            })
        })
        .collect()
}

fn build_and_send_mesh(
    generation: u64,
    mesh_generation: u64,
    blocks: Arc<Vec<BlockInput>>,
    layer: LayerView,
    visible_regions: std::collections::BTreeSet<u32>,
    token: CancellationToken,
    messages: &Sender<PreviewMessage>,
) -> Result<(), ()> {
    let tx = messages.clone();
    let built = build_mesh_with_progress(
        blocks.as_ref(),
        layer,
        &visible_regions,
        || token.is_cancelled(),
        |completed, total| {
            let _ = tx.send(PreviewMessage::MeshProgress {
                generation,
                mesh_generation,
                completed,
                total,
            });
        },
    );
    match built {
        Ok(mesh) if !token.is_cancelled() => {
            let _ = messages.send(PreviewMessage::MeshReady {
                generation,
                mesh_generation,
                layer,
                mesh,
            });
        }
        Ok(_) => {
            let _ = messages.send(PreviewMessage::Failed {
                generation,
                mesh_generation,
                message: "预览已取消".into(),
                cancelled: true,
            });
        }
        Err(message) => {
            let cancelled = token.is_cancelled() || message.contains("取消");
            let _ = messages.send(PreviewMessage::Failed {
                generation,
                mesh_generation,
                message,
                cancelled,
            });
        }
    }
    Ok(())
}

impl eframe::App for BluePandaApp {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if self.capture_scenario.is_none() {
            eframe::set_value(storage, "theme_mode", &self.theme_mode);
        }
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(size) = self
            .capture_inner_size
            .filter(|_| !self.capture_size_requested)
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                size[0], size[1],
            )));
            self.capture_size_requested = true;
        }
        self.drain_preview();
        self.drain_conversion();
        self.maybe_start_capture_scenario();
        if self.conversion_running
            || matches!(
                self.preview_state,
                PreviewState::Loading { .. } | PreviewState::Building { .. }
            )
        {
            ctx.request_repaint_after(Duration::from_millis(50));
        }

        crate::theme::apply(ctx, self.theme_mode);
        let mut actions = self.render_titlebar(ctx);
        let dropped: Vec<PathBuf> = ctx.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .filter_map(|file| file.path.clone())
                .collect()
        });
        if !dropped.is_empty() && self.can_edit_queue() {
            self.add_paths(dropped);
        }

        match self.page {
            AppPage::Workspace => {
                let view = self.workspace_view();
                egui::CentralPanel::default()
                    .frame(
                        egui::Frame::new()
                            .fill(crate::theme::tokens(ctx).canvas)
                            .inner_margin(egui::Margin::same(16)),
                    )
                    .show(ctx, |ui| actions.extend(ui::workspace::show(ui, &view)));
            }
            AppPage::Preview => {
                let view = self.preview_view();
                egui::CentralPanel::default()
                    .frame(
                        egui::Frame::new()
                            .fill(crate::theme::tokens(ctx).canvas)
                            .inner_margin(egui::Margin::same(12)),
                    )
                    .show(ctx, |ui| {
                        actions.extend(ui::preview::show(ui, &view, &self.renderer))
                    });
            }
        }
        for action in actions {
            self.apply_action(ctx, action);
        }
        self.capture_debug_screenshot(ctx);
    }

    fn on_exit(&mut self, gl: Option<&eframe::glow::Context>) {
        if let Some(token) = &self.preview_token {
            token.cancel();
        }
        if let Some(token) = &self.mesh_token {
            token.cancel();
        }
        let _ = self.preview_tx.send(PreviewCommand::Stop);
        self.renderer.release_gl(gl);
    }
}

pub(crate) fn run() -> eframe::Result {
    let args = std::env::args_os().collect::<Vec<_>>();
    let inner_size = match arg_value(&args, "--capture-size").as_deref() {
        Some(value) if value == OsStr::new("small") => [900.0, 600.0],
        _ => [1200.0, 820.0],
    };
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        depth_buffer: 24,
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(inner_size)
            .with_min_inner_size([900.0, 600.0])
            .with_decorations(false)
            .with_title_shown(false),
        ..Default::default()
    };
    eframe::run_native(
        "BluePandaChange",
        options,
        Box::new(|cc| Ok(Box::new(BluePandaApp::new(cc)))),
    )
}

fn preview_paths_from_args(args: impl IntoIterator<Item = OsString>) -> Vec<PathBuf> {
    let mut args = args.into_iter().skip(1);
    let mut paths = Vec::new();
    while let Some(argument) = args.next() {
        if argument == OsStr::new("--preview-file") {
            if let Some(path) = args.next() {
                paths.push(PathBuf::from(path));
            }
        }
    }
    paths
}

fn arg_value(args: &[OsString], flag: &str) -> Option<OsString> {
    args.windows(2)
        .find(|pair| pair[0] == OsStr::new(flag))
        .map(|pair| pair[1].clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_kinds_separate_preview_only_nbt_from_convertible_inputs() {
        assert_eq!(file_kind(Path::new("院子.LITEMATIC")), FileKind::Schematic);
        assert_eq!(
            file_kind(Path::new("Datacenter.schem")),
            FileKind::Schematic
        );
        assert_eq!(file_kind(Path::new("built.nbt")), FileKind::JavaStructure);
        assert_eq!(file_kind(Path::new("readme.txt")), FileKind::Unsupported);
    }

    #[test]
    fn startup_preview_arguments_accept_multiple_paths() {
        let paths = preview_paths_from_args([
            OsString::from("bluepanda-gui"),
            OsString::from("--preview-file"),
            OsString::from("院子.litematic"),
            OsString::from("--preview-file"),
            OsString::from("built.nbt"),
        ]);
        assert_eq!(
            paths,
            [PathBuf::from("院子.litematic"), PathBuf::from("built.nbt")]
        );
    }

    #[test]
    fn document_palette_maps_sparse_blocks_without_redecoding_coordinates() {
        let document = PreviewDocument {
            format: bluepanda_core::PreviewFormat::JavaStructure,
            origin: [0; 3],
            size: [1, 1, 1],
            palette: vec![bluepanda_core::BlockState {
                name: "minecraft:stone".into(),
                properties: Default::default(),
            }],
            regions: vec![],
            blocks: vec![bluepanda_core::PreviewBlock {
                position: [0, 0, 0],
                state: 0,
                region_id: 0,
            }],
            entities_count: 0,
            warnings: vec![],
        };
        let blocks = document_to_blocks(&document).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].position, [0, 0, 0]);
        assert_eq!(blocks[0].name, "minecraft:stone");
    }

    #[test]
    fn nbt_rows_are_never_submitted_to_conversion() {
        let paths = [
            PathBuf::from("a.litematic"),
            PathBuf::from("already.nbt"),
            PathBuf::from("b.schem"),
        ];
        let convertible: Vec<_> = paths
            .into_iter()
            .filter(|path| file_kind(path) == FileKind::Schematic)
            .collect();
        assert_eq!(
            convertible,
            [PathBuf::from("a.litematic"), PathBuf::from("b.schem")]
        );
    }

    #[test]
    fn real_downloaded_sources_and_converted_outputs_fit_preview_budget() {
        if std::env::var_os("BLUEPANDA_REAL_GUI_SAMPLE_1_SOURCE").is_none()
            && std::env::var_os("BLUEPANDA_REAL_GUI_SAMPLE_2_SOURCE").is_none()
        {
            eprintln!(
                "skip real GUI samples: set BLUEPANDA_REAL_GUI_SAMPLE_1_SOURCE/OUTPUT and/or BLUEPANDA_REAL_GUI_SAMPLE_2_SOURCE/OUTPUT"
            );
            return;
        }
        let fixtures = [1, 2]
            .into_iter()
            .filter_map(|index| {
                let source_key = format!("BLUEPANDA_REAL_GUI_SAMPLE_{index}_SOURCE");
                let output_key = format!("BLUEPANDA_REAL_GUI_SAMPLE_{index}_OUTPUT");
                match (std::env::var_os(&source_key), std::env::var_os(&output_key)) {
                    (Some(source), Some(output)) => {
                        Some((PathBuf::from(source), PathBuf::from(output)))
                    }
                    (None, None) => None,
                    _ => panic!("both {source_key} and {output_key} must be set"),
                }
            })
            .collect::<Vec<_>>();
        assert!(!fixtures.is_empty());
        for (source, output) in fixtures {
            for path in [source, output] {
                let started = Instant::now();
                let token = CancellationToken::default();
                let scene = load_preview_scene(&path, &token, |_| {})
                    .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
                let visible_regions = scene
                    .document
                    .regions
                    .iter()
                    .map(|region| region.id)
                    .collect::<std::collections::BTreeSet<_>>();
                let blocks = document_to_blocks(&scene).unwrap();
                assert!(blocks.len() <= 1_000_000);
                let top = blocks
                    .iter()
                    .map(|block| block.position[1])
                    .max()
                    .unwrap_or(0);
                for layer in [LayerView::All, LayerView::Layer(0), LayerView::Layer(top)] {
                    let mesh_started = Instant::now();
                    let mesh = build_mesh_with_progress(
                        &blocks,
                        layer,
                        &visible_regions,
                        || false,
                        |_, _| {},
                    )
                    .unwrap_or_else(|error| panic!("{} {layer:?}: {error}", path.display()));
                    assert!(mesh.face_count <= preview::MAX_PREVIEW_FACES);
                    eprintln!(
                        "GUI_MESH {} mode={layer:?} blocks={} faces={} build_ms={} total_ms={}",
                        path.display(),
                        mesh.block_count,
                        mesh.face_count,
                        mesh_started.elapsed().as_millis(),
                        started.elapsed().as_millis()
                    );
                }
            }
        }
    }
}
