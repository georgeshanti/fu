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


