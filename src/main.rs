mod app;
mod config;
mod core;
mod theme;
mod ui;

fn main() -> eframe::Result<()> {
    eframe::run_native(
        "Featherpull",
        eframe::NativeOptions::default(),
        Box::new(|cc| {
            setup_custom_fonts(&cc.egui_ctx);
            Ok(Box::new(app::App::new()))
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
