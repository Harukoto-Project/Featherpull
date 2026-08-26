//! アプリ全体の状態管理と画面結線。
//!
//! `JobQueue`は内部で`tokio::spawn`を呼ぶため、eframeの同期的な描画スレッドから
//! そのまま呼び出すとパニックする(tokioランタイムのコンテキストが無いため)。
//! そのため`App`は`tokio::runtime::Runtime`を保持し、キュー操作の直前に
//! `Runtime::enter()`でスレッドローカルにコンテキストを設定してから呼び出す。
//! ランタイム自体をブロックして使う(`block_on`)わけではなく、あくまで
//! `tokio::spawn`が要求するコンテキストを一時的に持たせるだけの用途。

use std::path::PathBuf;
use std::time::Duration;

use egui_shadcn::{
    HeadingAs, HeadingProps, LightSwitchProps, SeparatorProps, TabItem, TabsProps, TextProps,
    Theme, TypographyColor, heading, light_switch, separator, tabs, text,
};
use tokio::sync::mpsc;

use crate::config::{self, Config};
use crate::core::binary_manager::{self, VersionCheckResult};
use crate::core::job::Job;
use crate::core::queue::{JobEvent, JobQueue, ProcessJobExecutor};
use crate::theme;
use crate::ui::download_view::{self, DownloadFormState};
use crate::ui::queue_view::{self, QueueViewAction};
use crate::ui::settings_view::{self, LogBuffer, LogEntry, LogLevel, LogPanelAction};

/// 起動時に一度だけyt-dlp/ffmpegの用意を試み、結果をログパネルへ流すバックグラウンドタスク。
///
/// ダウンロード開始時点でまだ準備が終わっていなくても、`ProcessJobExecutor`には
/// 最終的な設置先パスを先に渡してあるため、準備完了後は次回のジョブから正しく動作する。
/// 準備前にジョブが実行されて失敗した場合は、既存の`JobStatus::Failed`経路で
/// ユーザーに通知される(このタスク自体で二重にリトライ制御はしない)。
async fn prepare_binaries(log_tx: mpsc::UnboundedSender<LogEntry>, auto_update_check: bool) {
    match binary_manager::ensure_ytdlp_installed().await {
        Ok(path) => {
            let _ = log_tx.send(LogEntry::new(
                LogLevel::Info,
                format!("yt-dlpを準備しました: {}", path.display()),
            ));
        }
        Err(err) => {
            let _ = log_tx.send(LogEntry::new(
                LogLevel::Error,
                format!("yt-dlpの準備に失敗しました: {err}"),
            ));
        }
    }

    match binary_manager::ensure_ffmpeg_installed().await {
        Ok((ffmpeg, ffprobe)) => {
            let _ = log_tx.send(LogEntry::new(
                LogLevel::Info,
                format!(
                    "ffmpegを準備しました: {} / {}",
                    ffmpeg.display(),
                    ffprobe.display()
                ),
            ));
        }
        Err(err) => {
            let _ = log_tx.send(LogEntry::new(
                LogLevel::Error,
                format!("ffmpegの準備に失敗しました: {err}"),
            ));
        }
    }

    if !auto_update_check {
        return;
    }

    report_update_check(
        &log_tx,
        "yt-dlp",
        binary_manager::check_ytdlp_update().await,
    );
    report_update_check(
        &log_tx,
        "ffmpeg",
        binary_manager::check_ffmpeg_update().await,
    );
}

/// バージョン確認結果をログパネル向けの日本語メッセージへ変換して送る。
fn report_update_check(
    log_tx: &mpsc::UnboundedSender<LogEntry>,
    name: &str,
    result: Result<VersionCheckResult, binary_manager::BinaryManagerError>,
) {
    let entry = match result {
        Ok(VersionCheckResult::UpToDate) => {
            LogEntry::new(LogLevel::Info, format!("{name}は最新版です"))
        }
        Ok(VersionCheckResult::UpdateAvailable { current, latest }) => LogEntry::new(
            LogLevel::Warn,
            format!("{name}の更新があります: {current} -> {latest}"),
        ),
        Ok(VersionCheckResult::NotInstalled { latest }) => LogEntry::new(
            LogLevel::Info,
            format!("{name}の最新版は{latest}です(まだインストールされていません)"),
        ),
        Err(err) => LogEntry::new(
            LogLevel::Error,
            format!("{name}の更新確認に失敗しました: {err}"),
        ),
    };
    let _ = log_tx.send(entry);
}

