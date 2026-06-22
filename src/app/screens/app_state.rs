use bevy::state::state::States;

/// High-level phase the game is in. The app starts on the menu; the 3D scene is
/// only built once we transition to `Playing`.
#[derive(States, Default, Debug, Clone, PartialEq, Eq, Hash)]
pub enum AppState {
    #[default]
    Menu,
    JoinGame,
    Lobby,
    SpawningPlayers,
    Playing,
}