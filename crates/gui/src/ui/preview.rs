use crate::preview::GlowRenderer;
use crate::state::{AppAction, CameraPreset, DisplayMode, PreviewView, SliceMode};
use crate::theme;
use eframe::egui::{self, Color32, RichText, Stroke};

#[derive(Clone, Copy, PartialEq, Eq)]
enum DragMode {
    Rotate,
    Pan,
    Zoom,
}

pub(crate) fn show(
    ui: &mut egui::Ui,
    view: &PreviewView,
    renderer: &GlowRenderer,
) -> Vec<AppAction> {
    let tokens = theme::tokens(ui.ctx());
    let mut actions = Vec::new();

    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            if ui
                .add(
                    egui::Button::new(RichText::new("‹ 文件队列").color(tokens.muted)).frame(false),
                )
                .clicked()
            {
                actions.push(AppAction::BackToWorkspace);
            }
            ui.label(RichText::new("/  3D 建筑预览").heading().color(tokens.text));
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let filename = view
                .path
                .as_ref()
                .and_then(|path| path.file_name())
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| "未选择文件".into());
            let (icon_rect, _) =
                ui.allocate_exact_size(egui::vec2(28.0, 32.0), egui::Sense::hover());
            paint_file_icon(ui.painter(), icon_rect, tokens.blue);
            ui.label(
                RichText::new(filename)
                    .size(18.0)
                    .strong()
                    .family(egui::FontFamily::Name("noto_sc_semibold".into())),
            );
            if !view.format.is_empty() {
                let extension = view
                    .path
                    .as_ref()
                    .and_then(|path| path.extension())
                    .map(|extension| format!(".{}", extension.to_string_lossy()))
                    .unwrap_or_else(|| ".nbt".into());
                let (badge_bg, badge_fg) = if ui.ctx().theme() == egui::Theme::Dark {
                    (
                        Color32::from_rgb(28, 54, 83),
                        Color32::from_rgb(151, 202, 255),
                    )
                } else {
                    (
                        Color32::from_rgb(228, 241, 255),
                        Color32::from_rgb(31, 93, 163),
                    )
                };
                egui::Frame::new()
                    .fill(badge_bg)
                    .corner_radius(egui::CornerRadius::same(4))
                    .inner_margin(egui::Margin::symmetric(6, 3))
                    .show(ui, |ui| {
                        ui.label(RichText::new(extension).size(11.0).strong().color(badge_fg))
                    });
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(path) = &view.path {
                    let start = egui::Button::new(
                        RichText::new("▶  开始转换").strong().color(Color32::WHITE),
                    )
                    .fill(tokens.blue)
                    .corner_radius(egui::CornerRadius::same(6));
                    if ui
                        .add_enabled(view.can_convert, start.min_size(egui::vec2(170.0, 44.0)))
                        .clicked()
                    {
                        actions.push(AppAction::ConvertPath(path.clone()));
                    }
                    if ui
                        .add(egui::Button::new("▱  打开文件位置").min_size(egui::vec2(142.0, 40.0)))
                        .clicked()
                    {
                        actions.push(AppAction::OpenFileLocation(path.clone()));
                    }
                }
            });
        });
        ui.add_space(6.0);

        let available = ui.available_size();
        let footer_height = 24.0;
        let content_height = (available.y - footer_height - 4.0).max(220.0);
        ui.allocate_ui_with_layout(
            egui::vec2(available.x, content_height),
            egui::Layout::left_to_right(egui::Align::Min),
            |ui| {
                let sidebar_width = (available.x * 0.235)
                    .clamp(250.0, 340.0)
                    .min(available.x * 0.42);
                ui.allocate_ui_with_layout(
                    egui::vec2(
                        (available.x - sidebar_width - 8.0).max(180.0),
                        content_height,
                    ),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| canvas(ui, view, renderer, &mut actions),
                );
                ui.add_space(8.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(sidebar_width, content_height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| details_panel(ui, view, &tokens, &mut actions),
                );
            },
        );

        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("●").color(if view.error.is_some() {
                tokens.danger
            } else {
                tokens.success
            }));
            ui.label(
                RichText::new(
                    view.loading_text
                        .as_deref()
                        .or(view.error.as_deref())
                        .unwrap_or("预览就绪"),
                )
                .color(view.error.as_ref().map_or(tokens.muted, |_| tokens.danger)),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let visible_regions = view.regions.iter().filter(|region| region.visible).count();
                let projection = if renderer.is_orthographic() {
                    "正交"
                } else {
                    "透视"
                };
                let camera = match view.camera_preset {
                    CameraPreset::Fit => "适配视图",
                    CameraPreset::Reset => "默认视角",
                    CameraPreset::Front => "前视",
                    CameraPreset::BirdEye => "俯视",
                    CameraPreset::Top => "顶视",
                    CameraPreset::Perspective => "透视视角",
                    CameraPreset::Orthographic => "正交视角",
                };
                let slice = match view.slice_mode {
                    SliceMode::CurrentLayer => format!("当前层 Y={}", view.current_layer),
                    SliceMode::CurrentAndBelow => format!("当前层及以下 Y={}", view.current_layer),
                    SliceMode::WholeStructure => "完整结构".to_owned(),
                };
                for detail in [
                    format!("可见 Region {visible_regions}/{}", view.regions.len()),
                    format!("{} × {} × {}", view.size[0], view.size[1], view.size[2]),
                    slice,
                    format!("{camera} · {projection}"),
                ] {
                    ui.label(RichText::new(detail).color(tokens.muted));
                }
            });
        });
    });
    actions
}

