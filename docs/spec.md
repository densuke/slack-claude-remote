# slack-claude-remote 仕様書

本書は、テストを書く担当と実装する担当が共通で参照する唯一の仕様です。
`docs/plan/README.md` §4（共通契約）を正とし、それを詳細化しています。
タスクカード（`docs/plan/T*.md`）は本書の節番号（例: 「spec §5.2」）で参照するため、節番号は変更しないでください。

表記について:

- 「MUST」は必須、「しない」は禁止を表します。
- JSON の例はすべてそのままパースできる形で書いています。説明はコードブロックの外に置きます。
- `{name}` のような波括弧は、文言や URL の中の差し込み位置を表します。
- 時刻は特に断りがなければ Unix 秒（`i64`）です。

---

## 1. 用語

| 用語 | 定義 |
|---|---|
| session name | agent が `Hello.name` で申告するセッションの名前。relay 内でセッションを識別するキーです。既定値は `{host}:{カレントディレクトリ名}`（§4.1）。 |
| chat_id | Slack のスレッドを表す 1 つの文字列 `"{channel}:{thread_ts}"`。例: `C0AB12CD3:1789560000.000100`。`channel` は Slack のチャンネル ID、`thread_ts` はスレッドの親メッセージの `ts` です。 |
| binding | chat_id とセッションの対応付け。状態ファイルの `bindings` に `chat_id → {session, user, created}` として保存します（§11）。agent が切断しても削除しません。 |
| 許可ユーザー | 状態ファイルの `users` に含まれる Slack user ID。OIDC ログインに成功した管理者（`SCCR_ADMIN_SLACK_USER` と一致する user ID）だけが登録されます（§9）。 |
| agent トークン | agent が `/agent/ws` に接続するときの Bearer トークン。ランダム 32 byte を base64url（パディングなし、43 文字）にしたもの。relay は SHA-256 の hex だけを保存します（§8、§10）。 |
| request_id | Claude Code が権限要求ごとに発行する ID。`a`〜`z` から `l` を除いた小文字 5 文字です（§7）。 |
| pending | relay が保持する、未回答の権限要求の表。`request_id → {chat_id, session, created}`（§7）。 |
| hub | relay 内の、接続中 agent の表。`session name → 送信キュー`。 |

### 1.1 session name の制約

- 1 文字以上 150 文字以下（`char` 数）MUST。上限は Slack の option object の `value` 上限（150 文字）に合わせています。
- 制御文字（`char::is_control` が真の文字）を含まないこと MUST。
- 前後の空白は削りません。申告された文字列をそのままキーとして使います。
- 制約に違反する `Hello` を受けた relay は `Error` を送って切断します（§8）。

---

## 2. protocol メッセージ

agent と relay の間の WebSocket で、text frame 1 つに JSON 1 つを載せます。
型は README §4.1 のとおりです（`#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]`）。

`PROTOCOL_VERSION` は `1` です。

### 2.1 AgentMsg（agent → relay）

`Hello`: 接続後、最初に 1 回だけ送ります。`host` と `cwd` は表示・ログ用の情報で、relay はルーティングに使いません（hub にも保存しません）。

```json
{"type":"hello","version":1,"name":"macbook:proj","host":"macbook","cwd":"/Users/me/src/proj"}
```

`Reply`: Claude が `reply` ツールを呼んだときに送ります。

```json
{"type":"reply","chat_id":"C0AB12CD3:1789560000.000100","text":"Done. I updated README.md."}
```

`PermissionRequest`: Claude Code から権限要求の通知を受けたときに送ります。

```json
{"type":"permission_request","request_id":"abcde","tool_name":"Bash","description":"List files in the project root","input_preview":"{\"command\": \"ls -la\"}"}
```

### 2.2 RelayMsg（relay → agent）

`Welcome`: `Hello` を受理し hub に登録した直後に送ります。

```json
{"type":"welcome","name":"macbook:proj"}
```

`Inbound`: Slack のスレッド返信（または `/dev/inject`）を agent に渡します。`user` は Slack user ID です。

```json
{"type":"inbound","chat_id":"C0AB12CD3:1789560000.000100","user":"U0123ABCD","text":"What does main.rs do?"}
```

`PermissionVerdict`: 権限要求への返答です。`behavior` は小文字の `allow` または `deny` です。

```json
{"type":"permission_verdict","request_id":"abcde","behavior":"allow"}
```

```json
{"type":"permission_verdict","request_id":"abcde","behavior":"deny"}
```

`Error`: relay は送信後にソケットを閉じます（§8）。

```json
{"type":"error","message":"unsupported protocol version: 2"}
```

### 2.3 パースできないフレーム

- 未知の `type`、未知のフィールド、必須フィールドの欠落、JSON でない text frame はデシリアライズエラーです。
- relay 側: 最初のフレームでエラーなら §8 のとおり `Error` を送って切断します。2 フレーム目以降でエラーなら、ログに 1 行（本文は出さない）出して無視し、接続を維持します。binary frame も同様に扱います。
- agent 側: ログ（stderr）に 1 行出して無視し、接続を維持します。

### 2.4 version 不一致

- relay は `Hello.version != PROTOCOL_VERSION` のとき、`Error{message: "unsupported protocol version: {version}"}` を送って切断します。
- agent は `Error` を切断として扱い、§4.3 の backoff で再接続を続けます。version 不一致は再接続しても解消しないため、30 秒ごとに再試行し続けることになりますが、これは意図した挙動です（agent は自分からは終了しません。stderr に理由が出るので人が気づけます）。
- `deny_unknown_fields` のため、メッセージ形式を変えるときは両側を同時に更新し、`PROTOCOL_VERSION` を上げます。

---

## 3. agent の MCP 面

agent は Claude Code から stdio で起動される MCP サーバーです。
stdin から 1 行 1 JSON（JSON-RPC 2.0）を読み、stdout に 1 行 1 JSON を書きます。stdout には JSON-RPC 以外を一切書きません。ログは stderr です。

### 3.1 行の分類

1. 行が JSON としてパースできない、またはトップレベルがオブジェクトでない（配列によるバッチも含む）: stderr に 1 行出して無視します。応答は返しません（`-32700` も返しません）。
2. `id` キーがある: リクエストです。`id` は数値でも文字列でも、そのままの値を応答に入れます。
3. `id` キーがない: 通知です。

### 3.2 initialize

リクエスト:

```json
{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"claude-code","version":"2.1.240"}}}
```

応答:

```json
{"jsonrpc":"2.0","id":0,"result":{"protocolVersion":"2025-06-18","serverInfo":{"name":"sccr","version":"0.1.0"},"capabilities":{"experimental":{"claude/channel":{},"claude/channel/permission":{}},"tools":{}},"instructions":"Messages from Slack arrive as <channel source=\"sccr\" chat_id=\"...\" user=\"...\">. Always answer by calling the reply tool with the same chat_id. Keep replies concise; Slack renders plain text and mrkdwn."}}
```

- `protocolVersion` は要求の `params.protocolVersion` をそのまま返します。要求に無い（または文字列でない）場合は `"2025-06-18"` を返します。
- `serverInfo.version` は `sccr-agent` クレートのバージョン（`CARGO_PKG_VERSION`）です。
- `instructions` は README §4.2 の `INSTRUCTIONS` と一字一句同じ文字列です（T3-2 の実機確認で調整する場合は本書と README を同時に更新します）。

