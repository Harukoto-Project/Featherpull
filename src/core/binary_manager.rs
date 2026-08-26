//! yt-dlp/ffmpegバイナリの自動ダウンロード・バージョン確認・カスタムパス解決を担う。
//!
//! GitHub Releases APIのJSONレスポンスをパースする必要があるが、このプロジェクトの
//! 依存関係には`serde_json`が含まれていない(`Cargo.toml`の依存構成を変更しない方針
//! のため)。そのため、今回必要な範囲(文字列・数値・配列・オブジェクトの読み取り)に
//! 限定した最小限のJSONパーサを`json`サブモジュールとして自前で用意している。

use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::config::{self, Config, ConfigError};

const YTDLP_RELEASE_API: &str = "https://api.github.com/repos/yt-dlp/yt-dlp/releases/latest";
const FFMPEG_RELEASE_API: &str = "https://api.github.com/repos/BtbN/FFmpeg-Builds/releases/latest";

const YTDLP_ASSET_NAME: &str = "yt-dlp.exe";
const YTDLP_EXE_NAME: &str = "yt-dlp.exe";
const FFMPEG_EXE_NAME: &str = "ffmpeg.exe";
const FFPROBE_EXE_NAME: &str = "ffprobe.exe";
const FFMPEG_ARCHIVE_FFMPEG_SUFFIX: &str = "bin/ffmpeg.exe";
const FFMPEG_ARCHIVE_FFPROBE_SUFFIX: &str = "bin/ffprobe.exe";

/// 自動ダウンロードしたバイナリのバージョンを記録するメタファイル名。
/// 実行ファイル自体からバージョン文字列を都度取得するより単純なため、この方式を選ぶ。
const YTDLP_VERSION_META_FILE: &str = "yt-dlp.version";
const FFMPEG_VERSION_META_FILE: &str = "ffmpeg.version";

/// GitHubのAPIはUser-Agentヘッダーが無いリクエストを拒否するため、必ず付与する。
const GITHUB_USER_AGENT: &str = "featherpull";

