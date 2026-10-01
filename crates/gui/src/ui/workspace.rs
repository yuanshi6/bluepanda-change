use super::{IconGlyph, icon, icon_button};
use crate::state::{AppAction, ConversionDisplay, WorkspaceRowView, WorkspaceView};
use crate::theme;
use eframe::egui::{self, Color32, RichText, Stroke, Vec2};

pub(crate) fn show(ui: &mut egui::Ui, view: &WorkspaceView) -> Vec<AppAction> {
    let tokens = theme::tokens(ui.ctx());
    let mut actions = Vec::new();

    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            let (glyph, color) = title_icon(view, &tokens);
            if view.conversion_running
                || view
                    .selected
                    .and_then(|index| view.rows.get(index))
                    .is_some_and(|row| matches!(row.conversion, ConversionDisplay::Failed { .. }))
                || all_convertible_done(view)
            {
                icon(ui, glyph, color, 25.0);
            } else {
                cube_icon(ui, color, 25.0);
            }
            ui.heading(
                RichText::new(workspace_title(view))
                    .strong()
                    .color(tokens.text),
            );
        });
        ui.add_space(10.0);

        controls(ui, view, &tokens, &mut actions);
        ui.add_space(8.0);

        if let Some(notice) = &view.notice {
            ui.label(RichText::new(notice).color(tokens.warning));
            ui.add_space(4.0);
        }

        let body_size = ui.available_size();
        let details_width = 286.0_f32.min((body_size.x * 0.32).max(250.0));
        let table_width = (body_size.x - details_width - ui.spacing().item_spacing.x).max(300.0);
        let body_height = (body_size.y - 72.0).max(180.0);
        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(
                Vec2::new(table_width, body_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::Frame::group(ui.style())
                        .fill(tokens.surface)
                        .stroke(Stroke::new(1.0, tokens.border))
                        .corner_radius(tokens.radius)
                        .show(ui, |ui| {
                            ui.set_min_size(Vec2::new(table_width - 18.0, body_height - 14.0));
                            ui.set_max_size(Vec2::new(table_width - 18.0, body_height - 14.0));
                            queue_table(ui, view, &tokens, &mut actions);
                        });
                },
            );
            ui.allocate_ui_with_layout(
                Vec2::new(details_width, body_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::Frame::group(ui.style())
                        .fill(tokens.surface)
                        .stroke(Stroke::new(1.0, tokens.border))
                        .corner_radius(tokens.radius)
                        .show(ui, |ui| {
                            ui.set_min_size(Vec2::new(details_width - 18.0, body_height - 14.0));
                            ui.set_max_size(Vec2::new(details_width - 18.0, body_height - 14.0));
                            selected_details(ui, view, &tokens, &mut actions);
                        });
                },
            );
        });

        ui.add_space(4.0);
        footer(ui, view, &tokens, &mut actions);
    });
    actions
}

fn workspace_title(view: &WorkspaceView) -> &'static str {
    if view
        .selected
        .and_then(|index| view.rows.get(index))
        .is_some_and(|row| matches!(row.conversion, ConversionDisplay::Failed { .. }))
    {
        "文件处理详情"
    } else if view.conversion_running {
        "批量任务"
    } else if all_convertible_done(view) {
        "转换完成"
    } else {
        "转换工作台"
    }
}

fn all_convertible_done(view: &WorkspaceView) -> bool {
    view.rows.iter().any(|row| row.extension != "nbt")
        && view
            .rows
            .iter()
            .filter(|row| row.extension != "nbt")
            .all(|row| matches!(row.conversion, ConversionDisplay::Done { .. }))
}

fn title_icon(view: &WorkspaceView, tokens: &theme::Tokens) -> (IconGlyph, Color32) {
    if view
        .selected
        .and_then(|index| view.rows.get(index))
        .is_some_and(|row| matches!(row.conversion, ConversionDisplay::Failed { .. }))
    {
        (IconGlyph::Error, tokens.danger)
    } else if view.conversion_running {
        (IconGlyph::Clock, tokens.blue)
    } else if all_convertible_done(view) {
        (IconGlyph::Check, tokens.success)
    } else {
        (IconGlyph::File, tokens.blue)
    }
}

