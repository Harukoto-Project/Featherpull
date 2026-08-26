//! ffmpegプロセスの起動と`-progress pipe:1`出力のパースを担う。
//!
//! ffmpegの人間向け進捗表示(`-stats`相当)は行ごとの区切りが安定しておらず機械的な
//! パースに向かないため、`-progress pipe:1 -nostats`を強制的に付与し、標準出力に
//! `key=value`形式の進捗ブロックのみを出力させる方式に統一している。

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;

use thiserror::Error;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

use super::job::JobStatus;

#[derive(Debug, Error)]
pub enum FfmpegError {
    #[error("ffmpegプロセスの起動に失敗しました: {0}")]
    Spawn(#[source] std::io::Error),

    #[error("ffmpegの標準出力を取得できませんでした")]
    StdoutUnavailable,

    #[error("ffmpegの標準出力読み取り中にI/Oエラーが発生しました: {0}")]
    ReadStdout(#[source] std::io::Error),

    #[error("ffmpegプロセスの終了を待機できませんでした: {0}")]
    Wait(#[source] std::io::Error),

    #[error("ffmpegが終了コード{code}で失敗しました\n--- stderr ---\n{stderr}")]
    ExitFailure { code: i32, stderr: String },

    #[error("ffmpegがシグナル等により終了コードなしで終了しました\n--- stderr ---\n{stderr}")]
    TerminatedWithoutCode { stderr: String },
}

/// 1つの`-progress`更新ブロックの終端種別。`end`は変換完了を意味するが、プロセス自体の
/// 終了コードとは独立した概念(出力ストリーム側の完了通知)であるため別で保持する。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProgressState {
    #[default]
    Continue,
    End,
}

/// `-progress pipe:1`が1回の更新につき出力する`key=value`群をパースした結果。
///
/// ffmpegの出力キーは多数あるが、GUIの進捗表示に必要な項目のみを型付けして保持する。
/// 未知のキーは無視し、値が`N/A`または負値(未確定を示す)のキーは`None`として扱う。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FfmpegProgressBlock {
    pub frame: Option<u64>,
    pub fps: Option<f64>,
    pub bitrate: Option<String>,
    pub total_size: Option<u64>,
    /// 経過時間(マイクロ秒)。`out_time_us`を優先し、無ければ`out_time_ms`、
    /// さらに無ければ`out_time`(`HH:MM:SS.ffffff`)から算出する。
    pub out_time_us: Option<i64>,
    pub speed: Option<String>,
    pub state: ProgressState,
}

/// ffmpegの`bitrate`/`speed`は値が確定しない間`N/A`を出力するため、これを欠損として扱う。
fn non_na_string(value: Option<&str>) -> Option<String> {
    match value {
        Some(v) if !v.is_empty() && v != "N/A" => Some(v.to_string()),
        _ => None,
    }
}

/// `out_time`は`HH:MM:SS.microseconds`形式の文字列で渡されるため、秒に展開してから
/// マイクロ秒へ変換する。
fn parse_out_time_string(value: &str) -> Option<i64> {
    let mut parts = value.split(':');
    let hours: f64 = parts.next()?.parse().ok()?;
    let minutes: f64 = parts.next()?.parse().ok()?;
    let seconds: f64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    let total_seconds = hours * 3600.0 + minutes * 60.0 + seconds;
    Some((total_seconds * 1_000_000.0).round() as i64)
}

/// 1ブロック分の生行(末尾の`progress=continue`/`progress=end`を含む)から
/// [`FfmpegProgressBlock`]を組み立てる。
///
/// 同じブロック内に`out_time_us`/`out_time_ms`/`out_time`が複数出現しても順序に依らず
/// 優先度どおりに解決できるよう、行の到着順ではなく一度マップへ集約してから解決する。
pub fn parse_progress_block(block_lines: &[&str]) -> FfmpegProgressBlock {
    let mut raw: HashMap<&str, &str> = HashMap::new();
    for line in block_lines {
        if let Some((key, value)) = line.split_once('=') {
            raw.insert(key.trim(), value.trim());
        }
    }

    let out_time_us = raw
        .get("out_time_us")
        .copied()
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|&us| us >= 0)
        .or_else(|| {
            raw.get("out_time_ms")
                .copied()
                .and_then(|v| v.parse::<i64>().ok())
                .filter(|&ms| ms >= 0)
                .map(|ms| ms * 1_000)
        })
        .or_else(|| raw.get("out_time").copied().and_then(parse_out_time_string));

