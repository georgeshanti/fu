use std::{
    io::{self, Read, Write},
    net::{TcpListener, TcpStream},
    sync::mpsc::{self, Receiver, Sender},
    thread,
};
use bevy::ecs::event::Event;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tungstenite::{Message, WebSocket, handshake::derive_accept_key, protocol::Role};

/// Address the game server listens on. Plain HTTP, no TLS.
const LISTEN_ADDR: &str = "0.0.0.0:8765";

/// The one path that speaks the game protocol. Every other path is answered as
/// an ordinary HTTP request, which is why the listener parses the request head
/// itself instead of blindly running a WebSocket handshake on the socket.
pub const WS_PATH: &str = "/ws";

/// Ceiling on the request head we are willing to buffer. Without it a client
/// that opens a socket and never sends the terminating blank line would make us
/// allocate forever.
const MAX_HEAD_BYTES: usize = 8 * 1024;

/// Read granularity while slurping the request head. A WebSocket handshake from
/// a browser is comfortably under 1 KiB, so this is normally a single read.
const HEAD_CHUNK_BYTES: usize = 1024;

/// Header slots handed to `httparse`. Browsers send around a dozen; 32 leaves
/// room for proxies to add their own without us rejecting the request.
const MAX_HEADERS: usize = 32;

/// Starts an HTTP server on port 8765 and spawns a thread that accepts
/// connections on it.
///
/// Each connection is handled on its own thread: the request head is parsed,
/// and requests for [`WS_PATH`] are upgraded to WebSocket while everything else
/// is served as a normal HTTP response (see [`http_route`]).
///
/// For each upgraded connection, two more threads are spawned: a reader that
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
    let listener = TcpListener::bind(LISTEN_ADDR).expect("failed to bind to port 8765");
    let (request_sender, request_receiver) = mpsc::channel::<Request>();
    let (client_sender, client_receiver) = mpsc::channel::<(Option<Id>, Sender<Response>)>();
    let (kill_sender, _kill_receiver) = mpsc::channel::<()>();
    thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    // Handle the request off the accept loop. Parsing the head
                    // means blocking on a read, and a client that connects but
                    // never speaks must not stall everyone else's joins.
                    let request_sender = request_sender.clone();
                    let client_sender = client_sender.clone();
                    thread::spawn(move || {
                        handle_connection(stream, request_sender, client_sender);
                    });
                }
                Err(e) => eprintln!("connection failed: {e}"),
            }
        }
    });
    (request_receiver, client_receiver, kill_sender)
}