fn cube_icon(ui: &mut egui::Ui, color: Color32, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
    let center = rect.center();
    let half = size * 0.36;
    let top = egui::pos2(center.x, center.y - half);
    let left = egui::pos2(center.x - half, center.y - half * 0.45);
    let right = egui::pos2(center.x + half, center.y - half * 0.45);
    let middle = egui::pos2(center.x, center.y + half * 0.1);
    let low_left = egui::pos2(left.x, center.y + half * 0.65);
    let low_right = egui::pos2(right.x, center.y + half * 0.65);
    let bottom = egui::pos2(center.x, center.y + half);
    let stroke = Stroke::new(1.8, color);
    let painter = ui.painter();
    painter.line_segment([top, left], stroke);
    painter.line_segment([top, right], stroke);
    painter.line_segment([left, middle], stroke);
    painter.line_segment([right, middle], stroke);
    painter.line_segment([middle, bottom], stroke);
    painter.line_segment([left, low_left], stroke);
    painter.line_segment([right, low_right], stroke);
    painter.line_segment([low_left, bottom], stroke);
    painter.line_segment([low_right, bottom], stroke);
}

fn selection_fill(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(30, 63, 100)
    } else {
        Color32::from_rgb(0xE7, 0xF2, 0xFF)
    }
}

fn extension_badge_fill(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(30, 63, 100)
    } else {
        Color32::from_rgb(0xE3, 0xF0, 0xFF)
    }
}

fn icon_text_button(
    ui: &mut egui::Ui,
    glyph: IconGlyph,
    label: &str,
    enabled: bool,
    tokens: &theme::Tokens,
) -> bool {
    icon_text_button_with_tooltip(ui, glyph, label, enabled, tokens, None)
}

fn icon_text_button_with_tooltip(
    ui: &mut egui::Ui,
    glyph: IconGlyph,
    label: &str,
    enabled: bool,
    tokens: &theme::Tokens,
    tooltip: Option<&str>,
) -> bool {
    let mut clicked = false;
    ui.add_enabled_ui(enabled, |ui| {
        egui::Frame::new()
            .fill(tokens.surface)
            .stroke(Stroke::new(1.0, tokens.border))
            .corner_radius(tokens.radius)
            .inner_margin(egui::Margin::symmetric(8, 4))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    icon(ui, glyph, tokens.blue, 17.0);
                    let response = ui.add(egui::Button::new(label).frame(false));
                    let response = if let Some(tooltip) = tooltip {
                        response.on_hover_text(tooltip)
                    } else {
                        response
                    };
                    clicked = response.clicked();
                });
            });
    });
    clicked
}

