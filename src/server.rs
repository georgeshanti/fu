use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::{sync::{Arc, LazyLock, Mutex, mpsc::{self, Receiver, Sender}}, thread::{self, JoinHandle}};

use crate::connection::server::create_server;

pub static GAME_SERVER: Mutex<Option<GameServer>> = Mutex::new(None);
pub static CLIENT_EVENT_SENDER: Mutex<Option<Sender<ClientEvent>>> = Mutex::new(None);

/// Creates a new game server and stores it in the global `GAME_SERVER`.
/// Returns the sender clients use to push events to this server.
pub fn create_game_server() {
    let (mut server, client_event_sender) = GameServer::new();
    println!("Here1");
    *GAME_SERVER.lock().unwrap() = Some(server);
    *CLIENT_EVENT_SENDER.lock().unwrap() = Some(client_event_sender.clone());
    println!("Here2");

    // Network server: inbound = ClientEvent, outbound = ServerEvent.
    let (request_receiver, client_receiver, _kill_sender) =
        create_server::<ClientEvent, ServerEvent>();

    // Thread 1: forward inbound network requests into the game server's channel,
    // i.e. the sender handed back by GameServer::new().
    thread::spawn(move || {
        while let Ok(event) = request_receiver.recv() {
            if client_event_sender.send(event).is_err() {
                break; // game server channel closed
            }
        }
    });

    // Thread 2: register each newly-connected client's response sender with the
    // game server (the equivalent of attach_sender).
    thread::spawn(move || {
        while let Ok(client_sender) = client_receiver.recv() {
            GAME_SERVER.lock().unwrap().as_mut().unwrap().attach_sender(client_sender);
        }
    });

    GAME_SERVER.lock().unwrap().as_mut().unwrap().start_server();
}

/// Returns `true` if a game server instance currently exists in the global slot.
pub fn is_game_server_running() -> bool {
    GAME_SERVER.lock().unwrap().is_some()
}

enum GamePhase {
    Lobby,
    RoundStarting,
    RoundPlaying,
    RoundEnded,
}

/// Events originating from the server, sent out to clients.
#[derive(Event, Debug, Clone, Serialize, Deserialize)]
pub enum ServerEvent {
    Movement { player_id: u8, x: f32, y: f32 },
}

/// Events originating from a client, sent to the server.
#[derive(Event, Debug, Clone, Serialize, Deserialize)]
pub enum ClientEvent {
    Movement { player_id: u8, x: f32, y: f32 },
}

struct Player {
    id: u8,
    name: String,
}

/// Server-side hub: maintains game state
pub struct GameServer {
    pub phase: GamePhase,
    pub players: Vec<Player>,
    /// One sender per connected client, used to push events out to each client.
    pub clients: Arc<Mutex<Vec<Sender<ServerEvent>>>>,
    /// Channel of inbound events arriving from clients.
    pub receiver: Arc<Mutex<Receiver<ClientEvent>>>,
}

impl GameServer {
    pub fn new() -> (Self, Sender<ClientEvent>) {
        let (sender, receiver) = mpsc::channel();
        let server = GameServer {
            phase: GamePhase::Lobby,
            players: vec![],
            clients: Arc::new(Mutex::new(Vec::new())),
            receiver: Arc::new(Mutex::new(receiver)),
        };
        (server, sender)
    }

    pub fn attach_sender(&mut self, sender: Sender<ServerEvent>) {
        self.clients.lock().unwrap().push(sender);
    }

    pub fn start_server(&mut self) -> JoinHandle<()> {
        let receiver = self.receiver.clone();
        let clients = self.clients.clone();
        thread::spawn(move || {
            let receiver = receiver.lock().unwrap();
            loop {
                let event = receiver.recv().unwrap();
                let clients = clients.lock().unwrap();
                match event {
                    ClientEvent::Movement{player_id, x, y} => {
                        for client in clients.iter() {
                            let _ = client.send(ServerEvent::Movement { player_id, x, y });
                        }
                    }
                }
            }
        })
    }
}