//! ダウンロード画面のフォームUI。
//!
//! アーキテクチャ設計書9-A節の決定に基づき、この時点では実際のジョブキュー等の
//! 実データには結線せず、フォーム状態の保持と見た目の実装のみを担う。

use std::path::PathBuf;

use egui_shadcn::{
    Button, ButtonRadius, ButtonVariant, CardProps, CardSize, CardVariant, CollapsibleProps,
    ControlSize, ControlVariant, Input, InputRadius, Label, LabelVariant, RadioDirection,
    RadioGroup, RadioOption, Textarea, Theme, card, checkbox, collapsible,
};

use crate::core::job::{AudioFormat, FormatSelection, Job, VideoQuality};

/// ダウンロード画面の入力状態。
///
/// キューへの結線は行わないため、ここでは「フォームからJobをどう組み立てるか」の
/// ロジックのみを保持する。実際にキューへ投入するかどうかは呼び出し元に委ねる。
#[derive(Debug, Clone, PartialEq)]
pub struct DownloadFormState {
    /// 改行区切りで複数URLを貼り付けるテキストエリアの内容。
    pub urls_input: String,
    /// プリセットの画質/音声選択。
    pub quality: VideoQuality,
    /// 上級者向けの生の`-f`書式を使うかどうか。オフの場合は`quality`を使う。
    pub use_custom_format: bool,
    /// 上級者向けの生の`-f`書式の入力内容。
    pub custom_format: String,
    /// `quality`が`AudioOnly`の場合に使う音声抽出フォーマット。
    pub audio_format: AudioFormat,
    /// 詳細設定(上級者向け)の折り畳み開閉状態。
    pub advanced_open: bool,
}

impl Default for DownloadFormState {
    fn default() -> Self {
        Self {
            urls_input: String::new(),
            quality: VideoQuality::Best,
            use_custom_format: false,
            custom_format: String::new(),
            audio_format: AudioFormat::Mp3,
            advanced_open: false,
        }
    }
}

impl DownloadFormState {
    /// 空行・前後の空白を取り除いたURL一覧を返す。
    ///
    /// 同一URLの重複行はそのまま許容する(同じ動画を複数フォーマットで
    /// ダウンロードしたいケースを呼び出し元側で弾かないようにするため)。
    pub fn urls(&self) -> Vec<String> {
        self.urls_input
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect()
    }

    /// フォーム入力から実際に使うフォーマット選択を組み立てる。
    ///
    /// 上級者向け入力が有効かつ空でない場合はそれを優先する。チェックボックスが
    /// オンでも入力が空なら誤操作とみなし、安全にプリセット選択へフォールバックする。
    pub fn format_selection(&self) -> FormatSelection {
        if self.use_custom_format {
            let trimmed = self.custom_format.trim();
            if !trimmed.is_empty() {
                return FormatSelection::Custom(trimmed.to_string());
            }
        }
        FormatSelection::Preset(self.quality)
    }

    /// `AudioOnly`プリセット選択時のみ音声抽出フォーマットを返す。
    pub fn audio_only_format(&self) -> Option<AudioFormat> {
        (self.quality == VideoQuality::AudioOnly).then_some(self.audio_format)
    }

    /// 登録可能かどうか。URLが1件も入力されていない場合は登録させない。
    pub fn can_submit(&self) -> bool {
        !self.urls().is_empty()
    }

    /// フォーム入力から`Job`一覧を組み立てる。
    ///
    /// この関数はまだキューへの投入は行わない(タスクDの範囲外のため)。
    /// 呼び出し元が返り値をキューに渡すかどうかを決める。
    pub fn build_jobs(&self, output_dir: PathBuf) -> Vec<Job> {
        let format_selection = self.format_selection();
        let audio_only = self.audio_only_format();

        self.urls()
            .into_iter()
            .map(|url| {
                let mut job = Job::new(url, format_selection.clone(), output_dir.clone());
                job.audio_only = audio_only;
                job
            })
            .collect()
    }
}