/// ログエントリをログディレクトリ内のファイルへ追記する。
///
/// ディレクトリ作成やファイルI/Oに失敗しても、GUI側のログ表示は継続させたいため
/// エラーは無視する(ここで失敗を通知しようとすると`push_log`との再帰呼び出しに
/// なってしまうため、静かに諦める設計にしている)。
fn append_log_to_file(entry: &LogEntry) {
    let Ok(dir) = config::logs_dir() else {
        return;
    };
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }

    use std::io::Write;
    let line = format!(
        "[{}] {} {}\n",
        entry.timestamp.format("%Y-%m-%d %H:%M:%S"),
        entry.level.label(),
        entry.message
    );
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("featherpull.log"))
    {
        let _ = file.write_all(line.as_bytes());
    }
}

/// 保存先未指定(空文字列)の場合に使うOS標準のダウンロードフォルダを解決する。
/// 解決できない環境では作業ディレクトリにフォールバックする。
fn resolve_output_dir(config: &Config) -> PathBuf {
    if !config.general.save_dir.is_empty() {
        return PathBuf::from(&config.general.save_dir);
    }
    directories::UserDirs::new()
        .and_then(|dirs| dirs.download_dir().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// アプリ全体の状態。eframe::Appの実装本体。
pub struct App {
    runtime: tokio::runtime::Runtime,
    config: Config,
    queue: JobQueue<ProcessJobExecutor>,
    job_events: mpsc::UnboundedReceiver<JobEvent>,
    log_events: mpsc::UnboundedReceiver<LogEntry>,
    jobs: Vec<Job>,
    download_form: DownloadFormState,
    log_buffer: LogBuffer,
    active_tab: String,
    dark_mode: bool,
}

impl App {
    pub fn new() -> Self {
        let config = Config::load().unwrap_or_default();

        let runtime = tokio::runtime::Runtime::new().expect("tokioランタイムの起動に失敗しました");

        let ytdlp_path = binary_manager::resolve_ytdlp_path(&config)
            .unwrap_or_else(|_| PathBuf::from("yt-dlp.exe"));
        let ffmpeg_path = binary_manager::resolve_ffmpeg_path(&config)
            .unwrap_or_else(|_| PathBuf::from("ffmpeg.exe"));
        let executor = ProcessJobExecutor::new(ytdlp_path, ffmpeg_path);
        let (queue, job_events) = JobQueue::new(executor, config.general.concurrency);

        let (log_tx, log_events) = mpsc::unbounded_channel();
        {
            let _enter = runtime.enter();
            tokio::spawn(prepare_binaries(log_tx, config.binaries.auto_update_check));
        }

        let mut app = Self {
            runtime,
            config,
            queue,
            job_events,
            log_events,
            jobs: Vec::new(),
            download_form: DownloadFormState::default(),
            log_buffer: LogBuffer::default(),
            active_tab: "download".to_string(),
            dark_mode: false,
        };
        app.push_log(LogEntry::new(LogLevel::Info, "Featherpullを起動しました"));
        app
    }

    /// ログをGUIのリングバッファへ追加し、設定で有効な場合はファイルへも追記する。
    fn push_log(&mut self, entry: LogEntry) {
        if self.config.logging.file_logging {
            append_log_to_file(&entry);
        }
        self.log_buffer.push(entry);
    }

    fn drain_job_events(&mut self) {
        while let Ok(event) = self.job_events.try_recv() {
            match event {
                JobEvent::Progress { job_id, status }
                | JobEvent::StatusChanged { job_id, status } => {
                    if let Some(job) = self.jobs.iter_mut().find(|job| job.id == job_id) {
                        job.status = status;
                    }
                }
                JobEvent::Log { job_id, message } => {
                    let message = match job_id {
                        Some(id) => format!("[Job #{}] {message}", id.get()),
                        None => message,
                    };
                    self.push_log(LogEntry::new(LogLevel::Info, message));
                }
                JobEvent::Finished { job_id, result } => {
                    let entry = match result {
                        Ok(()) => LogEntry::new(
                            LogLevel::Info,
                            format!("Job #{} が完了しました", job_id.get()),
                        ),
                        Err(message) => LogEntry::new(
                            LogLevel::Warn,
                            format!("Job #{} が終了しました: {message}", job_id.get()),
                        ),
                    };
                    self.push_log(entry);
                }
            }
        }
    }

    fn drain_log_events(&mut self) {
        while let Ok(entry) = self.log_events.try_recv() {
            self.push_log(entry);
        }
    }

    fn show_header(&mut self, ui: &mut egui::Ui, theme: &Theme) {
        ui.horizontal(|ui| {
            heading(
                ui,
                theme,
                HeadingProps::new("Featherpull").as_tag(HeadingAs::H1),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if light_switch(ui, theme, LightSwitchProps::new(self.dark_mode)).clicked() {
                    self.dark_mode = !self.dark_mode;
                }
            });
        });

        text(
            ui,
            theme,
            TextProps::new("yt-dlp / ffmpeg GUIラッパー").color(TypographyColor::Muted),
        );
    }

    fn show_tabs(&mut self, ui: &mut egui::Ui, theme: &Theme) {
        let items = [
            TabItem::new("download", "ダウンロード"),
            TabItem::new("queue", "キュー"),
            TabItem::new("settings", "設定"),
        ];

        // `render_content`はselfを捕捉できないため、アクティブなタブIDだけを返し、
        // 実際の描画はtabs呼び出しの外(このメソッド内)で行う。
        let content = tabs(
            ui,
            theme,
            TabsProps::new(egui::Id::new("main-tabs"), &items, &mut self.active_tab),
            |_ui, tab| tab.id.clone(),
        )
        .content;

        ui.add_space(12.0);

        match content.as_str() {
            "download" => self.show_download_tab(ui, theme),
            "queue" => self.show_queue_tab(ui, theme),
            "settings" => self.show_settings_tab(ui, theme),
            _ => {}
        }
    }

    fn show_download_tab(&mut self, ui: &mut egui::Ui, theme: &Theme) {
        let submitted = download_view::show(ui, theme, &mut self.download_form);
        if submitted {
            let output_dir = resolve_output_dir(&self.config);
            let jobs = self.download_form.build_jobs(output_dir);

            let _enter = self.runtime.enter();
            for job in jobs {
                self.jobs.push(job.clone());
                self.queue.enqueue(job);
            }
            self.download_form.urls_input.clear();
        }
    }

    fn show_queue_tab(&mut self, ui: &mut egui::Ui, theme: &Theme) {
        match queue_view::show(ui, theme, &self.jobs) {
            QueueViewAction::Cancel(job_id) => {
                let _enter = self.runtime.enter();
                self.queue.cancel(job_id);
            }
            QueueViewAction::Retry(job_id) => {
                let _enter = self.runtime.enter();
                self.queue.retry(job_id);
            }
            QueueViewAction::None => {}
        }
    }

    fn show_settings_tab(&mut self, ui: &mut egui::Ui, theme: &Theme) {
        let changed = settings_view::show(ui, theme, &mut self.config);
        if changed {
            {
                let _enter = self.runtime.enter();
                self.queue.set_concurrency(self.config.general.concurrency);
            }
            if let Err(err) = self.config.save() {
                self.push_log(LogEntry::new(
                    LogLevel::Error,
                    format!("設定の保存に失敗しました: {err}"),
                ));
            }
        }

        ui.add_space(16.0);
        separator(ui, theme, SeparatorProps::default());
        ui.add_space(16.0);

        let action = {
            let logs = self.log_buffer.as_slice();
            settings_view::show_log_panel(ui, theme, logs)
        };
        if action == LogPanelAction::ClearRequested {
            self.log_buffer.clear();
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_job_events();
        self.drain_log_events();

        let theme = theme::build_theme(self.dark_mode);
        let frame = egui::Frame::default()
            .fill(theme.palette.background)
            .inner_margin(egui::Margin::same(24));

        egui::CentralPanel::default().frame(frame).show(ctx, |ui| {
            self.show_header(ui, &theme);
            ui.add_space(16.0);
            separator(ui, &theme, SeparatorProps::default());
            ui.add_space(16.0);

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    self.show_tabs(ui, &theme);
                });
        });

        // 進捗イベントはtokioタスク側から非同期に届くため、入力が無い間も
        // 一定間隔でポーリングできるよう再描画を要求しておく。
        ctx.request_repaint_after(Duration::from_millis(200));
    }
}
