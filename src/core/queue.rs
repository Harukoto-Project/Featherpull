//! ジョブキュー・並列実行スケジューラ。
//!
//! `tokio`ランタイム上でジョブ(ダウンロード→必要なら変換)を非同期に実行する。
//!
//! - **同時実行数制御**: `Semaphore`ではなく、実行中ジョブ数(`running.len()`)と
//!   設定値を比較するだけの単純な方式にしている。設定変更を「次にスロットが空いた
//!   時点から新しい上限を適用する」という仕様(実行中ジョブを途中で止めない)に
//!   素直に合致し、`Semaphore`の許可数を実行時に増減させる仕組み(permitの`forget`等)
//!   より単純で追いやすいため。
//! - **キャンセルと孫プロセス**: `tokio::process::Child::kill()`は直接の子プロセス
//!   (yt-dlp/ffmpeg本体)のみを終了させる。yt-dlpはフォーマット結合のためにffmpegを
//!   孫プロセスとして内部起動することがあり、Windowsではプロセスの親子関係だけで
//!   ツリー終了は保証されない(POSIXのプロセスグループやWindowsのJob Object相当の
//!   仕組みが別途必要)。本実装ではその確実な終了までは行わず、既知の制約として
//!   残している(Job Object APIの直接呼び出し等は本タスクの範囲外と判断した)。

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use tokio::sync::{mpsc, watch};

use super::ffmpeg::{run_convert, spawn_ffmpeg};
use super::job::{AudioFormat, FormatSelection, Job, JobId, JobStatus, VideoQuality};
use super::ytdlp::YtdlpProcess;

/// 同時実行数として受け付ける範囲。GUIの設定画面もこの範囲でスライダー等を
/// 制限する想定のため、定数として公開しておく。
pub const MIN_CONCURRENCY: u32 = 1;
pub const MAX_CONCURRENCY: u32 = 10;
/// `Config::default`の既定値と一致させるための公開定数。GUI側は`Config`側の
/// デフォルトをそのまま使うため現状は未参照だが、設定リセットUI等を追加する際に
/// 同じ値をここから取れるようにしておく。
#[allow(dead_code)]
pub const DEFAULT_CONCURRENCY: u32 = 3;

fn clamp_concurrency(value: u32) -> u32 {
    value.clamp(MIN_CONCURRENCY, MAX_CONCURRENCY)
}

/// GUIスレッドへ通知するジョブ関連イベント。
///
/// `Progress`(進捗)・`Log`(ログ)・`StatusChanged`(状態変化)・`Finished`(完了)の
/// 4種類の意味合いを分けているのは、GUI側で「進捗バーの更新」「ログパネルへの追記」
/// 「一覧上のステータス表示切り替え」「完了時の後処理(通知等)」をそれぞれ独立に
/// 実装できるようにするため。
#[derive(Debug, Clone)]
pub enum JobEvent {
    Progress {
        job_id: JobId,
        status: JobStatus,
    },
    Log {
        job_id: Option<JobId>,
        message: String,
    },
    StatusChanged {
        job_id: JobId,
        status: JobStatus,
    },
    Finished {
        job_id: JobId,
        result: Result<(), String>,
    },
}

/// 1ジョブの実行結果。`JobStatus`と1対1ではなく、キュー側が`JobStatus`と
/// `JobEvent::Finished`の両方をこの値から組み立てる。
///
/// `JobExecutor::execute`の戻り値型として公開APIに現れるため`pub`にしている。
#[derive(Debug, Clone)]
pub enum JobOutcome {
    Completed,
    Cancelled,
    Failed(String),
}

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// 1ジョブを実際に処理する実行エンジンの抽象。
///
/// 実プロセス(yt-dlp/ffmpeg)を起動する本番実装(`ProcessJobExecutor`)と、テストで
/// プロセスを起動せずにスケジューラのロジックだけを検証するモック実装を差し替え
/// 可能にするために切り出している。
pub trait JobExecutor: Send + Sync + 'static {
    fn execute(
        &self,
        job: Job,
        cancel: CancelSignal,
        events: mpsc::UnboundedSender<JobEvent>,
    ) -> BoxFuture<JobOutcome>;
}