/// Serves one connection: parse the HTTP request, then either upgrade it to a
/// WebSocket or answer it as plain HTTP and hang up.
fn handle_connection<Request, Response, Id>(
    mut stream: TcpStream,
    request_sender: Sender<Request>,
    client_sender: Sender<(Option<Id>, Sender<Response>)>,
) where
    Request: DeserializeOwned + Send + 'static,
    Response: Serialize + Send + 'static,
    Id: DeserializeOwned + Send + 'static,
{
    let head = match read_http_head(&mut stream) {
        Ok(head) => head,
        Err(e) => {
            eprintln!("bad http request: {e}");
            let _ = write_http_response(&mut stream, 400, "text/plain; charset=utf-8", "bad request\n");
            return;
        }
    };

    // Anything that is not a WebSocket handshake for our endpoint is an
    // ordinary HTTP request and gets an ordinary HTTP response.
    if !(head.method == "GET" && head.path == WS_PATH && head.is_websocket_upgrade()) {
        let (status, content_type, body) = http_route(&head);
        if let Err(e) = write_http_response(&mut stream, status, content_type, &body) {
            eprintln!("failed to write http response: {e}");
        }
        return;
    }

    // RFC 6455 §4.2.1: the key is mandatory and the accept header is derived
    // from it, so a handshake without one cannot be completed.
    let Some(key) = head.header("sec-websocket-key") else {
        let _ = write_http_response(
            &mut stream,
            400,
            "text/plain; charset=utf-8",
            "missing sec-websocket-key\n",
        );
        return;
    };
    let accept_key = derive_accept_key(key);

    // We only speak version 13. Anything else is told so explicitly rather than
    // left to fail on a malformed frame later.
    match head.header("sec-websocket-version") {
        Some(b"13") => {}
        _ => {
            let _ = write_http_response(
                &mut stream,
                426,
                "text/plain; charset=utf-8",
                "this endpoint requires sec-websocket-version: 13\n",
            );
            return;
        }
    }

    if let Err(e) = write_upgrade_response(&mut stream, &accept_key) {
        eprintln!("failed to write upgrade response: {e}");
        return;
    }

    // Split the connection into independent read/write halves by cloning the
    // underlying TCP stream (TCP is full-duplex).
    let write_stream = match stream.try_clone() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("failed to clone stream: {e}");
            return;
        }
    };
    // A well-behaved client waits for the 101 before sending frames, but if any
    // bytes did arrive alongside the head they belong to the WebSocket now.
    let mut reader = WebSocket::from_partially_read(stream, head.leftover, Role::Server, None);
    let mut writer = WebSocket::from_raw_socket(write_stream, Role::Server, None);

    // First frame after the upgrade identifies the client.
    let client_id = match reader.read() {
        Ok(Message::Text(txt)) => match serde_json::from_str::<Handshake<Id>>(&txt) {
            Ok(req) => req.id,
            Err(e) => {
                eprintln!("failed to deserialize handshake: {e}");
                return;
            }
        },
        Ok(Message::Close(_)) => return,
        Ok(_) => return, // ignore binary/ping/pong for now
        Err(e) => {
            eprintln!("ws read error: {e}");
            return;
        }
    };

    // Per-connection response channel.
    let (response_sender, response_receiver) = mpsc::channel::<Response>();

    // Reader thread: ws frame -> Request -> request_sender.
    thread::spawn(move || loop {
        match reader.read() {
            Ok(Message::Text(txt)) => match serde_json::from_str::<Request>(&txt) {
                Ok(req) => {
                    if request_sender.send(req).is_err() {
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
    let _ = client_sender.send((client_id, response_sender));
}

/// Responses for plain HTTP requests, i.e. everything that is not a WebSocket
/// handshake on [`WS_PATH`]. Add new routes here.
fn http_route(head: &HttpHead) -> (u16, &'static str, String) {
    match (head.method.as_str(), head.path.as_str()) {
        ("GET", "/") => (
            200,
            "text/plain; charset=utf-8",
            format!("fu game server\nwebsocket endpoint: {WS_PATH}\n"),
        ),
        // A plain GET on the game endpoint: almost always a browser or a curl
        // poking at the URL, so say what it is instead of 404ing.
        ("GET", WS_PATH) => (
            426,
            "text/plain; charset=utf-8",
            "this endpoint is a websocket endpoint\n".to_string(),
        ),
        _ => (404, "text/plain; charset=utf-8", "not found\n".to_string()),
    }
}

/// The parts of an inbound HTTP request this layer cares about, owned so the
/// borrowed `httparse` view can be dropped before the connection is upgraded.
struct HttpHead {
    method: String,
    path: String,
    /// Header names lowercased, since HTTP header names are case-insensitive
    /// and clients disagree about casing.
    headers: Vec<(String, Vec<u8>)>,
    /// Bytes already read past the blank line ending the head. These belong to
    /// whatever protocol takes the connection over next.
    leftover: Vec<u8>,
}

impl HttpHead {
    fn header(&self, name: &str) -> Option<&[u8]> {
        self.headers
            .iter()
            .find(|(header, _)| header == name)
            .map(|(_, value)| value.as_slice())
    }

    /// True when the request asks to switch protocols to WebSocket
    /// (RFC 6455 §4.2.1). `Connection` is a comma-separated list of tokens, and
    /// both tokens are matched case-insensitively.
    fn is_websocket_upgrade(&self) -> bool {
        let upgrade_to_websocket = self
            .header("upgrade")
            .is_some_and(|value| value.eq_ignore_ascii_case(b"websocket"));
        let connection_upgrade = self.header("connection").is_some_and(|value| {
            String::from_utf8_lossy(value)
                .split(',')
                .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
        });
        upgrade_to_websocket && connection_upgrade
    }
}

/// Reads and parses the HTTP request head from `stream`.
///
/// Reads until the blank line that terminates the head, so nothing is consumed
/// past it beyond what a single read may have over-fetched — and that surplus
/// is returned in [`HttpHead::leftover`] rather than being dropped on the floor.
fn read_http_head(stream: &mut TcpStream) -> Result<HttpHead, String> {
    let mut buf: Vec<u8> = Vec::with_capacity(HEAD_CHUNK_BYTES);
    let mut chunk = [0u8; HEAD_CHUNK_BYTES];
    let head_len = loop {
        if let Some(end) = find_head_end(&buf) {
            break end;
        }
        if buf.len() >= MAX_HEAD_BYTES {
            return Err(format!("request head exceeded {MAX_HEAD_BYTES} bytes"));
        }
        match stream.read(&mut chunk) {
            Ok(0) => return Err("connection closed before the request head was complete".to_string()),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) => return Err(format!("read error: {e}")),
        }
    };

    let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut request = httparse::Request::new(&mut headers);
    match request.parse(&buf[..head_len]) {
        Ok(httparse::Status::Complete(_)) => {}
        // The head is terminated, so `httparse` only reports `Partial` if it is
        // malformed in a way it cannot commit to rejecting.
        Ok(httparse::Status::Partial) => return Err("incomplete request head".to_string()),
        Err(e) => return Err(format!("malformed request: {e}")),
    }

    let method = request.method.ok_or("request has no method")?.to_string();
    let path = request.path.ok_or("request has no path")?;
    // Strip any query string: routing here is on the path alone.
    let path = path.split('?').next().unwrap_or(path).to_string();
    let headers = request
        .headers
        .iter()
        .map(|header| (header.name.to_ascii_lowercase(), header.value.to_vec()))
        .collect();

    Ok(HttpHead { method, path, headers, leftover: buf[head_len..].to_vec() })
}

/// Returns the length of the request head, including the terminating blank
/// line, once `buf` contains one.
fn find_head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|start| start + 4)
}

/// Writes the 101 that completes the WebSocket handshake. After this the socket
/// carries WebSocket frames, not HTTP.
fn write_upgrade_response(stream: &mut TcpStream, accept_key: &str) -> io::Result<()> {
    let response = format!(
        "HTTP/1.1 101 Switching Protocols\r\n\
         upgrade: websocket\r\n\
         connection: Upgrade\r\n\
         sec-websocket-accept: {accept_key}\r\n\
         \r\n"
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

/// Writes an ordinary HTTP response and closes the connection. `content-length`
/// is always set so clients need not rely on the close to frame the body.
fn write_http_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &str,
) -> io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        426 => "Upgrade Required",
        _ => "",
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         content-type: {content_type}\r\n\
         content-length: {}\r\n\
         connection: close\r\n\
         \r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
}