fn canvas(
    ui: &mut egui::Ui,
    view: &PreviewView,
    renderer: &GlowRenderer,
    actions: &mut Vec<AppAction>,
) {
    let mode = ui.ctx().data(|data| {
        data.get_temp::<DragMode>(egui::Id::new("preview_drag_mode"))
            .unwrap_or(DragMode::Rotate)
    });
    egui::Frame::new()
        .fill(Color32::from_rgb(34, 40, 49))
        .stroke(Stroke::new(1.0, egui::Color32::from_rgb(57, 66, 78)))
        .corner_radius(egui::CornerRadius::same(7))
        .inner_margin(egui::Margin::symmetric(6, 5))
        .show(ui, |ui| {
            egui::ScrollArea::horizontal()
                .id_salt("preview_toolbar_scroll")
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        tool_button(ui, DragMode::Rotate, mode, ToolIcon::Rotate);
                        tool_button(ui, DragMode::Pan, mode, ToolIcon::Pan);
                        tool_button(ui, DragMode::Zoom, mode, ToolIcon::Zoom);
                        camera_button(ui, "适配", CameraPreset::Fit, view.camera_preset, actions);
                        camera_button(ui, "重置", CameraPreset::Reset, view.camera_preset, actions);
                        ui.separator();
                        for (label, preset, icon) in [
                            ("前视", CameraPreset::Front, CameraIcon::Front),
                            ("俯视", CameraPreset::BirdEye, CameraIcon::BirdEye),
                            ("顶视", CameraPreset::Top, CameraIcon::Top),
                            ("透视", CameraPreset::Perspective, CameraIcon::Perspective),
                            ("正交", CameraPreset::Orthographic, CameraIcon::Orthographic),
                        ] {
                            camera_button_with_icon(
                                ui,
                                label,
                                preset,
                                view.camera_preset,
                                icon,
                                actions,
                            );
                        }
                        ui.separator();
                        let option_text = Color32::from_rgb(220, 227, 236);
                        let mut axes = view.axes_visible;
                        if ui
                            .checkbox(&mut axes, egui::RichText::new("坐标轴").color(option_text))
                            .changed()
                        {
                            actions.push(AppAction::SetAxesVisible(axes));
                        }
                        let mut grid = view.grid_visible;
                        if ui
                            .checkbox(
                                &mut grid,
                                egui::RichText::new("参考网格").color(option_text),
                            )
                            .changed()
                        {
                            actions.push(AppAction::SetGridVisible(grid));
                        }
                    });
                });
        });
    ui.add_space(4.0);
    let (rect, response) = ui.allocate_exact_size(
        ui.available_size().max(egui::vec2(180.0, 180.0)),
        egui::Sense::click_and_drag(),
    );
    ui.painter()
        .rect_filled(rect, 5.0, Color32::from_rgb(29, 35, 43));
    let hovered = response.hovered()
        || rect.contains(ui.input(|input| {
            input
                .pointer
                .hover_pos()
                .unwrap_or(egui::Pos2::new(-1.0, -1.0))
        }));
    if hovered {
        let delta = ui.input(|input| input.pointer.delta());
        let primary = ui.input(|input| input.pointer.button_down(egui::PointerButton::Primary));
        let secondary_pan = ui.input(|input| {
            input.pointer.button_down(egui::PointerButton::Secondary)
                || input.pointer.button_down(egui::PointerButton::Middle)
        });
        if delta != egui::Vec2::ZERO {
            if secondary_pan || (primary && mode == DragMode::Pan) {
                renderer.with_camera(|camera| camera.pan(delta, rect.height()));
            } else if primary && mode == DragMode::Zoom {
                renderer.with_camera(|camera| camera.zoom(-delta.y * 6.0));
            } else if primary {
                renderer.with_camera(|camera| camera.orbit(delta));
            }
        }
        let scroll = ui.input(|input| input.raw_scroll_delta.y);
        if scroll != 0.0 {
            renderer.with_camera(|camera| camera.zoom(scroll));
        }
    }
    renderer.draw(ui, rect);
    if let Some(message) = view.loading_text.as_deref() {
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            message,
            egui::FontId::proportional(17.0),
            Color32::WHITE,
        );
    } else if let Some(message) = view.error.as_deref() {
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            message,
            egui::FontId::proportional(15.0),
            Color32::LIGHT_RED,
        );
    } else if view.path.is_none() {
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "选择文件以加载结构预览",
            egui::FontId::proportional(17.0),
            Color32::LIGHT_GRAY,
        );
    }
}

