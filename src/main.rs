fn main() -> eframe::Result {
    eframe::run_native(
        "Featherpull",
        eframe::NativeOptions::default(),
        Box::new(|_cc| Ok(Box::new(FeatherpullApp::default()))),
    )
}

#[derive(Default)]
struct FeatherpullApp {}

impl eframe::App for FeatherpullApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.heading("Featherpull");
        ui.label("yt-dlp / ffmpeg GUIラッパー(開発中)");
    }
}
