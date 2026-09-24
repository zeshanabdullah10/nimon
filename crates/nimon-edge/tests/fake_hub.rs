//! End-to-end: the real edge actors against a minimal fake hub.
//! NI-SysCfg is disabled, so no NI driver is touched.

use std::collections::HashMap;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use nimon_core::actor::messages::{ActionResult, ActionType, EdgeHeartbeat, ExecuteAction};
use nimon_core::protocol::{WsMessage, WsMessageType};
use nimon_edge::config::{EdgeConfig, NodeConfig};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::WebSocketStream;

type Ws = WebSocketStream<TcpStream>;

async fn accept(listener: &TcpListener) -> (Ws, Option<String>) {
    let (stream, _) = tokio::time::timeout(Duration::from_secs(20), listener.accept())
        .await
        .expect("edge connects")
        .unwrap();
    let mut auth = None;
    let ws = tokio_tungstenite::accept_hdr_async(stream, |req: &Request, resp: Response| {
        auth = req
            .headers()
            .get("authorization")
            .map(|v| v.to_str().unwrap().to_string());
        Ok(resp)
    })
    .await
    .unwrap();
    (ws, auth)
}

async fn next_msg(ws: &mut Ws) -> Option<WsMessage> {
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(10), ws.next())
            .await
            .ok()??
            .ok()?;
        match frame {
            Message::Text(t) => return Some(WsMessage::from_json(&t).unwrap()),
            Message::Close(_) => return None,
            _ => continue,
        }
    }
}

async fn wait_for(ws: &mut Ws, t: WsMessageType) -> WsMessage {
    loop {
        let msg = next_msg(ws).await.expect("connection open");
        if msg.msg_type == t {
            return msg;
        }
    }
}

fn config(port: u16) -> EdgeConfig {
    let mut cfg = EdgeConfig {
        node: NodeConfig {
            reconnect_interval_secs: 1,
            max_reconnect_interval_secs: 2,
            heartbeat_interval_secs: 1,
            hub_token: Some("secret-token".into()),
            ..NodeConfig::new("it-edge", "IT Edge", format!("127.0.0.1:{port}"))
        },
        ..EdgeConfig::default()
    };
    cfg.api.syscfg.enabled = false;
    cfg.logging.file = String::new();
    cfg
}

#[actix::test]
async fn test_edge_against_fake_hub() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let edge = actix_rt::spawn(nimon_edge::start_with_shutdown(config(port), async {
        let _ = stop_rx.await;
    }));

    // 1. registration is the first frame; auth header present
    let (mut ws, auth) = accept(&listener).await;
    assert_eq!(auth.as_deref(), Some("Bearer secret-token"));
    let first = next_msg(&mut ws).await.unwrap();
    assert_eq!(first.msg_type, WsMessageType::EdgeRegister);
    let reg: nimon_core::actor::messages::EdgeRegister = first.payload().unwrap();
    assert_eq!(reg.edge_id, "it-edge");
    assert_eq!(reg.ip_address.as_deref(), Some("127.0.0.1"));

    // 2. heartbeat carries version and uptime
    let hb: EdgeHeartbeat = wait_for(&mut ws, WsMessageType::Heartbeat)
        .await
        .payload()
        .unwrap();
    assert_eq!(hb.version.as_deref(), Some(env!("CARGO_PKG_VERSION")));
    assert!(hb.uptime_secs.is_some());

    // 3. actions are answered with reply_to = request msg_id
    let request = WsMessage::execute_action(ExecuteAction {
        action_id: "a-1".into(),
        device_id: "other-edge:Dev1".into(),
        action_type: ActionType::PowerCycle,
        parameters: HashMap::new(),
    });
    let request_id = request.msg_id.clone();
    ws.send(Message::Text(request.to_json().unwrap()))
        .await
        .unwrap();
    let reply = wait_for(&mut ws, WsMessageType::ActionResult).await;
    assert_eq!(reply.reply_to.as_deref(), Some(request_id.as_str()));
    let result: ActionResult = reply.payload().unwrap();
    assert!(!result.success, "devices of other edges are rejected");

    // 4. hub drops the connection -> edge reconnects and registers again
    drop(ws);
    let (mut ws, _) = accept(&listener).await;
    let first = next_msg(&mut ws).await.unwrap();
    assert_eq!(first.msg_type, WsMessageType::EdgeRegister);

    // 5. graceful shutdown sends a close frame and returns
    stop_tx.send(()).unwrap();
    let mut closed = false;
    while let Ok(Some(frame)) = tokio::time::timeout(Duration::from_secs(5), ws.next()).await {
        match frame {
            Ok(Message::Close(_)) | Err(_) => {
                closed = true;
                break;
            }
            _ => {}
        }
    }
    assert!(closed, "edge closed the socket on shutdown");
    tokio::time::timeout(Duration::from_secs(10), edge)
        .await
        .expect("start_with_shutdown returns")
        .unwrap();
}
