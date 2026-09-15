# slack-claude-remote

Slack から、手元で動いている Claude Code の対話セッションを選び、スレッドで会話するツール。

```
[Mac] claude --channels ─stdio─▶ sccr-agent ─WSS(outbound)─▶ [e2] sccr-relay ◀─HTTPS─ Slack
                                                                   ▲ Caddy (TLS 終端, reverse_proxy)
```

- `crates/protocol`: relay と agent の間のメッセージ型
- `crates/agent`: Claude Code の channel（MCP stdio サーバー）。relay へ WebSocket で接続する
- `crates/relay`: Slack Events API、Sign in with Slack、agent の中継。手元 Mac ではなく e2（常時稼働するサーバー）に置く

設計の背景は [docs/plan/README.md](docs/plan/README.md)、詳細な仕様は [docs/spec.md](docs/spec.md) を参照。

## 開発

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test --all
```

---

## デプロイ手順

構成は「e2 で `sccr-relay` を常時稼働させ、Caddy で TLS 終端して Slack から HTTPS で叩けるようにする。手元 Mac では `sccr-agent` を Claude Code の channel として起動し、e2 の relay へ WebSocket でつなぎに行く」というものです。

以下では公開ホスト名を `sccr.example.jp` と書いています。**これはプレースホルダです**。実際のホスト名に置き換える箇所は次のとおりです。

- `deploy/Caddyfile.snippet` の `sccr.example.jp {` の行
- `deploy/env.example`（実運用では `/etc/sccr/env`）の `SCCR_PUBLIC_URL`
- `deploy/slack-manifest.json` の 4 箇所の URL（`oauth_config.redirect_urls`、`features.slash_commands[0].url`、`settings.event_subscriptions.request_url`、`settings.interactivity.request_url`）
- DNS で `sccr.example.jp` が e2 の IP を向いていること

### 1. e2 側: ビルド

`sccr-relay` を e2 上で動かすバイナリにする方法は 2 通りです。

**方法 A: e2 上でビルドする**

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
git clone <このリポジトリ> sccr && cd sccr
cargo build --release -p sccr-relay
# 成果物: target/release/sccr-relay
```

**方法 B: Mac でクロスビルドして転送する**

e2 の CPU アーキテクチャを先に確認します。

```bash
ssh e2 uname -m
# x86_64 → x86_64-unknown-linux-musl
# aarch64 / arm64 → aarch64-unknown-linux-musl
```

Mac 側（`cross` を使うとリンカ周りの面倒がありません。Docker が必要です）:

```bash
cargo install cross --git https://github.com/cross-rs/cross
cross build --release --target x86_64-unknown-linux-musl -p sccr-relay
# arm64 の e2 なら --target aarch64-unknown-linux-musl
scp target/x86_64-unknown-linux-musl/release/sccr-relay e2:/tmp/sccr-relay
```

どちらの方法でも、最終的に e2 の `/usr/local/bin/sccr-relay` に置きます（`root` 所有、`755`）。

### 2. e2 側: ユーザーと配置

```bash
sudo useradd --system --home-dir /var/lib/sccr --shell /usr/sbin/nologin sccr
sudo mkdir -p /var/lib/sccr /etc/sccr
sudo chown sccr:sccr /var/lib/sccr
sudo mv /tmp/sccr-relay /usr/local/bin/sccr-relay
sudo chmod 755 /usr/local/bin/sccr-relay
sudo cp deploy/env.example /etc/sccr/env
sudo chown root:sccr /etc/sccr/env
sudo chmod 640 /etc/sccr/env
```

`/etc/sccr/env` を編集し、`deploy/env.example` のコメントに従って値を埋めます（Slack 側の秘密は手順 4 で取得します）。特に次の 2 つは省略すると起動できません（`SCCR_DEV=1` を除く）。

- `SCCR_PUBLIC_URL`: OIDC の redirect_uri の組み立てに使うため、開発モード以外では必須です
- `SCCR_ADMIN_SLACK_USER`: 空のままだと誰もログインできず、agent トークンを一切発行できません

### 3. e2 側: systemd と Caddy

この時点では `/etc/sccr/env` の `SLACK_*` と `SCCR_ADMIN_SLACK_USER` はまだ空です。`SCCR_DEV=1` にしていない限り、relay はこのままでは `config::from_env` の必須変数チェックで起動に失敗します。ユニット自体はここで有効化して構いませんが、**`/healthz` の確認は手順 4 で秘密を埋めて再起動した後**に行ってください。

```bash
sudo cp deploy/sccr-relay.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now sccr-relay
sudo systemctl status sccr-relay
```

Caddy の設定ファイル（通常 `/etc/caddy/Caddyfile`）に `deploy/Caddyfile.snippet` の内容を追記し、reload します。

```bash
sudo tee -a /etc/caddy/Caddyfile < deploy/Caddyfile.snippet
sudo systemctl reload caddy
```

### 4. Slack 側: アプリ作成

1. https://api.slack.com/apps → **Create New App** → **From an app manifest** を選び、ワークスペースを選択します。
2. `deploy/slack-manifest.json` の内容を貼り付けます（URL のプレースホルダは事前に置き換えておきます）。
3. 作成後、**Install to Workspace** でインストールします。
4. 次の秘密を取得し、`/etc/sccr/env` に設定します。
   - **Basic Information** → App Credentials → Signing Secret → `SLACK_SIGNING_SECRET`
   - **Basic Information** → App Credentials → Client ID / Client Secret → `SLACK_CLIENT_ID` / `SLACK_CLIENT_SECRET`
   - **OAuth & Permissions** → Bot User OAuth Token（`xoxb-` で始まる）→ `SLACK_BOT_TOKEN`
5. 値を反映したら relay を再起動します（`sudo systemctl restart sccr-relay`）。`GET https://sccr.example.jp/healthz` が `ok` を返せばここまで正常です。
6. Slack アプリの **Event Subscriptions** 画面で Request URL が Verified になっているか確認します。manifest インポート直後は relay がまだ正しい `SLACK_SIGNING_SECRET` で起動していないため未検証のことがあり、その場合は URL 欄で **Retry** を押すと検証し直せます。
7. 会話に使う Slack チャンネルに bot を招待します（`/invite @sccr`。表示名は manifest の `bot_user.display_name` です）。招待していないチャンネルで `/cc` を実行すると `Invite the bot to this channel first.` と返ります。

**Sign in with Slack（OIDC）の scope について**: manifest の `oauth_config.scopes` には `openid` / `profile` を含めていません。ログイン用の `openid`/`profile` スコープは通常の OAuth インストールのスコープとは別物で、`GET /login`（`crates/relay/src/auth/oidc.rs` の `authorize_url`）が `https://slack.com/openid/connect/authorize` への redirect URL に `scope=openid%20profile` を直接埋め込みます。Slack の公式ドキュメント（Sign in with Slack）は、Sign in with Slack 用のスコープを通常の bot/user スコープと同じ OAuth フローに混ぜると scope conflict になると明記しており、manifest の `oauth_config.scopes.user` に `openid`/`profile` を追加する必要はなく、追加しない方が安全です。**未検証**: Slack 側アプリ設定の別画面（OAuth & Permissions の User Token Scopes）で `openid`/`profile` の有効化が別途必要かどうかはドキュメントから確認できませんでした。`/login` が scope エラーで失敗する場合は、そこに `openid`/`profile` を追加してから再試行してください（T7-2 の実機確認で確定させます）。

### 5. 初回ログインと agent トークン発行

1. ブラウザで `https://sccr.example.jp/` を開くと `/login` にリダイレクトされ、Slack の認可画面に進みます。
2. `SCCR_ADMIN_SLACK_USER` に設定した Slack user ID でログインした場合だけ成功します。それ以外のユーザーは `403` です。
3. ログイン後の管理画面でラベルを入力してトークンを発行します。**表示は 1 回だけです**（relay が保存するのは SHA-256 ハッシュのみ）。控えて次の手順で使います。

自分の Slack user ID が分からない場合は、Slack のプロフィール → `...` メニュー → **Copy member ID** で確認できます。

### 6. Mac 側: agent を Claude Code の channel として登録

`sccr-agent`（`cargo build --release -p sccr-agent` で `target/release/sccr-agent` にビルドしたもの、または `cargo install` したもの）を MCP サーバーとして登録します。

```bash
claude mcp add sccr \
  -e SCCR_RELAY_URL=wss://sccr.example.jp/agent/ws \
  -e SCCR_TOKEN=<手順5で発行したトークン> \
  -- /path/to/sccr-agent
```

同じディレクトリで動かす別のセッションと区別したい場合は `SCCR_SESSION_NAME` も一緒に渡します（省略時は `{hostname}:{カレントディレクトリ名}`）。

```bash
claude mcp add sccr \
  -e SCCR_RELAY_URL=wss://sccr.example.jp/agent/ws \
  -e SCCR_TOKEN=<トークン> \
  -e SCCR_SESSION_NAME=macbook:myproject \
  -- /path/to/sccr-agent
```

登録したセッションで channel を有効にして起動します。

```bash
claude --dangerously-load-development-channels server:sccr
```

channels は research preview の機能なので、`--dangerously-load-development-channels` を付けたときに全画面の確認ダイアログが出ます（**I am using this for local development** を選びます）。`claude mcp add` をプロジェクトスコープ（`-s project`、`.mcp.json` に書かれる）で登録した場合は、そのプロジェクトで初めて `sccr` を使うときに「New MCP server found in this project」の確認も出ます（既定のローカルスコープでは出ません）。起動後、起動バナーの下に `Channels (experimental) messages from server:sccr inject directly in this session` のような通知が出ていれば接続できています。

### 7. 使い方

1. bot を招待した Slack チャンネルで `/cc` を実行すると、接続中のセッション一覧から選ぶ picker が出ます（ephemeral）。選ぶとそのチャンネルに root 投稿がされ、その投稿へのスレッド返信が Claude とのやり取りになります。
2. `/cc list`: 接続中のセッション一覧を表示します。
3. `/cc unbind`: そのチャンネルで自分が作った束縛（binding）をすべて解除します。
4. `/cc help`: 使い方を表示します。
5. Claude が権限を求めるとスレッドに `Claude wants to run ...` という投稿が来ます。`yes <5文字のID>` または `no <ID>` とスレッドに返信すると、その束縛を行った本人からの返信だけが有効になり、Claude 側に許可/拒否が返ります。

---

## 既知の制限・運用上の注意

- **Cookie は `Secure` 属性付き**です。管理画面（`/login` `/` `/tokens`）は HTTPS（Caddy 経由）または `localhost` でしかログインできません。IP アドレスや平文 HTTP の e2 に直接アクセスすると Cookie が保存されずログインできません。
- `SCCR_ADMIN_SLACK_USER` は必ず設定してください。空のままだと OIDC ログインの許可判定が常に失敗し、誰もログインできず、agent トークンも一切発行できません。
- Cookie セッションと OIDC の pending state はメモリ保持です。relay を再起動すると全員ログアウトされ、進行中のログインもやり直しになります。agent トークンと束縛（binding）は状態ファイル（`SCCR_STATE_FILE`）に永続化されるため、再起動しても消えません。
- `HttpSlack` には現状リクエストタイムアウトがありません（既知の課題）。Slack API 呼び出しが詰まると、その呼び出しをしている agent 分の relay ループが止まる可能性があります。
- channels は research preview 機能で、`--channels` / `--dangerously-load-development-channels` の仕様は今後変わる可能性があります。開発時に動作確認した Claude Code のバージョンは **2.1.236** です。
- 接続中セッションが 100 件を超えると、101 件目以降は `/cc` の選択肢に出ません。
- `thread_broadcast`（チャンネルにも送信付きの返信）やファイル添付（`file_share`）、メッセージ編集（`message_changed`）は Claude に転送されません。
- agent が未接続の間に Claude が `reply` を呼んでも、その返信は捨てられます（Claude 側には成功したと返ります）。
- 同名のセッションが接続中のときに別の agent が同じ名前で接続しようとすると拒否されます（agent は 30 秒ごとに再試行し続けます）。

## 意図的にテストしていないもの

`docs/spec.md` §14 の判断基準（「壊れたときに気づけるか、戻せるか」）に基づき、次のものは自動テストの対象にしていません。

- `relay` / `agent` の `main.rs`（起動・配線）と `config::from_env` の環境変数読み込み
- `HttpSlack`、`HttpOidc` の実際の HTTP 呼び出し（トレイトの偽物で経路のみテスト）
- JWT の署名検証（実装していません。理由は spec §9.4）
- 管理画面 HTML の文言・レイアウト
- `LogSlack`（開発モード）の出力形式
- 実時間スケール（1〜30 秒）の backoff
- Claude Code が実際に `reply` ツールを呼ぶかどうか、channels の実際の JSON 形式
- Slack 上での Block Kit の見た目、3 秒以内の応答
- `deploy/` の資材そのもの（`slack-manifest.json` が JSON としてパースできることだけ確認済み）
- relay 再起動でログアウト・pending が消えること（仕様上許容した挙動）

## 動作確認メモ

未実施。
