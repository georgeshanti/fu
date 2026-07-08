use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::{Arc, LazyLock, Mutex, mpsc::{self, Receiver, Sender}}, thread::{self, JoinHandle}};

use crate::connection::server::create_server;

pub static GAME_SERVER: Mutex<Option<GameServer>> = Mutex::new(None);
pub static CLIENT_EVENT_SENDER: Mutex<Option<Sender<ClientEvent>>> = Mutex::new(None);

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct OrderedF32(pub f32);

impl Eq for OrderedF32 {}

impl PartialEq for OrderedF32 {
    fn eq(&self, other: &Self) -> bool {
        match (self.0.is_nan(), other.0.is_nan()) {
            (true, true) => true,
            (true, false) => false,
            (false, true) => false,
            (false, false) => self.0 == other.0,
        }
    }
}

impl PartialOrd for OrderedF32 {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OrderedF32 {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match (self.0.is_nan(), other.0.is_nan()) {
            (true, true) => std::cmp::Ordering::Equal,
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            (false, false) => if self.0 == other.0 {
                std::cmp::Ordering::Equal
            } else if self.0 < other.0 {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            },
        }
    }
}

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

/// Entry point for `--server`: starts the game server and blocks the main
/// thread forever, since all server work happens on background threads.
pub fn run_dedicated_server() {
    create_game_server();
    println!("Dedicated server listening on 0.0.0.0:8765");
    loop {
        thread::park();
    }
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
    /// Roster of every player currently connected to the game server.
    LobbyInfo { players: Vec<Player> },
    /// Sent to a freshly-connected client to inform it of its assigned id.
    ClientRegistered { client_id: u8 },
    /// Round is starting; carries each player and their initial spawn location.
    SpawnPlayers { spawns: Vec<(Player, Vec3)> },
    /// Players have been spawned by all clients and now the round may start.
    StartRound,
    PlayerAction {tick: u64, game_event: PlayerAction},
    GameEffect {tick: u64, game_event: GameEffect},
}

/// Events originating from a client, sent to the server.
#[derive(Event, Debug, Clone, Serialize, Deserialize)]
pub enum ClientEvent {
    /// Registers a player with the given name and chosen input controller.
    JoinLobby { client_id: u8, name: String, controller: Controller },
    /// Asks the server to reply with the current lobby roster (`LobbyInfo`).
    FetchLobby,
    /// Asks the server to begin the round (sent from the lobby "Start Game" button).
    StartGame,
    /// Sent once a client has finished spawning the platform and its players.
    PlayersSpawned { client_id: u8 },
    PlayerAction {tick: u64, game_event: PlayerAction},
    GameEffect {tick: u64, game_event: GameEffect},
}

#[derive(Event, Debug, Clone, Serialize, Deserialize, PartialEq, PartialOrd, Eq, Ord)]
pub enum PlayerAction {
    Movement { player_id: u8, x: OrderedF32, y: OrderedF32 },
    /// A player pressed their swing input; starts a boomerang swing for that player.
    Swing { player_id: u8 },
}

#[derive(Event, Debug, Clone, Serialize, Deserialize, PartialEq, PartialOrd, Eq, Ord)]
pub enum GameEffect {
    /// A striker's boomerang hit another player; carries both player ids.
    StrikePlayer { striker_id: u8, struck_id: u8 },
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
    pub alive: bool,
}

/// Server-side hub: maintains game state
pub struct GameServer {
    pub phase: Arc<Mutex<GamePhase>>,
    pub players: Arc<Mutex<Vec<Player>>>,
    /// Senders keyed by client id, used to push events out to each client.
    /// The tuple's first element is the next-id counter.
    pub clients: Arc<Mutex<(u8, BTreeMap<u8, Sender<ServerEvent>>)>>,
    /// Client ids that have reported PlayersSpawned for the current round.
    pub pending_client_starts: Arc<Mutex<Vec<u8>>>,
    /// Channel of inbound events arriving from clients.
    pub receiver: Arc<Mutex<Receiver<ClientEvent>>>,
}

