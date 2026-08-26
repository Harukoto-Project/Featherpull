//! ジョブキュー・進捗表示画面(タスクE)。
//!
//! フェーズ2(タスクG)で実際のキュー制御ロジックと接続する前提のため、ここでは
//! `&[Job]`を受け取って描画し、ユーザー操作の意図を[`QueueViewAction`]として返すだけの
//! 純粋なUI関数として実装する。このモジュール自身はジョブの状態を変更しない。

use crate::core::job::{Job, JobId, JobStatus};
use egui_shadcn::{
    BadgeProps, BadgeVariant, Button, ButtonRadius, ButtonVariant, CardProps, CardSize,
    CardVariant, EmptyProps, ProgressProps, TextProps, Theme, TypographyColor, badge, card, empty,
    progress, text,
};

/// このビューに対するユーザー操作の結果。
///
/// 呼び出し元(フェーズ2のキュー管理側)がどのジョブに対してキャンセル/再試行が
/// 押されたかを判別し、実際のジョブ制御に反映するために使う。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QueueViewAction {
    #[default]
    None,
    Cancel(JobId),
    Retry(JobId),
}

/// ジョブ一覧を描画し、押下されたボタンに対応する操作を返す。
///
/// 1フレーム内で複数のボタンが押されることは通常無いため、検出した操作のうち
/// 最後の1件だけを返す単純な実装にしている。
pub fn show(ui: &mut egui::Ui, theme: &Theme, jobs: &[Job]) -> QueueViewAction {
    if jobs.is_empty() {
        empty(
            ui,
            theme,
            EmptyProps::new("ダウンロード待ちのジョブはありません")
                .description("URLを追加すると、ここにキューの状況が表示されます。"),
        );
        return QueueViewAction::None;
    }

    let mut action = QueueViewAction::None;
    for job in jobs {
        let job_action = show_job_card(ui, theme, job);
        if job_action != QueueViewAction::None {
            action = job_action;
        }
        ui.add_space(8.0);
    }
    action
}

fn show_job_card(ui: &mut egui::Ui, theme: &Theme, job: &Job) -> QueueViewAction {
    let mut action = QueueViewAction::None;

    card(
        ui,
        theme,
        CardProps::default()
            .size(CardSize::Size2)
            .variant(CardVariant::Surface),
        |ui| {
            ui.horizontal(|ui| {
                text(ui, theme, TextProps::new(job.url.as_str()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (label, variant) = status_badge(&job.status);
                    badge(ui, theme, BadgeProps::new(label).variant(variant));
                });
            });

            match &job.status {
                JobStatus::Downloading {
                    progress: ratio,
                    speed,
                    eta,
                } => {
                    ui.add_space(6.0);
                    progress(
                        ui,
                        theme,
                        ProgressProps::new(Some(progress_percent(*ratio))),
                    );
                    ui.horizontal(|ui| {
                        if let Some(speed) = speed {
                            text(
                                ui,
                                theme,
                                TextProps::new(format!("速度: {speed}"))
                                    .size(12.0)
                                    .color(TypographyColor::Muted),
                            );
                        }
                        if let Some(eta) = eta {
                            text(
                                ui,
                                theme,
                                TextProps::new(format!("残り時間: {eta}"))
                                    .size(12.0)
                                    .color(TypographyColor::Muted),
                            );
                        }
                    });
                }
                JobStatus::Converting { progress: ratio } => {
                    ui.add_space(6.0);
                    progress(
                        ui,
                        theme,
                        ProgressProps::new(Some(progress_percent(*ratio))),
                    );
                }
                JobStatus::Failed { message } => {
                    ui.add_space(6.0);
                    ui.colored_label(theme.palette.destructive, message.as_str());
                }
                JobStatus::Queued | JobStatus::Completed | JobStatus::Cancelled => {}
            }

            let can_cancel = matches!(
                job.status,
                JobStatus::Queued | JobStatus::Downloading { .. } | JobStatus::Converting { .. }
            );
            let can_retry = matches!(job.status, JobStatus::Failed { .. });

            if can_cancel || can_retry {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if can_cancel {
                        let clicked = Button::new("キャンセル")
                            .variant(ButtonVariant::Outline)
                            .radius(ButtonRadius::Medium)
                            .show(ui, theme)
                            .clicked();
                        if clicked {
                            action = QueueViewAction::Cancel(job.id);
                        }
                    }
                    if can_retry {
                        let clicked = Button::new("再試行")
                            .variant(ButtonVariant::Soft)
                            .radius(ButtonRadius::Medium)
                            .show(ui, theme)
                            .clicked();
                        if clicked {
                            action = QueueViewAction::Retry(job.id);
                        }
                    }
                });
            }
        },
    );

    action
}

/// 状態ごとの日本語ラベルとバッジの見た目を対応付ける。
fn status_badge(status: &JobStatus) -> (&'static str, BadgeVariant) {
    match status {
        JobStatus::Queued => ("待機中", BadgeVariant::Secondary),
        JobStatus::Downloading { .. } => ("取得中", BadgeVariant::Default),
        JobStatus::Converting { .. } => ("変換中", BadgeVariant::Default),
        JobStatus::Completed => ("完了", BadgeVariant::Secondary),
        JobStatus::Failed { .. } => ("失敗", BadgeVariant::Destructive),
        JobStatus::Cancelled => ("キャンセル済み", BadgeVariant::Outline),
    }
}

/// `JobStatus`の進捗は0.0〜1.0の比率で保持しているため、`ProgressProps`が
/// 期待する0〜100のパーセント値へスケール変換する。パース元(yt-dlpの出力)が
/// 想定外の値(負数や1.0超)を返しても描画が壊れないようクランプする。
fn progress_percent(ratio: f32) -> f32 {
    (ratio * 100.0).clamp(0.0, 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_percent_converts_ratio_to_percentage() {
        assert_eq!(progress_percent(0.0), 0.0);
        assert_eq!(progress_percent(0.5), 50.0);
        assert_eq!(progress_percent(1.0), 100.0);
    }

    #[test]
    fn progress_percent_clamps_out_of_range_values() {
        assert_eq!(progress_percent(-0.5), 0.0);
        assert_eq!(progress_percent(1.5), 100.0);
    }

    #[test]
    fn status_badge_maps_each_status_to_japanese_label() {
        assert_eq!(status_badge(&JobStatus::Queued).0, "待機中");
        assert_eq!(
            status_badge(&JobStatus::Downloading {
                progress: 0.0,
                speed: None,
                eta: None
            })
            .0,
            "取得中"
        );
        assert_eq!(
            status_badge(&JobStatus::Converting { progress: 0.0 }).0,
            "変換中"
        );
        assert_eq!(status_badge(&JobStatus::Completed).0, "完了");
        assert_eq!(
            status_badge(&JobStatus::Failed {
                message: "エラー".to_string()
            })
            .0,
            "失敗"
        );
        assert_eq!(status_badge(&JobStatus::Cancelled).0, "キャンセル済み");
    }

    #[test]
    fn queue_view_action_default_is_none() {
        assert_eq!(QueueViewAction::default(), QueueViewAction::None);
    }
}
