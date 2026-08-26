use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

use super::job::JobStatus;

/// `--progress-template "download:%(progress)j"` を付与した際にyt-dlpが出力する1行分のJSON。
///
/// yt-dlpはフィールドをnullにしたり省略したりすることがあるため、必須級のキー以外は
/// すべて`Option`で受け取る。`_percent_str`等の`_`始まりフィールドは表示専用の非安定APIのため
/// あえて定義しない。
///
/// `serde_json`はこのリポジトリの依存関係に含まれておらず(Cargo.tomlは複数タスクが並行して
/// 触るため今回は変更しない方針)、代わりにこの用途に必要な最小限のフラットJSONパーサーを
/// 下部に自作している。
#[derive(Debug, Clone, PartialEq)]
struct YtdlpProgressLine {
    status: String,
    downloaded_bytes: Option<u64>,
    total_bytes: Option<u64>,
    total_bytes_estimate: Option<u64>,
    eta: Option<f64>,
    speed: Option<f64>,
}

/// yt-dlpの`download:`行に含まれるJSONのプレフィックス。
const PROGRESS_LINE_PREFIX: &str = "download:";

#[derive(Debug, thiserror::Error)]
pub enum YtdlpError {
    #[error("yt-dlpプロセスの起動に失敗しました: {0}")]
    Spawn(#[source] std::io::Error),

    #[error("yt-dlpの標準出力の読み取りに失敗しました: {0}")]
    ReadStdout(#[source] std::io::Error),

    #[error("yt-dlpプロセスの終了待機に失敗しました: {0}")]
    Wait(#[source] std::io::Error),

    #[error("yt-dlpがエラー終了しました(終了コード: {code:?})\n{stderr}")]
    NonZeroExit { code: Option<i32>, stderr: String },

    #[error("プロセスの強制終了に失敗しました: {0}")]
    Kill(#[source] std::io::Error),
}

/// yt-dlpの進捗行1件から`JobStatus`を組み立てる。
fn build_job_status(line: &YtdlpProgressLine) -> JobStatus {
    let progress = match (line.downloaded_bytes, total_bytes(line)) {
        (Some(downloaded), Some(total)) if total > 0 => {
            (downloaded as f32 / total as f32 * 100.0).clamp(0.0, 100.0)
        }
        _ => 0.0,
    };

    JobStatus::Downloading {
        progress,
        speed: line.speed.map(format_speed),
        eta: line.eta.map(format_eta),
    }
}

/// `total_bytes`がnullの場合、yt-dlpは代わりに`total_bytes_estimate`を返すことがある。
fn total_bytes(line: &YtdlpProgressLine) -> Option<u64> {
    line.total_bytes.or(line.total_bytes_estimate)
}

/// バイト/秒を人間が読みやすい単位(B/s, KB/s, MB/s, GB/s)の文字列に整形する。
fn format_speed(bytes_per_sec: f64) -> String {
    const UNITS: [&str; 4] = ["B/s", "KB/s", "MB/s", "GB/s"];
    let mut value = bytes_per_sec;
    let mut unit_index = 0;
    while value >= 1024.0 && unit_index < UNITS.len() - 1 {
        value /= 1024.0;
        unit_index += 1;
    }
    format!("{:.1}{}", value, UNITS[unit_index])
}

/// 残り秒数を`mm:ss`形式の文字列に整形する。
fn format_eta(eta_seconds: f64) -> String {
    let total_seconds = eta_seconds.max(0.0).round() as u64;
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    format!("{minutes:02}:{seconds:02}")
}

/// yt-dlpの1行分の標準出力を受け取り、進捗行であれば`JobStatus`を返す。
///
/// `download:`で始まらない行(バージョン情報等の通常の標準出力)は`None`を返して無視する。
fn parse_progress_output_line(line: &str) -> Result<Option<JobStatus>, JsonParseError> {
    let Some(json_part) = line.strip_prefix(PROGRESS_LINE_PREFIX) else {
        return Ok(None);
    };

    let parsed = parse_progress_line(json_part)?;
    Ok(Some(build_job_status(&parsed)))
}

fn parse_progress_line(json_part: &str) -> Result<YtdlpProgressLine, JsonParseError> {
    let map = parse_flat_json_object(json_part)?;
    Ok(YtdlpProgressLine {
        status: get_string(&map, "status")?,
        downloaded_bytes: get_optional_u64(&map, "downloaded_bytes")?,
        total_bytes: get_optional_u64(&map, "total_bytes")?,
        total_bytes_estimate: get_optional_u64(&map, "total_bytes_estimate")?,
        eta: get_optional_f64(&map, "eta")?,
        speed: get_optional_f64(&map, "speed")?,
    })
}

/// yt-dlpダウンロードプロセスのハンドル。
///
/// `spawn`と`wait_with_progress`を分離しているのは、呼び出し元(将来のキュー実装)が
/// 進捗待機と並行して`kill`でキャンセルできるようにするため。
pub struct YtdlpProcess {
    child: Child,
}

impl YtdlpProcess {
    /// yt-dlpを起動する。
    ///
    /// `--newline --progress-template "download:%(progress)j"` を付与することで、進捗更新1件が
    /// 標準出力の1行として出力されるようにし、行単位でのパースを可能にしている。
    pub fn spawn(ytdlp_path: &Path, url: &str, extra_args: &[String]) -> Result<Self, YtdlpError> {
        let mut command = Command::new(ytdlp_path);
        command
            .arg("--newline")
            .arg("--progress-template")
            .arg("download:%(progress)j")
            .args(extra_args)
            .arg(url)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let child = command.spawn().map_err(YtdlpError::Spawn)?;
        Ok(Self { child })
    }

    /// プロセスを強制終了する。キュー側でジョブがキャンセルされた際に呼び出す想定。
    pub async fn kill(&mut self) -> Result<(), YtdlpError> {
        self.child.kill().await.map_err(YtdlpError::Kill)
    }

    /// 標準出力・標準エラーを読み進めながらプロセスの終了を待つ。
    ///
    /// `on_progress`は進捗行を検出するたびに呼び出される。
    pub async fn wait_with_progress(
        &mut self,
        mut on_progress: impl FnMut(JobStatus) + Send,
    ) -> Result<YtdlpOutcome, YtdlpError> {
        let stdout = self.child.stdout.take().expect("stdoutはpipedで設定済み");
        let stderr = self.child.stderr.take().expect("stderrはpipedで設定済み");

        // 標準出力・標準エラーの両方を同時に読み切らないとパイプが詰まってプロセスが停止する
        // 可能性があるため、それぞれ別タスクで並行して読み進める。進捗コールバックは`Send`のみを
        // 要求しており`'static`ではない場合があるため、子タスク内では呼ばずに一旦結果として集約し、
        // 両タスク完了後にこのタスク上でまとめて呼び出す。
        let stdout_task = tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            let mut progress_updates = Vec::new();
            loop {
                match reader.next_line().await {
                    Ok(Some(line)) => {
                        if let Ok(Some(status)) = parse_progress_output_line(&line) {
                            progress_updates.push(status);
                        }
                    }
                    Ok(None) => break,
                    Err(err) => return Err(YtdlpError::ReadStdout(err)),
                }
            }
            Ok(progress_updates)
        });

        let stderr_task = tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            let mut buffer = String::new();
            loop {
                match reader.next_line().await {
                    Ok(Some(line)) => {
                        buffer.push_str(&line);
                        buffer.push('\n');
                    }
                    Ok(None) => break,
                    Err(err) => return Err(YtdlpError::ReadStdout(err)),
                }
            }
            Ok(buffer)
        });

        let progress_updates = stdout_task
            .await
            .expect("stdout読み取りタスクがpanicした")?;
        for status in progress_updates {
            on_progress(status);
        }

        let stderr_output = stderr_task
            .await
            .expect("stderr読み取りタスクがpanicした")?;

        let exit_status = self.child.wait().await.map_err(YtdlpError::Wait)?;

        if !exit_status.success() {
            return Err(YtdlpError::NonZeroExit {
                code: exit_status.code(),
                stderr: stderr_output,
            });
        }

        Ok(YtdlpOutcome {
            stderr: stderr_output,
        })
    }
}

