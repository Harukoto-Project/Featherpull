# コミットメッセージ規約

このプロジェクトは [Conventional Commits](https://www.conventionalcommits.org/ja/v1.0.0/) をベースにしたコミットメッセージ規約を採用しています。

## 基本フォーマット

```
<type>(<scope>): <subject>

<body>

<footer>
```

- `<scope>` と `<body>` / `<footer>` は省略可能です。
- `<subject>` は日本語・英語のどちらでも構いませんが、1行で簡潔にまとめてください(目安50文字以内)。
- 本文(`<body>`)には「何を」ではなく「なぜ」その変更が必要だったのかを書くことを推奨します。

## type一覧

| type | 説明 |
| --- | --- |
| `feat` | 新機能の追加 |
| `fix` | バグ修正 |
| `docs` | ドキュメントのみの変更(README, CONTRIBUTING等) |
| `style` | フォーマット等、動作に影響しない変更(セミコロン、空白など) |
| `refactor` | 機能追加・バグ修正を含まないコードの再構成 |
| `perf` | パフォーマンス改善 |
| `test` | テストの追加・修正 |
| `build` | ビルドシステムや依存関係(Cargo.toml等)の変更 |
| `ci` | CI設定ファイル・スクリプトの変更 |
| `chore` | その他の雑務的な変更(上記に当てはまらないもの) |
| `revert` | 以前のコミットの取り消し |

## scopeの例

`ui`, `core`, `ytdlp`, `ffmpeg`, `config`, `queue` など、変更対象のモジュール名を使用してください。

例:

```
feat(ytdlp): フォーマット選択オプションのパースに対応
fix(queue): 並列実行数の上限を超えてジョブが開始される問題を修正
docs: CONTRIBUTINGにPR作成手順を追記
```

## Breaking Changeの表記

後方互換性のない変更を行った場合は、footerに `BREAKING CHANGE:` を付けて説明を記載してください。

```
feat(config): 設定ファイルのフォーマットをTOMLからRONに変更

BREAKING CHANGE: 既存のconfig.tomlは読み込めなくなります。config.ronへの移行が必要です。
```

## 注意事項(OSSプロジェクトとして)

本プロジェクトはOSSであり、コミットメッセージも全世界に公開されます。以下を**コミットメッセージに含めないこと**:

- Notionなど非公開ツールへのリンクやURL
- 個人情報、非公開の組織情報
- APIキー・トークンなどの認証情報やシークレット

詳細は [CONTRIBUTING.md](./CONTRIBUTING.md) を参照してください。