fn primary_icon_button(
    ui: &mut egui::Ui,
    glyph: IconGlyph,
    label: &str,
    enabled: bool,
    tokens: &theme::Tokens,
) -> bool {
    let size = Vec2::new(150.0, 40.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let response = response.on_hover_text(label);
    let fill = if !enabled {
        tokens.blue.linear_multiply(0.45)
    } else if response.hovered() {
        tokens.blue.linear_multiply(0.88)
    } else {
        tokens.blue
    };
    ui.painter().rect_filled(rect, tokens.radius, fill);
    let color = Color32::WHITE;
    let center_y = rect.center().y;
    let x = rect.left() + 31.0;
    if matches!(glyph, IconGlyph::Play) {
        ui.painter().add(egui::Shape::convex_polygon(
            vec![
                egui::pos2(x - 5.0, center_y - 7.0),
                egui::pos2(x + 7.0, center_y),
                egui::pos2(x - 5.0, center_y + 7.0),
            ],
            color,
            Stroke::NONE,
        ));
    }
    ui.painter().text(
        egui::pos2(rect.center().x + 9.0, center_y),
        egui::Align2::CENTER_CENTER,
        label,
        egui::TextStyle::Button.resolve(ui.style()),
        color,
    );
    response.clicked() && enabled
}

fn controls(
    ui: &mut egui::Ui,
    view: &WorkspaceView,
    tokens: &theme::Tokens,
    actions: &mut Vec<AppAction>,
) {
    ui.horizontal(|ui| {
        if icon_text_button(ui, IconGlyph::Add, "添加文件", view.queue_editable, tokens) {
            actions.push(AppAction::AddFiles);
        }
        let removable = !view.checked.is_empty() || view.selected.is_some();
        if icon_text_button(
            ui,
            IconGlyph::Trash,
            "移除选中",
            view.queue_editable && removable,
            tokens,
        ) {
            actions.push(AppAction::RemoveSelected);
        }
        if icon_text_button(
            ui,
            IconGlyph::Folder,
            "选择输出目录",
            view.queue_editable,
            tokens,
        ) {
            actions.push(AppAction::ChooseOutputDirectory);
        }
    });
    ui.add_space(6.0);
    ui.label(RichText::new("输出目录").color(tokens.muted));
    ui.horizontal(|ui| {
        let directory = view
            .output_dir
            .as_deref()
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        let dir_width = (ui.available_width() - 410.0).max(100.0);
        let mut path_display = directory;
        egui::Frame::new()
            .fill(tokens.surface)
            .stroke(Stroke::new(1.0, tokens.border))
            .corner_radius(tokens.radius)
            .inner_margin(egui::Margin::symmetric(6, 2))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut path_display)
                            .desired_width(dir_width)
                            .hint_text("选择输出目录")
                            .interactive(false)
                            .text_color(if view.conversion_running {
                                tokens.muted
                            } else {
                                tokens.text
                            })
                            .frame(false),
                    );
                    ui.add_enabled_ui(view.queue_editable, |ui| {
                        let response = icon_button(ui, IconGlyph::Folder, tokens.muted, 22.0);
                        if response.clicked() {
                            actions.push(AppAction::ChooseOutputDirectory);
                        }
                    });
                });
            });
        ui.label("覆盖已有文件");
        ui.add_enabled_ui(view.queue_editable, |ui| {
            let (rect, response) =
                ui.allocate_exact_size(Vec2::new(42.0, 24.0), egui::Sense::click());
            if response.clicked() {
                actions.push(AppAction::SetOverwrite(!view.overwrite));
            }
            let track = if view.overwrite {
                tokens.blue
            } else {
                tokens.border
            };
            ui.painter().rect_filled(rect, 12.0, track);
            let knob_x = if view.overwrite {
                rect.right() - 12.0
            } else {
                rect.left() + 12.0
            };
            ui.painter()
                .circle_filled(egui::pos2(knob_x, rect.center().y), 9.0, Color32::WHITE);
        });
        let can_convert = view.queue_editable
            && view.output_dir.is_some()
            && view.rows.iter().any(|row| row.extension != "nbt");
        let convert_label = if view.conversion_running {
            "正在转换…"
        } else {
            "开始转换"
        };
        if primary_icon_button(
            ui,
            IconGlyph::Play,
            convert_label,
            can_convert && !view.conversion_running,
            tokens,
        ) {
            actions.push(AppAction::ConvertBatch);
        }
    });
}

fn queue_table(
    ui: &mut egui::Ui,
    view: &WorkspaceView,
    tokens: &theme::Tokens,
    actions: &mut Vec<AppAction>,
) {
    let mut toggle_all = !view.rows.is_empty() && view.rows.iter().all(|row| row.checked);
    let header_rect =
        egui::Rect::from_min_size(ui.cursor().min, Vec2::new(ui.available_width(), 32.0));
    ui.painter()
        .rect_filled(header_rect, 0.0, tokens.surface_alt);
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                view.queue_editable,
                egui::Checkbox::without_text(&mut toggle_all),
            )
            .clicked()
        {
            actions.push(AppAction::ToggleAllChecked(toggle_all));
        }
        ui.add_space(18.0);
        let name_width = (ui.available_width() - 84.0 - 86.0 - 132.0).max(160.0);
        ui.allocate_ui_with_layout(
            Vec2::new(name_width, 22.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.set_min_width(name_width);
                ui.set_max_width(name_width);
                ui.add(egui::Label::new(RichText::new("文件名").strong()).halign(egui::Align::Min));
            },
        );
        ui.add_sized(
            [84.0, 22.0],
            egui::Label::new(RichText::new("格式").strong()),
        );
        ui.add_sized(
            [86.0, 22.0],
            egui::Label::new(RichText::new("大小").strong()),
        );
        ui.label(RichText::new("状态").strong());
    });
    ui.separator();
    egui::ScrollArea::vertical()
        .id_salt("workspace_queue_rows")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if view.rows.is_empty() {
                ui.vertical_centered(|ui| {
                    ui.add_space(24.0);
                    icon(ui, IconGlyph::File, tokens.muted, 46.0);
                    ui.label(
                        RichText::new("添加或拖入 .litematic、.schem 文件开始转换")
                            .color(tokens.muted),
                    );
                    ui.label(
                        RichText::new("Java Structure .nbt 可加入队列预览")
                            .color(tokens.muted)
                            .small(),
                    );
                });
                return;
            }
            for (index, row) in view.rows.iter().enumerate() {
                queue_row(ui, row, index, view, tokens, actions);
            }
        });
}