### 3.3 notifications/initialized

```json
{"jsonrpc":"2.0","method":"notifications/initialized"}
```

何もしません（応答なし）。

### 3.4 tools/list

リクエスト:

```json
{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}
```

応答（`tools` は `reply` の 1 件だけ）:

```json
{"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":"reply","description":"Send a message to the Slack thread identified by chat_id.","inputSchema":{"type":"object","properties":{"chat_id":{"type":"string","description":"The chat_id attribute from the channel tag"},"text":{"type":"string","description":"The message to post"}},"required":["chat_id","text"]}}]}}
```

### 3.5 tools/call（reply）

リクエスト:

```json
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"reply","arguments":{"chat_id":"C0AB12CD3:1789560000.000100","text":"Hello from Claude"}}}
```

応答:

```json
{"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"sent"}]}}
```

同時に `AgentMsg::Reply{chat_id, text}` を relay へ送ります（T2-1 の `Outcome::Both`）。
WebSocket が未接続でも応答は `"sent"` です。この場合 `Reply` は agent 内で捨てられます（§4.4）。

エラー:

- `params.name` が `reply` 以外、`params.arguments` がオブジェクトでない、`chat_id` または `text` が無い／文字列でない: `-32602`

```json
{"jsonrpc":"2.0","id":3,"error":{"code":-32602,"message":"Invalid params"}}
```

### 3.6 ping

```json
{"jsonrpc":"2.0","id":4,"method":"ping"}
```

```json
{"jsonrpc":"2.0","id":4,"result":{}}
```

### 3.7 未知のメソッド

`resources/list` や `prompts/list` を含め、§3.2〜§3.6 以外のリクエストはすべて `-32601` です。

```json
{"jsonrpc":"2.0","id":5,"method":"resources/list","params":{}}
```

```json
{"jsonrpc":"2.0","id":5,"error":{"code":-32601,"message":"Method not found"}}
```

未知の通知（例 `notifications/cancelled`）は無視します。

### 3.8 通知 1: notifications/claude/channel（agent → Claude）

`RelayMsg::Inbound` を受けたら stdout に次の 1 行を書きます。`id` キーは付けません。
`meta` のキーは `chat_id` と `user` の 2 つだけです（Claude Code は英数字とアンダースコア以外を含むキーを捨てるため、この 2 つは安全です）。

```json
{"jsonrpc":"2.0","method":"notifications/claude/channel","params":{"content":"What does main.rs do?","meta":{"chat_id":"C0AB12CD3:1789560000.000100","user":"U0123ABCD"}}}
```

Claude のコンテキストには次のように届きます（参考。`source` は Claude Code が MCP サーバー名から付けます）。

```text
<channel source="sccr" chat_id="C0AB12CD3:1789560000.000100" user="U0123ABCD">
What does main.rs do?
</channel>
```

### 3.9 通知 2: notifications/claude/channel/permission_request（Claude → agent）

```json
{"jsonrpc":"2.0","method":"notifications/claude/channel/permission_request","params":{"request_id":"abcde","tool_name":"Bash","description":"List files in the project root","input_preview":"{\"command\": \"ls -la\"}"}}
```

- `params` の 4 フィールド（すべて文字列）を `AgentMsg::PermissionRequest` にして relay へ送ります。応答は返しません。
- いずれかが欠けている、または文字列でない場合は stderr に 1 行出して無視します。
- `description` と `input_preview` は Claude Code 側でサニタイズ済み（空白の畳み込み、長さ 3,500 code point で省略記号付き切り詰め、認証情報の `[REDACTED]` 置換など）ですが、信頼できない入力として扱います。

### 3.10 通知 3: notifications/claude/channel/permission（agent → Claude）

`RelayMsg::PermissionVerdict` を受けたら stdout に次の 1 行を書きます。

```json
{"jsonrpc":"2.0","method":"notifications/claude/channel/permission","params":{"request_id":"abcde","behavior":"allow"}}
```

### 3.11 エラーコード一覧

| code | message | 条件 |
|---|---|---|
| `-32601` | `Method not found` | 未知のメソッドのリクエスト |
| `-32602` | `Invalid params` | `tools/call` の名前・引数が不正 |

不正な JSON 行には応答しません（§3.1）。

---

## 4. agent の WebSocket 挙動

### 4.1 起動と識別情報

- `SCCR_RELAY_URL` と `SCCR_TOKEN` の両方が空でなく設定されているときだけ WebSocket クライアントを起動します。どちらかが無ければ stdio だけで動き、stderr にその旨を 1 行出します。
- `host`: OS のホスト名。取得できなければ `unknown`。
- `cwd`: カレントディレクトリの絶対パス文字列。
- `name`: `SCCR_SESSION_NAME` が空でなければその値。無ければ `{host}:{cwd の最後の要素}`。最後の要素が無い（`/`）場合は `{host}:/`。
  例: `host="mac"`、`cwd="/a/proj"` → `mac:proj`。

### 4.2 接続

1. `SCCR_RELAY_URL`（例 `wss://sccr.example.jp/agent/ws`）へ WebSocket 接続します。リクエストヘッダに `Authorization: Bearer {SCCR_TOKEN}` を付けます。
2. ハンドシェイク成功後、最初のフレームとして `Hello{version: 1, name, host, cwd}` を送ります。
3. 以後は送受信ループです。
   - relay から `Welcome` を受けたら stderr に `connected as {name}` を出し、backoff を初期値に戻します。
   - `Inbound` → §3.8 の通知を stdout 側へ渡します。
   - `PermissionVerdict` → §3.10 の通知を stdout 側へ渡します。
   - `Error{message}` → stderr に `relay error: {message}` を出し、切断として扱います（§4.3）。
- agent は `Welcome` を待たずに MCP 側からの `Reply` / `PermissionRequest` を送ってかまいません。relay はフレームを順に処理し、`Hello` の処理（登録）が先に終わるためです。
- WebSocket の Ping には tungstenite の既定動作で Pong を返します。

### 4.3 再接続 backoff

- 接続失敗（HTTP 401 などハンドシェイク失敗を含む）、切断、`Error` 受信のいずれでも、待ってから再接続します。
- 待ち時間は初期値 1 秒から始め、失敗のたびに 2 倍、上限 30 秒: `1, 2, 4, 8, 16, 30, 30, ...` 秒。
- `Welcome` を受けたら次回の待ち時間を初期値に戻します。
- 初回の接続は待たずに行います。
- テストでは `Backoff{initial, max}` に小さい値（例 50ms）を与えます。
- agent は自分からは終了しません（stdin の EOF でプロセスごと終了します）。

### 4.4 未接続時の送信

- 未接続（接続試行中、backoff 待機中）に MCP 側から来た `AgentMsg` は捨てます。stderr に `dropped {type} while offline` を 1 行出します（`{type}` は `reply` など。本文は出しません）。
- 捨てる処理で MCP 側の送信がブロックしてはいけません MUST。

---

## 5. relay の Slack 受信

### 5.1 署名検証

`POST /slack/events`、`POST /slack/commands`、`POST /slack/interactions` のすべてのリクエストで検証します。

