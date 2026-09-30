use bluepanda_core::{convert_batch, ConversionOptions, ConversionResult, ProgressEvent};
use eframe::egui;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::thread;

#[derive(Clone, Debug)]
enum RowState {
    Waiting,
    Running(String),
    Done(ConversionResult),
    Failed(String),
}
struct Row {
    path: PathBuf,
    state: RowState,
}
enum WorkerMessage {
    Progress(ProgressEvent),
    Done(Vec<(PathBuf, Result<ConversionResult, String>)>),
}

struct BluePandaApp {
    rows: Vec<Row>,
    selected: Option<usize>,
    output_dir: Option<PathBuf>,
    overwrite: bool,
    running: bool,
    rx: Receiver<WorkerMessage>,
}

impl BluePandaApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "noto_sc".into(),
            egui::FontData::from_static(include_bytes!("../assets/NotoSansSC-VF.ttf")).into(),
        );
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "noto_sc".into());
        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .push("noto_sc".into());
        cc.egui_ctx.set_fonts(fonts);
        let (_, rx) = mpsc::channel();
        Self {
            rows: Vec::new(),
            selected: None,
            output_dir: None,
            overwrite: false,
            running: false,
            rx,
        }
    }

    fn can_edit_queue(&self) -> bool {
        !self.running
    }

    fn can_start(&self) -> bool {
        self.can_edit_queue() && !self.rows.is_empty() && self.output_dir.is_some()
    }

    fn add_paths(&mut self, paths: impl IntoIterator<Item = PathBuf>) -> bool {
        if !self.can_edit_queue() {
            return false;
        }
        for path in paths {
            if !self.rows.iter().any(|row| row.path == path) {
                self.rows.push(Row {
                    path,
                    state: RowState::Waiting,
                });
            }
        }
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
        self.rows.remove(index);
        true
    }

    fn start(&mut self) {
        if !self.can_start() {
            return;
        }
        let output_dir = self
            .output_dir
            .clone()
            .expect("can_start requires output directory");
        let inputs: Vec<PathBuf> = self.rows.iter().map(|row| row.path.clone()).collect();
        let (tx, rx) = mpsc::channel();
        self.rx = rx;
        self.running = true;
        for row in &mut self.rows {
            row.state = RowState::Waiting;
        }
        let options = ConversionOptions {
            overwrite: self.overwrite,
        };
        thread::spawn(move || {
            let converted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                convert_batch(&inputs, &output_dir, &options, |event| {
                    let _ = tx.send(WorkerMessage::Progress(event));
                })
            }));
            let results = match converted {
                Ok(batch) => batch
                    .results
                    .into_iter()
                    .map(|file| (file.input, file.result.map_err(|e| e.to_string())))
                    .collect(),
                Err(_) => inputs
                    .into_iter()
                    .map(|input| (input, Err("转换线程意外终止".to_string())))
                    .collect(),
            };
            let _ = tx.send(WorkerMessage::Done(results));
        });
    }

    fn drain_worker(&mut self) {
        loop {
            let message = match self.rx.try_recv() {
                Ok(message) => message,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    if self.running {
                        for row in &mut self.rows {
                            if matches!(row.state, RowState::Waiting | RowState::Running(_)) {
                                row.state = RowState::Failed("转换线程连接中断".to_string());
                            }
                        }
                        self.running = false;
                    }
                    break;
                }
            };
            match message {
                WorkerMessage::Progress(event) => {
                    if let Some(row) = self.rows.iter_mut().find(|row| row.path == event.input) {
                        row.state = RowState::Running(format!("{:?}", event.stage));
                    }
                }
                WorkerMessage::Done(results) => {
                    for (input, result) in results {
                        if let Some(row) = self.rows.iter_mut().find(|row| row.path == input) {
                            row.state = match result {
                                Ok(result) => RowState::Done(result),
                                Err(error) => RowState::Failed(error),
                            };
                        }
                    }
                    self.running = false;
                }
            }
        }
    }
}

