fn main() -> eframe::Result {
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
    use egui::epaint::text::{FontInsert, FontPriority, InsertFontFamily};

    // 先に日本語フォントを最優先で登録しておく。
    ctx.add_font(FontInsert::new(
        "NotoSansJP",
        egui::FontData::from_static(include_bytes!("../assets/fonts/NotoSansJP-Variable.ttf")),
        vec![
            InsertFontFamily {
                family: egui::FontFamily::Proportional,
                priority: FontPriority::Highest,
            },
            InsertFontFamily {
                family: egui::FontFamily::Monospace,
                priority: FontPriority::Highest,
            },
        ],
    ));

    // 後から英語フォントを最優先で登録することで、日本語フォントより先に評価されるようにする
    // (英数字はJetBrains Monoが持つグリフを使い、日本語のみNoto Sans JPにフォールバックする)。
    ctx.add_font(FontInsert::new(
        "JetBrainsMono",
        egui::FontData::from_static(include_bytes!("../assets/fonts/JetBrainsMono-Variable.ttf")),
        vec![
            InsertFontFamily {
                family: egui::FontFamily::Proportional,
                priority: FontPriority::Highest,
            },
            InsertFontFamily {
                family: egui::FontFamily::Monospace,
                priority: FontPriority::Highest,
            },
        ],
    ));
}

#[derive(Default)]
struct FeatherpullApp {}

impl eframe::App for FeatherpullApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.heading("Featherpull");
        ui.label("yt-dlp / ffmpeg GUIラッパー(開発中)");
    }
}