/// yt-dlpの実行結果として蓄積された標準エラー出力。
pub struct YtdlpOutcome {
    pub stderr: String,
}

/// yt-dlpを起動し、進捗を`on_progress`へ流しながらダウンロードが完了するまで待機する。
///
/// キャンセルが不要な単純な呼び出しのための便利関数。キャンセルが必要な場合は
/// `YtdlpProcess::spawn`と`wait_with_progress`を直接使い、ハンドルを保持しておくこと。
pub async fn run_download(
    ytdlp_path: &Path,
    url: &str,
    extra_args: &[String],
    on_progress: impl FnMut(JobStatus) + Send,
) -> Result<YtdlpOutcome, YtdlpError> {
    let mut process = YtdlpProcess::spawn(ytdlp_path, url, extra_args)?;
    process.wait_with_progress(on_progress).await
}

// --- 以下、`serde_json`の代わりに使う最小限のフラットJSONパーサー ---
//
// yt-dlpの進捗テンプレートが出力するJSONはネストのないフラットなオブジェクト1つのみ
// (値は文字列/数値/真偽値/null)なので、汎用のJSONパーサーを新規依存として追加せず、
// この用途に必要な範囲だけを自前で実装する。

#[derive(Debug, Clone, PartialEq)]
enum JsonValue {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
}

