# slack-claude-remote

Slack から、手元で動いている Claude Code の対話セッションを選び、スレッドで会話するツール。

```
[Mac] claude --channels ─stdio─▶ sccr-agent ─WSS─▶ [e2] sccr-relay ◀─HTTPS─ Slack
```

- `crates/protocol`: relay と agent の間のメッセージ型
- `crates/agent`: Claude Code の channel（MCP stdio サーバー）。relay へ WebSocket で接続する
- `crates/relay`: Slack Events API、Sign in with Slack、agent の中継

開発中。計画は [docs/plan/README.md](docs/plan/README.md)、仕様は `docs/spec.md`（作成予定）。

## 開発

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test --all
```

## 動作確認メモ

未実施。