- ヘッダ: `X-Slack-Request-Timestamp`（Unix 秒の 10 進文字列）、`X-Slack-Signature`（`v0=` + 64 桁の小文字 hex）。ヘッダ名は大文字小文字を区別しません。
- 基底文字列: `v0:{timestamp}:{raw body}`。body はデコード前のバイト列そのものです。
- 期待値: `"v0=" + hex(HMAC-SHA256(key = SLACK_SIGNING_SECRET, message = 基底文字列))`
- 比較は定数時間（`subtle::ConstantTimeEq`）で行います。
- 判定順と結果（T4-1 の `VerifyError`）:
  1. timestamp が `i64` の 10 進として解釈できない → `BadTimestamp`
  2. `|now - timestamp| > 300` → `Stale`（ちょうど 300 秒差は通す）
  3. signature が `v0=` で始まらない、hex として不正、または一致しない → `BadSignature`
- ルート側の扱い: ヘッダ欠落、検証失敗、`SLACK_SIGNING_SECRET` が空文字（開発モードで未設定の場合）はすべて `401`、body は空です。署名検証を省略する経路は設けません。

テストベクタ（`sign` の期待値）:

| 項目 | 値 |
|---|---|
| secret | `test-signing-secret` |
| timestamp | `1789560000` |
| body | `token=x&team_id=T0001&user_id=U0123&command=%2Fcc&text=list` |
| signature | `v0=a19c040b7bcee240257b5dc0fc2c1601cce758204a931ec68a6e95b13446607a` |

### 5.2 Events API envelope と判定規則

#### url_verification

```json
{"token":"Jhj5dZrVaK7ZwHHjRyZWjbDl","challenge":"3eZbrw1aBm2rZgRNFdxV2595E9CY3gmdALWMmHkvFXO7tYXAYM8P","type":"url_verification"}
```

署名検証の後、`200`、`Content-Type: text/plain`、body に `challenge` の値そのものを返します。

#### event_callback: スレッド返信（候補になる）

```json
{
  "token": "XXYYZZ",
  "team_id": "T0001ABCD",
  "api_app_id": "A0001ABCD",
  "event": {
    "type": "message",
    "channel": "C0AB12CD3",
    "user": "U0123ABCD",
    "text": "What does main.rs do?",
    "ts": "1789560100.000200",
    "thread_ts": "1789560000.000100",
    "parent_user_id": "U0BOT0001",
    "event_ts": "1789560100.000200",
    "channel_type": "channel"
  },
  "type": "event_callback",
  "event_id": "Ev0001ABCD",
  "event_time": 1789560100,
  "authorizations": [
    {"enterprise_id": null, "team_id": "T0001ABCD", "user_id": "U0BOT0001", "is_bot": true, "is_enterprise_install": false}
  ],
  "is_ext_shared_channel": false,
  "event_context": "4-eyJldCI6Im1lc3NhZ2UiLCJ0aWQiOiJUMDAwMUFCQ0QiLCJhaWQiOiJBMDAwMUFCQ0QiLCJjaWQiOiJDMEFCMTJDRDMifQ"
}
```

候補: `chat_id = "C0AB12CD3:1789560000.000100"`、`channel = "C0AB12CD3"`、`thread_ts = "1789560000.000100"`、`user = "U0123ABCD"`、`text = "What does main.rs do?"`。

プライベートチャンネル（`message.groups`）では `channel_type` が `"group"` になります。判定は公開チャンネルと同じです。

#### event_callback: チャンネル直下の投稿（候補にならない）

```json
{"token":"XXYYZZ","team_id":"T0001ABCD","api_app_id":"A0001ABCD","event":{"type":"message","channel":"C0AB12CD3","user":"U0123ABCD","text":"hello channel","ts":"1789560200.000300","event_ts":"1789560200.000300","channel_type":"channel"},"type":"event_callback","event_id":"Ev0002ABCD","event_time":1789560200,"authorizations":[{"enterprise_id":null,"team_id":"T0001ABCD","user_id":"U0BOT0001","is_bot":true,"is_enterprise_install":false}],"is_ext_shared_channel":false}
```

`thread_ts` が無いため `Message{event_id: "Ev0002ABCD", candidate: None}` です。

スレッドの親メッセージは `thread_ts == ts` になっており、これも候補になりません。

#### event_callback: bot の投稿（候補にならない）

relay 自身が `chat.postMessage` で投稿したメッセージ（root 投稿や Reply）も Events API で届きます。
現行の Slack アプリの投稿は `subtype` を持たず、`bot_id` と `app_id` を持ちます。

```json
{"token":"XXYYZZ","team_id":"T0001ABCD","api_app_id":"A0001ABCD","event":{"type":"message","channel":"C0AB12CD3","user":"U0BOT0001","bot_id":"B0001ABCD","app_id":"A0001ABCD","text":"Done. I updated README.md.","ts":"1789560300.000400","thread_ts":"1789560000.000100","parent_user_id":"U0BOT0001","event_ts":"1789560300.000400","channel_type":"channel"},"type":"event_callback","event_id":"Ev0003ABCD","event_time":1789560300,"authorizations":[{"enterprise_id":null,"team_id":"T0001ABCD","user_id":"U0BOT0001","is_bot":true,"is_enterprise_install":false}],"is_ext_shared_channel":false}
```

旧形式の統合 bot の投稿は `subtype: "bot_message"` と `bot_id` を持ちます。

```json
{"token":"XXYYZZ","team_id":"T0001ABCD","api_app_id":"A0001ABCD","event":{"type":"message","subtype":"bot_message","channel":"C0AB12CD3","bot_id":"B0002ABCD","username":"github","text":"Pushing is the answer","ts":"1789560400.000500","thread_ts":"1789560000.000100","channel_type":"channel"},"type":"event_callback","event_id":"Ev0004ABCD","event_time":1789560400,"authorizations":[{"enterprise_id":null,"team_id":"T0001ABCD","user_id":"U0BOT0001","is_bot":true,"is_enterprise_install":false}],"is_ext_shared_channel":false}
```

どちらも `bot_id` があるため候補になりません。判定を `subtype` ではなく `bot_id` の有無で行うのは、現行形式が `subtype` を持たないためです。

#### event_callback: message_changed（候補にならない）

```json
{"token":"XXYYZZ","team_id":"T0001ABCD","api_app_id":"A0001ABCD","event":{"type":"message","subtype":"message_changed","hidden":true,"channel":"C0AB12CD3","ts":"1789560500.000600","event_ts":"1789560500.000600","message":{"type":"message","user":"U0123ABCD","text":"What does lib.rs do?","ts":"1789560100.000200","thread_ts":"1789560000.000100","edited":{"user":"U0123ABCD","ts":"1789560500.000000"}},"previous_message":{"type":"message","user":"U0123ABCD","text":"What does main.rs do?","ts":"1789560100.000200","thread_ts":"1789560000.000100"},"channel_type":"channel"},"type":"event_callback","event_id":"Ev0005ABCD","event_time":1789560500,"authorizations":[{"enterprise_id":null,"team_id":"T0001ABCD","user_id":"U0BOT0001","is_bot":true,"is_enterprise_install":false}],"is_ext_shared_channel":false}
```

`subtype` があるため候補になりません（編集は Claude に再送しません）。`channel_join`、`message_deleted`、`thread_broadcast`、`file_share` なども同じく候補になりません。

#### 分類規則（T4-2 `classify`）

1. `body.type == "url_verification"` かつ `challenge` が文字列 → `UrlVerification{challenge}`
2. `body.type == "event_callback"` かつ `body.event.type == "message"` かつ `body.event_id` が文字列 → `Message{event_id, candidate}`
3. それ以外 → `Other`（relay は `200` を返して何もしません）

