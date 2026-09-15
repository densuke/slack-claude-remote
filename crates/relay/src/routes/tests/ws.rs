use std::net::SocketAddr;

use futures_util::{SinkExt, StreamExt};
use protocol::{AgentMsg, PROTOCOL_VERSION};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

use super::*;

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

async fn serve(state: Arc<AppState>) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router(state)).await.unwrap() });
    addr
}

async fn connect(addr: SocketAddr, token: &str) -> Ws {
    let mut req = format!("ws://{addr}/agent/ws")
        .into_client_request()
        .unwrap();
    req.headers_mut()
        .insert("authorization", format!("Bearer {token}").parse().unwrap());
    connect_async(req).await.unwrap().0
}

async fn send_msg(ws: &mut Ws, msg: &AgentMsg) {
    let text = serde_json::to_string(msg).unwrap();
    ws.send(Message::Text(text.into())).await.unwrap();
}

/// Next relay message, or None once the socket is closed.
async fn recv_msg(ws: &mut Ws) -> Option<RelayMsg> {
    loop {
        match timeout(Duration::from_secs(1), ws.next()).await.unwrap() {
            Some(Ok(Message::Text(t))) => return Some(serde_json::from_str(&t).unwrap()),
            Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return None,
            Some(Ok(_)) => {}
        }
    }
}

fn hello(version: u32, name: &str) -> AgentMsg {
    AgentMsg::Hello {
        version,
        name: name.into(),
        host: "mac".into(),
        cwd: "/a/proj".into(),
    }
}

fn ws_request(auth: Option<&str>) -> Request<Body> {
    let builder = Request::get("/agent/ws");
    let builder = match auth {
        Some(value) => builder.header(header::AUTHORIZATION, value),
        None => builder,
    };
    builder.body(Body::empty()).unwrap()
}

async fn error_for(addr: SocketAddr, first: AgentMsg) -> Option<RelayMsg> {
    let mut ws = connect(addr, "dev").await;
    send_msg(&mut ws, &first).await;
    let msg = recv_msg(&mut ws).await;
    assert_eq!(
        recv_msg(&mut ws).await,
        None,
        "socket must close after error"
    );
    msg
}

fn error(message: &str) -> Option<RelayMsg> {
    Some(RelayMsg::Error {
        message: message.into(),
    })
}

#[tokio::test]
async fn ws_without_bearer_401() {
    let s = setup();
    let unauthorized = (StatusCode::UNAUTHORIZED, String::new());
    assert_eq!(call(&s.state, ws_request(None)).await, unauthorized);
    assert_eq!(
        call(&s.state, ws_request(Some("Basic ZGV2"))).await,
        unauthorized
    );
    assert_eq!(call(&s.state, ws_request(Some("dev"))).await, unauthorized);
}

#[tokio::test]
async fn ws_bad_token_401() {
    let s = setup();
    let unauthorized = (StatusCode::UNAUTHORIZED, String::new());
    assert_eq!(
        call(&s.state, ws_request(Some("Bearer nope"))).await,
        unauthorized
    );
    assert_eq!(
        call(&s.state, ws_request(Some(&format!("Bearer {DEV_SHA256}")))).await,
        unauthorized
    );

    // "dev" is accepted only in dev mode when it is not a stored token.
    let prod = setup_with(FakeSlack::new(), config(false, SECRET), false);
    assert_eq!(
        call(&prod.state, ws_request(Some("Bearer dev"))).await,
        unauthorized
    );
    let dev = setup_with(FakeSlack::new(), config(true, ""), false);
    let (status, _) = call(&dev.state, ws_request(Some("Bearer dev"))).await;
    assert_ne!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn ws_hello_wrong_version_gets_error() {
    let s = setup();
    let addr = serve(s.state.clone()).await;

    assert_eq!(
        error_for(addr, hello(PROTOCOL_VERSION + 1, "mac:proj")).await,
        error("unsupported protocol version: 2")
    );
    let reply = AgentMsg::Reply {
        chat_id: CHAT.into(),
        text: "hi".into(),
    };
    assert_eq!(
        error_for(addr, reply).await,
        error("first frame must be hello")
    );
    assert_eq!(
        error_for(addr, hello(PROTOCOL_VERSION, "")).await,
        error("invalid session name")
    );
    assert_eq!(
        error_for(addr, hello(PROTOCOL_VERSION, "bad\nname")).await,
        error("invalid session name")
    );
    assert!(s.state.hub.list().await.is_empty());
}

#[tokio::test]
async fn ws_duplicate_name_gets_error() {
    let s = setup();
    let addr = serve(s.state.clone()).await;
    let mut first = connect(addr, "dev").await;
    send_msg(&mut first, &hello(PROTOCOL_VERSION, "mac:proj")).await;
    assert_eq!(
        recv_msg(&mut first).await,
        Some(RelayMsg::Welcome {
            name: "mac:proj".into()
        })
    );

    assert_eq!(
        error_for(addr, hello(PROTOCOL_VERSION, "mac:proj")).await,
        error("session name already connected: mac:proj")
    );
    assert_eq!(s.state.hub.list().await, vec!["mac:proj"]);
}

#[tokio::test]
async fn ws_reply_posts_chunked_to_thread() {
    let s = setup();
    let addr = serve(s.state.clone()).await;
    let mut ws = connect(addr, "dev").await;
    send_msg(&mut ws, &hello(PROTOCOL_VERSION, "mac:proj")).await;
    assert_eq!(
        recv_msg(&mut ws).await,
        Some(RelayMsg::Welcome {
            name: "mac:proj".into()
        })
    );

    let first = format!("{}\n", "a".repeat(3000));
    let second = "b".repeat(2000);
    let reply = AgentMsg::Reply {
        chat_id: CHAT.into(),
        text: format!("{first}{second}"),
    };
    send_msg(&mut ws, &reply).await;
    let calls = wait_calls(&s.slack, 2).await;
    let post = |text: &str| Call::Post {
        channel: CHANNEL.into(),
        thread_ts: Some(THREAD.into()),
        text: text.into(),
    };
    assert_eq!(calls, vec![post(&first), post(&second)]);

    // Relay messages from the hub reach the socket.
    let inbound = RelayMsg::Inbound {
        chat_id: CHAT.into(),
        user: USER.into(),
        text: "hello".into(),
    };
    s.state.hub.send("mac:proj", inbound.clone()).await.unwrap();
    assert_eq!(recv_msg(&mut ws).await, Some(inbound));

    ws.close(None).await.unwrap();
    timeout(Duration::from_secs(1), async {
        while !s.state.hub.list().await.is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("agent not unregistered after close");
}