#[derive(Debug, Error)]
pub enum BinaryManagerError {
    #[error("設定ディレクトリの解決に失敗しました: {0}")]
    Config(#[from] ConfigError),
    #[error("HTTPリクエストに失敗しました: {0}")]
    Http(#[from] reqwest::Error),
    #[error("ファイル操作に失敗しました: {0}")]
    Io(#[from] std::io::Error),
    #[error("zipアーカイブの処理に失敗しました: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("GitHub Releases APIのレスポンスを解析できませんでした: {0}")]
    InvalidJson(String),
    #[error("リリースの中に必要なアセットが見つかりませんでした: {0}")]
    AssetNotFound(String),
    #[error("zipアーカイブ内に対象ファイルが見つかりませんでした: {0}")]
    FileNotFoundInArchive(String),
}

/// GitHub Releases APIレスポンスのJSONを解析するための、本モジュール専用の最小実装。
/// 汎用ライブラリではないため、公開APIも今回利用する分だけに絞っている。
mod json {
    use std::iter::Peekable;
    use std::str::Chars;

    #[derive(Debug, Clone, PartialEq)]
    pub enum Value {
        Null,
        Bool(bool),
        Number(f64),
        String(String),
        Array(Vec<Value>),
        Object(Vec<(String, Value)>),
    }

    impl Value {
        pub fn get(&self, key: &str) -> Option<&Value> {
            match self {
                Value::Object(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
                _ => None,
            }
        }

        pub fn as_str(&self) -> Option<&str> {
            match self {
                Value::String(s) => Some(s.as_str()),
                _ => None,
            }
        }

        pub fn as_array(&self) -> Option<&[Value]> {
            match self {
                Value::Array(items) => Some(items.as_slice()),
                _ => None,
            }
        }
    }

    type CharStream<'a> = Peekable<Chars<'a>>;

    pub fn parse(input: &str) -> Result<Value, String> {
        let mut chars = input.chars().peekable();
        parse_value(&mut chars)
    }

    fn skip_whitespace(chars: &mut CharStream) {
        while matches!(chars.peek(), Some(c) if c.is_whitespace()) {
            chars.next();
        }
    }

    fn expect(chars: &mut CharStream, expected: char) -> Result<(), String> {
        match chars.next() {
            Some(c) if c == expected => Ok(()),
            other => Err(format!("'{expected}'を期待しましたが{other:?}でした")),
        }
    }

    fn parse_value(chars: &mut CharStream) -> Result<Value, String> {
        skip_whitespace(chars);
        match chars.peek() {
            Some('{') => parse_object(chars),
            Some('[') => parse_array(chars),
            Some('"') => parse_string(chars).map(Value::String),
            Some('t') | Some('f') => parse_bool(chars),
            Some('n') => parse_null(chars),
            Some(c) if c.is_ascii_digit() || *c == '-' => parse_number(chars),
            other => Err(format!("予期しないトークンです: {other:?}")),
        }
    }

    fn parse_object(chars: &mut CharStream) -> Result<Value, String> {
        expect(chars, '{')?;
        let mut entries = Vec::new();
        skip_whitespace(chars);
        if chars.peek() == Some(&'}') {
            chars.next();
            return Ok(Value::Object(entries));
        }
        loop {
            skip_whitespace(chars);
            let key = parse_string(chars)?;
            skip_whitespace(chars);
            expect(chars, ':')?;
            let value = parse_value(chars)?;
            entries.push((key, value));
            skip_whitespace(chars);
            match chars.next() {
                Some(',') => continue,
                Some('}') => break,
                other => return Err(format!("','または'}}'を期待しましたが{other:?}でした")),
            }
        }
        Ok(Value::Object(entries))
    }

    fn parse_array(chars: &mut CharStream) -> Result<Value, String> {
        expect(chars, '[')?;
        let mut items = Vec::new();
        skip_whitespace(chars);
        if chars.peek() == Some(&']') {
            chars.next();
            return Ok(Value::Array(items));
        }
        loop {
            items.push(parse_value(chars)?);
            skip_whitespace(chars);
            match chars.next() {
                Some(',') => continue,
                Some(']') => break,
                other => return Err(format!("','または']'を期待しましたが{other:?}でした")),
            }
        }
        Ok(Value::Array(items))
    }

    fn parse_string(chars: &mut CharStream) -> Result<String, String> {
        skip_whitespace(chars);
        expect(chars, '"')?;
        let mut result = String::new();
        loop {
            match chars.next() {
                Some('"') => break,
                Some('\\') => match chars.next() {
                    Some('"') => result.push('"'),
                    Some('\\') => result.push('\\'),
                    Some('/') => result.push('/'),
                    Some('b') => result.push('\u{0008}'),
                    Some('f') => result.push('\u{000C}'),
                    Some('n') => result.push('\n'),
                    Some('r') => result.push('\r'),
                    Some('t') => result.push('\t'),
                    Some('u') => result.push(parse_unicode_escape(chars)?),
                    other => return Err(format!("不正なエスケープシーケンスです: {other:?}")),
                },
                Some(c) => result.push(c),
                None => return Err("文字列が閉じられていません".to_string()),
            }
        }
        Ok(result)
    }

    fn parse_unicode_escape(chars: &mut CharStream) -> Result<char, String> {
        let hex: String = (0..4)
            .map(|_| {
                chars
                    .next()
                    .ok_or_else(|| "\\uエスケープが不完全です".to_string())
            })
            .collect::<Result<String, String>>()?;
        let code = u32::from_str_radix(&hex, 16).map_err(|e| e.to_string())?;
        char::from_u32(code).ok_or_else(|| "不正なUnicodeコードポイントです".to_string())
    }

    fn parse_bool(chars: &mut CharStream) -> Result<Value, String> {
        if chars.clone().take(4).collect::<String>() == "true" {
            for _ in 0..4 {
                chars.next();
            }
            Ok(Value::Bool(true))
        } else if chars.clone().take(5).collect::<String>() == "false" {
            for _ in 0..5 {
                chars.next();
            }
            Ok(Value::Bool(false))
        } else {
            Err("真偽値のパースに失敗しました".to_string())
        }
    }

    fn parse_null(chars: &mut CharStream) -> Result<Value, String> {
        if chars.clone().take(4).collect::<String>() == "null" {
            for _ in 0..4 {
                chars.next();
            }
            Ok(Value::Null)
        } else {
            Err("nullのパースに失敗しました".to_string())
        }
    }

    fn parse_number(chars: &mut CharStream) -> Result<Value, String> {
        let mut raw = String::new();
        if chars.peek() == Some(&'-') {
            raw.push(chars.next().expect("直前にpeekで確認済み"));
        }
        while matches!(chars.peek(), Some(c) if c.is_ascii_digit()) {
            raw.push(chars.next().expect("直前にpeekで確認済み"));
        }
        if chars.peek() == Some(&'.') {
            raw.push(chars.next().expect("直前にpeekで確認済み"));
            while matches!(chars.peek(), Some(c) if c.is_ascii_digit()) {
                raw.push(chars.next().expect("直前にpeekで確認済み"));
            }
        }
        if matches!(chars.peek(), Some('e') | Some('E')) {
            raw.push(chars.next().expect("直前にpeekで確認済み"));
            if matches!(chars.peek(), Some('+') | Some('-')) {
                raw.push(chars.next().expect("直前にpeekで確認済み"));
            }
            while matches!(chars.peek(), Some(c) if c.is_ascii_digit()) {
                raw.push(chars.next().expect("直前にpeekで確認済み"));
            }
        }
        raw.parse::<f64>()
            .map(Value::Number)
            .map_err(|e| e.to_string())
    }
}

struct ReleaseAsset {
    name: String,
    browser_download_url: String,
}

struct ReleaseInfo {
    tag_name: String,
    assets: Vec<ReleaseAsset>,
}

fn parse_release_info(body: &str) -> Result<ReleaseInfo, BinaryManagerError> {
    let root = json::parse(body).map_err(BinaryManagerError::InvalidJson)?;
    let tag_name = root
        .get("tag_name")
        .and_then(json::Value::as_str)
        .ok_or_else(|| {
            BinaryManagerError::InvalidJson("tag_nameフィールドがありません".to_string())
        })?
        .to_string();
    let assets = root
        .get("assets")
        .and_then(json::Value::as_array)
        .ok_or_else(|| BinaryManagerError::InvalidJson("assetsフィールドがありません".to_string()))?
        .iter()
        .filter_map(|asset| {
            let name = asset.get("name")?.as_str()?.to_string();
            let browser_download_url = asset.get("browser_download_url")?.as_str()?.to_string();
            Some(ReleaseAsset {
                name,
                browser_download_url,
            })
        })
        .collect();
    Ok(ReleaseInfo { tag_name, assets })
}

/// アセット名が正確に`yt-dlp.exe`であるものを選ぶ。
/// yt-dlpのリリースは`yt-dlp`(拡張子なし, Unix向け)や`yt-dlp.tar.gz`等も含むため、
/// Windows向け単体exeだけを名前の完全一致で絞り込む。
fn select_ytdlp_asset(assets: &[ReleaseAsset]) -> Option<&ReleaseAsset> {
    assets
        .iter()
        .find(|asset| asset.name.eq_ignore_ascii_case(YTDLP_ASSET_NAME))
}

/// BtbN/FFmpeg-Buildsのアセット名は日時等を含み変動するため、固定名一致ではなく
/// ファイル名をトークン分解して「win64」かつ「gpl」または「essentials」を含み、
/// 共有ライブラリ版(shared)ではないものを選ぶ。
fn select_ffmpeg_asset(assets: &[ReleaseAsset]) -> Option<&ReleaseAsset> {
    assets
        .iter()
        .find(|asset| is_ffmpeg_win64_asset(&asset.name))
}

fn is_ffmpeg_win64_asset(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let Some(stem) = lower.strip_suffix(".zip") else {
        return false;
    };
    let tokens: Vec<&str> = stem.split(['-', '_', '.']).collect();
    tokens.contains(&"win64")
        && (tokens.contains(&"gpl") || tokens.contains(&"essentials"))
        && !tokens.contains(&"shared")
}

/// カスタムパスと自動管理パスのどちらを使うか判定する。空文字列は「未設定」を表す
/// (`Config`側の規約に従う)。
fn resolve_binary_path(
    custom_path: &str,
    auto_managed_file_name: &str,
) -> Result<PathBuf, BinaryManagerError> {
    if custom_path.is_empty() {
        Ok(config::bin_dir()?.join(auto_managed_file_name))
    } else {
        Ok(PathBuf::from(custom_path))
    }
}

pub fn resolve_ytdlp_path(config: &Config) -> Result<PathBuf, BinaryManagerError> {
    resolve_binary_path(&config.binaries.ytdlp_path, YTDLP_EXE_NAME)
}

pub fn resolve_ffmpeg_path(config: &Config) -> Result<PathBuf, BinaryManagerError> {
    resolve_binary_path(&config.binaries.ffmpeg_path, FFMPEG_EXE_NAME)
}

/// 現在保存済みのバージョンと最新版を比較した結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionCheckResult {
    UpToDate,
    UpdateAvailable { current: String, latest: String },
    NotInstalled { latest: String },
}

/// yt-dlpは日付ベースのバージョニング、ffmpegビルドはビルドごとに異なる命名のtagを
/// 使うため、厳密なバージョン順序比較ではなく「文字列が一致しなければ更新がある」
/// という単純な判定に留める(過度に複雑にしないための設計判断)。
fn check_for_update(current: Option<&str>, latest: &str) -> VersionCheckResult {
    match current {
        None => VersionCheckResult::NotInstalled {
            latest: latest.to_string(),
        },
        Some(current) if current == latest => VersionCheckResult::UpToDate,
        Some(current) => VersionCheckResult::UpdateAvailable {
            current: current.to_string(),
            latest: latest.to_string(),
        },
    }
}

fn read_version_meta(bin_dir: &Path, file_name: &str) -> Option<String> {
    std::fs::read_to_string(bin_dir.join(file_name))
        .ok()
        .map(|content| content.trim().to_string())
}

fn write_version_meta(
    bin_dir: &Path,
    file_name: &str,
    version: &str,
) -> Result<(), BinaryManagerError> {
    std::fs::write(bin_dir.join(file_name), version)?;
    Ok(())
}

fn build_http_client() -> Result<reqwest::Client, BinaryManagerError> {
    Ok(reqwest::Client::builder()
        .user_agent(GITHUB_USER_AGENT)
        .build()?)
}

async fn fetch_release_info(api_url: &str) -> Result<ReleaseInfo, BinaryManagerError> {
    let client = build_http_client()?;
    let body = client
        .get(api_url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    parse_release_info(&body)
}

async fn download_to_file(url: &str, dest: &Path) -> Result<(), BinaryManagerError> {
    let client = build_http_client()?;
    let bytes = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    std::fs::write(dest, &bytes)?;
    Ok(())
}

/// yt-dlpが未ダウンロードなら最新版を取得して`bin_dir()`直下に保存し、実行ファイルの
/// パスを返す。既にダウンロード済みならネットワークアクセスせずそのパスを返す。
pub async fn ensure_ytdlp_installed() -> Result<PathBuf, BinaryManagerError> {
    let bin_dir = config::bin_dir()?;
    std::fs::create_dir_all(&bin_dir)?;
    let exe_path = bin_dir.join(YTDLP_EXE_NAME);
    if exe_path.exists() {
        return Ok(exe_path);
    }

    let release = fetch_release_info(YTDLP_RELEASE_API).await?;
    let asset = select_ytdlp_asset(&release.assets)
        .ok_or_else(|| BinaryManagerError::AssetNotFound(YTDLP_ASSET_NAME.to_string()))?;
    download_to_file(&asset.browser_download_url, &exe_path).await?;
    write_version_meta(&bin_dir, YTDLP_VERSION_META_FILE, &release.tag_name)?;
    Ok(exe_path)
}

/// ffmpeg/ffprobeが未ダウンロードなら最新のWindows向けビルドを取得し、zipから
/// 実行ファイルのみを`bin_dir()`直下に展開する。既にダウンロード済みならそのまま返す。
pub async fn ensure_ffmpeg_installed() -> Result<(PathBuf, PathBuf), BinaryManagerError> {
    let bin_dir = config::bin_dir()?;
    std::fs::create_dir_all(&bin_dir)?;
    let ffmpeg_path = bin_dir.join(FFMPEG_EXE_NAME);
    let ffprobe_path = bin_dir.join(FFPROBE_EXE_NAME);
    if ffmpeg_path.exists() && ffprobe_path.exists() {
        return Ok((ffmpeg_path, ffprobe_path));
    }

    let release = fetch_release_info(FFMPEG_RELEASE_API).await?;
    let asset = select_ffmpeg_asset(&release.assets).ok_or_else(|| {
        BinaryManagerError::AssetNotFound("win64向けのgpl/essentialsビルド".to_string())
    })?;

    let client = build_http_client()?;
    let zip_bytes = client
        .get(asset.browser_download_url.as_str())
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    extract_ffmpeg_binaries(&zip_bytes, &ffmpeg_path, &ffprobe_path)?;
    write_version_meta(&bin_dir, FFMPEG_VERSION_META_FILE, &release.tag_name)?;
    Ok((ffmpeg_path, ffprobe_path))
}

fn extract_ffmpeg_binaries(
    zip_bytes: &[u8],
    ffmpeg_dest: &Path,
    ffprobe_dest: &Path,
) -> Result<(), BinaryManagerError> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes))?;
    extract_single_file_by_suffix(&mut archive, FFMPEG_ARCHIVE_FFMPEG_SUFFIX, ffmpeg_dest)?;
    extract_single_file_by_suffix(&mut archive, FFMPEG_ARCHIVE_FFPROBE_SUFFIX, ffprobe_dest)?;
    Ok(())
}

/// BtbNのビルドは`<ルート>/bin/ffmpeg.exe`のようにトップレベルフォルダ名がバージョン
/// ごとに変わるため、フルパス一致ではなく末尾一致で対象ファイルを探す。
fn extract_single_file_by_suffix<R: std::io::Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    suffix: &str,
    dest: &Path,
) -> Result<(), BinaryManagerError> {
    let index = archive
        .file_names()
        .position(|name| name.replace('\\', "/").ends_with(suffix))
        .ok_or_else(|| BinaryManagerError::FileNotFoundInArchive(suffix.to_string()))?;
    let mut entry = archive.by_index(index)?;
    let mut out_file = std::fs::File::create(dest)?;
    std::io::copy(&mut entry, &mut out_file)?;
    Ok(())
}