/// `watch`チャンネルを介したキャンセル通知の受信側ラッパー。
///
/// `tokio_util::sync::CancellationToken`は依存関係に無いため、この用途に必要な
/// 最小限の機能(「キャンセルされるまで待つ」)だけを自前で実装している。
#[derive(Clone)]
pub struct CancelSignal {
    rx: watch::Receiver<bool>,
}

impl CancelSignal {
    fn new(rx: watch::Receiver<bool>) -> Self {
        Self { rx }
    }

    /// キャンセルが通知されるまで待機する。すでに通知済みなら即座に返る。
    pub async fn cancelled(&mut self) {
        wait_until_true(&mut self.rx).await;
    }
}

async fn wait_until_true(rx: &mut watch::Receiver<bool>) {
    loop {
        if *rx.borrow() {
            return;
        }
        if rx.changed().await.is_err() {
            // 送信側(キュー)が破棄された場合。待ち続ける意味がないため終了する。
            return;
        }
    }
}

struct RunningEntry {
    cancel_tx: watch::Sender<bool>,
}

struct QueueState {
    concurrency: u32,
    pending: VecDeque<Job>,
    running: HashMap<JobId, RunningEntry>,
    /// 完了・失敗・キャンセル済みジョブの最終状態。`retry`が同一設定で再キューする際に
    /// 元のURL・フォーマット指定・出力先を参照する必要があるため保持している。
    finished: HashMap<JobId, Job>,
}

/// 待機中ジョブのキューと実行中ジョブの管理を行う本体。
///
/// `Arc`で包んだ内部状態を持つため`Clone`で安価に複製でき、複製したハンドルを
/// 実行中タスク側にも渡すことで、タスク完了時に自分自身をスケジューラの状態から
/// 取り除いて次のジョブを開始する、という流れを実現している。
pub struct JobQueue<E> {
    executor: Arc<E>,
    state: Arc<Mutex<QueueState>>,
    events_tx: mpsc::UnboundedSender<JobEvent>,
}

impl<E> Clone for JobQueue<E> {
    fn clone(&self) -> Self {
        Self {
            executor: Arc::clone(&self.executor),
            state: Arc::clone(&self.state),
            events_tx: self.events_tx.clone(),
        }
    }
}

impl<E: JobExecutor> JobQueue<E> {
    /// キューを作成する。戻り値のレシーバーからGUI側が`JobEvent`を受け取る。
    pub fn new(executor: E, concurrency: u32) -> (Self, mpsc::UnboundedReceiver<JobEvent>) {
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let queue = Self {
            executor: Arc::new(executor),
            state: Arc::new(Mutex::new(QueueState {
                concurrency: clamp_concurrency(concurrency),
                pending: VecDeque::new(),
                running: HashMap::new(),
                finished: HashMap::new(),
            })),
            events_tx,
        };
        (queue, events_rx)
    }

    /// 同時実行数を変更する。既に実行中のジョブを止めることはせず、次にスロットが
    /// 空いた時点から新しい上限が適用される。
    pub fn set_concurrency(&self, concurrency: u32) {
        {
            let mut state = self.lock_state();
            state.concurrency = clamp_concurrency(concurrency);
        }
        self.try_schedule();
    }

    /// ジョブをキューに追加する。実行スロットが空いていれば即座に開始する。
    pub fn enqueue(&self, job: Job) {
        {
            let mut state = self.lock_state();
            state.pending.push_back(job);
        }
        self.try_schedule();
    }

    /// ジョブをキャンセルする。
    ///
    /// 待機中であれば即座にキューから取り除く。実行中であればキャンセル通知を送るのみで、
    /// 実際に`Cancelled`状態へ遷移するのは子プロセスの終了を待った実行タスク自身が行う
    /// (プロセスの終了は非同期にしか確認できないため)。
    pub fn cancel(&self, job_id: JobId) {
        let removed_pending = {
            let mut state = self.lock_state();
            let position = state.pending.iter().position(|job| job.id == job_id);
            position.and_then(|position| state.pending.remove(position))
        };

        if let Some(mut job) = removed_pending {
            job.status = JobStatus::Cancelled;
            {
                let mut state = self.lock_state();
                state.finished.insert(job_id, job);
            }
            let _ = self.events_tx.send(JobEvent::StatusChanged {
                job_id,
                status: JobStatus::Cancelled,
            });
            let _ = self.events_tx.send(JobEvent::Finished {
                job_id,
                result: Err("キャンセルされました".to_string()),
            });
            return;
        }

        let state = self.lock_state();
        if let Some(entry) = state.running.get(&job_id) {
            let _ = entry.cancel_tx.send(true);
        }
    }

