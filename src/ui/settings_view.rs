//! 設定画面とログパネルの描画コンポーネント。
//!
//! アプリ全体のイベントループやジョブ実行との連携は呼び出し元(`app.rs`)に委ねる。
//! ここでは`Config`を直接編集する描画関数と、ログ表示に必要な型のみを提供する。

use std::collections::VecDeque;

use chrono::{DateTime, Local};
use egui::Color32;
use egui_shadcn::{
    Button, ButtonVariant, ControlSize, ControlVariant, FieldProps, HeadingAs, HeadingProps, Input,
    ScrollAreaProps, ScrollDirection, SelectPropsSimple, SliderProps, TextProps, Theme,
    TypographyColor, field, heading, scroll_area, select, slider_with_props, switch, text,
};

use crate::config::Config;

/// macOSのシステムカラー(System Orange)に寄せた警告色。
/// shadcnパレットには警告用の色トークンが無いため、`theme::build_theme`の
/// アクセントカラーと同様にApple系システムカラーを直接定義している。
const WARN_COLOR: Color32 = Color32::from_rgb(255, 159, 10);

/// 設定画面を描画する。
///
/// 戻り値は呼び出し前後で`config`の内容が変化したかどうかを示す。呼び出し元は
/// これを`Config::save`を呼び出すトリガーとして使うことを想定している。
pub fn show(ui: &mut egui::Ui, theme: &Theme, config: &mut Config) -> bool {
    let before = config.clone();

    ui.spacing_mut().item_spacing.y = 12.0;

    show_general_section(ui, theme, config);
    section_gap(ui, theme);
    show_defaults_section(ui, theme, config);
    section_gap(ui, theme);
    show_binaries_section(ui, theme, config);
    section_gap(ui, theme);
    show_logging_section(ui, theme, config);

    *config != before
}

/// セクション間の区切り(見出しと同様、`main.rs`のセパレータ運用に合わせている)。
fn section_gap(ui: &mut egui::Ui, theme: &Theme) {
    ui.add_space(4.0);
    egui_shadcn::separator(ui, theme, egui_shadcn::SeparatorProps::default());
    ui.add_space(4.0);
}

fn section_heading(ui: &mut egui::Ui, theme: &Theme, title: &str) {
    heading(ui, theme, HeadingProps::new(title).as_tag(HeadingAs::H3));
}

fn show_general_section(ui: &mut egui::Ui, theme: &Theme, config: &mut Config) {
    section_heading(ui, theme, "一般");

    field(
        ui,
        theme,
        FieldProps::new()
            .label("保存先フォルダ")
            .description("空欄の場合、OS標準のダウンロードフォルダを使用します。"),
        |ui| {
            let width = ui.available_width();
            Input::new("settings.general.save_dir")
                .placeholder("例: D:/Videos")
                .width(width)
                .show(ui, theme, &mut config.general.save_dir);
        },
    );

    field(
        ui,
        theme,
        FieldProps::new()
            .label("同時実行数")
            .description("1〜10の範囲で同時にダウンロードするジョブ数を指定します。"),
        |ui| {
            ui.horizontal(|ui| {
                let mut values = vec![config.general.concurrency as f32];
                let response = slider_with_props(
                    ui,
                    theme,
                    SliderProps::new("settings.general.concurrency", &mut values)
                        .min(1.0)
                        .max(10.0)
                        .step(1.0),
                );
                if response.changed() {
                    config.general.concurrency = values[0].round().clamp(1.0, 10.0) as u32;
                }
                ui.label(config.general.concurrency.to_string());
            });
        },
    );
}

/// 画質/フォーマット選択の候補を`select`に渡す前に`Vec<String>`へ変換する。
/// `SelectPropsSimple`が`&[String]`を要求するため、固定候補でも都度組み立てが必要。
fn string_options(options: &[&str]) -> Vec<String> {
    options.iter().map(|s| s.to_string()).collect()
}

fn show_defaults_section(ui: &mut egui::Ui, theme: &Theme, config: &mut Config) {
    section_heading(ui, theme, "既定値");

    let quality_options = string_options(&["best", "1080p", "720p", "480p"]);
    field(ui, theme, FieldProps::new().label("既定画質"), |ui| {
        let mut selected = Some(config.defaults.video_quality.clone());
        select(
            ui,
            theme,
            SelectPropsSimple {
                id_source: "settings.defaults.video_quality",
                selected: &mut selected,
                options: &quality_options,
                placeholder: "画質を選択",
                size: ControlSize::Md,
                enabled: true,
                is_invalid: false,
            },
        );
        if let Some(value) = selected {
            config.defaults.video_quality = value;
        }
    });

    let audio_options = string_options(&["mp3", "m4a", "opus"]);
    field(
        ui,
        theme,
        FieldProps::new().label("既定音声フォーマット"),
        |ui| {
            let mut selected = Some(config.defaults.audio_format.clone());
            select(
                ui,
                theme,
                SelectPropsSimple {
                    id_source: "settings.defaults.audio_format",
                    selected: &mut selected,
                    options: &audio_options,
                    placeholder: "フォーマットを選択",
                    size: ControlSize::Md,
                    enabled: true,
                    is_invalid: false,
                },
            );
            if let Some(value) = selected {
                config.defaults.audio_format = value;
            }
        },
    );
}

