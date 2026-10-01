pub(crate) mod preview;
pub(crate) mod workspace;

use eframe::egui;

/// Paint simple geometric icons so the desktop UI does not need an icon-font dependency.
pub(crate) fn icon(ui: &mut egui::Ui, glyph: IconGlyph, color: egui::Color32, size: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    draw_icon(ui.painter(), rect, glyph, color);
}

pub(crate) fn icon_button(
    ui: &mut egui::Ui,
    glyph: IconGlyph,
    color: egui::Color32,
    size: f32,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, 6.0, ui.visuals().widgets.hovered.bg_fill);
    }
    draw_icon(ui.painter(), rect, glyph, color);
    response
}

fn draw_icon(painter: &egui::Painter, rect: egui::Rect, glyph: IconGlyph, color: egui::Color32) {
    let center = rect.center();
    let size = rect.width().min(rect.height());
    let stroke = egui::Stroke::new((size / 11.0).max(1.3), color);
    let inset = size * 0.2;
    match glyph {
        IconGlyph::Add => {
            painter.line_segment(
                [
                    center - egui::vec2(inset, 0.0),
                    center + egui::vec2(inset, 0.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    center - egui::vec2(0.0, inset),
                    center + egui::vec2(0.0, inset),
                ],
                stroke,
            );
        }
        IconGlyph::Folder => {
            let left = rect.left() + size * 0.16;
            let right = rect.right() - size * 0.15;
            let top = rect.top() + size * 0.30;
            let tab_end = left + size * 0.30;
            let body_top = rect.top() + size * 0.43;
            let bottom = rect.bottom() - size * 0.18;
            let points = [
                egui::pos2(left, bottom),
                egui::pos2(left, top),
                egui::pos2(tab_end, top),
                egui::pos2(tab_end + size * 0.12, body_top),
                egui::pos2(right, body_top),
                egui::pos2(right - size * 0.07, bottom),
                egui::pos2(left, bottom),
            ];
            painter.add(egui::Shape::line(points.to_vec(), stroke));
        }
        IconGlyph::File => {
            let left = rect.left() + size * 0.27;
            let right = rect.right() - size * 0.23;
            let top = rect.top() + size * 0.13;
            let fold = size * 0.20;
            let bottom = rect.bottom() - size * 0.13;
            let points = [
                egui::pos2(left, bottom),
                egui::pos2(left, top),
                egui::pos2(right - fold, top),
                egui::pos2(right, top + fold),
                egui::pos2(right, bottom),
                egui::pos2(left, bottom),
            ];
            painter.add(egui::Shape::line(points.to_vec(), stroke));
            painter.line_segment(
                [
                    egui::pos2(right - fold, top),
                    egui::pos2(right - fold, top + fold),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(right - fold, top + fold),
                    egui::pos2(right, top + fold),
                ],
                stroke,
            );
            for row in 0..2 {
                let y = top + fold + size * (0.20 + row as f32 * 0.16);
                painter.line_segment(
                    [
                        egui::pos2(left + size * 0.10, y),
                        egui::pos2(right - size * 0.10, y),
                    ],
                    stroke,
                );
            }
        }
        IconGlyph::Settings => {
            painter.circle_stroke(center, size * 0.28, stroke);
            painter.circle_filled(center, size * 0.08, color);
            for angle in 0..8 {
                let radians = angle as f32 * std::f32::consts::TAU / 8.0;
                let direction = egui::vec2(radians.cos(), radians.sin());
                painter.line_segment(
                    [
                        center + direction * size * 0.31,
                        center + direction * size * 0.42,
                    ],
                    stroke,
                );
            }
        }
        IconGlyph::Minimize => {
            painter.line_segment(
                [
                    center + egui::vec2(-inset, inset * 0.4),
                    center + egui::vec2(inset, inset * 0.4),
                ],
                stroke,
            );
        }
        IconGlyph::Maximize => {
            painter.rect_stroke(rect.shrink(inset), 1.0, stroke, egui::StrokeKind::Inside);
        }
        IconGlyph::Close => {
            painter.line_segment(
                [
                    center - egui::vec2(inset, inset),
                    center + egui::vec2(inset, inset),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    center + egui::vec2(-inset, inset),
                    center + egui::vec2(inset, -inset),
                ],
                stroke,
            );
        }
        IconGlyph::Back => {
            painter.line_segment(
                [
                    center + egui::vec2(inset, 0.0),
                    center - egui::vec2(inset, 0.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    center - egui::vec2(inset, 0.0),
                    center - egui::vec2(0.0, inset),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    center - egui::vec2(inset, 0.0),
                    center + egui::vec2(0.0, inset),
                ],
                stroke,
            );
        }
        IconGlyph::Trash => {
            painter.line_segment(
                [
                    center + egui::vec2(-inset, -inset * 0.55),
                    center + egui::vec2(inset, -inset * 0.55),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    center + egui::vec2(-inset * 0.65, -inset),
                    center + egui::vec2(inset * 0.65, -inset),
                ],
                stroke,
            );
            painter.rect_stroke(
                egui::Rect::from_center_size(
                    center + egui::vec2(0.0, inset * 0.35),
                    egui::vec2(size * 0.38, size * 0.42),
                ),
                2.0,
                stroke,
                egui::StrokeKind::Inside,
            );
        }
        IconGlyph::Play => {
            painter.add(egui::Shape::convex_polygon(
                vec![
                    center + egui::vec2(-inset * 0.5, -inset),
                    center + egui::vec2(inset, 0.0),
                    center + egui::vec2(-inset * 0.5, inset),
                ],
                color,
                egui::Stroke::NONE,
            ));
        }
        IconGlyph::Check => {
            painter.line_segment(
                [
                    center + egui::vec2(-inset, 0.0),
                    center + egui::vec2(-inset * 0.15, inset * 0.65),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    center + egui::vec2(-inset * 0.15, inset * 0.65),
                    center + egui::vec2(inset, -inset * 0.75),
                ],
                stroke,
            );
        }
        IconGlyph::Clock => {
            painter.circle_stroke(center, size * 0.34, stroke);
            painter.line_segment([center, center + egui::vec2(0.0, -size * 0.19)], stroke);
            painter.line_segment(
                [center, center + egui::vec2(size * 0.16, size * 0.08)],
                stroke,
            );
        }
        IconGlyph::Warning => {
            let points = [
                center + egui::vec2(0.0, -size * 0.38),
                center + egui::vec2(size * 0.39, size * 0.34),
                center + egui::vec2(-size * 0.39, size * 0.34),
            ];
            painter.add(egui::Shape::closed_line(points.to_vec(), stroke));
            painter.line_segment(
                [
                    center + egui::vec2(0.0, -size * 0.13),
                    center + egui::vec2(0.0, size * 0.12),
                ],
                stroke,
            );
            painter.circle_filled(center + egui::vec2(0.0, size * 0.24), size * 0.035, color);
        }
        IconGlyph::Error => {
            painter.circle_stroke(center, size * 0.36, stroke);
            painter.line_segment(
                [
                    center + egui::vec2(0.0, -size * 0.17),
                    center + egui::vec2(0.0, size * 0.10),
                ],
                stroke,
            );
            painter.circle_filled(center + egui::vec2(0.0, size * 0.22), size * 0.035, color);
        }
        IconGlyph::Eye => {
            let upper = (0..=12).map(|i| {
                let t = i as f32 / 12.0;
                let x = egui::lerp(-size * 0.40..=size * 0.40, t);
                let y = -size * 0.25 * (1.0 - (2.0 * t - 1.0).powi(2)).sqrt();
                center + egui::vec2(x, y)
            });
            let lower = (0..=12).rev().map(|i| {
                let t = i as f32 / 12.0;
                let x = egui::lerp(-size * 0.40..=size * 0.40, t);
                let y = size * 0.25 * (1.0 - (2.0 * t - 1.0).powi(2)).sqrt();
                center + egui::vec2(x, y)
            });
            let shape = upper.chain(lower).collect::<Vec<_>>();
            painter.add(egui::Shape::closed_line(shape, stroke));
            painter.circle_stroke(center, size * 0.105, stroke);
            painter.circle_filled(center, size * 0.035, color);
        }
        IconGlyph::Copy => {
            painter.rect_stroke(rect.shrink(inset), 2.0, stroke, egui::StrokeKind::Inside);
            painter.rect_stroke(
                rect.shrink(inset * 1.7)
                    .translate(egui::vec2(size * 0.13, -size * 0.13)),
                2.0,
                stroke,
                egui::StrokeKind::Inside,
            );
        }
        IconGlyph::Rotate => {
            painter.circle_stroke(center, inset, stroke);
        }
        IconGlyph::Pan => {
            painter.line_segment(
                [
                    center - egui::vec2(inset, 0.0),
                    center + egui::vec2(inset, 0.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    center - egui::vec2(0.0, inset),
                    center + egui::vec2(0.0, inset),
                ],
                stroke,
            );
        }
        IconGlyph::Zoom => {
            painter.circle_stroke(
                center - egui::vec2(size * 0.05, size * 0.05),
                inset * 0.7,
                stroke,
            );
            painter.line_segment(
                [
                    center + egui::vec2(size * 0.08, size * 0.08),
                    center + egui::vec2(inset, inset),
                ],
                stroke,
            );
        }
        IconGlyph::Fit => {
            painter.rect_stroke(rect.shrink(inset), 1.0, stroke, egui::StrokeKind::Inside);
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum IconGlyph {
    Add,
    Folder,
    File,
    Settings,
    Minimize,
    Maximize,
    Close,
    Back,
    Trash,
    Play,
    Check,
    Clock,
    Warning,
    Error,
    Eye,
    Copy,
    Rotate,
    Pan,
    Zoom,
    Fit,
}