`candidate` は、次をすべて満たすときだけ `Some` です。

- `event.subtype` が無い
- `event.bot_id` が無い
- `event.thread_ts` が文字列
- `event.ts` が文字列で、`thread_ts != ts`
- `event.channel` と `event.user` が文字列

`text` は `event.text`（無ければ空文字）です。Slack の mrkdwn エスケープ（`&amp;` `&lt;` `&gt;`、`<@U123>` など）はそのまま Claude に渡します。

### 5.3 event_id による重複排除

- Slack は 3 秒以内に 2xx を返さないと再送します（`X-Slack-Retry-Num` ヘッダ付き）。relay は再送ヘッダを特別扱いせず、`event_id` で重複を除きます。
- `Dedupe` の容量は 1000。FIFO で最も古いものから追い出します。
- `Message` に分類されたすべてのイベントで、候補判定より前に `first_seen(event_id)` を呼びます。偽なら以後の処理をしません。

### 5.4 slash command

`POST /slack/commands`、`Content-Type: application/x-www-form-urlencoded`。body の例（1 行）:

```text
token=gIkuvaNzQIHg97ATvDxqgjtO&team_id=T0001ABCD&team_domain=example&channel_id=C0AB12CD3&channel_name=dev&user_id=U0123ABCD&user_name=me&command=%2Fcc&text=list&api_app_id=A0001ABCD&is_enterprise_install=false&response_url=https%3A%2F%2Fhooks.slack.com%2Fcommands%2FT0001ABCD%2F1234567890%2FabcdEFGH&trigger_id=13345224609.738474920.8088930838d88f008e0
```

relay が使うフィールドは `user_id`、`channel_id`、`text`、`response_url` です。`command` は見ません（manifest で `/cc` だけを登録するため）。
`token`（旧式の verification token）は使いません。form としてパースできない、または `user_id` / `channel_id` が無い場合は `400` です。

`text` の解釈（T4-3 `parse`）: 前後の空白を削り、ASCII 小文字にしてから比較します。

| text | Cmd |
|---|---|
| 空 | `Pick` |
| `list` | `List` |
| `unbind` | `Unbind` |
| それ以外（`help` を含む） | `Help` |

応答は常に `200`、`Content-Type: application/json` の body です。

`Pick` の応答（接続中セッションがあるとき）:

```json
{
  "response_type": "ephemeral",
  "text": "Select a Claude Code session to connect to this channel.",
  "blocks": [
    {
      "type": "section",
      "text": {"type": "mrkdwn", "text": "Select a Claude Code session to connect to this channel."},
      "accessory": {
        "type": "static_select",
        "action_id": "sccr_select",
        "placeholder": {"type": "plain_text", "text": "Choose a session"},
        "options": [
          {"text": {"type": "plain_text", "text": "macbook:proj"}, "value": "macbook:proj"},
          {"text": {"type": "plain_text", "text": "macbook:other"}, "value": "macbook:other"}
        ]
      }
    }
  ]
}
```

- `options` は hub の名前一覧（昇順）を先頭から最大 100 件（Slack の上限）入れます。
- option の `text.text` は名前を 75 文字（`char` 数）で切り詰めたもの、`value` は名前そのもの（§1.1 により 150 文字以内）です。

`Pick` の応答（接続中セッションが 0 件のとき）: `blocks` を付けません。

```json
{"response_type":"ephemeral","text":"No sessions connected."}
```

`text_response(text)` の形（`List`、`Unbind`、`Help`、拒否文で使います）:

```json
{"response_type":"ephemeral","text":"Unbound 2 thread(s) in this channel."}
```

各コマンドの処理内容は §6.4 です。

### 5.5 interactions（block_actions）

`POST /slack/interactions`、`Content-Type: application/x-www-form-urlencoded`。body は `payload={URL エンコードされた JSON}` の 1 フィールドです。

デコード後の `payload` の例（ephemeral メッセージ上の `static_select` を選んだとき）:

```json
{
  "type": "block_actions",
  "user": {"id": "U0123ABCD", "username": "me", "name": "me", "team_id": "T0001ABCD"},
  "api_app_id": "A0001ABCD",
  "token": "9s8d9as89d8as9d8as989",
  "container": {"type": "message", "message_ts": "1789560600.000700", "channel_id": "C0AB12CD3", "is_ephemeral": true},
  "trigger_id": "12321423423.333649436676.d8c1bb837935619ccad0f624c448ffb3",
  "team": {"id": "T0001ABCD", "domain": "example"},
  "enterprise": null,
  "is_enterprise_install": false,
  "channel": {"id": "C0AB12CD3", "name": "dev"},
  "state": {"values": {}},
  "response_url": "https://hooks.slack.com/actions/T0001ABCD/1232321423432/D09sSasdasdAS9091209",
  "actions": [
    {
      "type": "static_select",
      "action_id": "sccr_select",
      "block_id": "Xy1z",
      "selected_option": {"text": {"type": "plain_text", "text": "macbook:proj", "emoji": true}, "value": "macbook:proj"},
      "placeholder": {"type": "plain_text", "text": "Choose a session", "emoji": true},
      "action_ts": "1789560610.123456"
    }
  ]
}
```

`selected_session`（T4-3）は次をすべて満たすときだけ `Some(Selection)` を返します。

- `type == "block_actions"`
- `actions[0].action_id == "sccr_select"`
- `actions[0].selected_option.value` が文字列 → `session`
- `channel.id` が文字列 → `channel`
- `user.id` が文字列 → `user`
- `response_url` が文字列 → `response_url`

`payload` フィールドが無い、または JSON として不正な場合は `400` です。それ以外の応答は、処理結果にかかわらず `200`、body は空です。処理内容は §6.1 です。

---

## 6. relay のルーティング規則

### 6.1 セッションの選択（interactions）

処理は HTTP 応答を返す前に同期的に行います（テストで結果を決定的に観測するためです）。

1. 署名検証（§5.1）。失敗 → `401`
2. `payload` をパースし `selected_session`。`None` → `200`
3. `selection.user` が許可ユーザーでない → `200`（何もしない）
4. `post_message(channel, None, "Connected to session *{name}*. Reply in this thread.")`（`{name}` は §13 のエスケープ規則を適用）
5. 成功して `ts` が返ったら、`bindings["{channel}:{ts}"] = {session, user, created: now}` を追加し、状態ファイルを保存する
6. 投稿に失敗したら §6.5
7. `200`、body は空

- 選択時にそのセッションが接続中かどうかは確認しません（束縛は名前に対して行い、後で接続されれば使えます）。
- 状態ファイルの保存に失敗した場合は、メモリ上の binding は残し、エラーをログに出して `200` を返します。

### 6.2 スレッド返信（events）

処理順は次のとおりです MUST。

1. 署名検証（§5.1）。失敗 → `401`
2. body を JSON としてパース。失敗 → `400`
3. `classify`
   - `UrlVerification` → challenge を返して終了
   - `Other` → `200` を返して終了
   - `Message` → `200` を即座に返し、以降を `tokio::spawn` で非同期に処理する