fn queue_row(
    ui: &mut egui::Ui,
    row: &WorkspaceRowView,
    index: usize,
    view: &WorkspaceView,
    tokens: &theme::Tokens,
    actions: &mut Vec<AppAction>,
) {
    let selected = view.selected == Some(index);
    let hit_rect =
        egui::Rect::from_min_size(ui.cursor().min, Vec2::new(ui.available_width(), 38.0));
    // Leave the checkbox outside this hit target so toggling it never changes the details row.
    let row_response = ui.interact(
        egui::Rect::from_min_max(
            egui::pos2(hit_rect.left() + 34.0, hit_rect.top()),
            hit_rect.right_bottom(),
        ),
        ui.id().with(("workspace_row", index)),
        egui::Sense::click(),
    );
    let bg = if selected {
        selection_fill(ui)
    } else if row_response.hovered() {
        selection_fill(ui)
    } else if index % 2 == 1 {
        tokens.surface_alt
    } else {
        tokens.surface
    };
    let extension_color = if ui.visuals().dark_mode {
        Color32::from_rgb(150, 195, 255)
    } else {
        tokens.blue
    };
    let row_frame = egui::Frame::new()
        .fill(bg)
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let mut checked = row.checked;
                let checkbox = ui.add_enabled(
                    view.queue_editable,
                    egui::Checkbox::without_text(&mut checked),
                );
                if checkbox.changed() {
                    actions.push(AppAction::ToggleChecked(index, checked));
                }
                icon(ui, IconGlyph::File, tokens.blue, 18.0);
                let name_width = (ui.available_width() - 84.0 - 86.0 - 132.0).max(160.0);
                ui.allocate_ui_with_layout(
                    Vec2::new(name_width, 30.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.set_min_width(name_width);
                        ui.set_max_width(name_width);
                        ui.add(
                            egui::Label::new(RichText::new(&row.name).strong())
                                .truncate()
                                .halign(egui::Align::Min)
                                .sense(egui::Sense::hover()),
                        )
                    },
                )
                .inner;
                egui::Frame::new()
                    .fill(extension_badge_fill(ui))
                    .corner_radius(egui::CornerRadius::same(4))
                    .inner_margin(egui::Margin::symmetric(5, 2))
                    .show(ui, |ui| {
                        ui.add_sized(
                            [74.0, 20.0],
                            egui::Label::new(
                                RichText::new(format!(".{}", row.extension))
                                    .color(extension_color)
                                    .small(),
                            ),
                        );
                    });
                ui.add_sized(
                    [86.0, 24.0],
                    egui::Label::new(
                        RichText::new(format_size(row.size))
                            .color(tokens.muted)
                            .small(),
                    ),
                );
                status_chip(ui, &row.conversion, tokens);
            });
        });
    if row_response.clicked() {
        actions.push(AppAction::SelectRow(index));
    }
    row_response.context_menu(|ui| {
        if ui.button("预览建筑").clicked() {
            actions.push(AppAction::OpenPreview(row.path.clone()));
            ui.close_menu();
        }
        if ui
            .add_enabled(
                view.queue_editable && !view.conversion_running && row.extension != "nbt",
                egui::Button::new("单个转换"),
            )
            .clicked()
        {
            actions.push(AppAction::ConvertPath(row.path.clone()));
            ui.close_menu();
        }
        if ui.button("打开文件位置").clicked() {
            actions.push(AppAction::OpenFileLocation(row.path.clone()));
            ui.close_menu();
        }
    });
    ui.painter().hline(
        row_frame.response.rect.x_range(),
        row_frame.response.rect.bottom(),
        Stroke::new(1.0, tokens.border.linear_multiply(0.55)),
    );
}