#[derive(Clone, Copy)]
enum ToolIcon {
    Rotate,
    Pan,
    Zoom,
}

#[derive(Clone, Copy)]
enum CameraIcon {
    Fit,
    Reset,
    Front,
    BirdEye,
    Top,
    Perspective,
    Orthographic,
}

fn tool_button(ui: &mut egui::Ui, value: DragMode, active: DragMode, icon: ToolIcon) {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(48.0, 48.0), egui::Sense::click());
    paint_toolbar_button(
        ui.painter(),
        rect,
        value == active,
        match value {
            DragMode::Rotate => "旋转",
            DragMode::Pan => "平移",
            DragMode::Zoom => "缩放",
        },
    );
    paint_tool_icon(ui.painter(), rect, icon);
    if response.clicked() {
        ui.ctx()
            .data_mut(|data| data.insert_temp(egui::Id::new("preview_drag_mode"), value));
    }
}

fn camera_button(
    ui: &mut egui::Ui,
    label: &str,
    preset: CameraPreset,
    selected: CameraPreset,
    actions: &mut Vec<AppAction>,
) {
    let icon = if preset == CameraPreset::Fit {
        CameraIcon::Fit
    } else {
        CameraIcon::Reset
    };
    camera_button_with_icon(ui, label, preset, selected, icon, actions);
}

fn camera_button_with_icon(
    ui: &mut egui::Ui,
    label: &str,
    preset: CameraPreset,
    selected: CameraPreset,
    icon: CameraIcon,
    actions: &mut Vec<AppAction>,
) {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(48.0, 48.0), egui::Sense::click());
    paint_toolbar_button(ui.painter(), rect, preset == selected, label);
    paint_camera_icon(ui.painter(), rect, icon);
    if response.clicked() {
        actions.push(AppAction::SetCamera(preset));
    }
}