4. `dedupe.first_seen(event_id)` が偽 → 終了
5. `candidate` が `None` → 終了
6. `candidate.user` が許可ユーザーでない → 終了（Slack には何も投稿しない）
7. `bindings[chat_id]` が無い → 終了
8. `sweep(pending, now)`（§7.4）
9. `parse_verdict(text)` が `Some((request_id, behavior))` で、かつ次をすべて満たす場合は権限返答として扱う
   - `pending[request_id]` が存在する
   - `pending[request_id].chat_id == chat_id`
   - `pending[request_id].session == binding.session`
   - `candidate.user == binding.user`

   このとき `pending` から `request_id` を削除し、`hub.send(binding.session, PermissionVerdict{request_id, behavior})` を行い、終了する（`Inbound` は送らない）。送信先がオフラインなら 10 と同じオフライン通知を投稿する。
10. それ以外（`parse_verdict` が `None`、または上の条件のどれかを満たさない）は通常の返信として `hub.send(binding.session, Inbound{chat_id, user, text})`。
    オフライン（`Offline`）なら `post_message(channel, Some(thread_ts), "Session *{name}* is offline. Your message was not delivered.")` を投稿する。

補足:

- 許可ユーザーだが binding の作成者ではない人が `yes abcde` と書いた場合は、権限返答にならず、通常の `Inbound` として Claude に届きます（README §4.6 の「束縛したユーザーからのものだけ有効」を満たします）。
- 許可ユーザーでない人の投稿は、権限返答の形であっても 6 で捨てます。
- ログには event_id、chat_id、判定結果だけを出し、本文は出しません。

### 6.3 agent の Reply

1. `chat_id` を最初の `:` で `channel` と `thread_ts` に分けます。`:` が無い、またはどちらかが空なら、ログを出して捨てます。
2. `text` が空文字なら何も投稿しません（Slack は空の `text` を `no_text` で拒否するためです）。
3. `chunk(text, 3900)`（§6.3.1）で分割し、先頭から順に `post_message(channel, Some(thread_ts), chunk)` を呼びます。失敗したらログ（Slack のエラーコードのみ）を出し、残りの投稿をやめます。

- relay は Reply の `chat_id` が送信元セッションに束縛されているかを確認しません。agent はトークンで認証済みであり、`/dev/inject` による束縛なしの検証（T3-2）を可能にするためです。
- 3900 は、`chat.postMessage` の `text` 推奨上限 4,000 文字に余裕を持たせた値です。

`chat.postMessage` の呼び出し（`HttpSlack`）:

- `POST https://slack.com/api/chat.postMessage`
- ヘッダ `Authorization: Bearer {SLACK_BOT_TOKEN}`、`Content-Type: application/json; charset=utf-8`
- body（スレッドへの投稿）:

```json
{"channel":"C0AB12CD3","thread_ts":"1789560000.000100","text":"Done. I updated README.md."}
```

- root 投稿では `thread_ts` を付けません。
- 成功応答の例（`ts` を `post_message` の戻り値にします）:

```json
{"ok":true,"channel":"C0AB12CD3","ts":"1789560000.000100","message":{"type":"message","text":"Connected to session *macbook:proj*. Reply in this thread.","bot_id":"B0001ABCD","app_id":"A0001ABCD","ts":"1789560000.000100"}}
```

- 失敗応答の例（HTTP ステータスは 200 のまま `ok: false`。`SlackError` に `error` を入れます）:

```json
{"ok":false,"error":"not_in_channel"}
```

#### 6.3.1 分割規則（T6-1 `chunk`）

- 長さは `char` 数で数えます。
- 残りが `max_chars` 以内なら、それを最後の 1 個にします。
- 超える場合は、残りの先頭 `max_chars` 文字の中で最後の `\n` を探し、その `\n` を含めた直後で切ります。`\n` が無ければ `max_chars` 文字ちょうどで切ります。
- 分割結果をすべて連結すると元の文字列と一致します MUST（改行は前の塊の末尾に残ります）。
- 空文字は `vec![""]` を返します（呼び出し側が 6.3 の 2 で投稿を避けます）。

例: `chunk("ab\ncd\nef", 6)` → `["ab\ncd\n", "ef"]`。`chunk("abcdefgh", 3)` → `["abc", "def", "gh"]`。

### 6.4 `/cc` コマンド

共通処理:

1. 署名検証（§5.1）。失敗 → `401`
2. form をパース。失敗 → `400`
3. `user_id` が許可ユーザーでない → `200`、`text_response("You are not authorized to use /cc.")`。コマンドは実行しません。
4. `parse(text)` に応じて以下

`Pick`（`/cc`）: `pick_response(hub.list())`（§5.4）。

`List`（`/cc list`）: 接続中セッションの一覧を返します。

- 0 件: `No sessions connected.`
- 1 件以上: `Connected sessions:` の後に、昇順で 1 件ずつ `\n- {name}` を続けた文字列。例: `Connected sessions:\n- macbook:other\n- macbook:proj`

`Unbind`（`/cc unbind`）: slash command の payload にはスレッドの情報（`thread_ts`）が含まれないため、特定のスレッドだけを外すことはできません。そこで、キーが `"{channel_id}:"` で始まり、かつ `user == user_id` の binding をすべて削除し、状態ファイルを保存します。削除件数 `{count}`（0 を含む）を使って `Unbound {count} thread(s) in this channel.` を返します。

`Help`（`/cc help` やその他の文字列）: §13 の help 文を返します。

### 6.5 bot 未招待で投稿に失敗したとき

§6.1 の root 投稿で `post_message` が失敗したら（`not_in_channel`、`channel_not_found` を含むすべての失敗）、binding は作らず、`respond(response_url, "Invite the bot to this channel first.")` を呼びます。

`respond`（`HttpSlack`）は `response_url` へ `Content-Type: application/json` で次を POST します。

```json
{"response_type":"ephemeral","replace_original":false,"text":"Invite the bot to this channel first."}
```

`respond` 自体の失敗はログに出すだけです。

`FakeSlack` は、`post_message` を指定した `SlackError` で失敗させる設定を持ちます（`interaction_post_failure_responds_invite_hint` 用）。

---

## 7. 権限中継

### 7.1 流れ

1. Claude Code → agent: `notifications/claude/channel/permission_request`（§3.9）
2. agent → relay: `PermissionRequest`
3. relay → Slack: 束縛スレッドにプロンプトを投稿
4. 束縛したユーザーがスレッドに `yes abcde` / `no abcde` と返信
5. relay → agent: `PermissionVerdict`（§6.2 の 9）
6. agent → Claude Code: `notifications/claude/channel/permission`（§3.10）

ローカル端末のダイアログも並行して開いたままで、先に届いた回答が適用されます。

### 7.2 PermissionRequest を受けたとき（relay）

1. `sweep(pending, now)`
2. `bindings` から `session == 送信元の session name` の binding を探します。
   - 複数あれば `created` が最大のもの。`created` が等しいものが複数あれば、キー（chat_id）の辞書順で最大のもの。
   - 無ければ、ログに `permission request {request_id} from {session} dropped: no binding` を 1 行出して捨てます（pending にも登録しません）。
3. `pending[request_id] = {chat_id, session, created: now}`（同じ `request_id` があれば上書き）
4. `chunk(prompt_text(...), 3900)` を順にスレッドへ投稿します。投稿に失敗したら `pending` から `request_id` を削除し、ログを出します。

### 7.3 プロンプト文（`prompt_text`）

````text
Claude wants to run *{tool_name}*: {description}
```{input_preview}```
Reply "yes {request_id}" or "no {request_id}" in this thread.
````