    /// 失敗済みジョブを同一設定(URL・フォーマット指定・出力先)で再キューする。
    ///
    /// 対象が失敗状態で保持されていない場合(実行中・待機中・未知のID・完了済み等)は
    /// 何もせず`false`を返す。
    pub fn retry(&self, job_id: JobId) -> bool {
        let requeued = {
            let mut state = self.lock_state();
            let is_failed = matches!(
                state.finished.get(&job_id).map(|job| &job.status),
                Some(JobStatus::Failed { .. })
            );
            if is_failed {
                state.finished.remove(&job_id).map(|mut job| {
                    job.status = JobStatus::Queued;
                    job
                })
            } else {
                None
            }
        };

        let Some(job) = requeued else {
            return false;
        };

        {
            let mut state = self.lock_state();
            state.pending.push_back(job);
        }
        let _ = self.events_tx.send(JobEvent::StatusChanged {
            job_id,
            status: JobStatus::Queued,
        });
        self.try_schedule();
        true
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, QueueState> {
        self.state.lock().expect("queue stateのロックに失敗した")
    }

    /// 実行スロットが空いている限り、待機中ジョブを先頭から開始する。
    fn try_schedule(&self) {
        loop {
            let next = {
                let mut state = self.lock_state();
                if state.running.len() >= state.concurrency as usize {
                    None
                } else {
                    state.pending.pop_front()
                }
            };

            match next {
                Some(job) => self.spawn_job(job),
                None => break,
            }
        }
    }

    fn spawn_job(&self, mut job: Job) {
        let job_id = job.id;
        let initial_status = JobStatus::Downloading {
            progress: 0.0,
            speed: None,
            eta: None,
        };
        job.status = initial_status.clone();

        let (cancel_tx, cancel_rx) = watch::channel(false);
        {
            let mut state = self.lock_state();
            state.running.insert(job_id, RunningEntry { cancel_tx });
        }
        let _ = self.events_tx.send(JobEvent::StatusChanged {
            job_id,
            status: initial_status,
        });

        let executor = Arc::clone(&self.executor);
        let events_tx = self.events_tx.clone();
        let completion_handle = self.clone();
        let cancel_signal = CancelSignal::new(cancel_rx);

        tokio::spawn(async move {
            let outcome = executor
                .execute(job.clone(), cancel_signal, events_tx)
                .await;
            completion_handle.on_job_finished(job, outcome);
        });
    }

    fn on_job_finished(&self, mut job: Job, outcome: JobOutcome) {
        let job_id = job.id;
        {
            let mut state = self.lock_state();
            state.running.remove(&job_id);
        }

        let (status, result) = match outcome {
            JobOutcome::Completed => (JobStatus::Completed, Ok(())),
            JobOutcome::Cancelled => (
                JobStatus::Cancelled,
                Err("キャンセルされました".to_string()),
            ),
            JobOutcome::Failed(message) => (
                JobStatus::Failed {
                    message: message.clone(),
                },
                Err(message),
            ),
        };
        job.status = status.clone();

        {
            let mut state = self.lock_state();
            state.finished.insert(job_id, job);
        }

        let _ = self
            .events_tx
            .send(JobEvent::StatusChanged { job_id, status });
        let _ = self.events_tx.send(JobEvent::Finished { job_id, result });

        self.try_schedule();
    }
}

#[cfg(test)]
impl<E: JobExecutor> JobQueue<E> {
    fn pending_ids(&self) -> Vec<JobId> {
        let state = self.lock_state();
        state.pending.iter().map(|job| job.id).collect()
    }

