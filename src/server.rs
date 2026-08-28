use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::{collections::{BTreeMap, BTreeSet}, sync::{Arc, LazyLock, Mutex, mpsc::{self, Receiver, Sender}}, thread::{self, JoinHandle, sleep}, time::Duration};

use crate::{connection::server::create_server, server::ServerEvent::GameStateRequest};

pub static GAME_SERVER: Mutex<Option<GameServer>> = Mutex::new(None);
pub static CLIENT_EVENT_SENDER: Mutex<Option<Sender<ClientEventOuter>>> = Mutex::new(None);

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
    let (request_receiver, client_receiver, _kill_sender) = create_server();

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

#[derive(PartialEq, Debug)]
enum GamePhase {
    Lobby,
    RoundStarting,
    RoundPlaying,
    RoundPaused,
    RoundEnded,
    GameEnded,
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub enum PlayerBoomerangState {
    Stationary,
    Swinging{elapsed: f32},
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub enum PlayerStatus {
    Alive,
    Dying { elapsed: f32 },
    Dead,
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub enum ThrowingState {
    StartThrow,
    Throwing{elapsed: f32},
}

/// A snapshot of one locally-controlled player's physics at a given tick.
#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub struct PlayerState {
    pub status: PlayerStatus,
    pub player_id: u8,
    pub color: Color,
    pub position: Vec3,
    pub velocity: Vec3,
    pub rotation: Quat,
    pub acceleration: Vec3,
    pub bommerang: Option<PlayerBoomerangState>,
    pub throwing_state: Option<ThrowingState>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ThrownBoomerangeState {
    pub player_id: Option<u8>,
    pub position: Vec3,
    pub velocity: Vec3,
    pub rotation: Quat,
    pub acceleration: Vec3,
    pub angular_veloctiy: Vec3,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GameState {
    pub players: Vec<PlayerState>,
    pub thrown_boomerangs: Vec<ThrownBoomerangeState>,
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
    GameStateRequest,
    OverrideGameState {tick: u64, game_state: GameState},
    RoundEnded{ max: u8, old_score: BTreeMap<u8, u8>, new_score: BTreeMap<u8, u8>},
    GameEnded{ max: u8, old_score: BTreeMap<u8, u8>, new_score: BTreeMap<u8, u8>, game_winners: Vec<Player> },
    BackToLobby,
}

#[derive(Event, Debug, Clone, Serialize, Deserialize)]
pub struct ClientEventOuter {
    pub client_id: u8,
    pub client_event_inner: ClientEvent,
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize)]
pub struct Color {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

/// Events originating from a client, sent to the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientEvent {
    /// Registers a player with the given name and chosen input controller.
    JoinLobby { client_id: u8, name: String, controller: Controller, color: Color },
    /// Asks the server to reply with the current lobby roster (`LobbyInfo`).
    FetchLobby,
    /// Asks the server to begin the round (sent from the lobby "Start Game" button).
    StartGame,
    /// Asks the server to reset its round state and start the next round (sent from
    /// the round-ended overlay's "Continue" button). Any client may send it; the
    /// server's phase guard collapses duplicates from several clients into one round.
    MoveToNextRound,
    /// Sent once a client has finished spawning the platform and its players.
    RoundPing{ tick: u64 },
    PlayersSpawned { client_id: u8 },
    PlayerAction {tick: u64, game_event: PlayerAction},
    GameEffect {tick: u64, game_event: GameEffect},
    UndoGameEffect {tick: u64, game_event: GameEffect},
    GameStateResponse {tick: u64, game_state: GameState},
    EndGame,
}

#[derive(Event, Debug, Clone, Serialize, Deserialize, PartialEq, PartialOrd, Eq, Ord)]
pub enum PlayerAction {
    Movement { player_id: u8, x: OrderedF32, y: OrderedF32 },
    Swing { player_id: u8 },
    Jump { player_id: u8, x: OrderedF32, y: OrderedF32  },
    StartingThrowing { player_id: u8, x: OrderedF32, y: OrderedF32  },
    // TurnThrow { player_id: u8, x: OrderedF32, y: OrderedF32  },
    ReleaseThrow { player_id: u8, power: OrderedF32, x: OrderedF32, y: OrderedF32  },
    StartingPulling { player_id: u8 },
    StoppingPulling { player_id: u8 },
}

#[derive(Event, Debug, Clone, Serialize, Deserialize, PartialEq, PartialOrd, Eq, Ord)]
pub enum GameEffect {
    /// A striker's boomerang hit another player; carries both player ids.
    StrikePlayer { striker_id: u8, struck_id: u8 },
    Parry { player_1_id: u8, player_2_id: u8 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Controller {
    Keyboard,
    Gamepad(u32),
}

#[derive(Ord, PartialEq, PartialOrd, Eq, Debug)]
struct PlayerDeathEvent {
    dead_player_id: u8,
    score_player_id: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Player {
    pub id: u8,
    pub client_id: u8,
    pub name: String,
    pub controller: Controller,
    pub alive: bool,
    pub color: Color,
}

/// Server-side hub: maintains game state
#[derive(Clone)]
pub struct GameServer {
    pub phase: Arc<Mutex<GamePhase>>,
    pub players: Arc<Mutex<Vec<Player>>>,
    /// Senders keyed by client id, used to push events out to each client.
    /// The tuple's first element is the next-id counter.
    pub clients: Arc<Mutex<(u8, BTreeMap<u8, Sender<ServerEvent>>)>>,
    /// Client ids that have reported PlayersSpawned for the current round.
    pub pending_client_starts: Arc<Mutex<Vec<u8>>>,
    /// Channel of inbound events arriving from clients.
    pub receiver: Arc<Mutex<Receiver<ClientEventOuter>>>,
    pub game_effects: Arc<Mutex<BTreeMap<u64, BTreeMap<GameEffect, BTreeSet<u8>>>>>,
    pub latest_tick_received: Arc<Mutex<Option<(u8, u64)>>>,
    pub clients_to_override: Arc<Mutex<Vec<u8>>>,
    pub player_death_events: Arc<Mutex<BTreeMap<u64, BTreeMap<PlayerDeathEvent, BTreeSet<u8>>>>>,
    pub active_clients_buffer: Arc<Mutex<BTreeSet<u8>>>,
    pub active_clients: Arc<Mutex<BTreeSet<u8>>>,
    pub alive_players: Arc<Mutex<BTreeSet<u8>>>,
    pub old_player_scores: Arc<Mutex<BTreeMap<u8, u8>>>,
    pub player_scores: Arc<Mutex<BTreeMap<u8, u8>>>,
}

trait MultipleSenders<T> {
    fn send(self: &Self, t: T);
}

impl MultipleSenders<()> for Vec<Sender<()>> {
    fn send(self: &Self, t: ()) {
        for sender in self {
            let _ = sender.send(t);
        }
    }
}

/// Pairs every player in the roster with the point it spawns at for a round:
/// evenly spaced along the X axis, 2 units apart, centered on the origin, and
/// dropped in from 6 units up. Shared by the first round (`StartGame`) and every
/// subsequent one (`MoveToNextRound`) so both lay the field out identically.
fn spawn_points(roster: Vec<Player>) -> Vec<(Player, Vec3)> {
    let n = roster.len();
    roster
        .into_iter()
        .enumerate()
        .map(|(i, player)| {
            let x = (i as f32 - (n as f32 - 1.0) / 2.0) * 2.0;
            (player, Vec3::new(x, 6.0, 0.0))
        })
        .collect()
}

impl GameServer {
    pub fn new() -> (Self, Sender<ClientEventOuter>) {
        let (sender, receiver) = mpsc::channel();
        let server = GameServer {
            phase: Arc::new(Mutex::new(GamePhase::Lobby)),
            players: Arc::new(Mutex::new(Vec::new())),
            clients: Arc::new(Mutex::new((0, BTreeMap::new()))),
            pending_client_starts: Arc::new(Mutex::new(Vec::new())),
            receiver: Arc::new(Mutex::new(receiver)),
            game_effects: Arc::new(Mutex::new(BTreeMap::new())),
            latest_tick_received: Arc::new(Mutex::new(None)),
            clients_to_override: Arc::new(Mutex::new(Vec::new())),
            player_death_events: Arc::new(Mutex::new(BTreeMap::new())),
            active_clients_buffer: Arc::new(Mutex::new(BTreeSet::new())),
            active_clients: Arc::new(Mutex::new(BTreeSet::new())),
            alive_players: Arc::new(Mutex::new(BTreeSet::new())),
            old_player_scores: Arc::new(Mutex::new(BTreeMap::new())),
            player_scores: Arc::new(Mutex::new(BTreeMap::new())),
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

    pub fn start_round_end_listener(self: &Self, senders: Vec<Sender<()>>) {
        let players = self.players.clone();
        let player_death_events = self.player_death_events.clone();
        let active_clients = self.active_clients.clone();
        let alive_players = self.alive_players.clone();
        let clients = self.clients.clone();
        let player_scores = self.player_scores.clone();
        let old_scores = self.old_player_scores.clone();
        let phase = self.phase.clone();
        thread::spawn(move || {
            loop {
                sleep(Duration::from_millis(250));
                let mut player_death_events = player_death_events.lock().unwrap();
                let mut ticks_to_remove: Vec<u64> = vec![];
                let mut alive_player_change = false;
                for player_death_event in player_death_events.iter() {
                    let mut acknowledge_dead_players_at_tick = true;
                    for player_death_event in player_death_event.1.iter() {
                        println!("player_death_event: {:?} {:?}", player_death_event.0, player_death_event.0);
                        let active_clients = active_clients.lock().unwrap();
                        if !active_clients.is_subset(player_death_event.1) {
                            acknowledge_dead_players_at_tick = false;
                        }
                    }
                    if acknowledge_dead_players_at_tick {
                        alive_player_change = true;
                        let mut alive_players = alive_players.lock().unwrap();
                        let mut player_scores = player_scores.lock().unwrap();
                        for player_death_event in player_death_event.1.iter() {
                            alive_players.remove(&player_death_event.0.dead_player_id);
                            let player_score = {
                                let mut player_score = player_scores.get(&player_death_event.0.score_player_id);
                                **player_score.get_or_insert(&0)
                            };
                            println!("Incrementing score for player: {} to {}", player_death_event.0.score_player_id, player_score+1);
                            player_scores.insert(player_death_event.0.score_player_id, player_score+1);
                        }
                        ticks_to_remove.push(*player_death_event.0);
                    } else {
                        break;
                    }
                }
                for tick_to_remove in ticks_to_remove {
                    player_death_events.remove(&tick_to_remove);
                }
                alive_player_change = false;
                if alive_player_change {
                    let winning_score = 2;
                    let alive_players = alive_players.lock().unwrap();
                    if alive_players.len() <=1 {
                        senders.send(());
                        for client in clients.lock().unwrap().1.iter() {
                            let winners: Vec<(u8, u8)> = player_scores.lock().unwrap().clone().iter().filter(|score| {
                                *score.1 >= winning_score
                            }).map(|score| {(*score.0, *score.1)}).collect();
                            let winners = if winners.len() > 0 {
                                let players= players.lock().unwrap();
                                winners.iter().map(|winner| {
                                    players.iter().filter(|player| { player.id == winner.0 }).map(|p| { p.clone() }).collect::<Vec<Player>>().get(0).unwrap().clone()
                                }).collect::<Vec<Player>>()
                            } else {
                                vec![]
                            };
                            let mut phase = phase.lock().unwrap();
                            if winners.len() > 0 {
                                let _ = client.1.send(ServerEvent::GameEnded {
                                    max: winning_score,
                                    old_score: old_scores.lock().unwrap().clone(),
                                    new_score: player_scores.lock().unwrap().clone(),
                                    game_winners: winners,
                                });
                                *phase = GamePhase::GameEnded;
                            } else {
                                let _ = client.1.send(ServerEvent::RoundEnded{
                                    max: winning_score,
                                    old_score: old_scores.lock().unwrap().clone(),
                                    new_score: player_scores.lock().unwrap().clone(),
                                });
                                *phase = GamePhase::RoundEnded;
                            }
                        }
                        // One listener per round: a fresh one is spawned on the next
                        // `PlayersSpawned`. Without this return the round-1 thread would
                        // still be polling during round 2 and race the new listener into
                        // a second `RoundEnded` broadcast.
                        return;
                    }
                }
            }
        });
    }

    pub fn monitor_active_clients(self: &Self, receiver: Receiver<()>) {
        let active_clients_buffer = self.active_clients_buffer.clone();
        let active_clients = self.active_clients.clone();
        thread::spawn(move || {
            loop {
                let res = receiver.recv_timeout(Duration::from_millis(500));
                if res.is_ok() {
                    return;
                }
                sleep(Duration::from_millis(500));
                let mut active_clients_buffer = active_clients_buffer.lock().unwrap();
                *(active_clients.lock().unwrap()) = active_clients_buffer.clone();
                *active_clients_buffer = BTreeSet::new();
            }
        });
    }

    pub fn start_server(&mut self) -> JoinHandle<()> {
        let receiver = self.receiver.clone();
        let clients = self.clients.clone();
        let clients2 = self.clients.clone();
        let players = self.players.clone();
        let phase = self.phase.clone();
        let pending_client_starts = self.pending_client_starts.clone();
        let game_effects = self.game_effects.clone();
        let last_received_tick = self.latest_tick_received.clone();
        let clients_to_override = self.clients_to_override.clone();
        let player_death_events = self.player_death_events.clone();
        let active_clients = self.active_clients_buffer.clone();
        let alive_players = self.alive_players.clone();
        let player_scores = self.player_scores.clone();
        let old_player_scores = self.old_player_scores.clone();
        let server = self.clone();

        // Event Handler
        thread::spawn(move || {
            let mut round_enders: Vec<Sender<()>> = Vec::new();
            let receiver = receiver.lock().unwrap();
            loop {
                let event = receiver.recv().unwrap();
                let clients_guard = clients.lock().unwrap();
                let mut phase = phase.lock().unwrap();
                match event.client_event_inner {
                    ClientEvent::JoinLobby { client_id, name, controller, color } => {
                        if *phase != GamePhase::Lobby { continue };
                        let roster = {
                            let mut players = players.lock().unwrap();
                            let id = players.len() as u8;
                            players.push(Player { id, client_id, name, controller, alive: true, color });
                            players.clone()
                        };
                        for client in clients_guard.1.values() {
                            let _ = client.send(ServerEvent::LobbyInfo { players: roster.clone() });
                        }
                    }
                    ClientEvent::FetchLobby => {
                        if *phase != GamePhase::Lobby { continue };
                        let roster = players.lock().unwrap().clone();
                        for client in clients_guard.1.values() {
                            let _ = client.send(ServerEvent::LobbyInfo { players: roster.clone() });
                        }
                    }
                    ClientEvent::StartGame => {
                        if *phase != GamePhase::Lobby { continue };
                        let players = players.lock().unwrap();
                        // if players.len() <= 1 {
                        //     continue;
                        // }
                        *phase = GamePhase::RoundStarting;
                        pending_client_starts.lock().unwrap().clear();
                        let spawns = spawn_points(players.clone());
                        for client in clients_guard.1.values() {
                            let _ = client.send(ServerEvent::SpawnPlayers { spawns: spawns.clone() });
                        }
                    }
                    ClientEvent::MoveToNextRound => {
                        // The phase guard doubles as the dedupe: the first client's
                        // "Continue" flips us out of `RoundPlaying`, so any others
                        // arriving for the same round are dropped here.
                        println!("Moving to next round from: {:?}", *phase);
                        if *phase != GamePhase::RoundEnded { continue };
                        *phase = GamePhase::RoundStarting;

                        // Carry this round's totals forward as the next round's starting
                        // point, so its `RoundEnded` scoreboard animates from them rather
                        // than replaying every point from zero.
                        *old_player_scores.lock().unwrap() = player_scores.lock().unwrap().clone();

                        // Clients restart their `Ticker` at 0 in `wait_for_start`, so the
                        // tick-keyed buffers from the finished round must not survive into
                        // the next one — their old ticks would collide with the new ones.
                        pending_client_starts.lock().unwrap().clear();
                        game_effects.lock().unwrap().clear();
                        player_death_events.lock().unwrap().clear();
                        *last_received_tick.lock().unwrap() = None;

                        // `alive_players` is deliberately left alone: the `PlayersSpawned`
                        // arm below repopulates it from the roster once every client has
                        // rebuilt the scene.
                        let spawns = spawn_points(players.lock().unwrap().clone());
                        for client in clients_guard.1.values() {
                            let _ = client.send(ServerEvent::SpawnPlayers { spawns: spawns.clone() });
                        }
                    }
                    ClientEvent::EndGame => {
                        // The phase guard doubles as the dedupe: the first client's
                        // "Continue" flips us out of `RoundPlaying`, so any others
                        // arriving for the same round are dropped here.
                        if *phase != GamePhase::GameEnded { continue };
                        *phase = GamePhase::Lobby;

                        // Carry this round's totals forward as the next round's starting
                        // point, so its `RoundEnded` scoreboard animates from them rather
                        // than replaying every point from zero.
                        *old_player_scores.lock().unwrap() = BTreeMap::new();
                        *player_scores.lock().unwrap() = BTreeMap::new();

                        // Clients restart their `Ticker` at 0 in `wait_for_start`, so the
                        // tick-keyed buffers from the finished round must not survive into
                        // the next one — their old ticks would collide with the new ones.
                        pending_client_starts.lock().unwrap().clear();
                        game_effects.lock().unwrap().clear();
                        player_death_events.lock().unwrap().clear();
                        *last_received_tick.lock().unwrap() = None;
                        alive_players.lock().unwrap().clear();

                        let roster = players.lock().unwrap().clone();
                        for client in clients_guard.1.values() {
                            let _ = client.send(ServerEvent::BackToLobby);
                        }
                    }
                    ClientEvent::PlayersSpawned { client_id } => {
                        if *phase != GamePhase::RoundStarting { continue };
                        let mut pending = pending_client_starts.lock().unwrap();
                        if !pending.contains(&client_id) {
                            pending.push(client_id);
                        }
                        if pending.len() >= clients_guard.1.len() {
                            {
                                let mut alive_players = alive_players.lock().unwrap();
                                *alive_players = players.lock().unwrap().iter().map(|player| { player.id }).collect();
                            }
                            {
                                let (sender, receiver) = mpsc::channel();
                                // Previous rounds' monitors have already been stopped, so
                                // drop their senders rather than pulsing dead channels.
                                round_enders.clear();
                                round_enders.push(sender.clone());
                                server.monitor_active_clients(receiver);

                                server.start_round_end_listener(round_enders.clone())
                            }
                            *phase = GamePhase::RoundPlaying;
                            for client in clients_guard.1.values() {
                                let _ = client.send(ServerEvent::StartRound);
                            }
                            pending.clear();
                        }
                    }
                    ClientEvent::PlayerAction { tick, game_event } => {
                        if *phase != GamePhase::RoundPlaying { continue };
                        {
                            let mut last_received_tick = last_received_tick.lock().unwrap();
                            if last_received_tick.is_none() {
                                *last_received_tick = Some((event.client_id, tick))
                            } else if let Some((last_client_id, last_tick)) = *last_received_tick {
                                if last_tick < tick {
                                    *last_received_tick = Some((event.client_id, tick))
                                } else if last_tick > tick + 1000 {
                                    {
                                        let mut clients_to_override = clients_to_override.lock().unwrap();
                                        clients_to_override.push(event.client_id);
                                        let clients = clients.lock().unwrap();
                                        clients.1.get(&last_client_id).unwrap().send(GameStateRequest).unwrap();
                                    }
                                }
                            }
                        }
                        let clients = clients2.clone();
                        std::thread::spawn(move || {
                            // sleep(Duration::from_millis(50));
                            let clients = clients.lock().unwrap();
                            for client in clients.1.values() {
                                let _ = client.send(ServerEvent::PlayerAction { tick, game_event: game_event.clone() });
                            }
                        });
                    }
                    ClientEvent::GameEffect { tick, game_event } => {
                        println!("tick: {}, Received game effect: {:?}", tick, game_event);
                        if *phase != GamePhase::RoundPlaying { continue };
                        let mut game_effects = game_effects.lock().unwrap();
                        if !game_effects.contains_key(&tick) {
                            game_effects.insert(tick, BTreeMap::new());
                        }
                        if !game_effects.get(&tick).unwrap().contains_key(&game_event) {
                            game_effects.get_mut(&tick).unwrap().insert(game_event.clone(), BTreeSet::new());
                        }
                        game_effects.get_mut(&tick).unwrap().get_mut(&game_event).unwrap().insert(event.client_id);
                        match game_event {
                            GameEffect::StrikePlayer { struck_id, striker_id } => {
                                {
                                    let mut player_death_events = player_death_events.lock().unwrap();
                                    if !player_death_events.contains_key(&tick) {
                                        player_death_events.insert(tick, BTreeMap::new());
                                    }
                                    if !player_death_events.get(&tick).unwrap().contains_key(&PlayerDeathEvent { dead_player_id: struck_id, score_player_id: striker_id }) {
                                        player_death_events.get_mut(&tick).unwrap().insert(PlayerDeathEvent { dead_player_id: struck_id, score_player_id: striker_id }, BTreeSet::new());
                                    }
                                    player_death_events.get_mut(&tick).unwrap().get_mut(&PlayerDeathEvent { dead_player_id: struck_id, score_player_id: striker_id }).unwrap().insert(event.client_id);
                                }
                                for client in clients_guard.1.values() {
                                    let _ = client.send(ServerEvent::GameEffect {tick, game_event: GameEffect::StrikePlayer { struck_id, striker_id }});
                                }
                            },
                            GameEffect::Parry { player_1_id, player_2_id } => {
                                
                            }
                        }
                    }
                    ClientEvent::UndoGameEffect { tick, game_event } => {
                        if *phase != GamePhase::RoundPlaying { continue };
                        let mut game_effects = game_effects.lock().unwrap();
                        if game_effects.contains_key(&tick) {
                            if game_effects.get(&tick).unwrap().contains_key(&game_event) {
                                game_effects.get_mut(&tick).unwrap().get_mut(&game_event).unwrap().remove(&event.client_id);
                                if game_effects.get_mut(&tick).unwrap().get(&game_event).unwrap().is_empty() {
                                    game_effects.get_mut(&tick).unwrap().remove(&game_event);
                                }
                                if game_effects.get(&tick).unwrap().is_empty() {
                                    game_effects.remove(&tick);
                                }
                            }
                        }
                    }
                    ClientEvent::GameStateResponse { tick, game_state } => {
                        let mut clients_to_override = clients_to_override.lock().unwrap();
                        let clients = clients.lock().unwrap();
                        for client_id in clients_to_override.iter() {
                            clients.1.get(&client_id).unwrap().send(ServerEvent::OverrideGameState { tick, game_state: game_state.clone() }).unwrap();
                        }
                        clients_to_override.clear();
                    }
                    ClientEvent::RoundPing { tick } => {
                        active_clients.lock().unwrap().insert(event.client_id);
                    }
                }
            }
        })
    }
}