    FfmpegProgressBlock {
        frame: raw.get("frame").copied().and_then(|v| v.parse().ok()),
        fps: raw.get("fps").copied().and_then(|v| v.parse().ok()),
        bitrate: non_na_string(raw.get("bitrate").copied()),
        total_size: raw
            .get("total_size")
            .copied()
            .and_then(|v| v.parse::<i64>().ok())
            .filter(|&size| size >= 0)
            .map(|size| size as u64),
        out_time_us,
        speed: non_na_string(raw.get("speed").copied()),
        state: match raw.get("progress").copied() {
            Some("end") => ProgressState::End,
            _ => ProgressState::Continue,
        },
    }
}

/// 経過時間と総時間から進捗率(0.0〜100.0)を算出する。
///
/// 総時間が不明、または経過時間が未確定(`None`)の場合は、呼び出し元に不定値として
/// `0.0`を返す。中断直後の1ブロック目など経過時間が総時間をわずかに超えて報告される
/// ケースもあるため、範囲外の値は`clamp`で吸収する。
pub fn progress_percentage(out_time_us: Option<i64>, total_duration_secs: Option<f64>) -> f32 {
    match (out_time_us, total_duration_secs) {
        (Some(us), Some(total)) if total > 0.0 => {
            let elapsed_secs = us as f64 / 1_000_000.0;
            ((elapsed_secs / total).clamp(0.0, 1.0) * 100.0) as f32
        }
        _ => 0.0,
    }
}

/// パース済みブロックから`JobStatus::Converting`を組み立てる。
///
/// `progress=end`ブロックはffmpegプロセスの出力ストリーム上の完了通知であり、実際に
/// 変換が正常終了したかどうか(終了コード)は呼び出し元が[`run_convert`]の戻り値で
/// 判断する必要があるため、ここでは`JobStatus::Completed`への遷移は行わない。
pub fn job_status_from_block(
    block: &FfmpegProgressBlock,
    total_duration_secs: Option<f64>,
) -> JobStatus {
    JobStatus::Converting {
        progress: progress_percentage(block.out_time_us, total_duration_secs),
    }
}