/// ダウンロード画面のフォームを描画する。
///
/// 実データには結線しないため、戻り値は「登録ボタンが押されたか」のみを表す。
/// 呼び出し元はこれを見てキュー投入等の後続処理を行う想定。
pub fn show(ui: &mut egui::Ui, theme: &Theme, state: &mut DownloadFormState) -> bool {
    let mut submitted = false;

    card(
        ui,
        theme,
        CardProps::default()
            .size(CardSize::Size4)
            .variant(CardVariant::Surface)
            .rounding(egui::CornerRadius::same(16))
            .heading("ダウンロード")
            .description("URLを貼り付けてフォーマットを選択してください。"),
        |ui| {
            Label::new("URL(1行に1件、複数行貼り付け可)").show(ui, theme);
            Textarea::new("download-view-urls")
                .placeholder("https://example.com/watch?v=...")
                .rows(6)
                .show(ui, theme, &mut state.urls_input);

            ui.add_space(16.0);

            Label::new("フォーマット").show(ui, theme);
            let quality_options = [
                RadioOption::new(VideoQuality::Best, "最高画質"),
                RadioOption::new(VideoQuality::P1080, "1080p"),
                RadioOption::new(VideoQuality::P720, "720p"),
                RadioOption::new(VideoQuality::AudioOnly, "音声のみ"),
            ];
            RadioGroup::new(
                "download-view-quality",
                &mut state.quality,
                &quality_options,
            )
            .direction(RadioDirection::Horizontal)
            .show(ui, theme);

            if state.quality == VideoQuality::AudioOnly {
                ui.add_space(8.0);
                Label::new("音声フォーマット").show(ui, theme);
                let audio_options = [
                    RadioOption::new(AudioFormat::Mp3, "MP3"),
                    RadioOption::new(AudioFormat::M4a, "M4A"),
                    RadioOption::new(AudioFormat::Opus, "Opus"),
                ];
                RadioGroup::new(
                    "download-view-audio-format",
                    &mut state.audio_format,
                    &audio_options,
                )
                .direction(RadioDirection::Horizontal)
                .show(ui, theme);
            }

            ui.add_space(16.0);

            // 折り畳みの開閉状態は`state`側に持たせて呼び出し元をまたいで保持したいが、
            // `collapsible`はローカルの`&mut bool`を要求するため一時変数を介して書き戻す。
            let advanced_id = egui::Id::new("download-view-advanced");
            let mut advanced_open = state.advanced_open;
            collapsible(
                ui,
                theme,
                CollapsibleProps::new(advanced_id, &mut advanced_open),
                |ui, ctx| {
                    let trigger_label = if ctx.is_open() {
                        "詳細設定(上級者向け) ▲"
                    } else {
                        "詳細設定(上級者向け) ▼"
                    };
                    ctx.trigger(ui, |ui| {
                        Button::new(trigger_label)
                            .variant(ButtonVariant::Ghost)
                            .show(ui, theme)
                    });

                    ctx.content(ui, |ui| {
                        ui.add_space(8.0);
                        checkbox(
                            ui,
                            theme,
                            &mut state.use_custom_format,
                            "生のフォーマット文字列(-f)を使用する",
                            ControlVariant::Secondary,
                            ControlSize::Md,
                            true,
                        );
                        ui.add_space(4.0);
                        let available_width = ui.available_width();
                        Input::new("download-view-custom-format")
                            .placeholder("bestvideo+bestaudio/best")
                            .radius(InputRadius::Medium)
                            .enabled(state.use_custom_format)
                            .width(available_width)
                            .show(ui, theme, &mut state.custom_format);
                    });
                },
            );
            state.advanced_open = advanced_open;

            ui.add_space(16.0);

            let can_submit = state.can_submit();
            ui.horizontal(|ui| {
                let clicked = Button::new("登録")
                    .variant(ButtonVariant::Solid)
                    .radius(ButtonRadius::Large)
                    .enabled(can_submit)
                    .show(ui, theme)
                    .clicked();
                if clicked && can_submit {
                    submitted = true;
                }

                if !can_submit {
                    ui.add_space(8.0);
                    Label::new("URLを入力してください")
                        .variant(LabelVariant::Muted)
                        .show(ui, theme);
                }
            });
        },
    );

    submitted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_trims_and_skips_blank_lines() {
        let state = DownloadFormState {
            urls_input: "  https://example.com/a  \n\n\thttps://example.com/b\n   \n".to_string(),
            ..Default::default()
        };

        assert_eq!(
            state.urls(),
            vec![
                "https://example.com/a".to_string(),
                "https://example.com/b".to_string(),
            ]
        );
    }

    #[test]
    fn can_submit_requires_at_least_one_url() {
        let mut state = DownloadFormState::default();
        assert!(!state.can_submit());

        state.urls_input = "https://example.com/a".to_string();
        assert!(state.can_submit());
    }

    #[test]
    fn format_selection_defaults_to_preset() {
        let state = DownloadFormState::default();
        assert_eq!(
            state.format_selection(),
            FormatSelection::Preset(VideoQuality::Best)
        );
    }

    #[test]
    fn format_selection_uses_custom_when_enabled_and_non_empty() {
        let state = DownloadFormState {
            use_custom_format: true,
            custom_format: "  bestvideo+bestaudio  ".to_string(),
            ..Default::default()
        };

        assert_eq!(
            state.format_selection(),
            FormatSelection::Custom("bestvideo+bestaudio".to_string())
        );
    }

    #[test]
    fn format_selection_falls_back_to_preset_when_custom_is_blank() {
        let state = DownloadFormState {
            use_custom_format: true,
            custom_format: "   ".to_string(),
            quality: VideoQuality::P720,
            ..Default::default()
        };

        assert_eq!(
            state.format_selection(),
            FormatSelection::Preset(VideoQuality::P720)
        );
    }

    #[test]
    fn audio_only_format_is_none_unless_audio_only_selected() {
        let mut state = DownloadFormState {
            quality: VideoQuality::Best,
            audio_format: AudioFormat::Opus,
            ..Default::default()
        };
        assert_eq!(state.audio_only_format(), None);

        state.quality = VideoQuality::AudioOnly;
        assert_eq!(state.audio_only_format(), Some(AudioFormat::Opus));
    }

    #[test]
    fn build_jobs_creates_one_job_per_url_with_shared_format_and_audio() {
        let state = DownloadFormState {
            urls_input: "https://example.com/a\nhttps://example.com/b".to_string(),
            quality: VideoQuality::AudioOnly,
            audio_format: AudioFormat::M4a,
            ..Default::default()
        };

        let jobs = state.build_jobs(PathBuf::from("C:/downloads"));

        assert_eq!(jobs.len(), 2);
        for job in &jobs {
            assert_eq!(
                job.format_selection,
                FormatSelection::Preset(VideoQuality::AudioOnly)
            );
            assert_eq!(job.audio_only, Some(AudioFormat::M4a));
            assert_eq!(job.output_dir, PathBuf::from("C:/downloads"));
        }
        assert_eq!(jobs[0].url, "https://example.com/a");
        assert_eq!(jobs[1].url, "https://example.com/b");
    }

    #[test]
    fn build_jobs_is_empty_when_no_urls_entered() {
        let state = DownloadFormState::default();
        assert!(state.build_jobs(PathBuf::from("C:/downloads")).is_empty());
    }
}
