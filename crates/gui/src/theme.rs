use crate::state::ThemeMode;
use eframe::egui::{self, Color32, CornerRadius, Stroke};

#[derive(Clone, Copy)]
pub(crate) struct Tokens {
    pub canvas: Color32,
    pub surface: Color32,
    pub surface_alt: Color32,
    pub border: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub blue: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub danger: Color32,
    pub radius: CornerRadius,
}

pub(crate) fn apply(ctx: &egui::Context, mode: ThemeMode) {
    ctx.set_theme(match mode {
        ThemeMode::System => egui::ThemePreference::System,
        ThemeMode::Light => egui::ThemePreference::Light,
        ThemeMode::Dark => egui::ThemePreference::Dark,
    });

    let mut style = (*ctx.style()).clone();
    let dark = ctx.theme() == egui::Theme::Dark;
    style.visuals = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    style.visuals.selection.bg_fill = Color32::from_rgb(35, 112, 238);
    style.visuals.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    style.visuals.widgets.noninteractive.corner_radius = CornerRadius::same(4);
    style.visuals.widgets.inactive.corner_radius = CornerRadius::same(4);
    style.visuals.widgets.hovered.corner_radius = CornerRadius::same(4);
    style.visuals.widgets.active.corner_radius = CornerRadius::same(4);
    if !dark {
        for widgets in [
            &mut style.visuals.widgets.inactive,
            &mut style.visuals.widgets.hovered,
        ] {
            widgets.bg_fill = Color32::WHITE;
            widgets.bg_stroke = Stroke::new(1.0, Color32::from_rgb(210, 218, 228));
            widgets.corner_radius = CornerRadius::same(3);
        }
    }
    if let Some(font) = style.text_styles.get_mut(&egui::TextStyle::Heading) {
        font.family = egui::FontFamily::Name("noto_sc_semibold".into());
    }
    style.spacing.item_spacing = egui::vec2(6.0, 6.0);
    style.spacing.button_padding = egui::vec2(9.0, 6.0);
    ctx.set_style(style);
}

pub(crate) fn tokens(ctx: &egui::Context) -> Tokens {
    if ctx.theme() == egui::Theme::Dark {
        Tokens {
            canvas: Color32::from_rgb(22, 26, 33),
            surface: Color32::from_rgb(31, 36, 45),
            surface_alt: Color32::from_rgb(39, 45, 55),
            border: Color32::from_rgb(61, 70, 83),
            text: Color32::from_rgb(235, 239, 245),
            muted: Color32::from_rgb(163, 173, 188),
            blue: Color32::from_rgb(47, 126, 255),
            success: Color32::from_rgb(36, 164, 99),
            warning: Color32::from_rgb(224, 145, 22),
            danger: Color32::from_rgb(225, 64, 71),
            radius: CornerRadius::same(9),
        }
    } else {
        Tokens {
            canvas: Color32::from_rgb(250, 251, 253),
            surface: Color32::WHITE,
            surface_alt: Color32::from_rgb(241, 245, 249),
            border: Color32::from_rgb(215, 222, 231),
            text: Color32::from_rgb(27, 36, 51),
            muted: Color32::from_rgb(103, 114, 130),
            blue: Color32::from_rgb(20, 105, 245),
            success: Color32::from_rgb(34, 151, 84),
            warning: Color32::from_rgb(220, 136, 20),
            danger: Color32::from_rgb(218, 54, 62),
            radius: CornerRadius::same(6),
        }
    }
}