- 3 行を `\n` で連結した文字列です。最終行の後に改行は付けません。
- `{tool_name}`、`{description}`、`{input_preview}` には §13 のエスケープ規則を適用します。`{request_id}` はそのまま入れます。
- `input_preview` が空文字でも 2 行目は省略しません（2 行目はバッククォート 6 個だけになります）。

例（JSON 文字列として）:

```json
"Claude wants to run *Bash*: List files in the project root\n```{\"command\": \"ls -la\"}```\nReply \"yes abcde\" or \"no abcde\" in this thread."
```

### 7.4 pending 表と失効

- 型: `Pending { chat_id: String, session: String, created: i64 }`、キーは `request_id`。
- メモリのみ（relay 再起動で消えます）。
- `sweep(pending, now)`: `now - created > 1800` のものを削除します。ちょうど 1800 秒のものは残します。
- `sweep` はスレッド返信の処理時（§6.2 の 8）と PermissionRequest の受信時（§7.2 の 1）に呼びます。
- 失効した `request_id` への `yes xxxxx` は、pending に無いため通常の `Inbound` になります。

### 7.5 parse_verdict の規則

参照正規表現（大文字小文字無視）: `^\s*(y|yes|n|no)\s+([a-km-z]{5})\s*$`

regex クレートは使わず、次の手順で手書きします。

1. `text.trim()`（`char::is_whitespace` に当たる文字を前後から除く）
2. 最初の空白文字の位置で、単語 1 と残りに分けます。空白が無ければ `None`。
3. 残りの先頭の空白をすべて除いたものを単語 2 とします。
4. 単語 2 に空白文字が含まれていれば `None`（語が 3 つ以上）。
5. 単語 1 を ASCII 小文字にして `y` または `yes` → `Allow`、`n` または `no` → `Deny`、それ以外 → `None`。
6. 単語 2 を ASCII 小文字にしたものが、ちょうど 5 文字で、各文字が `a`〜`k` または `m`〜`z` でなければ `None`（`l` と `L` は不可）。
7. `Some((小文字にした単語 2, behavior))`

| 入力 | 結果 |
|---|---|
| `yes abcde` | `Some(("abcde", Allow))` |
| `y abcde` | `Some(("abcde", Allow))` |
| `no abcde` | `Some(("abcde", Deny))` |
| `  YES ABCDE  ` | `Some(("abcde", Allow))` |
| `yes abcle` | `None` |
| `yes abcd` | `None` |
| `yes abcde please` | `None` |
| `yeah abcde` | `None` |
| `hello` | `None` |

---

## 8. agent WebSocket 認証（relay 側 `GET /agent/ws`）

1. `Authorization` ヘッダが `Bearer ` で始まらない、または無い → `401`（upgrade しない、body は空）
2. `Bearer ` の後ろをトークンとし、`store.token_ok(token)` が偽、かつ（開発モードでない、またはトークンが `dev` でない）→ `401`
   - `token_ok`: `hex(SHA-256(token))`（小文字 64 桁）が `tokens[].sha256` のいずれかと一致すれば真
3. WebSocket に upgrade
4. 最初のフレームを 10 秒待ちます。来なければ `Error{"hello timeout"}` を送って切断
5. 最初のフレームが text でない、`AgentMsg` としてパースできない、または `Hello` 以外 → `Error{"first frame must be hello"}` を送って切断
6. `version != 1` → `Error{"unsupported protocol version: {version}"}` を送って切断
7. `name` が §1.1 に違反 → `Error{"invalid session name"}` を送って切断
8. `hub.register(name, tx)` が `Duplicate` → `Error{"session name already connected: {name}"}` を送って切断（既存の接続はそのまま）
9. `Welcome{name}` を送り、ログに `name`、`host`、`cwd` を 1 行出す
10. ループ
    - agent から `Reply` → §6.3
    - agent から `PermissionRequest` → §7.2
    - agent から 2 回目以降の `Hello` → 無視
    - hub 経由の `RelayMsg` → そのままフレームとして送信
11. ソケットが閉じたら `hub.unregister(name)`

- `Error` の `message` 文字列は §13.2 のとおりです。
- 開発モードの `dev` トークンは状態ファイルのトークンに加えて受理されるもので、他のトークンを無効にはしません。

---

## 9. OIDC（Sign in with Slack）

### 9.1 authorize URL（`GET /login`）

1. `sessions.begin(now)` で `state` と `nonce`（どちらも `random_token()`）を作り、pending state として 600 秒保持します。
2. `302`、`Location` に次の URL を返します。

```text
https://slack.com/openid/connect/authorize?response_type=code&scope=openid%20profile&client_id={client_id}&state={state}&nonce={nonce}&redirect_uri={redirect_uri}
```

- パラメータの順序は上のとおりです。
- 各値はパーセントエンコードします。RFC 3986 の非予約文字（`A-Z a-z 0-9 - . _ ~`）以外を `%XX`（大文字 hex）にします。空白は `%20` です。
- `redirect_uri` は `{SCCR_PUBLIC_URL}/auth/callback` です（例 `https://sccr.example.jp/auth/callback` → `https%3A%2F%2Fsccr.example.jp%2Fauth%2Fcallback`）。
- `team` パラメータは付けません。

### 9.2 callback（`GET /auth/callback?code=...&state=...`）

1. `state` または `code` が無い → `401`（Slack が `error=access_denied` などで戻した場合もこれに当たります）
2. `sessions.take_pending(state, now)` が `None`（未知、使用済み、失効）→ `401`
3. `oidc.token(code, client_id, client_secret, redirect_uri)` が失敗 → `401`
4. `parse_id_token(id_token, client_id, nonce, now)` が失敗 → `401`
5. `claims.sub != SCCR_ADMIN_SLACK_USER` → `403`（users には追加しない）
6. `users` に `sub` が無ければ追加し、状態ファイルを保存します（保存失敗は `500`）
7. `sid = sessions.create_cookie(sub, now)`（86400 秒）
8. `302`、`Location: /`、`Set-Cookie: sccr_sid={sid}; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=86400`

### 9.3 token 交換（`HttpOidc`）

- `POST https://slack.com/api/openid.connect.token`
- `Content-Type: application/x-www-form-urlencoded`
- form: `code`、`client_id`、`client_secret`、`redirect_uri`（§9.1 と同じ値）

成功応答の例:

```json
{"ok":true,"access_token":"xoxp-1234","token_type":"Bearer","id_token":"eyJhbGciOiJSUzI1NiIsImtpZCI6IjEiLCJ0eXAiOiJKV1QifQ.eyJpc3MiOiJodHRwczovL3NsYWNrLmNvbSJ9.c2ln"}
```

失敗応答の例（`OidcError::Http(error)` にします）:

```json
{"ok":false,"error":"invalid_code"}
```

- `ok` が `true` で `id_token` が文字列のときだけ成功です。
- `access_token` は使わず、保存もしません。
- HTTP 通信の失敗、JSON として不正な応答も `OidcError::Http` です。

### 9.4 id_token の claims と検証

id_token（JWT）の payload をデコードした例:

```json
{
  "iss": "https://slack.com",
  "sub": "U0123ABCD",
  "aud": "25259531569.1115258246291",
  "exp": 1789560900,
  "iat": 1789560600,
  "auth_time": 1789560600,
  "nonce": "n0nce-value-from-login",
  "at_hash": "abc123",
  "https://slack.com/team_id": "T0001ABCD",
  "https://slack.com/user_id": "U0123ABCD",
  "name": "Me",
  "picture": "https://secure.gravatar.com/avatar/example.jpg",
  "given_name": "Me",
  "family_name": "",
  "locale": "en-US",
  "https://slack.com/team_name": "Example",
  "https://slack.com/team_domain": "example"
}
```

`parse_id_token(jwt, client_id, nonce, now)` の手順（エラーは T5-1 の `OidcError`）:

1. `.` で分割してちょうど 3 部分でなければ `Malformed`
2. 2 番目の部分を base64url（パディングなし。末尾の `=` があれば除いてから）でデコードし、JSON オブジェクトとしてパース。失敗なら `Malformed`
3. `iss` が文字列 `https://slack.com` と一致しない → `Issuer`
4. `aud` が文字列で `client_id` と一致しない → `Audience`
5. `nonce` が文字列で引数の `nonce` と一致しない → `Nonce`
6. `exp` が整数で `exp > now` でない → `Expired`
7. `sub` が文字列でなければ `Malformed`
8. `IdClaims{sub, name}`（`name` は文字列なら `Some`）

- JWT の署名は検証しません。id_token は client_secret を使った token 交換で Slack から TLS で直接受け取るためです（README §4.6）。
- `sub` は Slack の user ID で、`https://slack.com/user_id` と同じ値です。relay は `sub` を使います。

### 9.5 Cookie セッションと pending state

- どちらもメモリのみです（relay 再起動でログアウトされます）。
- pending state: `state → (nonce, expires = now + 600)`。`take_pending` は 1 回だけ成功し、取り出したものは削除します。`now < expires` のときだけ有効です。
- Cookie: 名前 `sccr_sid`、値は `random_token()`。`sid → (user, expires = now + 86400)`。`user_for` は `now < expires` のときだけ `Some` です。
- `random_token()`: `rand` によるランダム 32 byte を base64url（パディングなし）にした 43 文字。

---

## 10. 管理画面

HTML の値はすべてエスケープします: `&` → `&amp;`、`<` → `&lt;`、`>` → `&gt;`、`"` → `&quot;`、`'` → `&#39;`。

### 10.1 `GET /`

- Cookie `sccr_sid` が無い、または無効 → `302`、`Location: /login`
- 有効なら `200`、`Content-Type: text/html; charset=utf-8` で次を表示します。
  1. ログイン中の Slack user ID
  2. 接続中セッションの一覧（`hub.list()`、昇順）。0 件なら「なし」と分かる表示
  3. トークン発行フォーム: `<form method="post" action="/tokens">`、`<input name="label">`、送信ボタン
  4. 発行済みトークンの一覧: `label` と `created`（`sha256` は表示しません）

`Hello` の `host` と `cwd` は hub に保存しないため表示しません。

### 10.2 `POST /tokens`

- form（`application/x-www-form-urlencoded`）: `label`
- Cookie が無効 → `302`、`Location: /login`
- `label` を前後の空白を削ったうえで 1〜64 文字（`char` 数）でなければ `400`
- `token = random_token()`、`tokens` に `{sha256: hex(SHA-256(token)), label, created: 現在時刻の RFC 3339 UTC 秒精度（例 2026-09-16T00:00:00Z）}` を追加し、状態ファイルを保存（失敗は `500`、トークンは表示しない）
- `200`、HTML で平文トークンを表示します。この応答だけが平文を含みます。ヘッダ `Cache-Control: no-store` を付けます。
- CSRF 対策は Cookie の `SameSite=Lax` に依存します（README §4.6）。
- トークンの失効（削除）UI はありません。削除は状態ファイルを直接編集し relay を再起動して行います。

---

## 11. 状態ファイル

パスは `SCCR_STATE_FILE`（既定 `./sccr-state.json`）。

```json
{
  "users": ["U0123ABCD"],
  "tokens": [
    {"sha256": "ef260e9aa3c673af240d17a2660480361a8e081d1ffeca2a5ed0e3219fc18567", "label": "macbook", "created": "2026-09-16T00:00:00Z"}
  ],
  "bindings": {
    "C0AB12CD3:1789560000.000100": {"session": "macbook:proj", "user": "U0123ABCD", "created": 1789560000}
  }
}
```

| フィールド | 型 | 説明 |
|---|---|---|
| `users` | 文字列配列 | 許可ユーザー。重複なし |
| `tokens[].sha256` | 文字列 | トークン平文の SHA-256、小文字 hex 64 桁 |
| `tokens[].label` | 文字列 | 発行時のラベル |
| `tokens[].created` | 文字列 | RFC 3339 UTC |
| `bindings` | オブジェクト | キーは chat_id |
| `bindings.*.session` | 文字列 | session name |
| `bindings.*.user` | 文字列 | 束縛したユーザーの Slack user ID |
| `bindings.*.created` | 整数 | 束縛した時刻（Unix 秒）。省略時は `0`（`#[serde(default)]`） |

- README §4.5 の例（`created` なし）もそのまま読めます MUST。
- ファイルが存在しない → 空の `Store`（`Default`）。
- JSON として不正 → `load` はエラーを返し、relay は起動しません。
- 未知のフィールドは無視します（`deny_unknown_fields` は付けません）。
- 書き込み: 同じディレクトリの `{path}.tmp` に全体を書き、`rename` で置き換えます。成功後に `.tmp` は残りません。Unix では `.tmp` をパーミッション `0600` で作成します。
- 保存のタイミング: binding の追加（§6.1）、unbind（§6.4）、ユーザー追加（§9.2）、トークン追加（§10.2）。
- 上の例のトークンは平文 `dev` の SHA-256 です（テスト用の値。本番では使いません）。

---

## 12. 開発モード（`SCCR_DEV=1`）

`SCCR_DEV` が文字列 `1` のときだけ有効です。本番環境では設定しないでください。

変わる点:

1. `SLACK_SIGNING_SECRET`、`SLACK_BOT_TOKEN`、`SLACK_CLIENT_ID`、`SLACK_CLIENT_SECRET`、`SCCR_ADMIN_SLACK_USER` が未設定でも起動できます（空文字として扱います）。署名秘密が空の間、Slack の 3 エンドポイントは常に `401` です（§5.1）。
2. `/agent/ws` でトークン `dev` を受理します（§8）。
3. Slack クライアントに `HttpSlack` ではなく `LogSlack` を使います。
4. `POST /dev/inject` を有効にします。開発モードでなければこのルートは登録されず `404` です。

### 12.1 `POST /dev/inject`

body（`Content-Type: application/json`）:

```json
{"session":"smoke","chat_id":"C1:1.1","user":"U1","text":"say hi"}
```

- 4 フィールドはすべて文字列で必須です。JSON として不正、またはフィールドが欠けている → `400`、body `bad request`
- 署名検証、許可ユーザー判定、binding の確認、権限返答の判定は行いません。`hub.send(session, Inbound{chat_id, user, text})` をそのまま行います。
- 成功 → `200`、body `ok`
- セッションが接続されていない → `409`、body `offline`

### 12.2 `LogSlack`

stdout に 1 呼び出し 1 行を出します。`{text:?}` は Rust の `Debug` 形式（改行が `\n` になり 1 行に収まります）です。

| 呼び出し | 出力 |
|---|---|
| `post_message(channel, Some(thread_ts), text)` | `[slack] post chat_id={channel}:{thread_ts} ts={ts} text={text:?}` |
| `post_message(channel, None, text)` | `[slack] post channel={channel} ts={ts} text={text:?}` |
| `respond(response_url, text)` | `[slack] respond text={text:?}` |

- 戻り値の `ts` は `"{now}.{counter:06}"`（`counter` は 1 から始まり呼び出しごとに増える）です。
- 開発モードに限り、本文を stdout に出すことを許可します（README §4.6 の例外）。

---

## 13. Slack 表示文言一覧

テストはここの文字列で一致を確認します。文字列を変えるときは本書を先に更新します。

### 13.1 Slack に出る文言

次の JSON は `キー → 文字列` の一覧です（キー名は本書内の参照用で、実装の識別子を縛りません）。`{...}` は差し込み位置です。

```json
{
  "picker_prompt": "Select a Claude Code session to connect to this channel.",
  "picker_placeholder": "Choose a session",
  "no_sessions": "No sessions connected.",
  "connected_root": "Connected to session *{name}*. Reply in this thread.",
  "session_offline": "Session *{name}* is offline. Your message was not delivered.",
  "unauthorized_command": "You are not authorized to use /cc.",
  "invite_bot": "Invite the bot to this channel first.",
  "unbind_done": "Unbound {count} thread(s) in this channel.",
  "list_header": "Connected sessions:",
  "list_item": "\n- {name}",
  "permission_prompt": "Claude wants to run *{tool_name}*: {description}\n```{input_preview}```\nReply \"yes {request_id}\" or \"no {request_id}\" in this thread.",
  "help": "Usage:\n/cc - pick a session and start a thread in this channel\n/cc list - show connected sessions\n/cc unbind - remove all of your session threads in this channel\n/cc help - show this help\nIn a session thread, answer a permission prompt with \"yes <id>\" or \"no <id>\"."
}
```

| キー | 使う場所 | 投稿方法 |
|---|---|---|
| `picker_prompt` | `/cc` の応答 `text` と section の `text` | slash command 応答（ephemeral） |
| `picker_placeholder` | `static_select` の `placeholder` | 同上 |
| `no_sessions` | `/cc`、`/cc list` で 0 件 | 同上 |
| `connected_root` | §6.1 の root 投稿 | `chat.postMessage`（スレッドなし） |
| `session_offline` | §6.2 でセッションがオフライン | `chat.postMessage`（スレッド） |
| `unauthorized_command` | 許可ユーザー以外の `/cc` | slash command 応答（ephemeral） |
| `invite_bot` | §6.5 | `response_url`（ephemeral） |
| `unbind_done` | `/cc unbind` | slash command 応答（ephemeral） |
| `list_header` + `list_item` × n | `/cc list` で 1 件以上 | 同上 |
| `permission_prompt` | §7.3 | `chat.postMessage`（スレッド） |
| `help` | `/cc help` など | slash command 応答（ephemeral） |

応答しない場面:

- 許可ユーザー以外のスレッド返信（権限返答の形でも）: 何も投稿しません。
- 許可ユーザー以外による interactions: 何も投稿しません（HTTP `200`、body 空）。
- 束縛されていないスレッドへの返信: 何も投稿しません。

エスケープ規則（mrkdwn）: `{name}`、`{tool_name}`、`{description}`、`{input_preview}` に差し込む値は、`&` → `&amp;`、`<` → `&lt;`、`>` → `&gt;` の順に置換してから差し込みます（`<!channel>` などのメンションや偽リンクを防ぐためです）。`{count}` と `{request_id}` はそのまま差し込みます。`/cc` の `options` の `text` / `value`（plain_text）はエスケープしません。

### 13.2 WebSocket の `Error.message`

```json
[
  "hello timeout",
  "first frame must be hello",
  "unsupported protocol version: {version}",
  "invalid session name",
  "session name already connected: {name}"
]
```

### 13.3 relay の HTTP 応答 body（Slack 以外）

| 場面 | ステータス | body |
|---|---|---|
| `GET /healthz` | `200` | `ok` |
| `/dev/inject` 成功 | `200` | `ok` |
| `/dev/inject` 不正な body | `400` | `bad request` |
| `/dev/inject` オフライン | `409` | `offline` |
| 署名検証失敗、WS 認証失敗、OIDC callback 失敗 | `401` | 空 |
| 管理者以外のログイン | `403` | 空でもよい（HTML の文言はテストしません） |

---

## 14. 意図的にテストしないもの

判断基準は「壊れたときに気づけるか、戻せるか」です。次のものは壊れても起動時や手動確認ですぐ表面化し、データを失わないため、自動テストの対象にしません。
その代わり、判定ロジックはテスト可能な関数に切り出してあります（右列）。

| 対象 | テストしない理由 | 代わりに担保するもの |
|---|---|---|
| relay / agent の `main.rs`（ランタイム起動、配線） | 壊れていれば起動しないか、T3-2 / T7-2 の手動確認で即座に分かる | `lib.rs` 側の `router`、`ws::run`、`stdio::run` をテスト |
| `config::from_env` と環境変数の読み込み | 薄い読み込みのみ。誤りは起動直後に表面化する。edition 2024 では `std::env::set_var` が unsafe で並列テストと競合する | 設定値は `Config` を直接組み立ててテストに渡す |
| `HttpSlack`、`HttpOidc` の実 HTTP 呼び出し | 実 Slack が必要。誤りは T7-2 で即座に分かる | `SlackApi` / `OidcHttp` トレイトの偽物（`FakeSlack`、`FakeOidc`）で経路をテスト |
| JWT 署名検証 | 実装しない（§9.4 の理由による） | `iss`、`aud`、`nonce`、`exp` の検証をテスト |
| 管理画面 HTML の文言とレイアウト | 見れば分かり、外部に配信されない | エスケープ（`html_escapes_label`）と、リダイレクト・Cookie・ステータスはテスト |
| `LogSlack` の出力形式 | 開発モード専用で、人が読むだけ | T3-2 で目視 |
| 実時間スケールの backoff（1 秒〜30 秒） | テストが遅くなる | `Backoff{initial: 50ms}` で倍増と再接続をテスト |
| Claude Code が実際に `reply` を呼ぶか、channels の実際の JSON | research preview の外部挙動で、自動化できない | T3-2（ローカル）、T7-2（本番）で手動確認し、差異があれば本書と実装を直す |
| Slack 上での Block Kit の見た目、3 秒以内の応答 | 実 Slack が必要 | JSON の形はテスト。応答時間は T7-2 でログ確認 |
| `deploy/` の資材（Caddyfile、systemd unit、env.example） | 配置時に即座に失敗が分かる | `slack-manifest.json` が JSON としてパースできることだけ確認（T7-1） |
| relay 再起動でログアウト・pending が消えること | 仕様上許容した挙動（README §2.9） | なし |

既知の制限（テストではなく仕様上の割り切り）:

- `thread_broadcast`（「チャンネルにも送信」付きの返信）や添付ファイル付きの投稿（`file_share`）は `subtype` を持つため Claude に届きません。
- 編集（`message_changed`）は再送しません。
- 接続中セッションが 100 件を超えると、101 件目以降は `/cc` の選択肢に出ません。
- agent が未接続の間の `reply` は捨てられますが、Claude には `sent` と返ります。
- 同名のセッションが接続中の場合、後から来た agent は 30 秒ごとに再試行し続けます。
