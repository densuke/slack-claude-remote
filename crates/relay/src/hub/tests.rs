use super::*;

fn chan() -> (mpsc::Sender<RelayMsg>, mpsc::Receiver<RelayMsg>) {
    mpsc::channel(8)
}

#[tokio::test]
async fn register_then_list() {
    let hub = Hub::default();
    let (tx1, _rx1) = chan();
    let (tx2, _rx2) = chan();
    hub.register("mac:proj", tx1).await.unwrap();
    hub.register("mac:other", tx2).await.unwrap();
    assert_eq!(hub.list().await, vec!["mac:other", "mac:proj"]);
}

#[tokio::test]
async fn duplicate_name_rejected() {
    let hub = Hub::default();
    let (tx1, mut rx1) = chan();
    let (tx2, _rx2) = chan();
    hub.register("mac:proj", tx1).await.unwrap();
    assert_eq!(hub.register("mac:proj", tx2).await, Err(Duplicate));
    // The original connection stays registered.
    let msg = RelayMsg::Welcome {
        name: "mac:proj".into(),
    };
    hub.send("mac:proj", msg.clone()).await.unwrap();
    assert_eq!(rx1.recv().await, Some(msg));
}

#[tokio::test]
async fn send_to_offline_errors() {
    let hub = Hub::default();
    let msg = RelayMsg::Welcome { name: "x".into() };
    assert_eq!(hub.send("nobody", msg.clone()).await, Err(Offline));

    let (tx, rx) = chan();
    hub.register("closed", tx).await.unwrap();
    drop(rx);
    assert_eq!(hub.send("closed", msg).await, Err(Offline));
}

#[tokio::test]
async fn unregister_removes() {
    let hub = Hub::default();
    let (tx, _rx) = chan();
    hub.register("mac:proj", tx).await.unwrap();
    hub.unregister("mac:proj").await;
    assert!(hub.list().await.is_empty());
    let (tx2, _rx2) = chan();
    assert_eq!(hub.register("mac:proj", tx2).await, Ok(()));
}
