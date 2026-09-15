# slack-claude-remote 開発計画

Slack から、手元の Mac で動いている Claude Code の対話セッションを選び、スレッドで会話するツール。

## 1. 背景と方式

公式の Remote Control（`claude --remote-control --name X` をスマホアプリから操作する機能）は
Anthropic 内部のリレーを使っており、第三者向けの API は公開されていない。
手元の対話セッションに外部から話しかける公式の経路は **channels 機構**（research preview、Claude Code 2.1.212 以降）だけである。

channels の制約（公式ドキュメント確認済み）:

- channel は、Claude Code が**同じマシン上で stdio で起動する MCP サーバー**である。リモートの HTTP MCP は使えない
- 投入: サーバーから Claude へ `notifications/claude/channel {content, meta}` を送る。Claude には `<channel source=... ...>` タグとして届く
- 返答: Claude が `reply` ツールを呼ぶ。`instructions` 文字列で誘導する必要がある
- 権限中継: Claude から `notifications/claude/channel/permission_request`、サーバーから `notifications/claude/channel/permission`
- セッション ID は channel に渡されないため、名前は自前で決める
- 起動: `claude --dangerously-load-development-channels server:<MCP名>`

参考:

- https://code.claude.com/docs/en/channels.md
- https://code.claude.com/docs/en/channels-reference.md
- https://code.claude.com/docs/en/remote-control.md

構成:

```
[Mac] claude --channels ─stdio─▶ sccr-agent ─WSS(outbound)─▶ [e2] sccr-relay ◀─HTTPS─ Slack
                                                                  ▲ Caddy (TLS終端, reverse_proxy)
```

決定事項: channels 中継 / Sign in with Slack (OIDC) の Web 画面 / スラッシュコマンド＋スレッド /
Events API (HTTPS webhook) / Rust / TDD。

## 2. 前提

1. OIDC ログイン画面の役割は、本人確認、agent トークン発行、接続中セッション一覧の 3 つ。会話は Slack スレッドで行う。
   `SCCR_ADMIN_SLACK_USER` と一致する Slack user ID だけがログインでき、ログインした user ID が許可ユーザーに登録される。
2. 単一テナント・個人用。agent は relay に紐付き、許可ユーザー全員から見える。
3. 永続化は JSON ファイル 1 個。DB は使わない。
4. agent の MCP 面は手書きの newline-delimited JSON-RPC (stdio)。rmcp は使わない。`mcp.rs` に隔離する。
5. `chat_id` は `"{channel}:{thread_ts}"` の 1 文字列。
6. セッションのキーは agent が申告する `name`。再接続しても束縛は残る。同名が接続中なら後から来た方を拒否する。
7. Rust 2024 edition、cargo workspace。
8. ドキュメントは日本語。コード内コメントは英語で最小限。
9. Cookie セッションはメモリ保持。relay 再起動でログアウトされる（許容）。

## 3. リポジトリ構成

```
Cargo.toml  rustfmt.toml  .gitignore  README.md  .github/workflows/ci.yml
docs/spec.md                   # 仕様（T0-2 で作成）。全タスクの正
docs/plan/README.md            # 本書
docs/plan/T*.md                # タスクカード
crates/protocol/src/lib.rs     # package: protocol
crates/agent/src/              # package: sccr-agent   main.rs mcp.rs ws.rs
crates/relay/src/              # package: sccr-relay   main.rs lib.rs config.rs state.rs routes.rs hub.rs
                               #   store.rs chunk.rs permission.rs
                               #   slack/{mod,verify,events,commands,interactions,api}.rs
                               #   auth/{mod,oidc,session,html}.rs
crates/relay/tests/e2e.rs
deploy/{Caddyfile.snippet, sccr-relay.service, slack-manifest.json, env.example}
```

- 1 ファイル 200-400 行、最大 800 行
- agent の stdout は MCP 専用。ログは必ず stderr
- relay は `lib.rs` にロジックを置き、`main.rs` は起動だけにする（統合テストから使うため）

## 4. 共通契約

詳細と例は `docs/spec.md` を正とする。ここは要約。

### 4.1 protocol クレート（WebSocket text frame、JSON）

```rust
pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentMsg {
    Hello { version: u32, name: String, host: String, cwd: String },
    Reply { chat_id: String, text: String },
    PermissionRequest { request_id: String, tool_name: String, description: String, input_preview: String },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RelayMsg {
    Welcome { name: String },
    Inbound { chat_id: String, user: String, text: String },
    PermissionVerdict { request_id: String, behavior: Behavior },
    Error { message: String }, // relay closes the socket after sending
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Behavior { Allow, Deny }
```