    fn running_count(&self) -> usize {
        self.lock_state().running.len()
    }

    fn concurrency(&self) -> u32 {
        self.lock_state().concurrency
    }
}

/// yt-dlp/ffmpegの実プロセスを起動してジョブを処理する本番用の実行エンジン。
pub struct ProcessJobExecutor {
    ytdlp_path: PathBuf,
    ffmpeg_path: PathBuf,
}

impl ProcessJobExecutor {
    pub fn new(ytdlp_path: PathBuf, ffmpeg_path: PathBuf) -> Self {
        Self {
            ytdlp_path,
            ffmpeg_path,
        }
    }
}

impl JobExecutor for ProcessJobExecutor {
    fn execute(
        &self,
        job: Job,
        cancel: CancelSignal,
        events: mpsc::UnboundedSender<JobEvent>,
    ) -> BoxFuture<JobOutcome> {
        let ytdlp_path = self.ytdlp_path.clone();
        let ffmpeg_path = self.ffmpeg_path.clone();
        Box::pin(run_job(ytdlp_path, ffmpeg_path, job, cancel, events))
    }
}

/// 1ジョブのライフサイクル(ダウンロード→音声のみ抽出が必要なら変換)を実行する。
///
/// `tokio::select!`はキャンセル通知が先に届いた場合、ダウンロード/変換待ちの
/// フューチャーを破棄するだけで子プロセス自体は終了させないため、各分岐で
/// 明示的に`kill()`を呼んでいる(モジュール先頭のドキュメント参照。孫プロセスまでの
/// 確実な終了は既知の制約として残す)。
async fn run_job(
    ytdlp_path: PathBuf,
    ffmpeg_path: PathBuf,
    job: Job,
    mut cancel: CancelSignal,
    events: mpsc::UnboundedSender<JobEvent>,
) -> JobOutcome {
    let job_id = job.id;
    let extra_args = build_ytdlp_args(&job);

    let mut process = match YtdlpProcess::spawn(&ytdlp_path, &job.url, &extra_args) {
        Ok(process) => process,
        Err(err) => return JobOutcome::Failed(err.to_string()),
    };

    let progress_events = events.clone();
    let download_result = tokio::select! {
        result = process.wait_with_progress(move |status| {
            let _ = progress_events.send(JobEvent::Progress { job_id, status });
        }) => result,
        _ = cancel.cancelled() => {
            let _ = process.kill().await;
            return JobOutcome::Cancelled;
        }
    };

    if let Err(err) = download_result {
        return JobOutcome::Failed(err.to_string());
    }

    let Some(audio_format) = job.audio_only else {
        return JobOutcome::Completed;
    };

    let _ = events.send(JobEvent::Log {
        job_id: Some(job_id),
        message: "ダウンロードが完了しました。音声抽出の変換を開始します".to_string(),
    });

    let convert_args = build_ffmpeg_args(&job, audio_format);
    let mut child = match spawn_ffmpeg(&ffmpeg_path, &convert_args) {
        Ok(child) => child,
        Err(err) => return JobOutcome::Failed(err.to_string()),
    };

    let progress_events = events.clone();
    let convert_result = tokio::select! {
        result = run_convert(&mut child, None, move |status| {
            let _ = progress_events.send(JobEvent::Progress { job_id, status });
        }) => result,
        _ = cancel.cancelled() => {
            let _ = child.kill().await;
            return JobOutcome::Cancelled;
        }
    };

    match convert_result {
        Ok(()) => JobOutcome::Completed,
        Err(err) => JobOutcome::Failed(err.to_string()),
    }
}

fn build_ytdlp_args(job: &Job) -> Vec<String> {
    let format = match &job.format_selection {
        FormatSelection::Preset(quality) => preset_format_string(*quality).to_string(),
        FormatSelection::Custom(raw) => raw.clone(),
    };
    vec![
        "-f".to_string(),
        format,
        "-P".to_string(),
        job.output_dir.to_string_lossy().into_owned(),
    ]
}

fn preset_format_string(quality: VideoQuality) -> &'static str {
    match quality {
        VideoQuality::Best => "bestvideo+bestaudio/best",
        VideoQuality::P1080 => "bestvideo[height<=1080]+bestaudio/best[height<=1080]",
        VideoQuality::P720 => "bestvideo[height<=720]+bestaudio/best[height<=720]",
        VideoQuality::AudioOnly => "bestaudio",
    }
}

