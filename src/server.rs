use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::{Arc, LazyLock, Mutex, mpsc::{self, Receiver, Sender}}, thread::{self, JoinHandle}};

use crate::connection::server::create_server;

pub static GAME_SERVER: Mutex<Option<GameServer>> = Mutex::new(None);
pub static CLIENT_EVENT_SENDER: Mutex<Option<Sender<ClientEvent>>> = Mutex::new(None);

/// Creates a new game server and stores it in the global `GAME_SERVER`.
/// Returns the sender clients use to push events to this server.
pub fn create_game_server() {
    let mut game_server = GAME_SERVER.lock().unwrap();
    if let Some(_) = *game_server {
        return;
    }
    let (server, client_event_sender) = GameServer::new();
    println!("Here1");
    *game_server = Some(server);
    drop(game_server);
    *CLIENT_EVENT_SENDER.lock().unwrap() = Some(client_event_sender.clone());
    println!("Here2");

    // Network server: inbound = ClientEvent, outbound = ServerEvent.
    let (request_receiver, client_receiver, _kill_sender) =
        create_server::<ClientEvent, ServerEvent, u8>();

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
        while let Ok((client_id, client_sender)) = client_receiver.recv() {
            GAME_SERVER.lock().unwrap().as_mut().unwrap().attach_sender(client_sender, client_id);
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
    /// Roster of every player currently connected to the game server.
    LobbyInfo { players: Vec<Player> },
    /// Sent to a freshly-connected client to inform it of its assigned id.
    ClientRegistered { client_id: u8 },
}

/// Events originating from a client, sent to the server.
#[derive(Event, Debug, Clone, Serialize, Deserialize)]
pub enum ClientEvent {
    Movement { player_id: u8, x: f32, y: f32 },
    /// Registers a player with the given name and chosen input controller.
    JoinLobby { client_id: u8, name: String, controller: Controller },
    /// Asks the server to reply with the current lobby roster (`LobbyInfo`).
    FetchLobby,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Controller {
    Keyboard,
    Gamepad(u32),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Player {
    pub id: u8,
    pub client_id: u8,
    pub name: String,
    pub controller: Controller,
}

/// Server-side hub: maintains game state
pub struct GameServer {
    pub phase: GamePhase,
    pub players: Arc<Mutex<Vec<Player>>>,
    /// Senders keyed by client id, used to push events out to each client.
    /// The tuple's first element is the next-id counter.
    pub clients: Arc<Mutex<(u8, BTreeMap<u8, Sender<ServerEvent>>)>>,
    /// Channel of inbound events arriving from clients.
    pub receiver: Arc<Mutex<Receiver<ClientEvent>>>,
}

impl GameServer {
    pub fn new() -> (Self, Sender<ClientEvent>) {
        let (sender, receiver) = mpsc::channel();
        let server = GameServer {
            phase: GamePhase::Lobby,
            players: Arc::new(Mutex::new(Vec::new())),
            clients: Arc::new(Mutex::new((0, BTreeMap::new()))),
            receiver: Arc::new(Mutex::new(receiver)),
        };
        (server, sender)
    }

    pub fn attach_sender(&mut self, sender: Sender<ServerEvent>, client_id: Option<u8>) {
        let mut clients = self.clients.lock().unwrap();
        match client_id {
            Some(id) => {
                clients.1.insert(id, sender);
            }
            None => {
                let id = clients.0;
                clients.0 += 1;
                let _ = sender.send(ServerEvent::ClientRegistered { client_id: id });
                clients.1.insert(id, sender);
            }
        }
    }

    pub fn start_server(&mut self) -> JoinHandle<()> {
        let receiver = self.receiver.clone();
        let clients = self.clients.clone();
        let players = self.players.clone();
        thread::spawn(move || {
            let receiver = receiver.lock().unwrap();
            loop {
                let event = receiver.recv().unwrap();
                let clients = clients.lock().unwrap();
                match event {
                    ClientEvent::Movement{player_id, x, y} => {
                        for client in clients.1.values() {
                            let _ = client.send(ServerEvent::Movement { player_id, x, y });
                        }
                    }
                    ClientEvent::JoinLobby { client_id, name, controller } => {
                        let roster = {
                            let mut players = players.lock().unwrap();
                            let id = players.len() as u8;
                            players.push(Player { id, client_id, name, controller });
                            players.clone()
                        };
                        for client in clients.1.values() {
                            let _ = client.send(ServerEvent::LobbyInfo { players: roster.clone() });
                        }
                    }
                    ClientEvent::FetchLobby => {
                        let roster = players.lock().unwrap().clone();
                        for client in clients.1.values() {
                            let _ = client.send(ServerEvent::LobbyInfo { players: roster.clone() });
                        }
                    }
                }
            }
        })
    }
}