pub async fn check_ytdlp_update() -> Result<VersionCheckResult, BinaryManagerError> {
    let bin_dir = config::bin_dir()?;
    let current = read_version_meta(&bin_dir, YTDLP_VERSION_META_FILE);
    let release = fetch_release_info(YTDLP_RELEASE_API).await?;
    Ok(check_for_update(current.as_deref(), &release.tag_name))
}

pub async fn check_ffmpeg_update() -> Result<VersionCheckResult, BinaryManagerError> {
    let bin_dir = config::bin_dir()?;
    let current = read_version_meta(&bin_dir, FFMPEG_VERSION_META_FILE);
    let release = fetch_release_info(FFMPEG_RELEASE_API).await?;
    Ok(check_for_update(current.as_deref(), &release.tag_name))
}

#[cfg(test)]
mod tests {
    use super::*;

    const YTDLP_RELEASE_JSON: &str = r#"
    {
        "tag_name": "2025.08.20",
        "assets": [
            { "name": "yt-dlp", "browser_download_url": "https://example.com/yt-dlp" },
            { "name": "yt-dlp.exe", "browser_download_url": "https://example.com/yt-dlp.exe" },
            { "name": "yt-dlp.tar.gz", "browser_download_url": "https://example.com/yt-dlp.tar.gz" },
            { "name": "yt-dlp_win.zip", "browser_download_url": "https://example.com/yt-dlp_win.zip" }
        ]
    }
    "#;