fn status_chip(ui: &mut egui::Ui, status: &ConversionDisplay, tokens: &theme::Tokens) {
    let (glyph, label, color): (IconGlyph, String, Color32) = match status {
        ConversionDisplay::NotApplicable => (IconGlyph::Eye, "仅预览".into(), tokens.muted),
        ConversionDisplay::Waiting => (IconGlyph::Clock, "等待中".into(), tokens.muted),
        ConversionDisplay::Running {
            stage,
            completed,
            total,
            ..
        } => {
            let label = if *total > 1 {
                format!(
                    "{} {}%",
                    stage_label(stage),
                    (completed.saturating_mul(100) / total).min(100)
                )
            } else {
                format!("{}…", stage_label(stage))
            };
            (IconGlyph::Clock, label, tokens.blue)
        }
        ConversionDisplay::Done { .. } => (IconGlyph::Check, "已完成".into(), tokens.success),
        ConversionDisplay::Failed { .. } => (IconGlyph::Error, "失败".into(), tokens.danger),
    };
    ui.horizontal(|ui| {
        icon(ui, glyph, color, 16.0);
        ui.label(RichText::new(label).color(color).small().strong());
    });
}

fn selected_details(
    ui: &mut egui::Ui,
    view: &WorkspaceView,
    tokens: &theme::Tokens,
    actions: &mut Vec<AppAction>,
) {
    let selected = view.selected.and_then(|index| view.rows.get(index));
    let Some(row) = selected else {
        ui.vertical_centered(|ui| {
            ui.add_space(42.0);
            icon(ui, IconGlyph::File, tokens.muted, 54.0);
            ui.add_space(10.0);
            ui.label(RichText::new("未选择文件").strong().color(tokens.text));
            ui.label(RichText::new("从列表中选择一个文件").color(tokens.muted));
            ui.label(RichText::new("查看详细信息").color(tokens.muted));
        });
        return;
    };
    ui.horizontal(|ui| {
        ui.heading(RichText::new("文件处理详情").size(16.0).strong());
        ui.label(RichText::new(&row.name).color(tokens.muted));
    });
    ui.separator();
    match &row.conversion {
        ConversionDisplay::Done {
            output,
            blocks,
            entities,
            warnings,
            elapsed_seconds,
            logs,
            output_bytes,
        } => {
            egui::ScrollArea::vertical().show(ui, |ui| {
                alert_card(
                    ui,
                    "转换完成",
                    tokens.success,
                    IconGlyph::Check,
                    "文件已写入并通过回读校验。",
                    tokens,
                );
                if detail_tabs(ui, view, "结果详情", tokens) {
                    logs_list(ui, logs, tokens);
                } else {
                    ui.horizontal_wrapped(|ui| {
                        info(ui, "方块数", &blocks.to_string(), tokens);
                        info(ui, "实体数", &entities.to_string(), tokens);
                        info(ui, "耗时", &format_duration(*elapsed_seconds), tokens);
                    });
                    if let Some(bytes) = output_bytes {
                        info(ui, "压缩输出", &format_size(Some(*bytes)), tokens);
                    }
                    ui.label(RichText::new("输出文件").strong());
                    egui::Frame::new()
                        .fill(tokens.surface_alt)
                        .stroke(Stroke::new(1.0, tokens.border))
                        .corner_radius(tokens.radius)
                        .inner_margin(egui::Margin::symmetric(6, 4))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let path = output.display().to_string();
                                let label = ui.add_sized(
                                    [ui.available_width() - 30.0, 24.0],
                                    egui::Label::new(
                                        RichText::new(path.clone())
                                            .font(egui::FontId::proportional(13.0)),
                                    )
                                    .selectable(true)
                                    .truncate()
                                    .halign(egui::Align::Min),
                                );
                                label.on_hover_text(path.clone());
                                let copy = icon_button(ui, IconGlyph::Copy, tokens.muted, 20.0);
                                if copy.clicked() {
                                    ui.ctx().copy_text(path);
                                }
                            });
                        });
                    if icon_text_button(ui, IconGlyph::Folder, "在文件夹中显示", true, tokens)
                    {
                        actions.push(AppAction::OpenFileLocation(output.clone()));
                    }
                    warnings_list(ui, warnings, tokens);
                }
            });
        }
        ConversionDisplay::Failed {
            stage,
            message,
            region,
            position,
            warnings,
            elapsed_seconds,
            logs,
        } => {
            egui::ScrollArea::vertical().show(ui, |ui| {
                alert_card(
                    ui,
                    "转换失败",
                    tokens.danger,
                    IconGlyph::Error,
                    "此文件处理失败，其他队列文件会继续处理。",
                    tokens,
                );
                ui.horizontal_wrapped(|ui| {
                    if let Some(stage) = stage {
                        info(ui, "处理阶段", stage_label(stage), tokens);
                    }
                    info(ui, "耗时", &format_duration(*elapsed_seconds), tokens);
                });
                if detail_tabs(ui, view, "错误详情", tokens) {
                    logs_list(ui, logs, tokens);
                } else {
                    ui.label(RichText::new("错误详情").strong().color(tokens.danger));
                    ui.label(message);
                    if let Some(region) = region {
                        info(ui, "区域", region, tokens);
                    }
                    if let Some([x, y, z]) = position {
                        info(ui, "坐标", &format!("({x}, {y}, {z})"), tokens);
                    }
                    warnings_list(ui, warnings, tokens);
                }
                ui.horizontal(|ui| {
                    if ui.button("打开源文件位置").clicked() {
                        actions.push(AppAction::OpenFileLocation(row.path.clone()));
                    }
                    if view.conversion_running
                        || view
                            .rows
                            .iter()
                            .any(|entry| matches!(entry.conversion, ConversionDisplay::Waiting))
                    {
                        if icon_text_button_with_tooltip(
                            ui,
                            IconGlyph::Play,
                            "继续处理其余文件",
                            true,
                            tokens,
                            view.conversion_running
                                .then_some("批次正在自动继续；点击可返回批次进度。"),
                        ) {
                            actions.push(AppAction::ContinueWaiting);
                        }
                    }
                });
            });
        }
        ConversionDisplay::Running {
            stage,
            completed,
            total,
            elapsed_seconds,
            logs,
        } => {
            egui::ScrollArea::vertical().show(ui, |ui| {
                alert_card(
                    ui,
                    "正在转换",
                    tokens.blue,
                    IconGlyph::Clock,
                    &format!("{}…", stage_label(stage)),
                    tokens,
                );
                ui.vertical(|ui| {
                    info(ui, "当前阶段", stage_label(stage), tokens);
                    info(ui, "已用时间", &format_duration(*elapsed_seconds), tokens);
                    if *total > 1 {
                        info(
                            ui,
                            "阶段进度",
                            &format!("{}%", completed.saturating_mul(100) / total),
                            tokens,
                        );
                    }
                });
                logs_list(ui, logs, tokens);
            });
        }
        ConversionDisplay::Waiting | ConversionDisplay::NotApplicable => {
            ui.vertical(|ui| {
                ui.label(RichText::new("源文件").color(tokens.muted));
                ui.add_sized(
                    [ui.available_width(), 26.0],
                    egui::Label::new(
                        RichText::new(row.path.display().to_string())
                            .font(egui::FontId::proportional(13.0)),
                    )
                    .truncate()
                    .halign(egui::Align::Min)
                    .selectable(true),
                );
                info(
                    ui,
                    "文件类型",
                    if row.extension == "nbt" {
                        "Java Structure（仅预览）"
                    } else {
                        "可转换蓝图"
                    },
                    tokens,
                );
                if icon_text_button(ui, IconGlyph::Folder, "打开文件位置", true, tokens) {
                    actions.push(AppAction::OpenFileLocation(row.path.clone()));
                }
            });
        }
    }
}

