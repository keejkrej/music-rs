use std::{
    net::{TcpListener, TcpStream},
    sync::mpsc::{self, Receiver, Sender},
    thread,
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tungstenite::{Message, accept};

use crate::commands::EditCommand;

const JSONRPC_VERSION: &str = "2.0";
const PARSE_ERROR: i32 = -32700;
const INVALID_REQUEST: i32 = -32600;
const METHOD_NOT_FOUND: i32 = -32601;
const INVALID_PARAMS: i32 = -32602;
const APP_ERROR: i32 = -32000;

#[derive(Debug)]
pub struct ControlServer {
    pub addr: String,
    receiver: Receiver<PendingControlRequest>,
}

#[derive(Debug)]
pub struct PendingControlRequest {
    pub envelope: ControlEnvelope,
    pub reply: Option<Sender<ControlReply>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ControlEnvelope {
    pub id: Option<Value>,
    pub request: ControlRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum ControlRequest {
    GetSummary,
    GetProject,
    ApplyCommands { commands: Vec<EditCommand> },
    Play { looping: Option<bool> },
    Stop,
    Undo,
    Redo,
    Save { path: Option<String> },
    Load { path: String },
    ExportWav { path: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ControlReply {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
}

#[derive(Debug, Deserialize)]
struct RawJsonRpcRequest {
    #[serde(default)]
    jsonrpc: Option<String>,
    #[serde(default)]
    id: Option<Value>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    params: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct ApplyCommandsParams {
    commands: Vec<EditCommand>,
}

#[derive(Debug, Deserialize)]
struct PlayParams {
    looping: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct SaveParams {
    path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LoadParams {
    path: String,
}

#[derive(Debug, Deserialize)]
struct ExportWavParams {
    path: String,
}

impl ControlServer {
    pub fn try_recv(&self) -> Option<PendingControlRequest> {
        self.receiver.try_recv().ok()
    }

    #[cfg(test)]
    fn recv_timeout(&self, timeout: std::time::Duration) -> Option<PendingControlRequest> {
        self.receiver.recv_timeout(timeout).ok()
    }
}

pub fn start_control_server(port: u16) -> Result<ControlServer> {
    let listener = TcpListener::bind(("127.0.0.1", port))
        .with_context(|| format!("binding DAW WebSocket control server on port {port}"))?;
    let addr = listener.local_addr()?.to_string();
    let (tx, rx) = mpsc::channel();

    thread::Builder::new()
        .name("daw-control-listener".to_owned())
        .spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => spawn_client(stream, tx.clone()),
                    Err(err) => eprintln!("control accept error: {err}"),
                }
            }
        })
        .context("spawning DAW control listener")?;

    Ok(ControlServer { addr, receiver: rx })
}

pub fn ok(id: Option<Value>, result: Value) -> ControlReply {
    ControlReply {
        jsonrpc: JSONRPC_VERSION.to_owned(),
        id: id.unwrap_or(Value::Null),
        result: Some(result),
        error: None,
    }
}

pub fn error(id: Option<Value>, error: impl ToString) -> ControlReply {
    error_with_code(id, APP_ERROR, error)
}

fn error_with_code(id: Option<Value>, code: i32, error: impl ToString) -> ControlReply {
    ControlReply {
        jsonrpc: JSONRPC_VERSION.to_owned(),
        id: id.unwrap_or(Value::Null),
        result: None,
        error: Some(JsonRpcError {
            code,
            message: error.to_string(),
        }),
    }
}

fn spawn_client(stream: TcpStream, tx: Sender<PendingControlRequest>) {
    let _ = thread::Builder::new()
        .name("daw-control-client".to_owned())
        .spawn(move || {
            if let Err(err) = handle_client(stream, tx) {
                eprintln!("control client error: {err}");
            }
        });
}

fn handle_client(stream: TcpStream, tx: Sender<PendingControlRequest>) -> Result<()> {
    let mut websocket = accept(stream).context("accepting WebSocket control client")?;

    loop {
        let message = match websocket.read() {
            Ok(message) => message,
            Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => break,
            Err(err) => return Err(err.into()),
        };

        let text = match message {
            Message::Text(text) => text.to_string(),
            Message::Close(_) => break,
            Message::Ping(payload) => {
                websocket.send(Message::Pong(payload))?;
                continue;
            }
            Message::Pong(_) => continue,
            Message::Binary(_) | Message::Frame(_) => {
                websocket.send(Message::text(serde_json::to_string(&error_with_code(
                    None,
                    INVALID_REQUEST,
                    "JSON-RPC requests must be text WebSocket messages",
                ))?))?;
                continue;
            }
        };

        let envelope = match parse_json_rpc_request(&text) {
            Ok(envelope) => envelope,
            Err(reply) => {
                websocket.send(Message::text(serde_json::to_string(&reply)?))?;
                continue;
            }
        };

        let id = envelope.id.clone();
        if id.is_none() {
            if tx
                .send(PendingControlRequest {
                    envelope,
                    reply: None,
                })
                .is_err()
            {
                bail!("DAW UI is not accepting control requests");
            }
            continue;
        }

        let (reply_tx, reply_rx) = mpsc::channel();
        if tx
            .send(PendingControlRequest {
                envelope,
                reply: Some(reply_tx),
            })
            .is_err()
        {
            websocket.send(Message::text(serde_json::to_string(&error_with_code(
                id,
                APP_ERROR,
                "DAW UI is not accepting control requests",
            ))?))?;
            continue;
        }

        match reply_rx.recv() {
            Ok(reply) => websocket.send(Message::text(serde_json::to_string(&reply)?))?,
            Err(_) => websocket.send(Message::text(serde_json::to_string(&error_with_code(
                id,
                APP_ERROR,
                "DAW UI closed before replying",
            ))?))?,
        }
    }

    Ok(())
}

fn parse_json_rpc_request(text: &str) -> std::result::Result<ControlEnvelope, ControlReply> {
    let raw: RawJsonRpcRequest = serde_json::from_str(text)
        .map_err(|err| error_with_code(None, PARSE_ERROR, format!("parse error: {err}")))?;
    if raw.jsonrpc.as_deref() != Some(JSONRPC_VERSION) {
        return Err(error_with_code(
            raw.id,
            INVALID_REQUEST,
            "jsonrpc must be \"2.0\"",
        ));
    }
    let Some(method) = raw.method.as_deref() else {
        return Err(error_with_code(
            raw.id,
            INVALID_REQUEST,
            "method is required",
        ));
    };

    let request = match method {
        "get_summary" => {
            ensure_no_params(&raw)?;
            ControlRequest::GetSummary
        }
        "get_project" => {
            ensure_no_params(&raw)?;
            ControlRequest::GetProject
        }
        "apply_commands" => {
            let params: ApplyCommandsParams = parse_params(&raw)?;
            ControlRequest::ApplyCommands {
                commands: params.commands,
            }
        }
        "play" => {
            let params: PlayParams = parse_params_or_default(&raw)?;
            ControlRequest::Play {
                looping: params.looping,
            }
        }
        "stop" => {
            ensure_no_params(&raw)?;
            ControlRequest::Stop
        }
        "undo" => {
            ensure_no_params(&raw)?;
            ControlRequest::Undo
        }
        "redo" => {
            ensure_no_params(&raw)?;
            ControlRequest::Redo
        }
        "save" => {
            let params: SaveParams = parse_params_or_default(&raw)?;
            ControlRequest::Save { path: params.path }
        }
        "load" => {
            let params: LoadParams = parse_params(&raw)?;
            ControlRequest::Load { path: params.path }
        }
        "export_wav" => {
            let params: ExportWavParams = parse_params(&raw)?;
            ControlRequest::ExportWav { path: params.path }
        }
        _ => {
            return Err(error_with_code(
                raw.id,
                METHOD_NOT_FOUND,
                format!("unknown method {method}"),
            ));
        }
    };

    Ok(ControlEnvelope {
        id: raw.id,
        request,
    })
}

fn ensure_no_params(raw: &RawJsonRpcRequest) -> std::result::Result<(), ControlReply> {
    match &raw.params {
        None => Ok(()),
        Some(Value::Object(map)) if map.is_empty() => Ok(()),
        Some(Value::Array(values)) if values.is_empty() => Ok(()),
        Some(_) => Err(error_with_code(
            raw.id.clone(),
            INVALID_PARAMS,
            format!(
                "method {} does not accept params",
                raw.method.as_deref().unwrap_or("<missing>")
            ),
        )),
    }
}

fn parse_params<T: DeserializeOwned>(
    raw: &RawJsonRpcRequest,
) -> std::result::Result<T, ControlReply> {
    let params = raw.params.clone().ok_or_else(|| {
        error_with_code(
            raw.id.clone(),
            INVALID_PARAMS,
            format!(
                "method {} requires params",
                raw.method.as_deref().unwrap_or("<missing>")
            ),
        )
    })?;
    serde_json::from_value(params).map_err(|err| {
        error_with_code(
            raw.id.clone(),
            INVALID_PARAMS,
            format!(
                "invalid params for {}: {err}",
                raw.method.as_deref().unwrap_or("<missing>")
            ),
        )
    })
}

fn parse_params_or_default<T: DeserializeOwned>(
    raw: &RawJsonRpcRequest,
) -> std::result::Result<T, ControlReply> {
    let params = raw.params.clone().unwrap_or_else(|| json!({}));
    serde_json::from_value(params).map_err(|err| {
        error_with_code(
            raw.id.clone(),
            INVALID_PARAMS,
            format!(
                "invalid params for {}: {err}",
                raw.method.as_deref().unwrap_or("<missing>")
            ),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpStream;
    use std::time::Duration;
    use tungstenite::{Message, client};

    #[test]
    fn parses_apply_commands_request() {
        let raw = r#"{"jsonrpc":"2.0","id":"1","method":"apply_commands","params":{"commands":[{"action":"set_tempo","bpm":124.0}]}}"#;
        let request = parse_json_rpc_request(raw).unwrap();
        assert_eq!(request.id, Some(json!("1")));
        assert!(matches!(
            request.request,
            ControlRequest::ApplyCommands { ref commands } if commands.len() == 1
        ));
    }

    #[test]
    fn rejects_non_json_rpc_request() {
        let raw = r#"{"id":"1","method":"get_summary"}"#;
        let reply = parse_json_rpc_request(raw).unwrap_err();
        assert_eq!(reply.error.unwrap().code, INVALID_REQUEST);
    }

    #[test]
    fn control_server_moves_json_rpc_between_websocket_and_ui() {
        let server = start_control_server(0).unwrap();
        let stream = TcpStream::connect(&server.addr).unwrap();
        let (mut client, _) = client(format!("ws://{}", server.addr), stream).unwrap();
        client
            .send(Message::text(
                r#"{"jsonrpc":"2.0","id":"summary","method":"get_summary"}"#,
            ))
            .unwrap();

        let pending = server.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(pending.envelope.id, Some(json!("summary")));
        assert!(matches!(
            pending.envelope.request,
            ControlRequest::GetSummary
        ));
        pending
            .reply
            .unwrap()
            .send(ok(Some(json!("summary")), json!({"summary":"empty"})))
            .unwrap();

        let reply = client.read().unwrap();
        let Message::Text(text) = reply else {
            panic!("expected text response");
        };
        let reply: ControlReply = serde_json::from_str(&text).unwrap();
        assert_eq!(reply.jsonrpc, JSONRPC_VERSION);
        assert_eq!(reply.id, json!("summary"));
        assert_eq!(reply.result, Some(json!({"summary":"empty"})));
        assert!(reply.error.is_none());
    }
}
