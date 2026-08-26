# Contributing / コントリビューションガイド

このプロジェクト(Featherpull)はOSS(オープンソースソフトウェア)です。Issue報告・Pull Requestなど、どなたからのコントリビューションも歓迎します。

## ⚠️ 非公開情報を含めないこと

本リポジトリはMIT Licenseの下で公開されており、コード・コミット履歴・Issue・Pull Requestは全世界に公開されます。Issue・PRの本文・コメント・コミットメッセージには以下を**含めないでください**:

- Notionなど非公開ツールへのリンクやURL
- 個人情報、非公開の組織情報
- APIキー・トークン・パスワードなどの認証情報やシークレット

社内/個人用の開発計画メモ等を参照する場合は、その内容を一般化・匿名化した上で記載してください。

## Issueを立てる

- バグ報告には、再現手順・期待する動作・実際の動作・環境情報(OS、Rustバージョン等)を記載してください。
- 機能提案には、背景(なぜ必要か)と、可能であれば実現方法のアイデアを記載してください。

## 開発環境のセットアップ

```bash
git clone https://github.com/Harukoto-Project/Featherpull.git
cd Featherpull
cargo build
```

動作確認には [yt-dlp](https://github.com/yt-dlp/yt-dlp) と [ffmpeg](https://ffmpeg.org/) が別途必要です。

## ブランチ命名規則

作業ブランチは `main` から作成し、以下の形式で命名してください。

```
<type>/<英語・ケバブケースの短い説明>
```

- `<type>` は [コミットメッセージ規約](./COMMIT_CONVENTION.md) の`type`と同じ語彙を使う: `feat`, `fix`, `docs`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`
- 説明部分は英語・ケバブケース(半角ハイフン区切り)とし、日本語・スペース・大文字は使わない
- 関連Issueがある場合は説明の先頭にIssue番号を付ける: `<type>/<issue番号>-<説明>`

例:

```
feat/job-queue-cancel
fix/42-progress-parse-crash
docs/contributing-update
chore/upgrade-egui
```

## Pull Requestの作成手順

1. リポジトリをFork し、上記の命名規則に従って作業用ブランチを作成する。
2. 変更を行い、[コミットメッセージ規約](./COMMIT_CONVENTION.md) に従ってコミットする。
3. `cargo fmt` / `cargo clippy` / `cargo test` を実行し、問題がないことを確認する。
4. Pull Requestを作成する。タイトル・本文にも非公開情報を含めないこと。
   - タイトルはコミットメッセージ規約に準じた形式にする(例: `feat: プレイリスト一括ダウンロードに対応`)。
   - 本文は以下のテンプレートに従って記載する:

     ```markdown
     ## Summary
     - 変更点を簡潔に(bullet)

     ## Motivation / Why
     - なぜこの変更が必要か

     ## Test plan
     - 動作確認した内容・手順
     ```
   - 1つのPRは1つの論理的な変更にまとめる(無関係な変更を混在させない)。
5. レビューでの指摘に対応し、CIが通ることを確認する。
6. マージ方式は基本的に Squash and merge を用います(コミット履歴をきれいに保つため)。

## コードスタイル

- `cargo fmt` でフォーマットを統一してください。
- `cargo clippy` の警告は解消するようにしてください。
- コメントは「何をしているか」ではなく「なぜそうしているか」を説明するものに限定してください。

## ライセンス

コントリビューションは本プロジェクトの [LICENSE](./LICENSE)(MIT License)の下で公開されることに同意したものとみなします。
