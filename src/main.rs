mod theme;

use egui_shadcn::{
    Button, ButtonRadius, ButtonVariant, CardProps, CardSize, CardVariant, HeadingAs,
    HeadingProps, LightSwitchProps, SeparatorProps, TextProps, TypographyColor, card, heading,
    light_switch, separator, text,
};

fn main() -> eframe::Result<()> {
    eframe::run_native(
        "Featherpull",
        eframe::NativeOptions::default(),
        Box::new(|cc| {
            setup_custom_fonts(&cc.egui_ctx);
            Ok(Box::new(FeatherpullApp::default()))
        }),
    )
}

/// 英語テキストはJetBrains Mono、日本語テキストはNoto Sans JPで表示する。
/// それ以外の文字はegui標準の同梱フォント(絵文字等)にフォールバックする。
fn setup_custom_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    fonts.font_data.insert(
        "JetBrainsMono".to_owned(),
        egui::FontData::from_static(include_bytes!("../assets/fonts/JetBrainsMono-Variable.ttf"))
            .into(),
    );
    fonts.font_data.insert(
        "NotoSansJP".to_owned(),
        egui::FontData::from_static(include_bytes!("../assets/fonts/NotoSansJP-Variable.ttf"))
            .into(),
    );

    // 英数字はJetBrainsMonoが優先され、JetBrainsMonoにグリフが無い日本語はNotoSansJPに
    // フォールバックする。
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        let list = fonts.families.entry(family).or_default();
        list.insert(0, "NotoSansJP".to_owned());
        list.insert(0, "JetBrainsMono".to_owned());
    }

    ctx.set_fonts(fonts);
}

#[derive(Default)]
struct FeatherpullApp {
    dark_mode: bool,
}

impl eframe::App for FeatherpullApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let theme = theme::build_theme(self.dark_mode);

        let frame = egui::Frame::default()
            .fill(theme.palette.background)
            .inner_margin(egui::Margin::same(24));

        egui::CentralPanel::default().frame(frame).show(ctx, |ui| {
            ui.horizontal(|ui| {
                heading(
                    ui,
                    &theme,
                    HeadingProps::new("Featherpull").as_tag(HeadingAs::H1),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if light_switch(ui, &theme, LightSwitchProps::new(self.dark_mode)).clicked() {
                        self.dark_mode = !self.dark_mode;
                    }
                });
            });

            text(
                ui,
                &theme,
                TextProps::new("yt-dlp / ffmpeg GUIラッパー").color(TypographyColor::Muted),
            );

            ui.add_space(16.0);
            separator(ui, &theme, SeparatorProps::default());
            ui.add_space(16.0);

            card(
                ui,
                &theme,
                CardProps::default()
                    .size(CardSize::Size4)
                    .variant(CardVariant::Surface)
                    .rounding(egui::CornerRadius::same(16))
                    .heading("開発中")
                    .description("ダウンロードキューやフォーマット設定は近日実装予定です。"),
                |ui| {
                    ui.horizontal(|ui| {
                        Button::new("はじめる")
                            .variant(ButtonVariant::Solid)
                            .radius(ButtonRadius::Large)
                            .show(ui, &theme);
                        Button::new("設定")
                            .variant(ButtonVariant::Soft)
                            .radius(ButtonRadius::Large)
                            .show(ui, &theme);
                    });
                },
            );
        });
    }
}