fn footer(
    ui: &mut egui::Ui,
    view: &WorkspaceView,
    tokens: &theme::Tokens,
    actions: &mut Vec<AppAction>,
) {
    if let Some(progress) = &view.progress {
        let has_waiting = view
            .rows
            .iter()
            .any(|row| matches!(row.conversion, ConversionDisplay::Waiting));
        ui.horizontal(|ui| {
            let is_done =
                progress.total_files > 0 && progress.completed_files >= progress.total_files;
            icon(
                ui,
                if is_done {
                    IconGlyph::Check
                } else {
                    IconGlyph::Clock
                },
                if is_done { tokens.success } else { tokens.blue },
                16.0,
            );
            ui.label(
                RichText::new(if is_done {
                    "批次已完成"
                } else {
                    "正在处理"
                })
                .color(if is_done { tokens.success } else { tokens.blue }),
            );
            ui.label(
                RichText::new(format!(
                    "{}/{} 已处理",
                    progress.completed_files, progress.total_files
                ))
                .color(tokens.muted),
            );
            let fraction = if progress.total_files == 0 {
                0.0
            } else {
                (progress.completed_files as f32 / progress.total_files as f32).clamp(0.0, 1.0)
            };
            let all_converted = view
                .rows
                .iter()
                .filter(|row| row.extension != "nbt")
                .all(|row| matches!(row.conversion, ConversionDisplay::Done { .. }));
            let fill = if is_done && all_converted {
                tokens.success
            } else {
                tokens.blue
            };
            let bar_width = (ui.available_width() - 330.0).max(90.0);
            ui.add_sized(
                [bar_width, 16.0],
                egui::ProgressBar::new(fraction).fill(fill),
            );
            ui.label(format!("{}%", (fraction * 100.0).round() as u32));
            if let Some(eta) = progress.estimated_remaining_seconds {
                ui.label(
                    RichText::new(format!("预计剩余 {}", format_duration(eta))).color(tokens.muted),
                );
            }
            if view.conversion_running || has_waiting {
                if icon_text_button_with_tooltip(
                    ui,
                    IconGlyph::Play,
                    "继续处理其余文件",
                    true,
                    tokens,
                    view.conversion_running
                        .then_some("批次正在自动继续；点击可返回批次进度。"),
                ) {
                    actions.push(AppAction::ContinueWaiting);
                }
            }
        });
    } else {
        ui.horizontal(|ui| {
            ui.horizontal(|ui| {
                icon(
                    ui,
                    if view.conversion_running {
                        IconGlyph::Clock
                    } else {
                        IconGlyph::Check
                    },
                    if view.conversion_running {
                        tokens.blue
                    } else {
                        tokens.success
                    },
                    16.0,
                );
                ui.label(
                    RichText::new(if view.conversion_running {
                        "正在处理"
                    } else {
                        "准备就绪"
                    })
                    .color(if view.conversion_running {
                        tokens.blue
                    } else {
                        tokens.success
                    }),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let completed = view
                    .rows
                    .iter()
                    .filter(|row| {
                        matches!(
                            row.conversion,
                            ConversionDisplay::Done { .. } | ConversionDisplay::Failed { .. }
                        )
                    })
                    .count();
                ui.label(
                    RichText::new(format!("{} / {} 已处理", completed, view.rows.len()))
                        .color(tokens.muted),
                );
            });
        });
    }
}

fn detail_tabs(
    ui: &mut egui::Ui,
    view: &WorkspaceView,
    detail_label: &str,
    tokens: &theme::Tokens,
) -> bool {
    let tab_id = ui.id().with(("workspace_detail_tab", view.selected));
    let mut show_logs = ui
        .ctx()
        .data(|data| data.get_temp::<bool>(tab_id).unwrap_or(false));
    ui.horizontal(|ui| {
        if ui
            .selectable_label(
                !show_logs,
                RichText::new(detail_label).color(if show_logs {
                    tokens.muted
                } else {
                    Color32::WHITE
                }),
            )
            .clicked()
        {
            show_logs = false;
        }
        if ui
            .selectable_label(
                show_logs,
                RichText::new("转换日志").color(if show_logs {
                    Color32::WHITE
                } else {
                    tokens.muted
                }),
            )
            .clicked()
        {
            show_logs = true;
        }
    });
    ui.ctx()
        .data_mut(|data| data.insert_temp(tab_id, show_logs));
    show_logs
}

fn stage_label(stage: &str) -> &str {
    match stage {
        "Read" => "读取文件",
        "Decompress" => "解压数据",
        "Parse" => "解析 NBT",
        "Validate" => "校验数据",
        "Decode" => "校验方块数据",
        "Normalize" => "标准化坐标",
        "Merge" => "合并区域",
        "Write" => "写入输出",
        "Verify" => "验证输出",
        "Commit" => "保存文件",
        other => other,
    }
}

fn warnings_list(ui: &mut egui::Ui, warnings: &[String], tokens: &theme::Tokens) {
    if warnings.is_empty() {
        return;
    }
    egui::Frame::new()
        .fill(tokens.warning.linear_multiply(0.09))
        .stroke(Stroke::new(1.0, tokens.warning.linear_multiply(0.45)))
        .corner_radius(tokens.radius)
        .inner_margin(8.0)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                icon(ui, IconGlyph::Warning, tokens.warning, 17.0);
                ui.label(
                    RichText::new(format!("警告 {} 条", warnings.len()))
                        .strong()
                        .color(tokens.warning),
                );
            });
            egui::ScrollArea::vertical()
                .max_height(112.0)
                .show(ui, |ui| {
                    for warning in warnings {
                        ui.label(RichText::new(warning_label(warning)).color(tokens.text))
                            .on_hover_text(warning);
                    }
                });
        });
}