fn paint_toolbar_button(painter: &egui::Painter, rect: egui::Rect, active: bool, label: &str) {
    let fill = if active {
        Color32::from_rgb(29, 111, 222)
    } else {
        Color32::from_rgba_unmultiplied(255, 255, 255, 10)
    };
    painter.rect_filled(rect.shrink(1.0), 5.0, fill);
    if !active {
        painter.rect_stroke(
            rect.shrink(1.0),
            5.0,
            Stroke::new(1.0, Color32::from_rgb(72, 82, 96)),
            egui::StrokeKind::Inside,
        );
    }
    painter.text(
        egui::pos2(rect.center().x, rect.bottom() - 8.0),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(11.0),
        Color32::from_rgb(232, 238, 246),
    );
}

fn paint_tool_icon(painter: &egui::Painter, rect: egui::Rect, icon: ToolIcon) {
    let c = Color32::from_rgb(224, 233, 243);
    let s = Stroke::new(1.7, c);
    let center = egui::pos2(rect.center().x, rect.top() + 15.0);
    match icon {
        ToolIcon::Rotate => {
            let points: Vec<_> = (0..=18)
                .map(|i| {
                    let a = -2.4 + i as f32 * 4.7 / 18.0;
                    center + egui::vec2(a.cos() * 7.0, a.sin() * 6.0)
                })
                .collect();
            painter.add(egui::Shape::line(points, s));
            painter.line_segment(
                [
                    center + egui::vec2(6.0, -5.0),
                    center + egui::vec2(8.0, -1.0),
                ],
                s,
            );
        }
        ToolIcon::Pan => {
            painter.line_segment(
                [center - egui::vec2(8.0, 0.0), center + egui::vec2(8.0, 0.0)],
                s,
            );
            painter.line_segment(
                [center - egui::vec2(0.0, 8.0), center + egui::vec2(0.0, 8.0)],
                s,
            );
            for (d, v) in [
                (egui::vec2(-8.0, 0.0), egui::vec2(3.0, -3.0)),
                (egui::vec2(8.0, 0.0), egui::vec2(-3.0, -3.0)),
                (egui::vec2(0.0, -8.0), egui::vec2(-3.0, 3.0)),
                (egui::vec2(0.0, 8.0), egui::vec2(-3.0, -3.0)),
            ] {
                painter.line_segment([center + d, center + d + v], s);
                painter.line_segment([center + d, center + d + egui::vec2(-v.x, v.y)], s);
            }
        }
        ToolIcon::Zoom => {
            painter.circle_stroke(center - egui::vec2(2.0, 1.0), 5.0, s);
            painter.line_segment(
                [center + egui::vec2(2.0, 3.0), center + egui::vec2(7.0, 8.0)],
                s,
            );
            painter.line_segment([center - egui::vec2(4.0, 1.0), center], s);
            painter.line_segment(
                [
                    center - egui::vec2(2.0, 3.0),
                    center - egui::vec2(2.0, -1.0),
                ],
                s,
            );
        }
    }
}