#[derive(Debug, thiserror::Error, PartialEq)]
enum JsonParseError {
    #[error("JSONの構文が不正です: {0}")]
    Syntax(String),

    #[error("必須フィールド'{0}'がありません")]
    MissingField(String),

    #[error("フィールド'{0}'の型が不正です")]
    UnexpectedType(String),
}

struct JsonCursor<'a> {
    remaining: &'a str,
}

impl<'a> JsonCursor<'a> {
    fn new(input: &'a str) -> Self {
        Self { remaining: input }
    }

    fn skip_whitespace(&mut self) {
        self.remaining = self.remaining.trim_start_matches([' ', '\t', '\n', '\r']);
    }

    fn peek(&self) -> Option<char> {
        self.remaining.chars().next()
    }

    fn advance(&mut self) -> Option<char> {
        let mut chars = self.remaining.chars();
        let c = chars.next()?;
        self.remaining = chars.as_str();
        Some(c)
    }

    fn expect(&mut self, expected: char) -> Result<(), JsonParseError> {
        match self.advance() {
            Some(c) if c == expected => Ok(()),
            other => Err(JsonParseError::Syntax(format!(
                "'{expected}'を期待しましたが{other:?}でした"
            ))),
        }
    }

    fn parse_object(&mut self) -> Result<HashMap<String, JsonValue>, JsonParseError> {
        self.skip_whitespace();
        self.expect('{')?;
        let mut map = HashMap::new();

        self.skip_whitespace();
        if self.peek() == Some('}') {
            self.advance();
            return Ok(map);
        }

        loop {
            self.skip_whitespace();
            let key = self.parse_string()?;
            self.skip_whitespace();
            self.expect(':')?;
            let value = self.parse_value()?;
            map.insert(key, value);

            self.skip_whitespace();
            match self.advance() {
                Some(',') => continue,
                Some('}') => break,
                other => {
                    return Err(JsonParseError::Syntax(format!(
                        "','または'}}'を期待しましたが{other:?}でした"
                    )));
                }
            }
        }

        Ok(map)
    }

    fn parse_value(&mut self) -> Result<JsonValue, JsonParseError> {
        self.skip_whitespace();
        match self.peek() {
            Some('"') => Ok(JsonValue::String(self.parse_string()?)),
            Some('t') => self.parse_literal("true", JsonValue::Bool(true)),
            Some('f') => self.parse_literal("false", JsonValue::Bool(false)),
            Some('n') => self.parse_literal("null", JsonValue::Null),
            Some(c) if c == '-' || c.is_ascii_digit() => {
                Ok(JsonValue::Number(self.parse_number()?))
            }
            other => Err(JsonParseError::Syntax(format!(
                "予期しない値です: {other:?}"
            ))),
        }
    }