    const FFMPEG_RELEASE_JSON: &str = r#"
    {
        "tag_name": "latest",
        "assets": [
            { "name": "ffmpeg-master-latest-linux64-gpl.tar.xz", "browser_download_url": "https://example.com/linux.tar.xz" },
            { "name": "ffmpeg-master-latest-win64-lgpl.zip", "browser_download_url": "https://example.com/lgpl.zip" },
            { "name": "ffmpeg-master-latest-win64-gpl-shared.zip", "browser_download_url": "https://example.com/shared.zip" },
            { "name": "ffmpeg-master-latest-win64-gpl.zip", "browser_download_url": "https://example.com/gpl.zip" }
        ]
    }
    "#;

    #[test]
    fn parses_tag_name_and_assets_from_release_json() {
        let release = parse_release_info(YTDLP_RELEASE_JSON).expect("パースに成功するはず");
        assert_eq!(release.tag_name, "2025.08.20");
        assert_eq!(release.assets.len(), 4);
    }

    #[test]
    fn selects_windows_exe_asset_for_ytdlp() {
        let release = parse_release_info(YTDLP_RELEASE_JSON).expect("パースに成功するはず");
        let asset = select_ytdlp_asset(&release.assets).expect("yt-dlp.exeが選ばれるはず");
        assert_eq!(asset.name, "yt-dlp.exe");
        assert_eq!(asset.browser_download_url, "https://example.com/yt-dlp.exe");
    }