fn paint_camera_icon(painter: &egui::Painter, rect: egui::Rect, icon: CameraIcon) {
    let c = Color32::from_rgb(224, 233, 243);
    let s = Stroke::new(1.5, c);
    let center = egui::pos2(rect.center().x, rect.top() + 15.0);
    match icon {
        CameraIcon::Fit => {
            let r = egui::Rect::from_center_size(center, egui::vec2(14.0, 12.0));
            for (p, x, y) in [
                (r.left_top(), 1.0, 1.0),
                (r.right_top(), -1.0, 1.0),
                (r.left_bottom(), 1.0, -1.0),
                (r.right_bottom(), -1.0, -1.0),
            ] {
                painter.line_segment([p, p + egui::vec2(4.0 * x, 0.0)], s);
                painter.line_segment([p, p + egui::vec2(0.0, 4.0 * y)], s);
            }
        }
        CameraIcon::Reset => {
            painter.circle_stroke(center, 7.0, s);
            painter.line_segment(
                [
                    center + egui::vec2(4.0, -6.0),
                    center + egui::vec2(8.0, -5.0),
                ],
                s,
            );
            painter.line_segment(
                [
                    center + egui::vec2(4.0, -6.0),
                    center + egui::vec2(4.0, -2.0),
                ],
                s,
            );
        }
        CameraIcon::Front => {
            painter.rect_stroke(
                egui::Rect::from_center_size(center, egui::vec2(13.0, 12.0)),
                2.0,
                s,
                egui::StrokeKind::Inside,
            );
            painter.line_segment(
                [
                    center + egui::vec2(-4.0, 0.0),
                    center + egui::vec2(4.0, 0.0),
                ],
                s,
            );
        }
        CameraIcon::BirdEye => {
            painter.line_segment(
                [
                    center + egui::vec2(-8.0, 1.0),
                    center + egui::vec2(0.0, -5.0),
                ],
                s,
            );
            painter.line_segment(
                [
                    center + egui::vec2(0.0, -5.0),
                    center + egui::vec2(8.0, 1.0),
                ],
                s,
            );
            painter.line_segment(
                [
                    center + egui::vec2(-8.0, 1.0),
                    center + egui::vec2(0.0, 6.0),
                ],
                s,
            );
            painter.line_segment(
                [center + egui::vec2(8.0, 1.0), center + egui::vec2(0.0, 6.0)],
                s,
            );
        }
        CameraIcon::Top => {
            painter.rect_stroke(
                egui::Rect::from_center_size(center, egui::vec2(14.0, 14.0)),
                1.0,
                s,
                egui::StrokeKind::Inside,
            );
            painter.circle_filled(center, 1.5, c);
        }
        CameraIcon::Perspective => {
            painter.line_segment(
                [
                    center + egui::vec2(-8.0, -5.0),
                    center + egui::vec2(8.0, -5.0),
                ],
                s,
            );
            painter.line_segment(
                [
                    center + egui::vec2(-8.0, 5.0),
                    center + egui::vec2(8.0, 5.0),
                ],
                s,
            );
            painter.line_segment(
                [
                    center + egui::vec2(-8.0, -5.0),
                    center + egui::vec2(-3.0, 5.0),
                ],
                s,
            );
            painter.line_segment(
                [
                    center + egui::vec2(8.0, -5.0),
                    center + egui::vec2(3.0, 5.0),
                ],
                s,
            );
        }
        CameraIcon::Orthographic => {
            for x in [-5.0, 0.0, 5.0] {
                painter.line_segment(
                    [center + egui::vec2(x, -6.0), center + egui::vec2(x, 6.0)],
                    s,
                );
            }
        }
    }
}

fn paint_file_icon(painter: &egui::Painter, rect: egui::Rect, color: Color32) {
    let r = egui::Rect::from_min_size(rect.min + egui::vec2(5.0, 3.0), egui::vec2(18.0, 25.0));
    painter.rect_stroke(r, 3.0, Stroke::new(1.5, color), egui::StrokeKind::Inside);
    painter.line_segment(
        [
            r.left_bottom() + egui::vec2(4.0, -7.0),
            r.right_bottom() - egui::vec2(4.0, 7.0),
        ],
        Stroke::new(1.2, color),
    );
    painter.line_segment(
        [
            r.left_bottom() + egui::vec2(4.0, -3.0),
            r.right_bottom() - egui::vec2(4.0, 3.0),
        ],
        Stroke::new(1.2, color),
    );
}