/// ffmpegへ渡す変換用引数。入力ファイル名の解決(yt-dlpの出力ファイル名テンプレートに
/// 依存する)は本タスクの範囲外のため、「ダウンロード完了後に変換ステップへ進む」という
/// 制御フローが成立することを示す最小限の内容にとどめている。
fn build_ffmpeg_args(job: &Job, audio_format: AudioFormat) -> Vec<String> {
    let output_path = job.output_dir.join(format!(
        "{}.{}",
        job.id.get(),
        audio_extension(audio_format)
    ));
    vec![
        "-y".to_string(),
        "-i".to_string(),
        "-".to_string(),
        output_path.to_string_lossy().into_owned(),
    ]
}

fn audio_extension(format: AudioFormat) -> &'static str {
    match format {
        AudioFormat::Mp3 => "mp3",
        AudioFormat::M4a => "m4a",
        AudioFormat::Opus => "opus",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn test_job(url: &str) -> Job {
        Job::new(
            url.to_string(),
            FormatSelection::Preset(VideoQuality::Best),
            PathBuf::from("C:/downloads"),
        )
    }

    /// `release`がtrueになるかキャンセルされるまで完了を保留するモック実行エンジン。
    /// 実プロセスを起動せずにスケジューラの並列数制御・キャンセル挙動を検証するために使う。
    struct GatedExecutor {
        current: Arc<AtomicUsize>,
        max_observed: Arc<AtomicUsize>,
        release: watch::Receiver<bool>,
    }

    impl JobExecutor for GatedExecutor {
        fn execute(
            &self,
            _job: Job,
            mut cancel: CancelSignal,
            _events: mpsc::UnboundedSender<JobEvent>,
        ) -> BoxFuture<JobOutcome> {
            let current = Arc::clone(&self.current);
            let max_observed = Arc::clone(&self.max_observed);
            let mut release = self.release.clone();
            Box::pin(async move {
                let running_now = current.fetch_add(1, Ordering::SeqCst) + 1;
                max_observed.fetch_max(running_now, Ordering::SeqCst);

                let outcome = tokio::select! {
                    _ = wait_until_true(&mut release) => JobOutcome::Completed,
                    _ = cancel.cancelled() => JobOutcome::Cancelled,
                };

                current.fetch_sub(1, Ordering::SeqCst);
                outcome
            })
        }
    }

    fn gated_executor() -> (GatedExecutor, watch::Sender<bool>, Arc<AtomicUsize>) {
        let current = Arc::new(AtomicUsize::new(0));
        let max_observed = Arc::new(AtomicUsize::new(0));
        let (release_tx, release_rx) = watch::channel(false);
        let executor = GatedExecutor {
            current: Arc::clone(&current),
            max_observed: Arc::clone(&max_observed),
            release: release_rx,
        };
        (executor, release_tx, max_observed)
    }

    /// 即座に失敗を返すだけのモック実行エンジン。`retry`のふるまい確認に使う。
    struct AlwaysFailExecutor;

    impl JobExecutor for AlwaysFailExecutor {
        fn execute(
            &self,
            _job: Job,
            _cancel: CancelSignal,
            _events: mpsc::UnboundedSender<JobEvent>,
        ) -> BoxFuture<JobOutcome> {
            Box::pin(async { JobOutcome::Failed("mock failure".to_string()) })
        }
    }

    async fn recv_until<F>(rx: &mut mpsc::UnboundedReceiver<JobEvent>, mut predicate: F)
    where
        F: FnMut(&JobEvent) -> bool,
    {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let event = rx.recv().await.expect("チャンネルが閉じられた");
                if predicate(&event) {
                    return;
                }
            }
        })
        .await
        .expect("期待するイベントが時間内に届かなかった");
    }

    #[tokio::test]
    async fn concurrency_limit_is_never_exceeded() {
        let (executor, release_tx, max_observed) = gated_executor();
        let (queue, mut events) = JobQueue::new(executor, 3);

        for i in 0..6 {
            queue.enqueue(test_job(&format!("https://example.com/{i}")));
        }

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(queue.running_count(), 3);

        release_tx.send(true).expect("release送信に失敗した");

        for _ in 0..6 {
            recv_until(&mut events, |event| {
                matches!(event, JobEvent::Finished { .. })
            })
            .await;
        }

        assert!(max_observed.load(Ordering::SeqCst) <= 3);
        assert_eq!(queue.running_count(), 0);
    }

    #[tokio::test]
    async fn enqueue_appends_to_pending_queue_in_order() {
        let (executor, _release_tx, _max_observed) = gated_executor();
        let (queue, _events) = JobQueue::new(executor, 1);

        let job1 = test_job("https://example.com/1");
        let job2 = test_job("https://example.com/2");
        let job3 = test_job("https://example.com/3");
        let (id2, id3) = (job2.id, job3.id);

        queue.enqueue(job1);
        queue.enqueue(job2);
        queue.enqueue(job3);

        assert_eq!(queue.running_count(), 1);
        assert_eq!(queue.pending_ids(), vec![id2, id3]);
    }

    #[tokio::test]
    async fn cancel_removes_pending_job_immediately() {
        let (executor, _release_tx, _max_observed) = gated_executor();
        let (queue, mut events) = JobQueue::new(executor, 1);

        let job1 = test_job("https://example.com/1");
        let job2 = test_job("https://example.com/2");
        let job2_id = job2.id;

        queue.enqueue(job1);
        queue.enqueue(job2);

        assert_eq!(queue.pending_ids(), vec![job2_id]);

        queue.cancel(job2_id);

        assert_eq!(queue.pending_ids(), Vec::<JobId>::new());
        recv_until(&mut events, |event| {
            matches!(
                event,
                JobEvent::Finished { job_id, result: Err(_) } if *job_id == job2_id
            )
        })
        .await;
    }

    #[tokio::test]
    async fn cancel_running_job_marks_it_cancelled() {
        let (executor, _release_tx, _max_observed) = gated_executor();
        let (queue, mut events) = JobQueue::new(executor, 1);

        let job = test_job("https://example.com/1");
        let job_id = job.id;
        queue.enqueue(job);

        assert_eq!(queue.running_count(), 1);

        queue.cancel(job_id);

        recv_until(&mut events, |event| {
            matches!(
                event,
                JobEvent::StatusChanged { job_id: id, status: JobStatus::Cancelled } if *id == job_id
            )
        })
        .await;
        recv_until(&mut events, |event| {
            matches!(
                event,
                JobEvent::Finished { job_id: id, result: Err(_) } if *id == job_id
            )
        })
        .await;

        assert_eq!(queue.running_count(), 0);
    }

    #[tokio::test]
    async fn concurrency_is_clamped_to_valid_range() {
        let (executor, _release_tx, _max_observed) = gated_executor();
        let (queue, _events) = JobQueue::new(executor, 0);
        assert_eq!(queue.concurrency(), MIN_CONCURRENCY);

        queue.set_concurrency(11);
        assert_eq!(queue.concurrency(), MAX_CONCURRENCY);

        queue.set_concurrency(5);
        assert_eq!(queue.concurrency(), 5);

        queue.set_concurrency(15);
        assert_eq!(queue.concurrency(), MAX_CONCURRENCY);
    }

    #[tokio::test]
    async fn retry_requeues_failed_job() {
        let (queue, mut events) = JobQueue::new(AlwaysFailExecutor, 1);

        let job = test_job("https://example.com/1");
        let job_id = job.id;
        queue.enqueue(job);

        recv_until(&mut events, |event| {
            matches!(
                event,
                JobEvent::Finished { job_id: id, result: Err(_) } if *id == job_id
            )
        })
        .await;

        assert!(queue.retry(job_id));

        recv_until(
            &mut events,
            |event| matches!(event, JobEvent::Finished { job_id: id, .. } if *id == job_id),
        )
        .await;

        assert!(!queue.retry(JobId::new()));
    }
}