fn show_binaries_section(ui: &mut egui::Ui, theme: &Theme, config: &mut Config) {
    section_heading(ui, theme, "yt-dlp / ffmpeg");

    field(
        ui,
        theme,
        FieldProps::new()
            .label("yt-dlpパス")
            .description("空欄の場合、自動管理されたパスを使用します。"),
        |ui| {
            let width = ui.available_width();
            Input::new("settings.binaries.ytdlp_path")
                .placeholder("例: C:/tools/yt-dlp.exe")
                .width(width)
                .show(ui, theme, &mut config.binaries.ytdlp_path);
        },
    );

    field(
        ui,
        theme,
        FieldProps::new()
            .label("ffmpegパス")
            .description("空欄の場合、自動管理されたパスを使用します。"),
        |ui| {
            let width = ui.available_width();
            Input::new("settings.binaries.ffmpeg_path")
                .placeholder("例: C:/tools/ffmpeg.exe")
                .width(width)
                .show(ui, theme, &mut config.binaries.ffmpeg_path);
        },
    );

    switch(
        ui,
        theme,
        &mut config.binaries.auto_update_check,
        "自動更新チェック",
        ControlVariant::Primary,
        ControlSize::Md,
        true,
    );
}

fn show_logging_section(ui: &mut egui::Ui, theme: &Theme, config: &mut Config) {
    section_heading(ui, theme, "ログ");

    switch(
        ui,
        theme,
        &mut config.logging.file_logging,
        "ファイルログを有効にする",
        ControlVariant::Primary,
        ControlSize::Md,
        true,
    );
}

/// ログの重要度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

impl LogLevel {
    /// ログパネルおよびコピー出力で使う短いラベル。
    pub fn label(self) -> &'static str {
        match self {
            LogLevel::Info => "INFO",
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERROR",
        }
    }

    fn color(self, theme: &Theme) -> Color32 {
        match self {
            LogLevel::Info => theme.palette.muted_foreground,
            LogLevel::Warn => WARN_COLOR,
            LogLevel::Error => theme.palette.destructive,
        }
    }
}

/// 1件のログエントリ。
#[derive(Debug, Clone, PartialEq)]
pub struct LogEntry {
    pub level: LogLevel,
    pub message: String,
    pub timestamp: DateTime<Local>,
}

impl LogEntry {
    pub fn new(level: LogLevel, message: impl Into<String>) -> Self {
        Self {
            level,
            message: message.into(),
            timestamp: Local::now(),
        }
    }
}

/// GUIに表示するログを保持するメモリ上のリングバッファ。
///
/// 常駐運用でもメモリを圧迫しないよう、直近`capacity`件のみを保持し、
/// それを超えた分は古いものから破棄する(ファイルログの有無とは独立)。
#[derive(Debug, Clone)]
pub struct LogBuffer {
    entries: VecDeque<LogEntry>,
    capacity: usize,
}

impl LogBuffer {
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            entries: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub fn push(&mut self, entry: LogEntry) {
        if self.entries.len() >= self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back(entry);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// スライスとして参照する。`VecDeque`内部表現を連続化する必要があるため
    /// `&mut self`を要求する(要素の追加・削除は行わない)。
    pub fn as_slice(&mut self) -> &[LogEntry] {
        self.entries.make_contiguous()
    }
}

impl Default for LogBuffer {
    fn default() -> Self {
        Self::new(500)
    }
}

/// ログパネル操作の結果。コピーはパネル内で完結するため含めず、呼び出し元の
/// 状態(`LogBuffer`)を変更する必要がある操作のみを返す。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LogPanelAction {
    #[default]
    None,
    ClearRequested,
}

