use std::{net::TcpListener, sync::mpsc::{self, Receiver, Sender}, thread};
use bevy::ecs::event::Event;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tungstenite::{accept, protocol::Role, Message, WebSocket};

/// Starts a WebSocket server listening on port 8765 and spawns a thread
/// that accepts incoming connections on that port.
///
/// For each successful connection, two threads are spawned: a reader that
/// deserializes inbound frames into `Request`s and forwards them on the shared
/// request channel, and a writer that serializes `Response`s from a
/// per-connection channel and sends them back over the socket. The sender half
/// of that per-connection response channel is handed out on the client channel.


#[derive(Event, Debug, Clone, Serialize, Deserialize)]
pub struct Handshake<Id> {
    pub id: Option<Id>,
}

pub fn create_server<Request, Response, Id>()
    -> (Receiver<Request>, Receiver<(Option<Id>, Sender<Response>)>, Sender<()>)
where
    Request: DeserializeOwned + Send + 'static,
    Response: Serialize + Send + 'static,
    Id: DeserializeOwned + Send + 'static,
{
    let listener = TcpListener::bind("0.0.0.0:8765").expect("failed to bind to port 8765");
    let (request_sender, request_receiver) = mpsc::channel::<Request>();
    let (client_sender, client_receiver) = mpsc::channel::<(Option<Id>, Sender<Response>)>();
    let (kill_sender, _kill_receiver) = mpsc::channel::<()>();
    thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    // Perform the WebSocket handshake on the accepted TCP stream.
                    match accept(stream) {
                        Ok(websocket) => {


                            // Split the connection into independent read/write halves by
                            // cloning the underlying TCP stream (TCP is full-duplex).
                            let write_stream = match websocket.get_ref().try_clone() {
                                Ok(s) => s,
                                Err(e) => {
                                    eprintln!("failed to clone stream: {e}");
                                    continue;
                                }
                            };
                            let mut reader = websocket;

                            let client_id = {
                                match reader.read() {
                                    Ok(Message::Text(txt)) => match serde_json::from_str::<Handshake<Id>>(&txt) {
                                        Ok(req) => req.id,
                                        Err(e) => {eprintln!("failed to deserialize request: {e}"); break;},
                                    },
                                    Ok(Message::Close(_)) => break,
                                    Ok(_) => { break } // ignore binary/ping/pong for now
                                    Err(e) => {
                                        eprintln!("ws read error: {e}");
                                        break;
                                    }
                                }
                            };

                            let mut writer = WebSocket::from_raw_socket(write_stream, Role::Server, None);

                            // Per-connection response channel.
                            let (response_sender, response_receiver) = mpsc::channel::<Response>();

                            // Reader thread: ws frame -> Request -> request_sender.
                            let req_tx = request_sender.clone();
                            thread::spawn(move || loop {
                                match reader.read() {
                                    Ok(Message::Text(txt)) => match serde_json::from_str::<Request>(&txt) {
                                        Ok(req) => {
                                            if req_tx.send(req).is_err() {
                                                break;
                                            }
                                        }
                                        Err(e) => eprintln!("failed to deserialize request: {e}"),
                                    },
                                    Ok(Message::Close(_)) => break,
                                    Ok(_) => {} // ignore binary/ping/pong for now
                                    Err(e) => {
                                        eprintln!("ws read error: {e}");
                                        break;
                                    }
                                }
                            });

                            // Writer thread: response_receiver -> Response -> ws frame.
                            thread::spawn(move || {
                                while let Ok(resp) = response_receiver.recv() {
                                    match serde_json::to_string(&resp) {
                                        Ok(json) => {
                                            if writer.send(Message::Text(json)).is_err() {
                                                break;
                                            }
                                        }
                                        Err(e) => eprintln!("failed to serialize response: {e}"),
                                    }
                                }
                            });

                            // Hand the response sender to the consumer.
                            if client_sender.send((client_id, response_sender)).is_err() {
                                break;
                            }
                        }
                        Err(e) => eprintln!("websocket handshake failed: {e}"),
                    }
                }
                Err(e) => eprintln!("connection failed: {e}"),
            }
        }
    });
    (request_receiver, client_receiver, kill_sender)
}