fn warning_label(warning: &str) -> &str {
    if warning.contains("metadata is not represented") {
        "元数据无法写入 Minecraft Structure NBT。"
    } else if warning.contains("region names are not represented") {
        "区域名称无法写入 Minecraft Structure NBT。"
    } else if warning.contains("pending block or fluid ticks are not represented") {
        "待处理的方块或流体刻无法写入 Minecraft Structure NBT。"
    } else if warning.contains("biome") && warning.contains("not represented") {
        "生物群系数据无法写入 Minecraft Structure NBT。"
    } else {
        warning
    }
}

fn logs_list(ui: &mut egui::Ui, logs: &[String], tokens: &theme::Tokens) {
    if logs.is_empty() {
        return;
    }
    ui.collapsing(format!("转换日志 · {} 条", logs.len()), |ui| {
        egui::ScrollArea::vertical()
            .max_height(90.0)
            .show(ui, |ui| {
                for log in logs {
                    ui.label(RichText::new(log).color(tokens.muted).small());
                }
            });
    });
}

fn alert_card(
    ui: &mut egui::Ui,
    title: &str,
    color: Color32,
    glyph: IconGlyph,
    body: &str,
    tokens: &theme::Tokens,
) {
    egui::Frame::new()
        .fill(color.linear_multiply(0.10))
        .stroke(Stroke::new(1.0, color.linear_multiply(0.55)))
        .corner_radius(tokens.radius)
        .inner_margin(8.0)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                icon(ui, glyph, color, 22.0);
                ui.vertical(|ui| {
                    ui.label(RichText::new(title).strong().color(color));
                    ui.label(body);
                });
            });
        });
}

fn info(ui: &mut egui::Ui, key: &str, value: &str, tokens: &theme::Tokens) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{key}：")).color(tokens.muted));
        ui.label(RichText::new(value).color(tokens.text));
    });
}

fn format_size(size: Option<u64>) -> String {
    let Some(size) = size else {
        return "—".into();
    };
    if size >= 1024 * 1024 {
        format!("{:.1} MiB", size as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.0} KiB", size as f64 / 1024.0)
    }
}

fn format_duration(seconds: u64) -> String {
    if seconds >= 3600 {
        format!(
            "{:02}:{:02}:{:02}",
            seconds / 3600,
            (seconds / 60) % 60,
            seconds % 60
        )
    } else {
        format!("{:02}:{:02}", seconds / 60, seconds % 60)
    }
}