/// ログパネルを描画する。
///
/// レベル別の色分け表示、全ログのクリップボードコピー、クリア要求の通知を行う。
/// クリア自体は呼び出し元が`LogBuffer::clear`を呼ぶことで反映する。
pub fn show_log_panel(ui: &mut egui::Ui, theme: &Theme, logs: &[LogEntry]) -> LogPanelAction {
    let mut action = LogPanelAction::None;

    ui.horizontal(|ui| {
        section_heading(ui, theme, "ログ");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if Button::new("クリア")
                .variant(ButtonVariant::Outline)
                .show(ui, theme)
                .clicked()
            {
                action = LogPanelAction::ClearRequested;
            }
            if Button::new("コピー")
                .variant(ButtonVariant::Soft)
                .show(ui, theme)
                .clicked()
            {
                ui.ctx().copy_text(format_logs_for_clipboard(logs));
            }
        });
    });

    ui.add_space(8.0);

    scroll_area(
        ui,
        theme,
        ScrollAreaProps::default()
            .direction(ScrollDirection::Vertical)
            .max_size(egui::Vec2::new(ui.available_width(), 240.0)),
        |ui| show_log_entries(ui, theme, logs),
    );

    action
}

fn show_log_entries(ui: &mut egui::Ui, theme: &Theme, logs: &[LogEntry]) {
    if logs.is_empty() {
        text(
            ui,
            theme,
            TextProps::new("ログはまだありません。").color(TypographyColor::Muted),
        );
        return;
    }

    for entry in logs {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(entry.timestamp.format("%H:%M:%S").to_string())
                    .monospace()
                    .color(theme.palette.muted_foreground),
            );
            ui.label(
                egui::RichText::new(entry.level.label())
                    .monospace()
                    .strong()
                    .color(entry.level.color(theme)),
            );
        });
        ui.label(egui::RichText::new(&entry.message).color(theme.palette.foreground));
        ui.add_space(6.0);
    }
}

/// コピー用にログ全体を`[時刻] レベル メッセージ`形式の複数行テキストへ整形する。
fn format_logs_for_clipboard(logs: &[LogEntry]) -> String {
    logs.iter()
        .map(|entry| {
            format!(
                "[{}] {} {}",
                entry.timestamp.format("%Y-%m-%d %H:%M:%S"),
                entry.level.label(),
                entry.message
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(level: LogLevel, message: &str) -> LogEntry {
        LogEntry::new(level, message)
    }

    #[test]
    fn log_buffer_push_keeps_entries_within_capacity() {
        let mut buffer = LogBuffer::new(3);
        buffer.push(entry(LogLevel::Info, "1"));
        buffer.push(entry(LogLevel::Info, "2"));
        buffer.push(entry(LogLevel::Info, "3"));

        assert_eq!(buffer.len(), 3);
    }

    #[test]
    fn log_buffer_discards_oldest_entry_when_over_capacity() {
        let mut buffer = LogBuffer::new(2);
        buffer.push(entry(LogLevel::Info, "oldest"));
        buffer.push(entry(LogLevel::Info, "middle"));
        buffer.push(entry(LogLevel::Warn, "newest"));

        assert_eq!(buffer.len(), 2);
        let messages: Vec<&str> = buffer
            .as_slice()
            .iter()
            .map(|e| e.message.as_str())
            .collect();
        assert_eq!(messages, vec!["middle", "newest"]);
    }

    #[test]
    fn log_buffer_clear_empties_entries() {
        let mut buffer = LogBuffer::new(5);
        buffer.push(entry(LogLevel::Error, "boom"));
        assert!(!buffer.is_empty());

        buffer.clear();

        assert!(buffer.is_empty());
        assert_eq!(buffer.len(), 0);
    }

    #[test]
    fn log_buffer_capacity_is_at_least_one() {
        let buffer = LogBuffer::new(0);
        assert_eq!(buffer.capacity(), 1);
    }

    #[test]
    fn log_buffer_default_has_reasonable_capacity() {
        let buffer = LogBuffer::default();
        assert_eq!(buffer.capacity(), 500);
        assert!(buffer.is_empty());
    }

    #[test]
    fn log_level_labels_match_expected_text() {
        assert_eq!(LogLevel::Info.label(), "INFO");
        assert_eq!(LogLevel::Warn.label(), "WARN");
        assert_eq!(LogLevel::Error.label(), "ERROR");
    }

    #[test]
    fn format_logs_for_clipboard_joins_entries_with_newlines() {
        let logs = vec![
            entry(LogLevel::Info, "first"),
            entry(LogLevel::Error, "second"),
        ];

        let formatted = format_logs_for_clipboard(&logs);
        let lines: Vec<&str> = formatted.lines().collect();

        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("INFO"));
        assert!(lines[0].contains("first"));
        assert!(lines[1].contains("ERROR"));
        assert!(lines[1].contains("second"));
    }

    #[test]
    fn log_panel_action_default_is_none() {
        assert_eq!(LogPanelAction::default(), LogPanelAction::None);
    }
}