### 4.2 agent の MCP 面（stdio、1 行 1 JSON）

- `initialize` の応答:
  `{protocolVersion: <要求値をそのまま返す>, serverInfo: {name: "sccr", version}, capabilities: {experimental: {"claude/channel": {}, "claude/channel/permission": {}}, tools: {}}, instructions: INSTRUCTIONS}`
- `INSTRUCTIONS`:
  `Messages from Slack arrive as <channel source="sccr" chat_id="..." user="...">. Always answer by calling the reply tool with the same chat_id. Keep replies concise; Slack renders plain text and mrkdwn.`
- `tools/list`: `reply` だけを返す。引数 `chat_id: string`、`text: string`、両方必須
- `tools/call reply`: 応答 `{content: [{type: "text", text: "sent"}]}` を返し、`AgentMsg::Reply` を WS へ流す
- `ping`: `{}` を返す
- 受信した `RelayMsg::Inbound` は stdout に次を出す:
  `{"jsonrpc":"2.0","method":"notifications/claude/channel","params":{"content":text,"meta":{"chat_id":..,"user":..}}}`
- stdin の `notifications/claude/channel/permission_request` は `AgentMsg::PermissionRequest` にして WS へ流す
- 受信した `RelayMsg::PermissionVerdict` は stdout に次を出す:
  `{"jsonrpc":"2.0","method":"notifications/claude/channel/permission","params":{"request_id":..,"behavior":"allow"|"deny"}}`
- 未知メソッドのリクエストは `error {code: -32601}`。未知の通知は無視。不正な JSON 行は stderr に 1 行出して継続

### 4.3 relay の HTTP 面

| path | 用途 |
|---|---|
| `POST /slack/events` | Events API。全リクエストで署名検証。`url_verification` は challenge を返す。他は即 200、処理は非同期 |
| `POST /slack/commands` | `/cc [list\|unbind\|help]`。署名検証。応答 body で ephemeral メッセージを返す |
| `POST /slack/interactions` | `payload=` form。`block_actions` の `action_id=sccr_select` |
| `GET /agent/ws` | `Authorization: Bearer <token>`。WebSocket upgrade |
| `GET /login` `GET /auth/callback` `GET /` `POST /tokens` | OIDC と管理画面 |
| `GET /healthz` | `ok` |
| `POST /dev/inject` | `SCCR_DEV=1` のときだけ有効。ローカル検証用 |

### 4.4 環境変数

relay:

- `SCCR_BIND`（既定 `127.0.0.1:8080`）
- `SCCR_PUBLIC_URL`（例 `https://sccr.example.jp`）
- `SCCR_STATE_FILE`（既定 `./sccr-state.json`）
- `SCCR_ADMIN_SLACK_USER`
- `SLACK_SIGNING_SECRET` `SLACK_BOT_TOKEN` `SLACK_CLIENT_ID` `SLACK_CLIENT_SECRET`
- `SCCR_DEV`（`1` で開発モード。Slack 秘密なしで起動、トークン `dev` を受理、Slack 投稿を stdout に出す）

agent:

- `SCCR_RELAY_URL`（例 `wss://sccr.example.jp/agent/ws`）
- `SCCR_TOKEN`
- `SCCR_SESSION_NAME`（任意。既定は `hostname:カレントディレクトリ名`）

### 4.5 状態ファイル

```json
{
  "users": ["U0123"],
  "tokens": [{"sha256": "...", "label": "macbook", "created": "2026-09-16T00:00:00Z"}],
  "bindings": {"C0AB:1700000000.000100": {"session": "macbook:proj", "user": "U0123"}}
}
```

書き込みは一時ファイルへ書いてから rename する。

### 4.6 セキュリティ境界（省略不可）

- Slack 署名 `v0=HMAC-SHA256(secret, "v0:{ts}:{body}")` を定数時間比較。`|now - ts| <= 300` 秒。失敗は 401
- 許可ユーザー以外のスレッド投稿・`/cc`・権限返答は捨てる。channel 通知は prompt injection の入口になる
- 権限返答（`yes abcde`）は、そのスレッドを束縛したユーザーからのものだけ有効
- agent トークンはランダム 32 byte を base64url で 1 回だけ表示。保存は SHA-256 のみ
- OIDC は `state`、`nonce`、`aud == client_id`、`iss == https://slack.com`、`exp` を検証する。
  id_token は client_secret を使った token 交換で Slack から TLS で直接受け取るため、JWT 署名検証は省略する