fn details_panel(
    ui: &mut egui::Ui,
    view: &PreviewView,
    tokens: &theme::Tokens,
    actions: &mut Vec<AppAction>,
) {
    ui.scope(|ui| {
      ui.spacing_mut().item_spacing.y = 3.0;
      ui.spacing_mut().button_padding = egui::vec2(4.0, 2.0);
      egui::ScrollArea::vertical()
        .id_salt("preview_details_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            panel(ui, tokens, "结构信息", |ui| {
                info_row(
                    ui,
                    "文件名称",
                    &view
                        .path
                        .as_ref()
                        .and_then(|path| path.file_name())
                        .map(|name| name.to_string_lossy().to_string())
                        .unwrap_or_else(|| "未选择".into()),
                    tokens,
                );
                info_row(ui, "文件格式", &view.format, tokens);
                info_row(
                    ui,
                    "结构尺寸",
                    &format!("{} × {} × {}", view.size[0], view.size[1], view.size[2]),
                    tokens,
                );
                info_row(ui, "总方块数", &format!("{}", view.blocks), tokens);
                info_row(ui, "方块实体", &format!("{}", view.block_entities), tokens);
                info_row(ui, "实体数量", &format!("{}", view.entities), tokens);
                info_row(
                    ui,
                    "Region 数量",
                    &format!("{}", view.regions.len()),
                    tokens,
                );
                info_row(
                    ui,
                    "DataVersion",
                    &view
                        .data_version
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "未提供".into()),
                    tokens,
                );
                info_row(
                    ui,
                    "源坐标原点",
                    &format!("({}, {}, {})", view.origin[0], view.origin[1], view.origin[2]),
                    tokens,
                );
                ui.add_space(3.0);
                ui.label(RichText::new("坐标范围").strong().color(tokens.text));
                for (axis, name) in ["X", "Y", "Z"].into_iter().enumerate() {
                    info_row(
                        ui,
                        name,
                        &format!(
                            "{} → {}",
                            view.coordinate_min[axis], view.coordinate_max[axis]
                        ),
                        tokens,
                    );
                }
            });

            panel(ui, tokens, "显示模式", |ui| {
                ui.horizontal(|ui| {
                    display_button(
                        ui,
                        "实体方块",
                        DisplayMode::Solid,
                        view.display_mode,
                        actions,
                    );
                    display_button(
                        ui,
                        "半透明",
                        DisplayMode::Translucent,
                        view.display_mode,
                        actions,
                    );
                    display_button(
                        ui,
                        "线框",
                        DisplayMode::Wireframe,
                        view.display_mode,
                        actions,
                    );
                });
            });

            panel(ui, tokens, "Region", |ui| {
                if view.regions.is_empty() {
                    ui.label(RichText::new("没有可显示的 Region").color(tokens.muted));
                }
                egui::ScrollArea::vertical()
                    .id_salt("preview_region_list_scroll")
                    .max_height(180.0)
                    .show(ui, |ui| {
                        for (index, region) in view.regions.iter().enumerate() {
                            let response = ui.horizontal(|ui| {
                                let dot_color = [
                                    Color32::from_rgb(153, 168, 187),
                                    Color32::from_rgb(96, 166, 224),
                                    Color32::from_rgb(101, 190, 135),
                                    Color32::from_rgb(225, 116, 112),
                                    Color32::from_rgb(224, 180, 107),
                                    Color32::from_rgb(166, 136, 213),
                                ][index % 6];
                                ui.label(RichText::new("●").color(dot_color));
                                ui.label(RichText::new(&region.name).color(tokens.text).strong());
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                let color = if region.visible {
                                            Color32::from_rgb(105, 181, 255)
                                        } else {
                                            tokens.muted
                                        };
                                        let (eye_rect, eye_response) = ui.allocate_exact_size(egui::vec2(28.0, 24.0), egui::Sense::click());
                                        paint_eye(ui.painter(), eye_rect, color, region.visible);
                                        if eye_response.on_hover_text(if region.visible { "隐藏此 Region" } else { "显示此 Region" }).clicked() {
                                            actions.push(AppAction::SetRegionVisible(
                                                region.id,
                                                !region.visible,
                                            ));
                                        }
                                    },
                                );
                            });
                            response.response.on_hover_text(format!(
                                "{} × {} × {} · {} 方块 · {} 实体 · 归一化位置 ({}, {}, {}) · 源位置 ({}, {}, {})",
                                region.size[0],
                                region.size[1],
                                region.size[2],
                                region.blocks,
                                region.entities,
                                region.position[0],
                                region.position[1],
                                region.position[2],
                                region.source_origin[0],
                                region.source_origin[1],
                                region.source_origin[2]
                            ));
                        }
                    });
            });

            panel(ui, tokens, "切片 / 分层查看", |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("Y 层 {} / {}", view.current_layer, view.max_layer))
                            .strong(),
                    );
                });
                let mut layer = view.current_layer.clamp(0, view.max_layer.max(0));
                if ui
                    .add(egui::Slider::new(&mut layer, 0..=view.max_layer.max(0)).show_value(false))
                    .changed()
                {
                    actions.push(AppAction::SetLayer(layer));
                }
                ui.horizontal_wrapped(|ui| {
                    slice_button(
                        ui,
                        "显示当前层",
                        SliceMode::CurrentLayer,
                        view.slice_mode,
                        actions,
                    );
                    slice_button(
                        ui,
                        "当前层及以下",
                        SliceMode::CurrentAndBelow,
                        view.slice_mode,
                        actions,
                    );
                });
                if ui
                    .add_sized(
                        [ui.available_width(), 28.0],
                        egui::Button::new("显示完整建筑"),
                    )
                    .clicked()
                {
                    actions.push(AppAction::SetSliceMode(SliceMode::WholeStructure));
                }
            });
        });
    });
}