    #[test]
    fn selects_win64_gpl_zip_asset_for_ffmpeg_and_skips_shared_and_lgpl() {
        let release = parse_release_info(FFMPEG_RELEASE_JSON).expect("パースに成功するはず");
        let asset = select_ffmpeg_asset(&release.assets).expect("win64のgplビルドが選ばれるはず");
        assert_eq!(asset.name, "ffmpeg-master-latest-win64-gpl.zip");
    }

    #[test]
    fn selects_essentials_build_when_gpl_keyword_is_absent() {
        let assets = vec![
            ReleaseAsset {
                name: "ffmpeg-n7.0-win64-essentials_build.zip".to_string(),
                browser_download_url: "https://example.com/essentials.zip".to_string(),
            },
            ReleaseAsset {
                name: "ffmpeg-n7.0-win64-full_build.zip".to_string(),
                browser_download_url: "https://example.com/full.zip".to_string(),
            },
        ];
        let asset = select_ffmpeg_asset(&assets).expect("essentialsビルドが選ばれるはず");
        assert_eq!(asset.name, "ffmpeg-n7.0-win64-essentials_build.zip");
    }

    #[test]
    fn returns_none_when_no_ffmpeg_asset_matches() {
        let assets = vec![ReleaseAsset {
            name: "ffmpeg-master-latest-linux64-gpl.tar.xz".to_string(),
            browser_download_url: "https://example.com/linux.tar.xz".to_string(),
        }];
        assert!(select_ffmpeg_asset(&assets).is_none());
    }