    fn parse_literal(
        &mut self,
        literal: &'static str,
        value: JsonValue,
    ) -> Result<JsonValue, JsonParseError> {
        if self.remaining.starts_with(literal) {
            self.remaining = &self.remaining[literal.len()..];
            Ok(value)
        } else {
            Err(JsonParseError::Syntax(format!("'{literal}'を期待しました")))
        }
    }

    fn parse_string(&mut self) -> Result<String, JsonParseError> {
        self.expect('"')?;
        let mut result = String::new();
        loop {
            match self.advance() {
                None => return Err(JsonParseError::Syntax("文字列が閉じられていません".into())),
                Some('"') => return Ok(result),
                Some('\\') => match self.advance() {
                    Some('"') => result.push('"'),
                    Some('\\') => result.push('\\'),
                    Some('/') => result.push('/'),
                    Some('n') => result.push('\n'),
                    Some('t') => result.push('\t'),
                    Some('r') => result.push('\r'),
                    Some('b') => result.push('\u{8}'),
                    Some('f') => result.push('\u{c}'),
                    Some('u') => {
                        let code = self.take_hex4()?;
                        result.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
                    }
                    other => {
                        return Err(JsonParseError::Syntax(format!(
                            "不正なエスケープです: {other:?}"
                        )));
                    }
                },
                Some(c) => result.push(c),
            }
        }
    }

    fn take_hex4(&mut self) -> Result<u32, JsonParseError> {
        let mut value = 0u32;
        for _ in 0..4 {
            let c = self
                .advance()
                .ok_or_else(|| JsonParseError::Syntax("\\uエスケープが不完全です".into()))?;
            let digit = c
                .to_digit(16)
                .ok_or_else(|| JsonParseError::Syntax("不正な16進数エスケープです".into()))?;
            value = value * 16 + digit;
        }
        Ok(value)
    }

    fn parse_number(&mut self) -> Result<f64, JsonParseError> {
        let start = self.remaining;
        if self.peek() == Some('-') {
            self.advance();
        }

        let mut has_digits = false;
        while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
            self.advance();
            has_digits = true;
        }
        if self.peek() == Some('.') {
            self.advance();
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                self.advance();
                has_digits = true;
            }
        }
        if matches!(self.peek(), Some('e' | 'E')) {
            self.advance();
            if matches!(self.peek(), Some('+' | '-')) {
                self.advance();
            }
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                self.advance();
            }
        }

        if !has_digits {
            return Err(JsonParseError::Syntax("数値の形式が不正です".into()));
        }

        let consumed_len = start.len() - self.remaining.len();
        start[..consumed_len]
            .parse::<f64>()
            .map_err(|_| JsonParseError::Syntax("数値のパースに失敗しました".into()))
    }
}

fn parse_flat_json_object(input: &str) -> Result<HashMap<String, JsonValue>, JsonParseError> {
    let mut cursor = JsonCursor::new(input);
    let map = cursor.parse_object()?;
    cursor.skip_whitespace();
    if !cursor.remaining.is_empty() {
        return Err(JsonParseError::Syntax(
            "JSONオブジェクトの後に余分な文字があります".into(),
        ));
    }
    Ok(map)
}

fn get_string(map: &HashMap<String, JsonValue>, key: &str) -> Result<String, JsonParseError> {
    match map.get(key) {
        Some(JsonValue::String(s)) => Ok(s.clone()),
        Some(_) => Err(JsonParseError::UnexpectedType(key.to_string())),
        None => Err(JsonParseError::MissingField(key.to_string())),
    }
}

fn get_optional_u64(
    map: &HashMap<String, JsonValue>,
    key: &str,
) -> Result<Option<u64>, JsonParseError> {
    match map.get(key) {
        None | Some(JsonValue::Null) => Ok(None),
        Some(JsonValue::Number(n)) if *n >= 0.0 => Ok(Some(*n as u64)),
        Some(_) => Err(JsonParseError::UnexpectedType(key.to_string())),
    }
}