impl eframe::App for BluePandaApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_worker();
        if self.running {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("BluePandaChange 蓝图转换");
            ui.label("将 .litematic 文件转换为 Structure .nbt");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(self.can_edit_queue(), egui::Button::new("添加文件…"))
                    .clicked()
                {
                    if let Some(paths) = rfd::FileDialog::new()
                        .add_filter("Litematica 蓝图", &["litematic"])
                        .pick_files()
                    {
                        self.add_paths(paths);
                    }
                }
                if ui
                    .add_enabled(
                        self.can_edit_queue() && self.selected.is_some(),
                        egui::Button::new("移除选中"),
                    )
                    .clicked()
                {
                    self.remove_selected();
                }
                if ui
                    .add_enabled(self.can_edit_queue(), egui::Button::new("选择输出目录…"))
                    .clicked()
                {
                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                        self.output_dir = Some(path);
                    }
                }
            });
            ui.label(format!(
                "输出目录：{}",
                self.output_dir
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "未选择".into())
            ));
            ui.add_enabled_ui(self.can_edit_queue(), |ui| {
                ui.checkbox(&mut self.overwrite, "覆盖已有文件");
            });
            ui.separator();
            egui::ScrollArea::vertical()
                .max_height(320.0)
                .show(ui, |ui| {
                    for (index, row) in self.rows.iter().enumerate() {
                        let status = match &row.state {
                            RowState::Waiting => "等待中".to_string(),
                            RowState::Running(stage) => format!("转换中：{stage}"),
                            RowState::Done(result) => format!(
                                "完成：{} 方块，{} 实体 → {}",
                                result.blocks,
                                result.entities,
                                result.output.display()
                            ),
                            RowState::Failed(error) => format!("失败：{error}"),
                        };
                        let selected = self.selected == Some(index);
                        let response = ui.selectable_label(
                            selected,
                            format!("{}\n  {}", row.path.display(), status),
                        );
                        if response.clicked() && !self.running {
                            self.selected = Some(index);
                        }
                    }
                });
            let ready = self.can_start();
            if ui
                .add_enabled(
                    ready,
                    egui::Button::new(if self.running {
                        "正在转换…"
                    } else {
                        "开始转换"
                    }),
                )
                .clicked()
            {
                self.start();
            }
        });
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([720.0, 540.0])
            .with_min_inner_size([560.0, 400.0]),
        ..Default::default()
    };
    eframe::run_native(
        "BluePandaChange",
        options,
        Box::new(|cc| Ok(Box::new(BluePandaApp::new(cc)))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use fastnbt::Value;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::collections::HashMap;
    use std::fs::{self, File};
    use std::time::{Duration, Instant};

    fn app_with_receiver(rx: Receiver<WorkerMessage>) -> BluePandaApp {
        BluePandaApp {
            rows: Vec::new(),
            selected: None,
            output_dir: None,
            overwrite: false,
            running: false,
            rx,
        }
    }

    fn compound(entries: impl IntoIterator<Item = (String, Value)>) -> Value {
        Value::Compound(entries.into_iter().collect::<HashMap<_, _>>())
    }

    fn sample_file(path: &std::path::Path) {
        let state = compound([("Name".into(), Value::String("minecraft:air".into()))]);
        let vec3 = |x, y, z| {
            compound([
                ("x".into(), Value::Int(x)),
                ("y".into(), Value::Int(y)),
                ("z".into(), Value::Int(z)),
            ])
        };
        let region = compound([
            ("Position".into(), vec3(0, 0, 0)),
            ("Size".into(), vec3(1, 1, 1)),
            ("BlockStatePalette".into(), Value::List(vec![state])),
            (
                "BlockStates".into(),
                Value::LongArray(fastnbt::LongArray::new(vec![0])),
            ),
        ]);
        let root = compound([
            ("Version".into(), Value::Int(6)),
            ("SubVersion".into(), Value::Int(1)),
            ("Regions".into(), compound([("GUI test".into(), region)])),
        ]);
        let mut gzip = GzEncoder::new(File::create(path).unwrap(), Compression::default());
        fastnbt::to_writer(&mut gzip, &root).unwrap();
        gzip.finish().unwrap();
    }

    fn wait_for_worker(app: &mut BluePandaApp) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.running && Instant::now() < deadline {
            app.drain_worker();
            std::thread::sleep(Duration::from_millis(2));
        }
        app.drain_worker();
        assert!(!app.running, "worker did not finish before test deadline");
    }

    #[test]
    fn start_requires_input_and_output_and_queue_guard_works_while_running() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("蓝图.litematic");
        sample_file(&input);
        let (_tx, rx) = mpsc::channel();
        let mut app = app_with_receiver(rx);

        app.start();
        assert!(!app.running, "empty queue must not start");
        app.rows.push(Row {
            path: input.clone(),
            state: RowState::Waiting,
        });
        app.start();
        assert!(!app.running, "missing output directory must not start");
        assert!(app.can_edit_queue());

        app.output_dir = Some(temp.path().join("输出"));
        app.start();
        assert!(app.running);
        assert!(!app.can_edit_queue());
        assert!(!app.can_start());
        app.selected = Some(0);
        assert!(
            !app.remove_selected(),
            "queue removal must be guarded while running"
        );
        assert!(
            !app.add_paths([temp.path().join("later.litematic")]),
            "adding paths must be guarded while running"
        );
        assert_eq!(app.rows.len(), 1);

        wait_for_worker(&mut app);
        assert!(
            app.can_edit_queue(),
            "queue controls should recover after worker completion"
        );
        assert!(app.can_start());
    }

    #[test]
    fn real_background_batch_maps_success_and_failure_rows_and_unlocks_queue() {
        let temp = tempfile::tempdir().unwrap();
        let good = temp.path().join("有效 蓝图.litematic");
        let broken = temp.path().join("损坏 蓝图.litematic");
        let output = temp.path().join("输出目录");
        sample_file(&good);
        fs::write(&broken, b"not gzip").unwrap();
        let (_tx, rx) = mpsc::channel();
        let mut app = app_with_receiver(rx);
        app.output_dir = Some(output.clone());
        app.rows = vec![
            Row {
                path: broken.clone(),
                state: RowState::Waiting,
            },
            Row {
                path: good.clone(),
                state: RowState::Waiting,
            },
        ];

        app.start();
        assert!(app.running);
        assert!(!app.can_edit_queue());
        wait_for_worker(&mut app);

        assert!(matches!(&app.rows[0].state, RowState::Failed(_)));
        match &app.rows[1].state {
            RowState::Done(result) => assert_eq!(result.input, good),
            other => panic!("expected success on the matching row, got {other:?}"),
        }
        assert!(output.join("有效 蓝图.litematic.nbt").exists());
        assert!(app.can_edit_queue());
    }

    #[test]
    fn done_messages_match_rows_by_input_even_when_arrival_order_differs() {
        let (tx, rx) = mpsc::channel();
        let a = PathBuf::from("a.litematic");
        let b = PathBuf::from("b.litematic");
        let mut app = app_with_receiver(rx);
        app.running = true;
        app.rows = vec![
            Row {
                path: a.clone(),
                state: RowState::Running("Write".into()),
            },
            Row {
                path: b.clone(),
                state: RowState::Running("Write".into()),
            },
        ];
        let success = ConversionResult {
            input: a.clone(),
            output: PathBuf::from("a.nbt"),
            blocks: 2,
            entities: 0,
            warnings: Vec::new(),
        };
        tx.send(WorkerMessage::Done(vec![
            (b, Err("broken input".into())),
            (a.clone(), Ok(success)),
        ]))
        .unwrap();
        app.drain_worker();
        assert!(!app.running);
        assert!(matches!(&app.rows[0].state, RowState::Done(result) if result.input == a));
        assert!(
            matches!(&app.rows[1].state, RowState::Failed(message) if message == "broken input")
        );
    }

    #[test]
    fn embedded_font_contains_common_simplified_chinese_ui_glyphs() {
        let bytes = include_bytes!("../assets/NotoSansSC-VF.ttf");
        let face = ttf_parser::Face::parse(bytes, 0).expect("embedded font must parse");
        for character in "蓝图转换添加文件选择输出目录覆盖正在转换完成失败".chars()
        {
            assert!(
                face.glyph_index(character).is_some(),
                "embedded font is missing glyph {character}"
            );
        }
    }
}