- Cookie は `HttpOnly; Secure; SameSite=Lax`。`POST /tokens` の CSRF 対策は SameSite=Lax に依存する
- ログにメッセージ本文、トークン、署名を出さない（`SCCR_DEV=1` の Slack 投稿出力を除く）

## 5. タスク一覧と順序

| ID | 内容 | モデル | 依存 |
|---|---|---|---|
| T0-1 | リポジトリ骨格 | 完了 | - |
| T0-2 | 仕様書 docs/spec.md | opus | T0-1 |
| T1-1 | protocol 型 | sonnet | T0-2 |
| T2-1 | agent MCP ディスパッチ | sonnet | T1-1 |
| T2-2 | agent stdio ループ | sonnet | T2-1 |
| T3-1 | agent WS クライアント | sonnet | T2-2 |
| T4-1 | Slack 署名検証 | sonnet | T0-2 |
| T4-2 | Slack events 分類 | sonnet | T0-2 |
| T4-3 | commands / interactions | sonnet | T0-2 |
| T4-4 | hub / store / routes | opus | T1-1 T4-1 T4-2 T4-3 T6-1 |
| T6-1 | 返答分割 chunk | sonnet | T0-2 |
| T3-2 | 手動スモーク（Claude 実機） | opus＋人 | T3-1 T4-4 |
| T5-1 | OIDC 基礎 | sonnet | T0-2 |
| T5-2 | 管理画面・トークン発行 | opus | T5-1 T4-4 |
| T6-2 | 権限中継 | sonnet | T4-4 |
| T6-3 | 統合テスト | opus | T2-1 T5-2 T6-2 |
| T7-1 | デプロイ資材 | sonnet | T6-3 |
| T7-2 | 実機 E2E | opus＋人 | T7-1 |

```
T0-1 → T0-2 → T1-1 → T2-1 → T2-2 → T3-1 ─────────────┐
            ├→ T4-1 ┐                                 ├→ T3-2
            ├→ T4-2 ├→ T4-4 → T6-2 ┐                  │
            ├→ T4-3 ┤        └─────┼→ T5-2 → T6-3 → T7-1 → T7-2
            ├→ T6-1 ┘              │
            └→ T5-1 ───────────────┘
```

並列可能なまとまり: `{T1-1→T2-*→T3-1}`、`{T4-1, T4-2, T4-3, T6-1, T5-1}`。
同じ Cargo.toml を触るタスクを並列にするときは worktree を分ける。

## 6. サブエージェントへの共通指示

各タスクカードは、次のテンプレートに `<ID>` を入れてサブエージェントに渡す。

```
docs/plan/README.md、docs/spec.md、docs/plan/<ID>.md を読み、<ID> を実施せよ。
手順:
1. カードの「テスト」を先に書く。単体テストは対象モジュールの子ファイル
   src/<module>/tests.rs（または src/<dir>/<module>/tests.rs）に置き、本体に
   #[cfg(test)] mod tests; を 1 行だけ足す。統合テストは crates/<crate>/tests/。
2. cargo test を実行し、コンパイルエラーまたは失敗で落ちることを確認する（本体は todo!() でよい）。
3. 実装してテストを通す。テストを実装に合わせて書き換えない。仕様と矛盾するなら報告して止まる。
4. cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test --all を緑にする。
5. 依存は cargo add で追加する（バージョンを手書きしない）。カードにない依存を足したら理由を報告する。
6. 1 タスク 1 コミット。メッセージは conventional commits（例 feat(agent): ...）。
報告: 変更ファイル、追加依存、テスト名一覧、仕様との差分や疑問点。
禁止: カードの範囲外のファイル変更、unwrap() の本体コードでの使用（テストは可）、本文やトークンのログ出力。
```

## 7. 人に用意してもらうもの（T7 までに）

- e2 のホスト名と Caddy への追記。DNS が e2 を向いていること
- e2 に Rust toolchain があるか（なければ Mac でクロスビルドして転送）
- Slack アプリ作成（T7-1 の manifest を使う）と 4 つの秘密
- 自分の Slack user ID

## 8. 検証

- 各タスク: fmt、clippy `-D warnings`、test が緑
- T3-2: ローカルで Claude が `reply` を呼ぶこと（instructions が効くこと）
- T6-3: 全経路の統合テスト
- T7-2: 実 Slack と実 e2 で、権限プロンプトの往復まで
- 意図的にテストしないもの: `main.rs` の起動と環境変数読み込み、HTML の文言、
  `HttpSlack` と `OidcHttp` の実 HTTP 呼び出し（トレイト境界で偽物に差し替える）