fn get_optional_f64(
    map: &HashMap<String, JsonValue>,
    key: &str,
) -> Result<Option<f64>, JsonParseError> {
    match map.get(key) {
        None | Some(JsonValue::Null) => Ok(None),
        Some(JsonValue::Number(n)) => Ok(Some(*n)),
        Some(_) => Err(JsonParseError::UnexpectedType(key.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_progress_line_with_total_bytes() {
        let line = r#"download:{"status": "downloading", "downloaded_bytes": 1024, "total_bytes": 988479, "eta": 1, "speed": 512342.5, "elapsed": 0.1, "filename": "video.mp4"}"#;

        let status = parse_progress_output_line(line)
            .expect("パースに成功するはず")
            .expect("進捗行として認識されるはず");

        match status {
            JobStatus::Downloading {
                progress,
                speed,
                eta,
            } => {
                let expected_progress = 1024.0 / 988479.0 * 100.0;
                assert!((progress - expected_progress).abs() < 0.01);
                assert_eq!(speed, Some("500.3KB/s".to_string()));
                assert_eq!(eta, Some("00:01".to_string()));
            }
            other => panic!("Downloadingになるはず: {other:?}"),
        }
    }

    #[test]
    fn falls_back_to_total_bytes_estimate_when_total_bytes_is_null() {
        let line = r#"download:{"status": "downloading", "downloaded_bytes": 500, "total_bytes": null, "total_bytes_estimate": 1000, "eta": 5, "speed": 1024.0}"#;

        let status = parse_progress_output_line(line)
            .expect("パースに成功するはず")
            .expect("進捗行として認識されるはず");

        match status {
            JobStatus::Downloading { progress, .. } => {
                assert!((progress - 50.0).abs() < 0.01);
            }
            other => panic!("Downloadingになるはず: {other:?}"),
        }
    }

    #[test]
    fn parses_finished_status_without_error() {
        let line = r#"download:{"status": "finished", "downloaded_bytes": 988479, "total_bytes": 988479, "eta": 0, "speed": null}"#;

        let status = parse_progress_output_line(line)
            .expect("パースに成功するはず")
            .expect("進捗行として認識されるはず");

        match status {
            JobStatus::Downloading { progress, eta, .. } => {
                assert!((progress - 100.0).abs() < 0.01);
                assert_eq!(eta, Some("00:00".to_string()));
            }
            other => panic!("Downloadingになるはず: {other:?}"),
        }
    }

    #[test]
    fn ignores_non_progress_lines() {
        let line = "[youtube] Extracting URL: https://example.com/watch";

        let result = parse_progress_output_line(line).expect("パース自体は失敗しないはず");
        assert_eq!(result, None);
    }

    #[test]
    fn returns_error_on_malformed_json() {
        let line = r#"download:{"status": "downloading", "downloaded_bytes": }"#;

        assert!(parse_progress_output_line(line).is_err());
    }

    #[test]
    fn returns_error_when_status_field_is_missing() {
        let line = r#"download:{"downloaded_bytes": 100, "total_bytes": 200}"#;

        assert_eq!(
            parse_progress_output_line(line),
            Err(JsonParseError::MissingField("status".to_string()))
        );
    }

    #[test]
    fn format_speed_scales_units_correctly() {
        assert_eq!(format_speed(512.0), "512.0B/s");
        assert_eq!(format_speed(2048.0), "2.0KB/s");
        assert_eq!(format_speed(3.0 * 1024.0 * 1024.0), "3.0MB/s");
    }

    #[test]
    fn format_eta_formats_as_minutes_and_seconds() {
        assert_eq!(format_eta(65.0), "01:05");
        assert_eq!(format_eta(0.0), "00:00");
    }

    #[test]
    fn parses_string_with_escaped_characters() {
        let mut cursor = JsonCursor::new(r#""line1\nline2\"quoted\"""#);
        let parsed = cursor.parse_string().expect("パースに成功するはず");
        assert_eq!(parsed, "line1\nline2\"quoted\"");
    }
}
