use std::{net::TcpStream, sync::mpsc::{self, Receiver, Sender}, thread};
use serde::{Serialize, de::DeserializeOwned};
use tungstenite::{connect, protocol::Role, stream::MaybeTlsStream, Message, WebSocket};

use crate::connection::server::{Handshake, WS_PATH};

/// Opens a WebSocket connection to `address` (expected as `hostname:port`) and
/// spawns two threads to pump traffic in each direction.
///
/// The connection is established with an ordinary HTTP request to [`WS_PATH`]
/// that the server upgrades; the server answers its other paths as plain HTTP.
///
/// A reader thread deserializes inbound frames into `Response`s and forwards
/// them on the response channel, and a writer thread serializes `Request`s from
/// the request channel and sends them over the socket. The caller is handed the
/// sender half of the request channel and the receiver half of the response
/// channel, mirroring the abstraction exposed by `create_server`.
pub fn create_client<Response, Request, Id>(address: String, id: Option<Id>)
    -> (Sender<Request>, Receiver<Response>)
where
    Request: Serialize + Send + 'static,
    Response: DeserializeOwned + Send + 'static,
    Id: Serialize + Send + 'static,
{
    let url = build_ws_url(&address);
    // `connect` performs the HTTP handshake and fails unless the server answers
    // 101 Switching Protocols, so a wrong path or a non-game server is caught
    // here rather than surfacing later as a framing error.
    let (websocket, _response) = connect(&url).unwrap_or_else(|e| panic!("failed to connect to {url}: {e}"));

    let (request_sender, request_receiver) = mpsc::channel::<Request>();
    let (response_sender, response_receiver) = mpsc::channel::<Response>();

    // Split the connection into independent read/write halves by cloning the
    // underlying TCP stream (TCP is full-duplex). Only plain `ws://` is used, so
    // the stream is always the `Plain` variant.
    let write_stream = match websocket.get_ref() {
        MaybeTlsStream::Plain(s) => s.try_clone().expect("failed to clone stream"),
        _ => panic!("only plain ws:// connections are supported"),
    };
    let mut reader = websocket;
    let mut writer = WebSocket::from_raw_socket(
        MaybeTlsStream::Plain(write_stream),
        Role::Client,
        None,
    );

    // Reader thread: ws frame -> Response -> response_sender.
    thread::spawn(move || loop {
        match reader.read() {
            Ok(Message::Text(txt)) => match serde_json::from_str::<Response>(&txt) {
                Ok(resp) => {
                    if response_sender.send(resp).is_err() {
                        break;
                    }
                }
                Err(e) => eprintln!("failed to deserialize response: {e}"),
            },
            Ok(Message::Close(_)) => break,
            Ok(_) => {} // ignore binary/ping/pong for now
            Err(e) => {
                eprintln!("ws read error: {e}");
                break;
            }
        }
    });

    let h = Handshake::<Id> {id: id};
    match serde_json::to_string(&h) {
        Ok(json) => {
            if let Err(e) = writer.send(Message::Text(json)) {
                eprintln!("failed to send handshake: {e}");
            }
        }
        Err(e) => eprintln!("failed to serialize request: {e}"),
    }

    // Writer thread: request_receiver -> Request -> ws frame.
    thread::spawn(move || {
        while let Ok(req) = request_receiver.recv() {
            match serde_json::to_string(&req) {
                Ok(json) => {
                    if writer.send(Message::Text(json)).is_err() {
                        break;
                    }
                }
                Err(e) => eprintln!("failed to serialize request: {e}"),
            }
        }
    });

    (request_sender, response_receiver)
}

/// Turns what the player typed on the join screen into a full WebSocket URL.
///
/// The join screen asks for a bare `host:port`, which gets the game's
/// [`WS_PATH`] appended — the server's other paths are plain HTTP and would not
/// upgrade. Accepting a full URL costs nothing and lets a player paste one,
/// including an `http(s)://` one copied out of a browser.
fn build_ws_url(address: &str) -> String {
    let address = address.trim().trim_end_matches('/');
    let url = match address.split_once("://") {
        Some(("http", rest)) => format!("ws://{rest}"),
        Some(("https", rest)) => format!("wss://{rest}"),
        Some(_) => address.to_string(), // already ws:// or wss://
        None => format!("ws://{address}"),
    };
    // A path the player spelled out themselves wins over the default endpoint.
    let has_path = url
        .split_once("://")
        .is_some_and(|(_, rest)| rest.contains('/'));
    if has_path { url } else { format!("{url}{WS_PATH}") }
}