fn panel(
    ui: &mut egui::Ui,
    tokens: &theme::Tokens,
    title: &str,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    egui::Frame::group(ui.style())
        .fill(tokens.surface)
        .stroke(Stroke::new(1.0, tokens.border))
        .corner_radius(tokens.radius)
        .inner_margin(egui::Margin::symmetric(8, 5))
        .show(ui, |ui| {
            ui.label(
                RichText::new(title)
                    .size(15.0)
                    .strong()
                    .color(tokens.text)
                    .family(egui::FontFamily::Name("noto_sc_semibold".into())),
            );
            ui.add_space(2.0);
            add_contents(ui);
        });
    ui.add_space(3.0);
}

fn info_row(ui: &mut egui::Ui, key: &str, value: &str, tokens: &theme::Tokens) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(key).size(13.0).color(tokens.muted));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add(
                egui::Label::new(RichText::new(value).size(13.0).color(tokens.text).strong())
                    .truncate(),
            )
            .on_hover_text(value);
        });
    });
}

fn paint_eye(painter: &egui::Painter, rect: egui::Rect, color: Color32, visible: bool) {
    let center = rect.center();
    let left = center + egui::vec2(-8.5, 0.0);
    let right = center + egui::vec2(8.5, 0.0);
    let stroke = Stroke::new(1.45, color);
    painter.add(egui::Shape::line(
        vec![
            left,
            center + egui::vec2(-4.0, -5.0),
            center + egui::vec2(0.0, -5.5),
            center + egui::vec2(4.0, -5.0),
            right,
        ],
        stroke,
    ));
    painter.add(egui::Shape::line(
        vec![
            left,
            center + egui::vec2(-4.0, 5.0),
            center + egui::vec2(0.0, 5.5),
            center + egui::vec2(4.0, 5.0),
            right,
        ],
        stroke,
    ));
    if visible {
        painter.circle_filled(center, 2.4, color);
    } else {
        painter.line_segment(
            [center - egui::vec2(7.5, 7.0), center + egui::vec2(7.5, 7.0)],
            Stroke::new(1.6, color),
        );
    }
}

fn display_button(
    ui: &mut egui::Ui,
    label: &str,
    value: DisplayMode,
    selected: DisplayMode,
    actions: &mut Vec<AppAction>,
) {
    if ui.selectable_label(value == selected, label).clicked() {
        actions.push(AppAction::SetDisplayMode(value));
    }
}

fn slice_button(
    ui: &mut egui::Ui,
    label: &str,
    value: SliceMode,
    selected: SliceMode,
    actions: &mut Vec<AppAction>,
) {
    if ui.selectable_label(value == selected, label).clicked() {
        actions.push(AppAction::SetSliceMode(value));
    }
}