    #[test]
    fn resolve_binary_path_uses_custom_path_when_present() {
        let mut config = Config::default();
        config.binaries.ytdlp_path = "D:/tools/yt-dlp.exe".to_string();

        let resolved = resolve_ytdlp_path(&config).expect("解決に成功するはず");
        assert_eq!(resolved, PathBuf::from("D:/tools/yt-dlp.exe"));
    }

    #[test]
    fn resolve_binary_path_uses_auto_managed_path_when_empty() {
        let config = Config::default();

        let resolved = resolve_ytdlp_path(&config).expect("解決に成功するはず");
        let expected = config::bin_dir()
            .expect("bin_dirが解決できるはず")
            .join(YTDLP_EXE_NAME);
        assert_eq!(resolved, expected);
    }

    #[test]
    fn resolve_ffmpeg_path_respects_custom_path_too() {
        let mut config = Config::default();
        config.binaries.ffmpeg_path = "D:/tools/ffmpeg.exe".to_string();

        let resolved = resolve_ffmpeg_path(&config).expect("解決に成功するはず");
        assert_eq!(resolved, PathBuf::from("D:/tools/ffmpeg.exe"));
    }

    #[test]
    fn check_for_update_reports_not_installed_when_no_current_version() {
        let result = check_for_update(None, "2025.08.20");
        assert_eq!(
            result,
            VersionCheckResult::NotInstalled {
                latest: "2025.08.20".to_string()
            }
        );
    }

    #[test]
    fn check_for_update_reports_up_to_date_when_versions_match() {
        let result = check_for_update(Some("2025.08.20"), "2025.08.20");
        assert_eq!(result, VersionCheckResult::UpToDate);
    }

    #[test]
    fn check_for_update_reports_update_available_when_versions_differ() {
        let result = check_for_update(Some("2025.07.01"), "2025.08.20");
        assert_eq!(
            result,
            VersionCheckResult::UpdateAvailable {
                current: "2025.07.01".to_string(),
                latest: "2025.08.20".to_string(),
            }
        );
    }

    #[test]
    fn json_parser_handles_nested_structures_and_escaped_strings() {
        let value = json::parse(r#"{"a": [1, 2.5, "x\ny", true, false, null]}"#)
            .expect("パースに成功するはず");
        let array = value.get("a").expect("aキーが存在するはず");
        let array = array.as_array().expect("配列であるはず");
        assert_eq!(array.len(), 6);
        assert_eq!(array[2], json::Value::String("x\ny".to_string()));
        assert_eq!(array[3], json::Value::Bool(true));
        assert_eq!(array[4], json::Value::Bool(false));
        assert_eq!(array[5], json::Value::Null);
    }
}
