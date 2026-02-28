//! OpenClaw Gateway WebSocket client
//!
//! Implements OpenClaw's node protocol for VPS fleet management integration.
//! Forked from clawbernetes/crates/clawnode/src/client.rs — GPU logic removed,
//! VPS capabilities substituted.

use crate::SharedState;
use crate::commands::{CommandRequest, handle_command};
use crate::identity::{DeviceIdentity, DeviceParams};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::{interval, timeout};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};
use url::Url;
use uuid::Uuid;

const PROTOCOL_VERSION: u32 = 3; // Must match gateway's PROTOCOL_VERSION
const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");

// ─── Frame types ─────────────────────────────────────────────────────────────

/// OpenClaw gateway frame types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum GatewayFrame {
    Request(RequestFrame),
    Response(ResponseFrame),
    Event(EventFrame),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestFrame {
    #[serde(rename = "type")]
    pub frame_type: String, // Always "req"
    pub id: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl RequestFrame {
    pub fn new(id: String, method: String, params: Option<Value>) -> Self {
        Self {
            frame_type: "req".to_string(),
            id,
            method,
            params,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseFrame {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorShape>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventFrame {
    pub event: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorShape {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

// ─── Connect params ───────────────────────────────────────────────────────────

/// Connect params sent on connection
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectParams {
    pub min_protocol: u32,
    pub max_protocol: u32,
    pub client: ClientInfo,
    pub caps: Vec<String>,
    pub commands: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth: Option<AuthParams>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceParams>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scopes: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientInfo {
    pub id: String,
    pub display_name: String,
    pub version: String,
    pub platform: String,
    pub mode: String,
    pub instance_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuthParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

// ─── Pairing ─────────────────────────────────────────────────────────────────

/// Node pair request params
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodePairRequestParams {
    pub node_id: String,
    pub display_name: String,
    pub platform: String,
    pub version: String,
    pub caps: Vec<String>,
    pub commands: Vec<String>,
    pub silent: bool,
}

/// Node invoke result params
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeInvokeResultParams {
    pub id: String,
    pub node_id: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload_json: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<InvokeError>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InvokeError {
    pub code: String,
    pub message: String,
}

/// Incoming node invoke request event
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeInvokeRequestEvent {
    pub id: String,
    pub node_id: String,
    pub command: String,
    #[serde(default, rename = "paramsJSON")]
    pub params_json: Option<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

// ─── Gateway Client ───────────────────────────────────────────────────────────

/// Gateway WebSocket client
pub struct GatewayClient {
    state: SharedState,
    identity: DeviceIdentity,
    outgoing_tx: Option<mpsc::Sender<RequestFrame>>,
}

impl GatewayClient {
    pub fn new(state: SharedState, identity_path: PathBuf) -> Self {
        let identity = DeviceIdentity::load_or_create(&identity_path).unwrap_or_else(|e| {
            warn!(error = %e, "failed to load identity, generating new one");
            DeviceIdentity::generate()
        });

        info!(device_id = %identity.device_id, "using device identity");

        Self {
            state,
            identity,
            outgoing_tx: None,
        }
    }

    pub fn with_identity(state: SharedState, identity: DeviceIdentity) -> Self {
        Self {
            state,
            identity,
            outgoing_tx: None,
        }
    }

    /// Connect to gateway and run the event loop
    pub async fn connect(
        &mut self,
        gateway_url: &str,
        auth_token: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let url = Url::parse(gateway_url)?;
        info!("connecting to gateway: {}", url);

        let (ws_stream, _) = timeout(Duration::from_secs(10), connect_async(url.as_str()))
            .await
            .map_err(|_| "connection timeout")??;

        let (write, read) = ws_stream.split();
        let mut write = write;
        let mut read = read;
        let (outgoing_tx, mut outgoing_rx) = mpsc::channel::<RequestFrame>(32);
        self.outgoing_tx = Some(outgoing_tx.clone());

        let hostname = hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_else(|_| "unknown".to_string());
        let platform = format!("{} {}", std::env::consts::OS, std::env::consts::ARCH);

        let caps = self.state.capabilities.clone();
        let commands = self.state.commands.clone();
        let node_id = self.identity.device_id.clone();

        // Wait for challenge from gateway
        info!("waiting for challenge from gateway...");
        let challenge_response = timeout(Duration::from_secs(10), read.next())
            .await
            .map_err(|_| "challenge timeout")?
            .ok_or("connection closed waiting for challenge")??;

        let challenge_nonce = match &challenge_response {
            Message::Text(text) => {
                let frame: Value = serde_json::from_str(text)?;
                info!("received from gateway: {}", text);

                if let Some(event) = frame.get("event").and_then(|e| e.as_str()) {
                    if event == "connect.challenge" {
                        frame
                            .get("payload")
                            .and_then(|p| p.get("nonce"))
                            .and_then(|n| n.as_str())
                            .map(|s| s.to_string())
                    } else {
                        warn!("unexpected event: {}", event);
                        None
                    }
                } else if let Some(error) = frame.get("error") {
                    let msg = error
                        .get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("unknown");
                    return Err(format!("gateway error on connect: {}", msg).into());
                } else {
                    warn!("unexpected frame type");
                    None
                }
            }
            Message::Close(frame) => {
                let reason = frame
                    .as_ref()
                    .map(|f| f.reason.to_string())
                    .unwrap_or_default();
                return Err(format!("gateway closed connection: {}", reason).into());
            }
            other => {
                warn!("unexpected message type: {:?}", other);
                None
            }
        };

        let challenge_nonce = challenge_nonce.ok_or("no challenge nonce received")?;
        info!("got challenge nonce: {}", challenge_nonce);

        // Build device params with the challenge nonce
        let device_params = self.identity.device_params(
            "node-host",
            "node",
            "node",
            &["node".to_string()],
            auth_token,
            Some(&challenge_nonce),
        );

        let connect_params = ConnectParams {
            min_protocol: PROTOCOL_VERSION,
            max_protocol: PROTOCOL_VERSION,
            client: ClientInfo {
                id: "node-host".to_string(),
                display_name: hostname.clone(),
                version: CLIENT_VERSION.to_string(),
                platform: platform.clone(),
                mode: "node".to_string(),
                instance_id: Uuid::new_v4().to_string(),
            },
            caps: caps.clone(),
            commands: commands.clone(),
            auth: auth_token.map(|t| AuthParams {
                token: Some(t.to_string()),
            }),
            device: Some(device_params),
            role: Some("node".to_string()),
            scopes: Some(vec!["node".to_string()]),
        };

        let connect_frame = json!({
            "type": "req",
            "id": Uuid::new_v4().to_string(),
            "method": "connect",
            "params": &connect_params
        });
        info!("sending signed connect...");
        debug!("connect frame: {}", connect_frame);

        match write.send(Message::Text(connect_frame.to_string())).await {
            Ok(_) => info!("sent connect frame successfully"),
            Err(e) => {
                error!("failed to send connect: {}", e);
                return Err(format!("failed to send connect: {}", e).into());
            }
        }

        if let Err(e) = write.flush().await {
            warn!("flush error (may be ok): {}", e);
        }

        // Wait for hello response
        info!("waiting for hello response...");
        let response = match timeout(Duration::from_secs(10), read.next()).await {
            Ok(Some(Ok(msg))) => msg,
            Ok(Some(Err(e))) => {
                error!("websocket read error: {}", e);
                return Err(format!("websocket error: {}", e).into());
            }
            Ok(None) => {
                error!("connection closed while waiting for hello");
                return Err("connection closed by gateway".into());
            }
            Err(_) => {
                error!("hello timeout");
                return Err("hello timeout".into());
            }
        };

        let mut already_paired = false;
        if let Message::Text(text) = response {
            let frame: Value = serde_json::from_str(&text)?;
            debug!("received after connect: {}", text);

            let is_hello_ok = frame.get("ok").and_then(|v| v.as_bool()).unwrap_or(false)
                && frame
                    .get("payload")
                    .and_then(|p| p.get("type"))
                    .and_then(|t| t.as_str())
                    .map(|t| t == "hello-ok")
                    .unwrap_or(false);

            if is_hello_ok {
                info!("connected to gateway successfully!");
                already_paired = true;
                if let Some(token) = frame
                    .get("payload")
                    .and_then(|p| p.get("auth"))
                    .and_then(|a| a.get("deviceToken"))
                    .and_then(|t| t.as_str())
                {
                    info!("received device token");
                    debug!("device token: {}", token);
                }
            } else if let Some(error) = frame.get("error") {
                let msg = error
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("unknown error");
                return Err(format!("gateway error: {}", msg).into());
            } else if frame.get("event").is_some() {
                warn!("gateway sent another challenge - signature rejected");
                return Err("device signature rejected".into());
            } else {
                warn!("unexpected response format");
            }
        }

        if already_paired {
            info!("already paired, skipping pairing request");
        } else {
            // Request pairing
            let pair_request = NodePairRequestParams {
                node_id: node_id.clone(),
                display_name: hostname.clone(),
                platform: platform.clone(),
                version: CLIENT_VERSION.to_string(),
                caps,
                commands,
                silent: true,
            };

            let request_id = Uuid::new_v4().to_string();
            let pair_frame = RequestFrame::new(
                request_id.clone(),
                "node.pair.request".to_string(),
                Some(serde_json::to_value(&pair_request)?),
            );

            debug!("sending node.pair.request");
            write
                .send(Message::Text(serde_json::to_string(&pair_frame)?))
                .await?;

            let mut paired = false;
            let pair_timeout = tokio::time::Instant::now() + Duration::from_secs(10);

            while tokio::time::Instant::now() < pair_timeout && !paired {
                match timeout(Duration::from_secs(2), read.next()).await {
                    Ok(Some(Ok(Message::Text(text)))) => {
                        let frame: Value = serde_json::from_str(&text)?;
                        debug!("received during pairing: {}", text);

                        if frame.get("id").is_some() {
                            if let Some(_result) = frame.get("result") {
                                if let Some(token) = _result.get("token").and_then(|t| t.as_str()) {
                                    info!("node paired successfully, token received");
                                    self.state.node_token = Some(token.to_string());
                                }
                                paired = true;
                            } else if let Some(error) = frame.get("error") {
                                let msg = error
                                    .get("message")
                                    .and_then(|m| m.as_str())
                                    .unwrap_or("pairing failed");
                                if msg.contains("already") || msg.contains("exists") {
                                    info!("node already registered");
                                    paired = true;
                                } else {
                                    warn!("pairing error: {}", msg);
                                    paired = true;
                                }
                            }
                        }
                    }
                    Ok(Some(Ok(Message::Ping(data)))) => {
                        let _ = write.send(Message::Pong(data)).await;
                    }
                    Ok(Some(Ok(Message::Close(_)))) => {
                        return Err("connection closed during pairing".into());
                    }
                    Ok(Some(Err(e))) => {
                        return Err(format!("websocket error: {}", e).into());
                    }
                    Ok(None) => {
                        return Err("connection closed".into());
                    }
                    Err(_) => {
                        debug!("pairing response timeout, continuing");
                        paired = true;
                    }
                    _ => {}
                }
            }
        }

        info!("node registered as {} ({})", hostname, node_id);

        // Main event loop
        let mut heartbeat_interval = interval(Duration::from_secs(30));
        let node_id_clone = node_id.clone();

        loop {
            tokio::select! {
                Some(frame) = outgoing_rx.recv() => {
                    let json = serde_json::to_string(&frame)?;
                    debug!("sending: {}", json);
                    if let Err(e) = write.send(Message::Text(json)).await {
                        error!("send error: {}", e);
                        break;
                    }
                }

                msg = read.next() => {
                    match msg {
                        Some(Ok(Message::Text(text))) => {
                            if let Err(e) = self.handle_message(&text, &node_id_clone, &outgoing_tx).await {
                                error!("message handling error: {}", e);
                            }
                        }
                        Some(Ok(Message::Ping(data))) => {
                            let _ = write.send(Message::Pong(data)).await;
                        }
                        Some(Ok(Message::Close(_))) => {
                            info!("gateway closed connection");
                            break;
                        }
                        Some(Err(e)) => {
                            error!("websocket error: {}", e);
                            break;
                        }
                        None => {
                            info!("connection closed");
                            break;
                        }
                        _ => {}
                    }
                }

                _ = heartbeat_interval.tick() => {
                    let heartbeat = RequestFrame::new(
                        Uuid::new_v4().to_string(),
                        "node.event".to_string(),
                        Some(json!({
                            "event": "heartbeat",
                            "payload": {
                                "nodeId": node_id_clone,
                            },
                        })),
                    );
                    if let Err(e) = outgoing_tx.send(heartbeat).await {
                        error!("heartbeat send error: {}", e);
                        break;
                    }
                }
            }
        }

        Ok(())
    }

    async fn handle_message(
        &self,
        text: &str,
        node_id: &str,
        outgoing_tx: &mpsc::Sender<RequestFrame>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let frame: Value = serde_json::from_str(text)?;
        debug!("received: {}", text);

        if let Some(event) = frame.get("event").and_then(|e| e.as_str()) {
            match event {
                "node.invoke.request" => {
                    if let Some(payload) = frame.get("payload") {
                        let invoke: NodeInvokeRequestEvent =
                            serde_json::from_value(payload.clone())?;
                        self.handle_invoke(invoke, node_id, outgoing_tx).await?;
                    }
                }
                "tick" => {
                    // Gateway tick, ignore
                }
                _ => {
                    debug!("unhandled event: {}", event);
                }
            }
        }

        if frame.get("id").is_some() && frame.get("result").is_some() {
            debug!("received response");
        }

        if let Some(error) = frame.get("error") {
            let msg = error
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown");
            warn!("gateway error: {}", msg);
        }

        Ok(())
    }

    async fn handle_invoke(
        &self,
        invoke: NodeInvokeRequestEvent,
        node_id: &str,
        outgoing_tx: &mpsc::Sender<RequestFrame>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("invoke request: {} (id={})", invoke.command, invoke.id);

        let params: Value = invoke
            .params_json
            .as_ref()
            .map(|s| serde_json::from_str(s).unwrap_or(Value::Null))
            .unwrap_or(Value::Null);

        let request = CommandRequest {
            command: invoke.command.clone(),
            params,
        };

        let result = handle_command(&self.state, request).await;

        let result_params = match result {
            Ok(payload) => NodeInvokeResultParams {
                id: invoke.id,
                node_id: node_id.to_string(),
                ok: true,
                payload: Some(payload),
                payload_json: None,
                error: None,
            },
            Err(e) => NodeInvokeResultParams {
                id: invoke.id,
                node_id: node_id.to_string(),
                ok: false,
                payload: None,
                payload_json: None,
                error: Some(InvokeError {
                    code: "COMMAND_ERROR".to_string(),
                    message: e.to_string(),
                }),
            },
        };

        let response = RequestFrame::new(
            Uuid::new_v4().to_string(),
            "node.invoke.result".to_string(),
            Some(serde_json::to_value(&result_params)?),
        );

        outgoing_tx.send(response).await?;
        Ok(())
    }
}

/// Send a command result back to the gateway
pub async fn send_result(
    tx: &mpsc::Sender<RequestFrame>,
    invoke_id: &str,
    node_id: &str,
    success: bool,
    payload: Value,
    error: Option<String>,
) -> Result<(), mpsc::error::SendError<RequestFrame>> {
    let result_params = NodeInvokeResultParams {
        id: invoke_id.to_string(),
        node_id: node_id.to_string(),
        ok: success,
        payload: if success { Some(payload) } else { None },
        payload_json: None,
        error: error.map(|msg| InvokeError {
            code: "ERROR".to_string(),
            message: msg,
        }),
    };

    let frame = RequestFrame::new(
        Uuid::new_v4().to_string(),
        "node.invoke.result".to_string(),
        Some(serde_json::to_value(&result_params).unwrap()),
    );

    tx.send(frame).await
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── RequestFrame ──────────────────────────────────────────────────────────

    #[test]
    fn request_frame_new_sets_frame_type_to_req() {
        let frame = RequestFrame::new("id-001".to_string(), "connect".to_string(), None);
        assert_eq!(frame.frame_type, "req");
        assert_eq!(frame.id, "id-001");
        assert_eq!(frame.method, "connect");
        assert!(frame.params.is_none());
    }

    #[test]
    fn request_frame_serializes_type_field() {
        let frame = RequestFrame::new(
            "id-002".to_string(),
            "node.pair.request".to_string(),
            Some(json!({"nodeId": "n-abc"})),
        );
        let s = serde_json::to_string(&frame).unwrap();
        assert!(s.contains(r#""type":"req""#), "missing type field: {s}");
        assert!(s.contains("node.pair.request"));
        assert!(s.contains("id-002"));
        assert!(s.contains("n-abc"));
    }

    #[test]
    fn request_frame_omits_params_when_none() {
        let frame = RequestFrame::new("id-003".to_string(), "ping".to_string(), None);
        let s = serde_json::to_string(&frame).unwrap();
        assert!(
            !s.contains("params"),
            "params must be omitted when None: {s}"
        );
    }

    #[test]
    fn request_frame_includes_params_when_present() {
        let frame = RequestFrame::new(
            "id-004".to_string(),
            "node.invoke.result".to_string(),
            Some(json!({"command": "health.check"})),
        );
        let s = serde_json::to_string(&frame).unwrap();
        assert!(s.contains("params"));
        assert!(s.contains("health.check"));
    }

    // ── ResponseFrame ─────────────────────────────────────────────────────────

    #[test]
    fn response_frame_with_result_omits_error_field() {
        let frame = ResponseFrame {
            id: "req-001".to_string(),
            result: Some(json!({"ok": true, "score": 95})),
            error: None,
        };
        let s = serde_json::to_string(&frame).unwrap();
        assert!(s.contains("req-001"));
        assert!(s.contains(r#""ok":true"#));
        assert!(
            !s.contains(r#""error""#),
            "error must be omitted when None: {s}"
        );
    }

    #[test]
    fn response_frame_with_error_omits_result_field() {
        let frame = ResponseFrame {
            id: "req-002".to_string(),
            result: None,
            error: Some(ErrorShape {
                code: 400,
                message: "bad request".to_string(),
                details: None,
            }),
        };
        let s = serde_json::to_string(&frame).unwrap();
        assert!(s.contains("bad request"));
        assert!(s.contains("400"));
        assert!(
            !s.contains(r#""result""#),
            "result must be omitted when None: {s}"
        );
    }

    // ── EventFrame ────────────────────────────────────────────────────────────

    #[test]
    fn event_frame_serializes_with_payload() {
        let frame = EventFrame {
            event: "node.invoke.request".to_string(),
            payload: Some(json!({"command": "vps.status", "id": "inv-1"})),
        };
        let s = serde_json::to_string(&frame).unwrap();
        assert!(s.contains("node.invoke.request"));
        assert!(s.contains("vps.status"));
    }

    #[test]
    fn event_frame_omits_payload_when_none() {
        let frame = EventFrame {
            event: "tick".to_string(),
            payload: None,
        };
        let s = serde_json::to_string(&frame).unwrap();
        assert!(s.contains("tick"));
        assert!(
            !s.contains("payload"),
            "payload must be omitted when None: {s}"
        );
    }

    // ── ErrorShape ────────────────────────────────────────────────────────────

    #[test]
    fn error_shape_omits_details_when_none() {
        let err = ErrorShape {
            code: 500,
            message: "internal server error".to_string(),
            details: None,
        };
        let s = serde_json::to_string(&err).unwrap();
        assert!(s.contains("500"));
        assert!(s.contains("internal server error"));
        assert!(
            !s.contains("details"),
            "details must be omitted when None: {s}"
        );
    }

    #[test]
    fn error_shape_includes_details_when_present() {
        let err = ErrorShape {
            code: 422,
            message: "validation error".to_string(),
            details: Some(json!({"field": "name", "reason": "required"})),
        };
        let s = serde_json::to_string(&err).unwrap();
        assert!(s.contains("validation error"));
        assert!(s.contains("details"));
        assert!(s.contains("required"));
    }

    // ── NodeInvokeRequestEvent ────────────────────────────────────────────────

    #[test]
    fn node_invoke_request_event_deserializes_full() {
        let raw = r#"{
            "id": "invoke-001",
            "nodeId": "node-abc123",
            "command": "health.check",
            "paramsJSON": "{\"verbose\":true}",
            "timeoutMs": 5000,
            "idempotencyKey": "key-xyz"
        }"#;
        let event: NodeInvokeRequestEvent = serde_json::from_str(raw).unwrap();
        assert_eq!(event.id, "invoke-001");
        assert_eq!(event.node_id, "node-abc123");
        assert_eq!(event.command, "health.check");
        assert_eq!(event.params_json, Some("{\"verbose\":true}".to_string()));
        assert_eq!(event.timeout_ms, Some(5000));
        assert_eq!(event.idempotency_key, Some("key-xyz".to_string()));
    }

    #[test]
    fn node_invoke_request_event_handles_missing_optionals() {
        let raw = r#"{
            "id": "invoke-002",
            "nodeId": "node-xyz",
            "command": "vps.info"
        }"#;
        let event: NodeInvokeRequestEvent = serde_json::from_str(raw).unwrap();
        assert_eq!(event.id, "invoke-002");
        assert_eq!(event.command, "vps.info");
        assert!(event.params_json.is_none());
        assert!(event.timeout_ms.is_none());
        assert!(event.idempotency_key.is_none());
    }

    // ── ConnectParams / ClientInfo ────────────────────────────────────────────

    #[test]
    fn connect_params_uses_camel_case_rename() {
        let params = ConnectParams {
            min_protocol: 3,
            max_protocol: 3,
            client: ClientInfo {
                id: "node-host".to_string(),
                display_name: "test-node-01".to_string(),
                version: "0.1.0".to_string(),
                platform: "linux x86_64".to_string(),
                mode: "node".to_string(),
                instance_id: "inst-abc".to_string(),
            },
            caps: vec!["vps".to_string(), "health".to_string()],
            commands: vec!["vps.info".to_string()],
            auth: None,
            device: None,
            role: Some("node".to_string()),
            scopes: Some(vec!["node".to_string()]),
        };
        let s = serde_json::to_string(&params).unwrap();
        assert!(s.contains("minProtocol"), "expected minProtocol in: {s}");
        assert!(s.contains("maxProtocol"), "expected maxProtocol in: {s}");
        assert!(s.contains("displayName"), "expected displayName in: {s}");
        assert!(s.contains("instanceId"), "expected instanceId in: {s}");
        assert!(s.contains("test-node-01"));
        assert!(s.contains("inst-abc"));
    }

    #[test]
    fn invoke_error_serializes_correctly() {
        let err = InvokeError {
            code: "COMMAND_ERROR".to_string(),
            message: "unknown command: bad.cmd".to_string(),
        };
        let s = serde_json::to_string(&err).unwrap();
        assert!(s.contains("COMMAND_ERROR"));
        assert!(s.contains("unknown command: bad.cmd"));
    }
}