/// ffmpegプロセスを起動する。
///
/// `-progress pipe:1 -nostats`は呼び出し元の指定に関わらず常に付与し、進捗パースの
/// 前提を崩さないようにする。プロセスの生存管理(キャンセル時の強制終了など)は
/// 呼び出し元が返り値の[`Child`]を保持して`kill()`を呼ぶことで行う想定のため、
/// `kill_on_drop`を有効にして`Child`が破棄された場合にも確実に後始末する。
pub fn spawn_ffmpeg(ffmpeg_path: &Path, args: &[String]) -> Result<Child, FfmpegError> {
    Command::new(ffmpeg_path)
        .args(args)
        .args(["-progress", "pipe:1", "-nostats"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(FfmpegError::Spawn)
}

/// 起動済みの`Child`から標準出力を読み取り、進捗ブロック単位で`on_progress`へ通知しつつ
/// プロセスの終了を待機する。
///
/// キャンセルは呼び出し元が別途`child.kill()`を呼ぶことを想定しており、この関数自体は
/// キャンセル操作を提供しない(全体のキュー制御は別タスクの範囲のため)。
pub async fn run_convert(
    child: &mut Child,
    total_duration_secs: Option<f64>,
    mut on_progress: impl FnMut(JobStatus) + Send,
) -> Result<(), FfmpegError> {
    let stdout = child.stdout.take().ok_or(FfmpegError::StdoutUnavailable)?;
    let stderr = child.stderr.take();

    // stderrパイプの空き容量が尽きるとffmpeg本体がブロックして進捗も止まってしまうため、
    // stdoutの読み取りと並行して別タスクでドレインしておく。
    let stderr_task = stderr.map(|stderr| {
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            let mut collected = String::new();
            while let Ok(Some(line)) = lines.next_line().await {
                collected.push_str(&line);
                collected.push('\n');
            }
            collected
        })
    });

    let mut lines = BufReader::new(stdout).lines();
    let mut pending_lines: Vec<String> = Vec::new();

    while let Some(line) = lines.next_line().await.map_err(FfmpegError::ReadStdout)? {
        let is_terminator = line == "progress=continue" || line == "progress=end";
        pending_lines.push(line);

        if is_terminator {
            let refs: Vec<&str> = pending_lines.iter().map(String::as_str).collect();
            let block = parse_progress_block(&refs);
            on_progress(job_status_from_block(&block, total_duration_secs));
            pending_lines.clear();
        }
    }

    let status = child.wait().await.map_err(FfmpegError::Wait)?;
    let stderr_output = match stderr_task {
        Some(task) => task.await.unwrap_or_default(),
        None => String::new(),
    };

    match status.code() {
        Some(0) => Ok(()),
        Some(code) => Err(FfmpegError::ExitFailure {
            code,
            stderr: stderr_output,
        }),
        None => Err(FfmpegError::TerminatedWithoutCode {
            stderr: stderr_output,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_continue_block_with_multiple_keys() {
        let lines = [
            "frame=120",
            "fps=29.97",
            "bitrate=1024.3kbits/s",
            "total_size=1048576",
            "out_time_us=4004004",
            "out_time_ms=4004",
            "out_time=00:00:04.004000",
            "speed=1.01x",
            "progress=continue",
        ];

        let block = parse_progress_block(&lines);

        assert_eq!(block.frame, Some(120));
        assert_eq!(block.fps, Some(29.97));
        assert_eq!(block.bitrate, Some("1024.3kbits/s".to_string()));
        assert_eq!(block.total_size, Some(1_048_576));
        assert_eq!(block.out_time_us, Some(4_004_004));
        assert_eq!(block.speed, Some("1.01x".to_string()));
        assert_eq!(block.state, ProgressState::Continue);
    }

    #[test]
    fn parses_end_block_as_completion_marker() {
        let lines = [
            "frame=300",
            "fps=30.0",
            "out_time_us=10000000",
            "speed=1.5x",
            "progress=end",
        ];

        let block = parse_progress_block(&lines);

        assert_eq!(block.state, ProgressState::End);
        assert_eq!(block.out_time_us, Some(10_000_000));
    }

    #[test]
    fn missing_keys_are_treated_as_none() {
        let lines = ["frame=10", "bitrate=N/A", "speed=N/A", "progress=continue"];

        let block = parse_progress_block(&lines);

        assert_eq!(block.frame, Some(10));
        assert_eq!(block.fps, None);
        assert_eq!(block.bitrate, None);
        assert_eq!(block.speed, None);
        assert_eq!(block.total_size, None);
        assert_eq!(block.out_time_us, None);
    }

    #[test]
    fn falls_back_to_out_time_ms_when_out_time_us_missing() {
        let lines = ["out_time_ms=2500", "progress=continue"];

        let block = parse_progress_block(&lines);

        assert_eq!(block.out_time_us, Some(2_500_000));
    }

    #[test]
    fn falls_back_to_out_time_string_when_other_keys_missing() {
        let lines = ["out_time=00:01:02.500000", "progress=continue"];

        let block = parse_progress_block(&lines);

        assert_eq!(block.out_time_us, Some(62_500_000));
    }

    #[test]
    fn negative_out_time_us_is_treated_as_unknown() {
        let lines = ["out_time_us=-1", "out_time_ms=-1", "progress=continue"];

        let block = parse_progress_block(&lines);

        assert_eq!(block.out_time_us, None);
    }

    #[test]
    fn progress_percentage_uses_elapsed_over_total_duration() {
        // 4.004004秒 / 40.04004秒 = 10%
        let percentage = progress_percentage(Some(4_004_004), Some(40.040_04));
        assert!((percentage - 10.0).abs() < 0.01);
    }

    #[test]
    fn progress_percentage_is_zero_when_duration_unknown() {
        assert_eq!(progress_percentage(Some(4_004_004), None), 0.0);
    }

    #[test]
    fn progress_percentage_is_zero_when_out_time_missing() {
        assert_eq!(progress_percentage(None, Some(40.0)), 0.0);
    }

    #[test]
    fn progress_percentage_clamps_to_100_when_overshooting() {
        let percentage = progress_percentage(Some(50_000_000), Some(40.0));
        assert_eq!(percentage, 100.0);
    }

    #[test]
    fn job_status_from_block_wraps_percentage_as_converting() {
        let lines = ["out_time_us=20000000", "progress=continue"];
        let block = parse_progress_block(&lines);

        let status = job_status_from_block(&block, Some(40.0));

        match status {
            JobStatus::Converting { progress } => assert!((progress - 50.0).abs() < 0.01),
            other => panic!("Convertingが返るはずが{other:?}が返った"),
        }
    }
}
