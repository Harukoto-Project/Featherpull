use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// ジョブの一意な識別子。
///
/// プロセス内でのみ有効なIDで十分なため、DBやファイル間で永続化する必要がある
/// UUID等ではなく、単純なインクリメントカウンタで生成する。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct JobId(u64);

/// カウンタは全ジョブで共有する必要があるため、プロセス全体でグローバルに1つだけ持つ。
static NEXT_JOB_ID: AtomicU64 = AtomicU64::new(1);

impl JobId {
    /// 新しい一意なIDを発行する。
    pub fn new() -> Self {
        Self(NEXT_JOB_ID.fetch_add(1, Ordering::Relaxed))
    }

    pub fn get(&self) -> u64 {
        self.0
    }
}

// 生成方式(インクリメント)を意識させたいため、`JobId::new()`を明示的に呼ぶ形にし、
// `Default`は実装しない。

/// yt-dlpの`-f`に渡す画質プリセット。カスタムフォーマット文字列は`FormatSelection::Custom`側で扱う。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoQuality {
    Best,
    P1080,
    P720,
    AudioOnly,
}

/// 音声のみ抽出時にffmpegへ渡す出力フォーマット。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioFormat {
    Mp3,
    M4a,
    Opus,
}

/// フォーマット選択は、GUIが提示するプリセットと、上級者向けの生の`-f`文字列指定の
/// 両方をサポートする必要があるため2択のenumにしている。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatSelection {
    Preset(VideoQuality),
    Custom(String),
}

/// ジョブの進行状態。ダウンロードと変換は別工程のため、進捗も別々に保持する。
#[derive(Debug, Clone, PartialEq)]
pub enum JobStatus {
    Queued,
    Downloading {
        progress: f32,
        speed: Option<String>,
        eta: Option<String>,
    },
    Converting {
        progress: f32,
    },
    Completed,
    Failed {
        message: String,
    },
    Cancelled,
}

/// 1件のダウンロード/変換タスク。1URL = 1Jobとする設計上の決定に基づく。
#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    pub id: JobId,
    pub url: String,
    pub status: JobStatus,
    pub format_selection: FormatSelection,
    pub audio_only: Option<AudioFormat>,
    pub output_dir: PathBuf,
    pub created_at: chrono::DateTime<chrono::Local>,
}

impl Job {
    /// キュー投入直後の状態で新規ジョブを作成する。
    pub fn new(url: String, format_selection: FormatSelection, output_dir: PathBuf) -> Self {
        Self {
            id: JobId::new(),
            url,
            status: JobStatus::Queued,
            format_selection,
            audio_only: None,
            output_dir,
            created_at: chrono::Local::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_id_is_unique_across_instances() {
        let a = JobId::new();
        let b = JobId::new();
        assert_ne!(a, b);
        assert!(b.get() > a.get());
    }

    #[test]
    fn new_job_starts_queued_with_no_audio_selection() {
        let job = Job::new(
            "https://example.com/watch".to_string(),
            FormatSelection::Preset(VideoQuality::Best),
            PathBuf::from("C:/downloads"),
        );

        assert_eq!(job.status, JobStatus::Queued);
        assert_eq!(job.audio_only, None);
        assert_eq!(
            job.format_selection,
            FormatSelection::Preset(VideoQuality::Best)
        );
    }

    #[test]
    fn custom_format_selection_preserves_raw_string() {
        let selection = FormatSelection::Custom("bestvideo+bestaudio".to_string());
        match selection {
            FormatSelection::Custom(ref raw) => assert_eq!(raw, "bestvideo+bestaudio"),
            FormatSelection::Preset(_) => panic!("Custom選択がPresetとして扱われた"),
        }
    }
}
