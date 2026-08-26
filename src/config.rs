use std::path::PathBuf;

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use thiserror::Error;

const CONFIG_FILE_NAME: &str = "config.toml";
const BIN_DIR_NAME: &str = "bin";
const LOGS_DIR_NAME: &str = "logs";

#[derive(Debug, Error)]
pub enum ConfigError {
    /// OSがユーザー設定ディレクトリの規約を持たない場合など、`directories`crateが
    /// パスを解決できないケースを区別できるようにしておく。
    #[error("設定ディレクトリを解決できませんでした")]
    ProjectDirsUnavailable,
    #[error("設定ファイルの読み書きに失敗しました: {0}")]
    Io(#[from] std::io::Error),
    #[error("設定ファイルの解析に失敗しました: {0}")]
    Deserialize(#[from] toml::de::Error),
    #[error("設定ファイルのシリアライズに失敗しました: {0}")]
    Serialize(#[from] toml::ser::Error),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralConfig {
    pub concurrency: u32,
    /// 空文字列はOS標準のダウンロードフォルダを使う意味に用いる(設定ファイル上で
    /// 環境依存のパスを直接埋め込まずに済ませるため)。
    pub save_dir: String,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            concurrency: 3,
            save_dir: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BinariesConfig {
    /// 空文字列は自動管理パス(`bin/`配下)を使う意味に用いる。
    pub ytdlp_path: String,
    pub ffmpeg_path: String,
    pub auto_update_check: bool,
}

impl Default for BinariesConfig {
    fn default() -> Self {
        Self {
            ytdlp_path: String::new(),
            ffmpeg_path: String::new(),
            auto_update_check: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DefaultsConfig {
    pub video_quality: String,
    pub audio_format: String,
}

impl Default for DefaultsConfig {
    fn default() -> Self {
        Self {
            video_quality: "best".to_string(),
            audio_format: "mp3".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    pub file_logging: bool,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self { file_logging: true }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub general: GeneralConfig,
    pub binaries: BinariesConfig,
    pub defaults: DefaultsConfig,
    pub logging: LoggingConfig,
}

impl Config {
    /// 設定ファイルを読み込む。ファイルが存在しない場合は初回起動とみなし、
    /// デフォルト設定を返す(エラーにはしない)。
    pub fn load() -> Result<Self, ConfigError> {
        let path = config_file_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }

        let raw = std::fs::read_to_string(&path)?;
        let config = toml::from_str(&raw)?;
        Ok(config)
    }

    /// 設定ファイルを保存する。親ディレクトリが無い場合は作成する。
    pub fn save(&self) -> Result<(), ConfigError> {
        let path = config_file_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let raw = toml::to_string_pretty(self)?;
        std::fs::write(&path, raw)?;
        Ok(())
    }
}

/// `Harukoto Project`名義でアプリを配置するプロジェクトディレクトリ解決結果。
/// qualifierを空にしているのは、Windows以外でもドメイン逆順記法を強制しないため。
fn project_dirs() -> Result<ProjectDirs, ConfigError> {
    ProjectDirs::from("", "Harukoto Project", "featherpull")
        .ok_or(ConfigError::ProjectDirsUnavailable)
}

/// 設定ファイル(`config.toml`)の配置先。
pub fn config_file_path() -> Result<PathBuf, ConfigError> {
    Ok(config_dir()?.join(CONFIG_FILE_NAME))
}

/// 設定ファイルを置くディレクトリそのもの(Windowsでは`%APPDATA%\Harukoto Project\featherpull`)。
pub fn config_dir() -> Result<PathBuf, ConfigError> {
    Ok(project_dirs()?.config_dir().to_path_buf())
}

/// yt-dlp/ffmpegの自動管理バイナリを配置するディレクトリ。
pub fn bin_dir() -> Result<PathBuf, ConfigError> {
    Ok(config_dir()?.join(BIN_DIR_NAME))
}

/// ログファイルを配置するディレクトリ。
pub fn logs_dir() -> Result<PathBuf, ConfigError> {
    Ok(config_dir()?.join(LOGS_DIR_NAME))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_matches_documented_values() {
        let config = Config::default();

        assert_eq!(config.general.concurrency, 3);
        assert_eq!(config.general.save_dir, "");
        assert_eq!(config.binaries.ytdlp_path, "");
        assert_eq!(config.binaries.ffmpeg_path, "");
        assert!(config.binaries.auto_update_check);
        assert_eq!(config.defaults.video_quality, "best");
        assert_eq!(config.defaults.audio_format, "mp3");
        assert!(config.logging.file_logging);
    }

    #[test]
    fn config_round_trips_through_toml() {
        let mut config = Config::default();
        config.general.concurrency = 5;
        config.general.save_dir = "D:/Videos".to_string();
        config.binaries.ytdlp_path = "D:/tools/yt-dlp.exe".to_string();
        config.defaults.video_quality = "1080p".to_string();
        config.logging.file_logging = false;

        let raw = toml::to_string_pretty(&config).expect("serialize should succeed");
        let restored: Config = toml::from_str(&raw).expect("deserialize should succeed");
        assert_eq!(config, restored);
    }

    #[test]
    fn missing_config_file_parses_as_empty_document() {
        // 設定ファイルが空でも各セクションのデフォルト値が補完されることを確認する。
        let config: Config = toml::from_str("").expect("empty document should parse");
        assert_eq!(config, Config::default());
    }

    #[test]
    fn config_dir_paths_are_nested_under_the_same_root() {
        let config_dir = config_dir().expect("config dir should resolve on this OS");
        let bin_dir = bin_dir().expect("bin dir should resolve on this OS");
        let logs_dir = logs_dir().expect("logs dir should resolve on this OS");

        assert!(config_dir.is_absolute());
        assert!(bin_dir.starts_with(&config_dir));
        assert!(logs_dir.starts_with(&config_dir));
    }
}