impl GameServer {
    pub fn new() -> (Self, Sender<ClientEvent>) {
        let (sender, receiver) = mpsc::channel();
        let server = GameServer {
            phase: Arc::new(Mutex::new(GamePhase::Lobby)),
            players: Arc::new(Mutex::new(Vec::new())),
            clients: Arc::new(Mutex::new((0, BTreeMap::new()))),
            pending_client_starts: Arc::new(Mutex::new(Vec::new())),
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
        let phase = self.phase.clone();
        let pending_client_starts = self.pending_client_starts.clone();
        thread::spawn(move || {
            let receiver = receiver.lock().unwrap();
            loop {
                let event = receiver.recv().unwrap();
                let clients = clients.lock().unwrap();
                println!("Got client event: {:?}", event);
                match event {
                    ClientEvent::JoinLobby { client_id, name, controller } => {
                        let roster = {
                            let mut players = players.lock().unwrap();
                            let id = players.len() as u8;
                            players.push(Player { id, client_id, name, controller, alive: true });
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
                    ClientEvent::StartGame => {
                        *phase.lock().unwrap() = GamePhase::RoundStarting;
                        pending_client_starts.lock().unwrap().clear();
                        let roster = players.lock().unwrap().clone();
                        let n = roster.len();
                        let spawns: Vec<(Player, Vec3)> = roster
                            .into_iter()
                            .enumerate()
                            .map(|(i, player)| {
                                // Evenly spaced along the X axis, 2 units apart, centered on origin.
                                let x = (i as f32 - (n as f32 - 1.0) / 2.0) * 2.0;
                                (player, Vec3::new(x, 6.0, 0.0))
                            })
                            .collect();
                        for client in clients.1.values() {
                            let _ = client.send(ServerEvent::SpawnPlayers { spawns: spawns.clone() });
                        }
                    }
                    ClientEvent::PlayersSpawned { client_id } => {
                        let mut pending = pending_client_starts.lock().unwrap();
                        if !pending.contains(&client_id) {
                            pending.push(client_id);
                        }
                        if pending.len() >= clients.1.len() {
                            *phase.lock().unwrap() = GamePhase::RoundPlaying;
                            for client in clients.1.values() {
                                let _ = client.send(ServerEvent::StartRound);
                            }
                            pending.clear();
                        }
                    }
                    ClientEvent::PlayerAction { tick, game_event } => {
                        match game_event {
                            PlayerAction::Movement{player_id, x, y} => {
                                for client in clients.1.values() {
                                    let _ = client.send(ServerEvent::PlayerAction { tick, game_event: PlayerAction::Movement { player_id, x, y }});
                                }
                            }
                            PlayerAction::Swing { player_id } => {
                                for client in clients.1.values() {
                                    let _ = client.send(ServerEvent::PlayerAction { tick, game_event: PlayerAction::Swing { player_id }});
                                }
                            }
                        }
                    }
                    ClientEvent::GameEffect { tick, game_event } => {
                        match game_event {
                            GameEffect::StrikePlayer { struck_id, striker_id } => {
                                // Only relay on the alive->dead transition, so the many
                                // collision events a single swing produces collapse to one
                                // PlayerStriked broadcast.
                                let relay = {
                                    let mut players = players.lock().unwrap();
                                    match players.iter_mut().find(|p| p.id == struck_id) {
                                        Some(p) if p.alive => { p.alive = false; true }
                                        _ => false,
                                    }
                                };
                                if relay {
                                    for client in clients.1.values() {
                                        let _ = client.send(ServerEvent::GameEffect {tick, game_event: GameEffect::StrikePlayer { struck_id, striker_id }});
                                    }
                                }
                            }
                        }
                    }
                }
            }
        })
    }
}