use std::{net::TcpStream, sync::mpsc::{self, Receiver, Sender}, thread};
use serde::{Serialize, de::DeserializeOwned};
use tungstenite::{connect, protocol::Role, stream::MaybeTlsStream, Message, WebSocket};

use crate::connection::server::Handshake;

/// Opens a WebSocket connection to `address` (expected as `hostname:port`) and
/// spawns two threads to pump traffic in each direction.
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
    let url = format!("ws://{address}");
    let (websocket, _response) = connect(&url).expect("failed to connect to server");

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
            writer.send(Message::Text(json));
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
