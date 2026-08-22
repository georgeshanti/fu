use std::{collections::{BTreeMap, BTreeSet}, f32::consts::PI, time::{Duration, SystemTime}};

use avian3d::{prelude::*};
use bevy::{ecs::{relationship::RelatedSpawnerCommands, system::SystemState}, prelude::*};

use crate::{
    app::{GameClientWrapper, screens::{app_state::AppState, lobby::PendingSpawns}}, server::{self, ClientEvent, Controller, GameEffect, GameState, OrderedF32, PlayerAction, PlayerBoomerangState, PlayerState, PlayerStatus, ServerEvent, ThrowingState, ThrownBoomerangeState},
};

/// Identifies an entity as a player-controlled body.
#[derive(Component)]
pub struct PlayerId {
    pub player_id: u8,
    pub color: server::Color,
}

struct PlayerInfo {
    pub player_id: u8,
    pub color: server::Color,
    pub name: String,
}

/// An L-shaped object held off a player's right side. Spawned as a child of the
/// player and anchored at the point where it meets the cube (the right face);
/// its two cuboid segments are children of this entity, positioned relative to
/// that anchor.
#[derive(Component)]
pub struct Boomerang;

/// Marks a collider entity as a boomerang blade segment (the L spine/foot).
#[derive(Component)]
pub struct BoomerangBlade;

/// Present on a player once struck; drives the shrink animation. Never removed —
/// a dead player stays on the field at half size.
#[derive(Component)]
pub struct Dying {
    elapsed: f32,
}

/// Present on a player once struck; drives the shrink animation. Never removed —
/// a dead player stays on the field at half size.
#[derive(Component)]
pub struct Alive;

/// Present on a player once struck; drives the shrink animation. Never removed —
/// a dead player stays on the field at half size.
#[derive(Component)]
pub struct Dead;

#[derive(Component)]
pub struct StartThrow;

#[derive(Component)]
pub struct ThrowingAnimation {
    elapsed: f32,
}

/// Physics collision layers. Living players and their blades stay on the implicit
/// `Default` layer; the platform gets its own layer so a `Dead` body can filter to
/// touch only the platform (and thus pass through every other player and boomerang).
#[derive(PhysicsLayer, Default, Clone, Copy)]
enum GameLayer {
    #[default]
    Default,
    Platform,
    Dead,
}

/// Total duration of the death shrink, in seconds, and the scale a dead body settles at.
const DEATH_DURATION: f32 = 0.4;
const DEAD_SCALE: f32 = 0.5;

/// Total duration of one swing (forward and back), in seconds.
const SWING_DURATION: f32 = 0.25;

/// Peak yaw of the swing. The spine rests along local +X and must reach the cube
/// front (local -Z). For `Quat::from_rotation_y(θ)`, local +X maps to
/// (cos θ, 0, -sin θ); reaching (0,0,-1) requires θ = +π/2. (A negative angle would
/// swing to the cube's back, +Z — wrong.)
const SWING_PEAK_ANGLE: f32 = std::f32::consts::FRAC_PI_2;

/// Present on an `LObject` only while a swing is animating; tracks elapsed time.
/// Removed (and rotation snapped to rest) when `elapsed >= SWING_DURATION`.
#[derive(Component)]
pub struct Swinging {
    elapsed: f32,
}

#[derive(Component)]
pub struct Thrown {player_id: Option<u8>}

/// Minimum delay after a swing ends before the player may swing again.
const SWING_COOLDOWN: std::time::Duration = std::time::Duration::from_millis(800);

#[derive(Resource)]
pub struct PlayerSwingCooldowns(BTreeMap<u8, std::time::SystemTime>);

/// Minimum delay after a swing ends before the player may swing again.
const JUMP_COOLDOWN: std::time::Duration = std::time::Duration::from_millis(900);

#[derive(Resource)]
pub struct PlayerJumpCooldowns(BTreeMap<u8, std::time::SystemTime>);

/// Horizontal movement speed of the player, in meters per second.
const PLAYER_SPEED: f32 = 5.0;

/// Minimum time between movement events when direction is unchanged (keep-alive heartbeat), in seconds.
const DIRECTION_EVENT_INTERVAL: u128 = 500;

/// Quantization factor for joystick axes: 2^7 / 2 = 64 levels per side,
/// giving 128 discrete steps across the clamped -1..1 range.
const DIRECTION_QUANTIZATION: f32 = 4.0;

/// Seconds to wait (showing the countdown overlay) before telling the server we're ready.
const COUNTDOWN_SECS: f32 = 3.0;

/// Remaining time on the pre-game countdown. Present only while counting down.
#[derive(Resource)]
pub struct Countdown {
    remaining: f32,
}

/// Counts `drain_server_events` invocations. Present only while `AppState::Playing`;
/// stamped onto outgoing `ClientEvent::GameEvent`s in place of the old hardcoded `tick: 0`.
#[derive(Resource, Default)]
pub struct Ticker(pub u64, pub bool);

/// Whether the client is currently replaying past ticks rather than playing live.
/// Present only while `AppState::Playing`; inserted alongside `Ticker`.
#[derive(Resource, Default)]
pub struct InReplay(pub bool);

#[derive(Clone)]
pub struct PlayerDirection {
    time: std::time::SystemTime,
    direction_facing: Vec3,
    velocity: Vec3,
}

#[derive(Resource, Default, Clone)]
pub struct PlayerDirections(BTreeMap<u8, PlayerDirection>);

#[derive(Resource, Default)]
pub struct PlayerInfos(Vec<PlayerInfo>);

#[derive(Clone)]
pub struct TickRecord {
    pub tick: u64,
    pub game_state: GameState,
    pub player_actions: BTreeSet<PlayerAction>,
    pub game_effects: BTreeSet<GameEffect>,
}

/// One entry per tick of local simulation recorded so far. Present only while
/// `AppState::Playing` (inserted alongside `Ticker`). Kept in non-decreasing `tick` order.
#[derive(Resource, Default)]
pub struct LocalGameEvents{
    pub base_tick: u64,
    pub game_events: Vec<TickRecord>
}

impl LocalGameEvents {
    /// Inserts a received (remote) `game_event` into the ledger keeping it ordered by `tick`.
    /// The snapshot is copied from the existing entry already recorded at that tick (the local
    /// state we simulated for it), or empty if none exists yet. The list is maintained in
    /// non-decreasing `tick` order, so a binary search finds the insertion point.
    pub fn insert_received_player_actions(&mut self, events: Vec<(u64, PlayerAction)>) -> Option<u64> {
        let mut lowest_tick = None;
        let mut pending_events: Vec<(u64, PlayerAction)> = vec![];
        for ticked_event in events {
            if let Some(current_lowest_tick) = lowest_tick {
                if current_lowest_tick < ticked_event.0 {
                    lowest_tick = Some(ticked_event.0);
                }
            } else {
                lowest_tick = Some(ticked_event.0);
            }
            self
                .game_events
                .iter_mut()
                .find(|e| e.tick == ticked_event.0)
                .map(|e| e.player_actions.insert(ticked_event.1))
                .unwrap_or_default();
            // Insert after any entries already at this tick, so the local same-tick snapshot
            // precedes the remote event.
        }
        lowest_tick
    }

    pub fn insert_received_game_effects(&mut self, events: Vec<(u64, GameEffect)>) -> Option<u64> {
        let mut lowest_tick = None;
        for ticked_event in events {
            if let Some(current_lowest_tick) = lowest_tick {
                if current_lowest_tick < ticked_event.0 {
                    lowest_tick = Some(ticked_event.0);
                }
            } else {
                lowest_tick = Some(ticked_event.0);
            }
            self
                .game_events
                .iter_mut()
                .find(|e| e.tick == ticked_event.0)
                .map(|e| e.game_effects.insert(ticked_event.1))
                .unwrap_or_default();
            // Insert after any entries already at this tick, so the local same-tick snapshot
            // precedes the remote event.
        }
        lowest_tick
    }

    pub fn add_state(&mut self, game_state: TickRecord) {
        self.game_events.push(game_state);
    }
}

/// Root node of the countdown overlay (despawned when the countdown ends).
#[derive(Component)]
pub struct CountdownOverlay;

/// The big number `Text` inside the overlay.
#[derive(Component)]
pub struct CountdownText;

/// Root node of the round-ended overlay (despawned on `OnExit(AppState::RoundEnded)`).
#[derive(Component)]
pub struct RoundEndedOverlay;

/// The "Continue" button on the round-ended overlay: starts the next round.
#[derive(Component)]
pub struct ContinueButton;

/// The static parts of the playfield — light and platform. Tagged so they can be
/// despawned between rounds; without that, re-entering `AppState::SpawningPlayers`
/// would stack a second copy of each on top of the first.
#[derive(Component)]
pub struct GamePlayRoot;

/// The gameplay `Camera3d`. Separate from `GamePlayRoot` only to follow the
/// `XxxRoot` + `XxxCamera` convention every other screen uses.
#[derive(Component)]
pub struct GamePlayCamera;

/// Fixed width of the scoreboard's name column, in logical pixels. Fixed (rather than
/// content-sized) so the point pips start at the same x on every row and read as columns.
const SCORE_NAME_COLUMN_WIDTH: f32 = 200.0;

/// Side length of one point pip. Square + `BorderRadius::MAX` (a radius of half the
/// smaller side) makes it a circle.
const SCORE_PIP_SIZE: f32 = 22.0;

/// Backing panel behind the scoreboard. Darker than the overlay's own 0.5 black so the
/// table reads as a distinct card against the frozen field showing through.
const SCORE_PANEL_BACKGROUND: Color = Color::srgba(0.0, 0.0, 0.0, 0.6);

#[derive(Resource)]
pub struct Score{
    time: SystemTime,
    max: u8,
    old_score: BTreeMap<u8, u8>,
    new_score: BTreeMap<u8, u8>,
    winners: Vec<u8>,
}

/// Constant linear acceleration applied to the player to overcome ground friction.
/// Derived from Coulomb friction: μ × g = 0.5 × 9.81 = 4.905 m/s²
const PLAYER_ACCEL: f32 = 4.905 * 2.0;

/// Builds the playfield when entering `AppState::SpawningPlayers`: a camera, a
/// light, the static platform, and one dynamic body per player from
/// `PendingSpawns` (delivered by the server's `SpawnPlayers` event). Once
/// everything is spawned, starts a short countdown overlay; `tick_countdown`
/// notifies the server with a bare `PlayersSpawned` event when it elapses.
///
/// Mirrors the scene set up by `app::setup`, but positions each player at the
/// spawn point the server assigned instead of a single hard-coded cube.
pub fn setup_game_play(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    spawns: Option<Res<PendingSpawns>>,
    players: Query<Entity, With<PlayerId>>,
    boomerangs: Query<Entity, (With<Boomerang>, With<Thrown>)>,
) {
    // Camera, positioned back and up, looking at the origin.
    commands.spawn((
        Camera3d::default(),
        // Transform::from_xyz(0.0, 12.0, 0.0).looking_at(Vec3::ZERO, Vec3::Y),
        Transform::from_xyz(0.0, 5.0, 12.0).looking_at(Vec3::ZERO, Vec3::Y),
        GamePlayCamera,
    ));

    // Directional light so the meshes are visible.
    commands.spawn((
        DirectionalLight {
            shadows_enabled: true,
            ..default()
        },
        Transform::from_xyz(4.0, 8.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
        GamePlayRoot,
    ));

    // Static platform (20 x 20).
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(20.0, 20.0))),
        MeshMaterial3d(materials.add(Color::srgb(0.3, 0.5, 0.3))),
        RigidBody::Static,
        Collider::cuboid(20.0, 0.01, 20.0),
        // Own layer so dead bodies can filter to collide with only the platform.
        CollisionLayers::new(GameLayer::Platform, LayerMask::ALL),
        Friction {
            static_coefficient: 1.0,
            dynamic_coefficient: 1.0,
            combine_rule: CoefficientCombine::Max,
        },
        GamePlayRoot,
    ));

    // One dynamic cube per player, at the spawn point assigned by the server.
    if let Some(spawns) = spawns {
        // Shared assets for the L-shaped object held off each player's right side.
        // The L lies flat in the horizontal (X-Z) plane, thin in Y, at cube mid-height.
        let game_state = GameState { players: spawns.0.iter().map(|(player, position)| {
            PlayerState {
                status: PlayerStatus::Alive,
                player_id: player.id,
                color: player.color,
                position: *position,
                velocity: Vec3::ZERO,
                rotation: Quat::from_rotation_x(0.0),
                acceleration: Vec3::ZERO,
                bommerang: Some(PlayerBoomerangState::Stationary),
                throwing_state: None,
            }
        }).collect(),
        thrown_boomerangs: vec![] };
        spawn_world(&mut commands, &Ticker(0, false), &mut materials, &mut meshes, players, boomerangs, game_state);
        commands.insert_resource(PlayerInfos(spawns.0.iter().map(|player| { PlayerInfo { player_id: player.0.id, color: player.0.color, name: player.0.name.clone()} }).collect()));
    }

    // Start the pre-game countdown; `PlayersSpawned` is sent once it elapses.
    commands.insert_resource(Countdown { remaining: COUNTDOWN_SECS });

    // Full-screen dimmed overlay with a centered countdown number.
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.5)),
            CountdownOverlay,
        ))
        .with_children(|parent| {
            parent.spawn((
                Text::new(COUNTDOWN_SECS.ceil().to_string()),
                TextFont { font_size: 120.0, ..default() },
                TextColor(Color::WHITE),
                CountdownText,
            ));
        });
}

/// Ticks the pre-game countdown each frame while in `AppState::SpawningPlayers`.
/// Updates the overlay number (3, 2, 1); when it reaches zero, notifies the
/// server with `PlayersSpawned` and tears the overlay down.
pub fn tick_countdown(
    time: Res<Time>,
    client: Res<GameClientWrapper>,
    countdown: Option<ResMut<Countdown>>,
    mut texts: Query<&mut Text, With<CountdownText>>,
) {
    let Some(mut countdown) = countdown else {
        return;
    };
    countdown.remaining -= time.delta_secs();

    if countdown.remaining >= 0.0 {
        // Show 3, 2, 1 — the ceiling of the remaining time.
        let label = (countdown.remaining.ceil() as i32).to_string();
        for mut text in &mut texts {
            if text.0 != label {
                text.0 = label.clone();
            }
        }
    }
    if countdown.remaining <= 0.0 {
        let client = client.client.read().unwrap();
        if let (Some(sender), Some(id)) = (&client.sender, *client.client_id.read().unwrap()) {
            sender.send(ClientEvent::PlayersSpawned { client_id: id }).ok();
        }
    }
}

pub fn wait_for_start(
    mut commands: Commands,
    client: Res<GameClientWrapper>,
    overlay: Query<Entity, With<CountdownOverlay>>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    let client = client.client.read().unwrap();
    let events = {
        let mut server_events = client.received_events.lock().unwrap();
        let events = server_events.clone();
        *server_events = vec![];
        events
    };
    for event in events {
        if let ServerEvent::StartRound = event {
            // Countdown finished: tell the server we're ready and remove the overlay.
            for entity in &overlay {
                commands.entity(entity).despawn();
            }
            commands.remove_resource::<Countdown>();
            commands.insert_resource(Ticker(0, false));
            commands.insert_resource(LocalGameEvents::default());
            commands.insert_resource(InReplay::default());
            commands.insert_resource(PlayerDirections(BTreeMap::new()));
            commands.insert_resource(PlayerJumpCooldowns(BTreeMap::new()));
            commands.insert_resource(PlayerSwingCooldowns(BTreeMap::new()));
            next_state.set(AppState::Playing);
        }
    }
}

trait ToBevyColor {
    fn to_bevy_color(self: &Self) -> Color;
}

impl ToBevyColor for server::Color {
    fn to_bevy_color(self: &Self) -> Color {
        Color::srgb(
            self.red as f32 / 256.0,
            self.green as f32 / 256.0,
            self.blue as f32 / 256.0,
        )
    }
}

#[derive(Component)]
pub struct RoundEndTitle;

#[derive(Component)]
pub struct PointRow {
    player_id: u8,
    color: server::Color,
}

#[derive(Component)]
pub struct PointIndex {
    index: u8,
}
/// Puts the round-ended overlay up on `OnEnter(AppState::RoundEnded)`.
///
/// The 3D scene is deliberately left standing: every `AppState::Playing` system
/// (movement, swings, strike detection, event draining) stops running on the state
/// change, so the field simply freezes as it was and this dimmed UI layer draws on
/// top of it — same trick as the pre-game countdown overlay.
pub fn start_round_end_animation(mut commands: Commands, score: Option<Res<Score>>, player_infos: Res<PlayerInfos>) {
    // The server ends the round as soon as at most one player is still alive, so a
    // lone survivor is the winner; anything else (a same-tick double KO, or a round
    // ended some other way) is reported as a draw.

    // Full-screen dimmed overlay with the result centered on it.
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                row_gap: Val::Px(16.0),
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.5)),
            RoundEndedOverlay,
        ))
        .with_children(|parent| {
            parent.spawn((
                Text::new("Round Over"),
                TextFont { font_size: 80.0, ..default() },
                TextColor(Color::WHITE),
                RoundEndTitle{},
            ));

            // Scoreboard, in its own translucent panel: one row per player, `max + 1`
            // columns — the name, then one pip per point of the round limit, filled up
            // to that player's score.
            let score = score.unwrap();
            parent
                .spawn((
                    Node {
                        flex_direction: FlexDirection::Column,
                        align_items: AlignItems::FlexStart,
                        row_gap: Val::Px(10.0),
                        padding: UiRect::axes(Val::Px(28.0), Val::Px(20.0)),
                        border_radius: BorderRadius::all(Val::Px(12.0)),
                        ..default()
                    },
                    BackgroundColor(SCORE_PANEL_BACKGROUND),
                ))
                .with_children(|table| {
                    for info in &player_infos.0 {
                        // A player with no entry in the map hasn't scored yet.
                        let points = score.old_score.get(&info.player_id).copied().unwrap_or(0);
                        table
                            .spawn((
                                Node {
                                    flex_direction: FlexDirection::Row,
                                    align_items: AlignItems::Center,
                                    column_gap: Val::Px(10.0),
                                    ..default()
                                },
                                PointRow {
                                    player_id: info.player_id,
                                    color: info.color,
                                },
                            ))
                            .with_children(|row| {
                                row.spawn((
                                    Text::new(info.name.clone()),
                                    TextFont { font_size: 28.0, ..default() },
                                    TextColor(info.color.to_bevy_color()),
                                    Node {
                                        width: Val::Px(SCORE_NAME_COLUMN_WIDTH),
                                        ..default()
                                    },
                                ));
                                for pip in 1..score.max+1 {
                                    row.spawn((
                                        PointIndex{index: pip},
                                        Node {
                                            width: Val::Px(SCORE_PIP_SIZE),
                                            height: Val::Px(SCORE_PIP_SIZE),
                                            border_radius: BorderRadius::MAX,
                                            ..default()
                                        },
                                        BackgroundColor(if pip <= points {
                                            info.color.to_bevy_color()
                                        } else {
                                            info.color.to_bevy_color().with_alpha(0.1)
                                        }),
                                    ));
                                }
                            });
                    }
                });
        });
}

pub fn setup_round_ended(
    mut commands: Commands,
    round_end_overlay: Query<Entity, With<RoundEndedOverlay>>,
    round_end_title: Query<&mut Text, With<RoundEndTitle>>,
    score: Res<Score>,
) {
    if score.winners.len() > 0 {
        for mut text in round_end_title {
            let mut winners = String::from("Winners:");
            let mut index = 0;
            for winner in score.winners.iter() {
                if index == 0 {
                    winners = format!("{} {},", winners, *winner);
                } else if index == score.winners.len() - 1 {
                    winners = format!("{} & {},", winners, *winner);
                } else {
                    winners = format!("{}, {},", winners, *winner);
                }
                index += 1;
            }
            *text = Text(winners);
        }
    }
    let game_ended = score.winners.len() > 0;
    for overlay in round_end_overlay {
        commands.entity(overlay).with_children(|parent| {
            // Action row, pinned to the bottom-right corner. The overlay root is itself
            // absolutely positioned at 100% x 100%, so this anchors against the whole
            // screen without disturbing the centered column above it. A flex row so more
            // buttons can sit alongside "Continue" later.
            parent
                .spawn(Node {
                    position_type: PositionType::Absolute,
                    bottom: Val::Px(24.0),
                    right: Val::Px(24.0),
                    flex_direction: FlexDirection::Row,
                    column_gap: Val::Px(12.0),
                    ..default()
                })
                .with_children(|row| {
                    row.spawn((
                        Button,
                        Node {
                            width: Val::Px(220.0),
                            height: Val::Px(60.0),
                            justify_content: JustifyContent::Center,
                            align_items: AlignItems::Center,
                            ..default()
                        },
                        BackgroundColor(Color::srgb(0.2, 0.2, 0.25)),
                        ContinueButton,
                    ))
                    .with_children(|button| {
                        button.spawn((Text::new(if game_ended { "End Game" } else { "Continue" }), TextColor(Color::WHITE)));
                    });
                });
            });
    }
}

const POINT_ANIMATION_TIME: Duration = Duration::from_millis(500);

pub fn animate_round_end(
    rows: Query<(&PointRow, &Children)>,
    mut points: Query<(&PointIndex, &mut BackgroundColor)>,
    mut score: ResMut<Score>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    if score.time.elapsed().unwrap() > POINT_ANIMATION_TIME {
        let player_ids: Vec<u8> = score.new_score.keys().map(|id| { *id }).collect();
        let mut index = 0;
        let player_id_score = loop {
            if index == player_ids.len() {
                break None;
            }
            let player_id = player_ids.get(index).unwrap();
            let new_player_score = *score.new_score.get(player_id).unwrap();
            if !score.old_score.contains_key(&player_id) {
                score.old_score.insert(*player_id, 0);
            }
            let old_player_score = score.old_score.get_mut(player_id).unwrap();
            if new_player_score != *old_player_score {
                *old_player_score += 1;
                break Some((*player_id, *old_player_score))
            }
            index += 1;
        };

        match player_id_score {
            Some((player_id, player_score)) => {
                for (point_row, children) in rows {
                    if point_row.player_id == player_id {
                        for child in children {
                            let Ok((point_index, mut background_color)) = points.get_mut(*child) else { continue };
                            if point_index.index == player_score {
                                *background_color = BackgroundColor(point_row.color.to_bevy_color());
                            }
                        }
                    }
                }
                score.time = SystemTime::now();
            },
            None => {
                next_state.set(AppState::RoundEnded);
            }
        };
    }
}

/// Asks the server for the next round when "Continue" is clicked. Any client may
/// press it; the server's phase guard collapses several clients' clicks into one
/// round, so no local dedupe is needed here.
pub fn handle_continue_button(
    interactions: Query<&Interaction, (Changed<Interaction>, With<ContinueButton>)>,
    client: Res<GameClientWrapper>,
    score: Res<Score>,
) {
    let event = if score.winners.len() > 0 {
        ClientEvent::EndGame
    } else {
        ClientEvent::MoveToNextRound
    };
    for interaction in &interactions {
        if *interaction == Interaction::Pressed {
            let client_guard = client.client.read().unwrap();
            if let Some(sender) = &client_guard.sender {
                sender.send(event.clone()).ok();
            }
        }
    }
}

/// Drains server events while the round-ended overlay is up. Any client's "Continue"
/// makes the server rebroadcast `SpawnPlayers`, which sends every client back through
/// `SpawningPlayers` for the next round — the same handoff `update_lobby` does for the
/// first round.
///
/// Deliberately not registered for `RoundEndAnimation`: an event arriving mid-animation
/// just sits in `received_events` until this runs, and keeping the transition inside
/// `RoundEnded` guarantees `cleanup_round_ended` (on `OnExit`) always tears the scene down.
pub fn wait_for_next_round(
    mut commands: Commands,
    client: Res<GameClientWrapper>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    let client = client.client.read().unwrap();
    let events = {
        let mut server_events = client.received_events.lock().unwrap();
        let events = server_events.clone();
        *server_events = vec![];
        events
    };
    for event in events {
        match event {
            ServerEvent::SpawnPlayers { spawns } => {
                commands.insert_resource(PendingSpawns(spawns));
                next_state.set(AppState::SpawningPlayers);
            }
            ServerEvent::BackToLobby => {
                // Leave the scene standing and hand over to `setup_round_ended`, which
                // draws the result overlay on top of the now-frozen field.
                next_state.set(AppState::Lobby);
                return;
            },
            _ => {},
        }
    }
}

/// Despawns the round-ended overlay *and the playfield* on `OnExit(AppState::RoundEnded)`.
///
/// The scene has to survive `Playing → RoundEndAnimation → RoundEnded` (the overlay draws
/// on top of the frozen field), so this is the first point at which it can go. Player
/// bodies are left to `spawn_world`, which despawns every `PlayerId` entity before
/// rebuilding them. `OnExit` runs before the next state's `OnEnter`, so `setup_game_play`
/// replaces the camera in the same state transition and no frame renders without one.
pub fn cleanup_round_ended(
    mut commands: Commands,
    overlay: Query<Entity, With<RoundEndedOverlay>>,
    scene: Query<Entity, Or<(With<GamePlayRoot>, With<GamePlayCamera>)>>,
) {
    for entity in &overlay {
        commands.entity(entity).despawn();
    }
    for entity in &scene {
        commands.entity(entity).despawn();
    }
}

pub fn drain_server_events(
    world: &mut World,
    // The queries and resources this system used to take as individual params are
    // now pulled out of the `World` via a cached `SystemState`. Keeping it in a
    // `Local` preserves the query/change-detection state across frames instead of
    // rebuilding it every call.
    mut params: Local<SystemState<(
        Commands,
        Res<GameClientWrapper>,
        Query<(Entity, &PlayerId, &mut LinearVelocity, &mut ConstantLinearAcceleration, &mut Transform, Option<&Children>), (With<PlayerId>, Without<Dying>, Without<Boomerang>)>,
        Query<(Entity, &Transform), (With<Boomerang>, Without<Swinging>)>,
        ResMut<Ticker>,
        ResMut<LocalGameEvents>,
        Query<Entity, With<PlayerId>>,
        Query<Entity, (With<Boomerang>, With<Thrown>)>,
        ResMut<Assets<Mesh>>,
        ResMut<Assets<StandardMaterial>>,
        ResMut<InReplay>,
        ResMut<NextState<AppState>>,
    )>>,
) {
    let (mut new_player_actions, server_events) = {
        let client = params.get_mut(world).1;
        let client = client.client.read().unwrap();
        let events = {
            let mut server_events = client.received_events.lock().unwrap();
            let events = server_events.clone();
            *server_events = vec![];
            events
        };
        let mut player_actions: Vec<(u64, PlayerAction)> = events.iter().filter_map(|event| { if let ServerEvent::PlayerAction{tick: tick, game_event: game_event} = event { Some((*tick, game_event.clone())) } else { None } }).collect();
        player_actions.sort_by(|a, b| {a.0.cmp(&b.0)});
        (player_actions, events)
    };
    for server_event in server_events {
        match server_event {
            ServerEvent::RoundEnded{ max, old_score, new_score} => {
                // Leave the scene standing and hand over to `setup_round_ended`, which
                // draws the result overlay on top of the now-frozen field.
                let (mut commands, _, _, _, _, _, _, _, _, _, _, mut next_state) = params.get_mut(world);
                commands.insert_resource(Score{time: SystemTime::now(), max, old_score, new_score, winners: vec![]});
                next_state.set(AppState::RoundEndAnimation);
                params.apply(world);
                return;
            },
            ServerEvent::GameEnded{ max, old_score, new_score, game_winners} => {
                // Leave the scene standing and hand over to `setup_round_ended`, which
                // draws the result overlay on top of the now-frozen field.
                let (mut commands, _, _, _, _, _, _, _, _, _, _, mut next_state) = params.get_mut(world);
                commands.insert_resource(Score{time: SystemTime::now(), max, old_score, new_score, winners: game_winners.iter().map(|winner| {winner.id}).collect()});
                next_state.set(AppState::RoundEndAnimation);
                params.apply(world);
                return;
            },
            _ => {}
        }
    }
    if !new_player_actions.is_empty() {
        let final_tick = {
            let ticker = params.get_mut(world).4;
            std::cmp::max(new_player_actions.last().unwrap().0, ticker.0)
        };
        let mut existing_records = {
            let first_tick = new_player_actions.first().unwrap().0;
            let (mut commands, _, _, _, mut ticker, mut local_game_events, players, boomerangs, mut meshes, mut materials, mut in_replay, _) = params.get_mut(world);
            let game_state = local_game_events.game_events.get(first_tick as usize).unwrap().game_state.clone();
            spawn_world(&mut commands, &ticker, &mut materials, &mut meshes, players, boomerangs, game_state);
            ticker.0 = first_tick;
            in_replay.0 = true;
            let existing_records = local_game_events.game_events[first_tick as usize..].to_vec();
            local_game_events.game_events.drain(((first_tick as usize)+1)..);
            existing_records
        };
        // Apply the despawn/respawn queued by `spawn_world` NOW. The replay loop below
        // queries entities and queues component inserts against them; without this flush
        // those queries would still match the old, about-to-be-despawned entities, and any
        // command targeting them (e.g. the `Swinging` insert) would be dropped when the
        // despawn finally landed.
        params.apply(world);
        let mut current_tick = {
            let (_, _, _, _, ticker, _, players, _, mut meshes, mut materials, mut in_replay, _) = params.get_mut(world);
            ticker.0
        };
        while current_tick <= final_tick {
            {
                while !new_player_actions.is_empty() && new_player_actions.first().unwrap().0 == current_tick {
                    let (mut commands, client, mut query, lobjects, mut ticker, mut sent_events, _, _, _, _, _, _) =
                        params.get_mut(world);
                    let first = new_player_actions.first().unwrap();
                    {
                        record_player_action(&client, &ticker, &mut sent_events, &first.1, false);
                    }
                    apply_action_to_world(&first.1, world, &mut params);
                    new_player_actions.remove(0);
                    params.apply(world);
                }
                {
                    if !existing_records.is_empty() {
                        for player_action in existing_records.first().unwrap().player_actions.iter() {
                            apply_action_to_world(player_action, world, &mut params);
                        }
                    }
                }
            }
            // Run the full physics frame, not just the inner solver step. Avian's
            // Transform->Position sync (Prepare), clock advancement, and
            // Position->Transform writeback all live in FixedPostUpdate around the
            // PhysicsSchedule; running PhysicsSchedule alone simulates nothing visible.
            world.run_schedule(FixedPostUpdate);
            params.apply(world);
            if !existing_records.is_empty() {
                let (_, client, _, _, _, mut local_game_events, _, _, _, _, _, _) = params.get_mut(world);
                let old_game_effects = existing_records.first().unwrap().game_effects.clone();
                let new_game_effects = local_game_events.game_events.get(current_tick as usize).unwrap().game_effects.clone();
                let missing_game_effects = new_game_effects.difference(&old_game_effects);
                for game_effect in missing_game_effects {
                    let _ = client.client.read().unwrap().sender.clone().unwrap().send(ClientEvent::GameEffect { tick: current_tick, game_event: game_effect.clone() });
                }
            }
            if !existing_records.is_empty() {
                existing_records.remove(0);
            }
            current_tick = {
                let (_, _, _, _, ticker, _, _, _, _, _, _, _) = params.get_mut(world);
                ticker.0
            };
        }
        {
            let (_, _, _, _, _, _, _, _, _, _, mut in_replay, _) = params.get_mut(world);
            in_replay.0 = false;
        }
        {
            params.apply(world);
        }
    }

    // Flush the entity insertions deferred through `commands`; a normal system does
    // this automatically, but an exclusive `&mut World` system must apply its own
    // `SystemState`.
}

fn apply_action_to_world(
    player_action: &PlayerAction,
    world: &mut World,
    // The queries and resources this system used to take as individual params are
    // now pulled out of the `World` via a cached `SystemState`. Keeping it in a
    // `Local` preserves the query/change-detection state across frames instead of
    // rebuilding it every call.
    params: &mut Local<SystemState<(
        Commands,
        Res<GameClientWrapper>,
        Query<(Entity, &PlayerId, &mut LinearVelocity, &mut ConstantLinearAcceleration, &mut Transform, Option<&Children>), (With<PlayerId>, Without<Dying>, Without<Boomerang>)>,
        Query<(Entity, &Transform), (With<Boomerang>, Without<Swinging>)>,
        ResMut<Ticker>,
        ResMut<LocalGameEvents>,
        Query<Entity, With<PlayerId>>,
        Query<Entity, (With<Boomerang>, With<Thrown>)>,
        ResMut<Assets<Mesh>>,
        ResMut<Assets<StandardMaterial>>,
        ResMut<InReplay>,
        ResMut<NextState<AppState>>,
    )>>,
) {
    let (mut commands, client, mut query, lobjects, mut ticker, mut sent_events, _, _, _, _, _, _) =
        params.get_mut(world);
    match player_action {
        PlayerAction::Movement { player_id, x, y } => {
            for (_entity, player, mut vel, mut accel, mut transform, _) in &mut query {
                if player.player_id == *player_id {
                    vel.x = x.0;
                    vel.z = y.0;
                    *accel = ConstantLinearAcceleration(Vec3::new(x.0, 0.0, y.0).normalize_or_zero() * PLAYER_ACCEL);
                    // Point the player (and its anchored L) toward the movement
                    // direction. Forward is -Z, so yaw = atan2(-x, -z). Leave the
                    // facing unchanged when stationary.
                    //
                    // Set BOTH the physics `Rotation` (source of truth used by
                    // collision/broadphase, i.e. where the blades are) AND the
                    // render `Transform`. If only `Rotation` were set, the next
                    // `run_schedule(PhysicsSchedule)` below would reconcile it
                    // against the stale `Transform` and clobber the new facing.
                    if Vec3::new(x.0, 0.0, y.0).length_squared() > 1e-6 {
                        let yaw = Quat::from_rotation_y(f32::atan2(-x.0, -y.0));
                        transform.rotation = yaw;
                    }
                }
            }
        },
        PlayerAction::Swing { player_id } => {
            for (_entity, entity_player_id, mut vel, mut accel, mut transform, children) in &mut query {
                if entity_player_id.player_id != *player_id { continue; }
                if let Some(children) = children {
                    for child in children.iter() {
                        if let Ok((boomerang, _)) = lobjects.get(child) {
                            *vel = LinearVelocity(Vec3::ZERO);
                            *accel = ConstantLinearAcceleration(Vec3::ZERO);
                            commands.entity(boomerang).insert(Swinging { elapsed: 0.0 });
                        }
                    }
                }
            }
        }
        PlayerAction::Jump { player_id, x, y } => {
            for (_entity, player, mut vel, mut accel, mut transform, _) in &mut query {
                if player.player_id == *player_id {
                    vel.x = x.0;
                    vel.z = y.0;
                    vel.y = 2.0;
                    *accel = ConstantLinearAcceleration(Vec3::ZERO);
                    if Vec3::new(x.0, 0.0, y.0).length_squared() > 1e-6 {
                        let yaw = Quat::from_rotation_y(f32::atan2(-x.0, -y.0));
                        transform.rotation = yaw;
                    }
                }
            }
        },
        PlayerAction::StartingThrowing { player_id, x, y } => {
            for (_entity, player, mut vel, mut accel, mut transform, children) in &mut query {
                if player.player_id == *player_id {
                    vel.x = 0.0;
                    vel.z = 0.0;
                    vel.y = 0.0;
                    *accel = ConstantLinearAcceleration(Vec3::ZERO);
                    if Vec3::new(x.0, 0.0, y.0).length_squared() > 1e-6 {
                        let yaw = Quat::from_rotation_y(f32::atan2(-x.0, -y.0));
                        transform.rotation = yaw;
                    }
                    let lobject_children: Vec<Entity> = if let Some(children) = children {
                        children.iter().filter(|child| { lobjects.get(*child).is_ok() }).collect()
                    } else {
                        vec![]
                    };
                    if !lobject_children.is_empty() {
                        commands.entity(_entity).insert(StartThrow {});
                    }
                }
            }
        },
        PlayerAction::ReleaseThrow { player_id, power, x, y } => {
            println!("Got release throw event");
            for (_entity, player, mut vel, mut accel, mut player_transform, children) in &mut query {
                if player.player_id == *player_id {
                    commands.entity(_entity).remove::<ThrowingAnimation>();
                    if let Some(children) = children {
                        for child in children {
                            if let Ok((boomrang, transform)) = lobjects.get(*child) {
                                let direction_vector = Vec3{x: x.0, y: 0.0, z: y.0}.normalize_or(Vec3 { x: 0.0, y: 0.0, z: -1.0 });
                                let new_origin_point = player_transform.translation + direction_vector * 1.0;
                                let offset_translation = Quat::from_rotation_y(PI/2.0).mul_vec3(direction_vector) * 0.5;
                                let mut new_transform = Transform::from_translation(new_origin_point + offset_translation).with_rotation(player_transform.rotation);
                                new_transform.rotate_around(new_origin_point, Quat::from_rotation_y(3.0*PI/4.0));
                                // let mut new_transform = Vec3{x:-0.5, y:0.0, z: -0.25});
                                // new_transform.rotate_around(new_transform.translation + Vec3{x: 0.5, y: 0.0, z: 0.0}, Quat::from_rotation_y(3.0*PI/4.0));
                                let velocity = LinearVelocity::from(Vec3{x: x.0, y: 0.0, z: y.0}.normalize_or_zero() * (4.0 + (power.0 * 8.0)));
                                let angular_velocity = AngularVelocity(Vec3{x:0.0, y:8.0, z:0.0});
                                println!("Transform: {:?}", new_transform);
                                println!("Velocity: {:?}", velocity);
                                commands.entity(_entity).detach_child(boomrang);
                                commands.entity(boomrang).insert((
                                    velocity,
                                    Thrown{player_id: Some(*player_id)},
                                    new_transform,
                                    angular_velocity,
                                    RigidBody::Dynamic,
                                ));
                            }
                        }
                    }
                }
            }
        },
    }
    params.apply(world);
}


/// Reads WASD/Gamepad input and drives the player's horizontal velocity, leaving the
/// vertical component to gravity / the physics solver.
///
/// Each local player picked a `Controller` in the lobby (`Keyboard` or a specific
/// `Gamepad`). We read every active input source, look up which local player it is
/// assigned to, and send a `Movement` event for that player's real id. The keyboard
/// is digital (full speed); the gamepad stick is analog (speed scales with tilt).
pub fn move_player(
    keyboard: Res<ButtonInput<KeyCode>>,
    gamepads: Query<(Entity, &Gamepad)>,
    client: Res<GameClientWrapper>,
    time: Res<Time>,
    ticker: Res<Ticker>,
    mut sent_events: ResMut<LocalGameEvents>,
    mut query: Query<(&mut PlayerId, &Transform, &LinearVelocity, &Rotation, &ConstantLinearAcceleration), (Without<Dying>, Without<Dead>)>,
    mut player_directions: ResMut<PlayerDirections>,
    player_move_cooldowns: Res<PlayerJumpCooldowns>,
) {

    // Snapshot this client's local roster (player_id -> controller), releasing the
    // lock before we touch the ECS.
    let roster: Vec<(u8, Controller)> = {
        let client = client.client.read().unwrap();
        let players = client.players.read().unwrap();
        players.iter().map(|p| (p.id, p.controller)).collect()
    };

    // Collect the velocity each active controller wants for its assigned player.
    let mut moves: Vec<(u8, Vec3)> = Vec::new();

    // Keyboard (arrow keys): digital, normalized to full speed.
    if let Some((id, _)) = roster.iter().find(|(_, c)| *c == Controller::Keyboard) {
        let mut direction = Vec3::ZERO;
        if keyboard.pressed(KeyCode::ArrowUp) {
            direction.z -= 1.0; // forward, away from the camera
        }
        if keyboard.pressed(KeyCode::ArrowDown) {
            direction.z += 1.0; // back
        }
        if keyboard.pressed(KeyCode::ArrowLeft) {
            direction.x -= 1.0; // left
        }
        if keyboard.pressed(KeyCode::ArrowRight) {
            direction.x += 1.0; // right
        }
        moves.push((*id, direction.normalize_or_zero() * PLAYER_SPEED));
    }

    // Gamepads: left stick + d-pad, analog (speed proportional to tilt, capped).
    for (entity, gamepad) in &gamepads {
        let controller = Controller::Gamepad(entity.index().index());
        if let Some((id, _)) = roster.iter().find(|(_, c)| *c == controller) {
            let stick = (gamepad.left_stick() + gamepad.dpad()).clamp_length_max(1.0); // x = right, y = up
            // Round each axis to the closest 7-bit level (128 steps over -1..1) so stick
            // jitter collapses to a stable value instead of flooding Movement events.
            let x = (stick.x * DIRECTION_QUANTIZATION).round() / DIRECTION_QUANTIZATION;
            let y = (stick.y * DIRECTION_QUANTIZATION).round() / DIRECTION_QUANTIZATION;
            let direction = Vec3::new(x, 0.0, -y); // stick up = forward = -z
            moves.push((*id, direction * PLAYER_SPEED));
        }
    }

    // Apply: route each velocity to its player entity, sending only when the
    // direction changed or the keep-alive interval has elapsed since the last event.
    let now = std::time::SystemTime::now();
    for (player_id, velocity) in moves {
        for (mut player, ..) in &mut query {
            if player.player_id == player_id {
                if !player_directions.0.contains_key(&player_id) {
                    player_directions.0.insert(player_id, PlayerDirection { time: now, direction_facing: Vec3::ZERO, velocity: Vec3::ZERO });
                }
                let direction = player_directions.0.get_mut(&player_id).unwrap();
                let direction_changed = direction.velocity.clone() != velocity;
                let interval_elapsed = direction.time.elapsed().unwrap().as_millis() >= DIRECTION_EVENT_INTERVAL;
                // let interval_elapsed = false;
                if direction_changed || interval_elapsed {
                    let player_move_cooldown = player_move_cooldowns.0.get(&player_id);
                    let proceed_with_move = match player_move_cooldown {
                        Some(player_cooldown) => *player_cooldown <= std::time::SystemTime::now(),
                        None => true,
                    };
                    if !proceed_with_move {
                        continue;
                    }
                    let mut direction_facing = velocity.normalize_or_zero();
                    direction_facing = if direction_facing.length_squared() < 0.01 {
                        direction.direction_facing
                    } else {
                        direction_facing
                    };
                    *direction = PlayerDirection { time: now, direction_facing: direction_facing, velocity };
                    direction.time = now;
                    let game_event = PlayerAction::Movement { player_id, x: OrderedF32(velocity.x), y: OrderedF32(velocity.z) };
                    record_player_action(&client, &ticker, &mut sent_events, &game_event, true);
                }
            }
        }
    }
}

/// On a gamepad West-button (left action) press, or the keyboard's Z key, start a
/// forward swing on the pressing player's `LObject`. Reuses the same controller→player
/// roster mapping as `move_player`. A press while a swing is already in flight is a
/// no-op (the `Without<Swinging>` filter).
pub fn start_swing(
    keyboard: Res<ButtonInput<KeyCode>>,
    gamepads: Query<(Entity, &Gamepad)>,
    client: Res<GameClientWrapper>,
    ticker: Res<Ticker>,
    mut sent_events: ResMut<LocalGameEvents>,
    players: Query<(&PlayerId, &Children), (Without<Dying>, Without<Dead>)>,
    lobjects: Query<(), With<Boomerang>>,
    mut player_swing_cooldowns: ResMut<PlayerSwingCooldowns>,
) {
    let roster: Vec<(u8, Controller)> = {
        let client = client.client.read().unwrap();
        let players = client.players.read().unwrap();
        players.iter().map(|p| (p.id, p.controller)).collect()
    };
    let mut swings = vec![];
    if keyboard.just_pressed(KeyCode::KeyZ) {
        if let Some((id, _)) = roster.iter().find(|(_, c)| *c == Controller::Keyboard) {
            for (player, children) in &players {
                if player.player_id != *id { continue; }
                for child in children.iter() {
                    if lobjects.get(child).is_ok() {
                        swings.push(*id);
                    }
                }
            }
        }
    }
    for (entity, gamepad) in &gamepads {
        if !gamepad.just_pressed(GamepadButton::West) { continue; }
        let controller = Controller::Gamepad(entity.index().index());
        let Some((id, _)) = roster.iter().find(|(_, c)| *c == controller) else { continue; };
        for (player, children) in &players {
            if player.player_id != *id { continue; }
            for child in children.iter() {
                if lobjects.get(child).is_ok() {
                    swings.push(*id);
                }
            }
        }
    }
    for id in swings {
        let player_move_cooldown = player_swing_cooldowns.0.get(&id);
        let proceed = match player_move_cooldown {
            Some(time) => {
                let now = std::time::SystemTime::now();
                *time<=now
            },
            None => true, 
        };
        if !proceed { continue; }
        player_swing_cooldowns.0.insert(id, std::time::SystemTime::now() + SWING_COOLDOWN);
        let game_event = PlayerAction::Swing { player_id: id };
        record_player_action(&client, &ticker, &mut sent_events, &game_event, true);
    }
}

/// On a gamepad West-button (left action) press, or the keyboard's Z key, start a
/// forward swing on the pressing player's `LObject`. Reuses the same controller→player
/// roster mapping as `move_player`. A press while a swing is already in flight is a
/// no-op (the `Without<Swinging>` filter).
pub fn start_jump(
    keyboard: Res<ButtonInput<KeyCode>>,
    gamepads: Query<(Entity, &Gamepad)>,
    client: Res<GameClientWrapper>,
    ticker: Res<Ticker>,
    mut sent_events: ResMut<LocalGameEvents>,
    players: Query<&PlayerId, (Without<Dying>, Without<Dead>)>,
    player_directions: Res<PlayerDirections>,
    mut player_move_cooldowns: ResMut<PlayerJumpCooldowns>,
) {
    let roster: Vec<(u8, Controller)> = {
        let client = client.client.read().unwrap();
        let players = client.players.read().unwrap();
        players.iter().map(|p| (p.id, p.controller)).collect()
    };
    let mut jumps = vec![];
    if keyboard.pressed(KeyCode::Space) {
        if let Some((id, _)) = roster.iter().find(|(_, c)| *c == Controller::Keyboard) {
            for player in &players {
                if player.player_id != *id { continue; }
                jumps.push(*id);
            }
        }
    }
    for (entity, gamepad) in &gamepads {
        if !gamepad.pressed(GamepadButton::South) { continue; }
        let controller = Controller::Gamepad(entity.index().index());
        let Some((id, _)) = roster.iter().find(|(_, c)| *c == controller) else { continue; };
        for player in &players {
            if player.player_id != *id { continue; }
            jumps.push(*id);
        }
    }
    for jump in jumps {
        let player_move_cooldown = player_move_cooldowns.0.get(&jump);
        let proceed = match player_move_cooldown {
            Some(time) => {
                let now = std::time::SystemTime::now();
                *time<=std::time::SystemTime::now()
            },
            None => true,
        };
        if !proceed { continue; }
        player_move_cooldowns.0.insert(jump, std::time::SystemTime::now() + JUMP_COOLDOWN);
        let player_direction = player_directions.0.get(&jump).unwrap().direction_facing;
        let player_direction = player_direction * 8.0;
        let game_event = PlayerAction::Jump { player_id: jump, x: OrderedF32(player_direction.x), y: OrderedF32(player_direction.z) };
        record_player_action(&client, &ticker, &mut sent_events, &game_event, true);
    }
}

/// On a gamepad West-button (left action) press, or the keyboard's Z key, start a
/// forward swing on the pressing player's `LObject`. Reuses the same controller→player
/// roster mapping as `move_player`. A press while a swing is already in flight is a
/// no-op (the `Without<Swinging>` filter).
pub fn start_throwing(
    keyboard: Res<ButtonInput<KeyCode>>,
    gamepads: Query<(Entity, &Gamepad)>,
    client: Res<GameClientWrapper>,
    ticker: Res<Ticker>,
    mut sent_events: ResMut<LocalGameEvents>,
    players: Query<&PlayerId, (Without<Dying>, Without<Dead>, Without<StartThrow>, Without<ThrowingAnimation>)>,
    player_directions: Res<PlayerDirections>,
) {
    let roster: Vec<(u8, Controller)> = {
        let client = client.client.read().unwrap();
        let players = client.players.read().unwrap();
        players.iter().map(|p| (p.id, p.controller)).collect()
    };
    let mut throwings = vec![];
    if keyboard.pressed(KeyCode::KeyX) {
        if let Some((id, _)) = roster.iter().find(|(_, c)| *c == Controller::Keyboard) {
            for player in &players {
                if player.player_id != *id { continue; }
                throwings.push(*id);
            }
        }
    }

    for (entity, gamepad) in &gamepads {
        if !gamepad.any_pressed([GamepadButton::RightTrigger2, GamepadButton::North]) { continue; }
        let controller = Controller::Gamepad(entity.index().index());
        let Some((id, _)) = roster.iter().find(|(_, c)| *c == controller) else { continue; };
        for player in &players {
            if player.player_id != *id { continue; }
            throwings.push(*id);
        }
    }

    for throwing in throwings {
        let player_direction = player_directions.0.get(&throwing).unwrap().direction_facing;
        let player_direction = player_direction * 8.0;
        let game_event = PlayerAction::StartingThrowing { player_id: throwing, x: OrderedF32(player_direction.x), y: OrderedF32(player_direction.z) };
        record_player_action(&client, &ticker, &mut sent_events, &game_event, true);
    }
}

pub fn release_throw(
    keyboard: Res<ButtonInput<KeyCode>>,
    gamepads: Query<(Entity, &Gamepad)>,
    client: Res<GameClientWrapper>,
    ticker: Res<Ticker>,
    mut sent_events: ResMut<LocalGameEvents>,
    players: Query<(&PlayerId, &ThrowingAnimation), (Without<Dying>, Without<Dead>, With<ThrowingAnimation>)>,
    player_directions: Res<PlayerDirections>,
) {
    let roster: Vec<(u8, Controller)> = {
        let client = client.client.read().unwrap();
        let players = client.players.read().unwrap();
        players.iter().map(|p| (p.id, p.controller)).collect()
    };
    let mut throwings = vec![];
    if !keyboard.pressed(KeyCode::KeyX) {
        if let Some((id, _)) = roster.iter().find(|(_, c)| *c == Controller::Keyboard) {
            for (player, throwing) in &players {
                if player.player_id != *id { continue; }
                throwings.push((*id, throwing.elapsed.clamp(0.0, 2.0) / 2.0));
            }
        }
    }

    for (entity, gamepad) in &gamepads {
        if gamepad.any_pressed([GamepadButton::RightTrigger2, GamepadButton::North]) { continue; }
        let controller = Controller::Gamepad(entity.index().index());
        let Some((id, _)) = roster.iter().find(|(_, c)| *c == controller) else { continue; };
        for (player, throwing) in &players {
            if player.player_id != *id { continue; }
            throwings.push((*id, throwing.elapsed.clamp(0.0, 2.0) / 2.0));
        }
    }

    for throwing in throwings {
        let player_direction = player_directions.0.get(&throwing.0).unwrap().direction_facing;
        let player_direction = player_direction * 2.0;
        let game_event = PlayerAction::ReleaseThrow { player_id: throwing.0, power: OrderedF32(throwing.1), x: OrderedF32(player_direction.x), y: OrderedF32(player_direction.z) };
        record_player_action(&client, &ticker, &mut sent_events, &game_event, true);
    }
}

#[derive(Component)]
pub struct ThrowIndicator;

/// On a gamepad West-button (left action) press, or the keyboard's Z key, start a
/// forward swing on the pressing player's `LObject`. Reuses the same controller→player
/// roster mapping as `move_player`. A press while a swing is already in flight is a
/// no-op (the `Without<Swinging>` filter).
pub fn start_throw_animation(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    client: Res<GameClientWrapper>,
    players: Query<(Entity, &PlayerId, &StartThrow, &Children), With<StartThrow>>,
) {
    let roster: Vec<(u8, Controller)> = {
        let client = client.client.read().unwrap();
        let players = client.players.read().unwrap();
        players.iter().map(|p| (p.id, p.controller)).collect()
    };

    let l_spine_mesh = meshes.add(Cuboid::new(0.4, 0.1, 0.1));
    let l_foot_mesh = meshes.add(Cuboid::new(0.1, 0.1, 0.5));
    let l_material = materials.add(Color::srgb(0.7, 0.7, 0.7));

    for (entity, player_id, start_throw, children) in players {
        commands.entity(entity).with_children(|parent| {
            parent.spawn((
                ThrowIndicator,
                Transform::from_xyz(0.0, 0.0, -1.0).with_rotation(Quat::from_rotation_y(f32::atan2(-1.0, 1.0))),
            ))
                .with_children(|l| {
                    // L spine: runs along +X out from the anchor (cube right face).
                    l.spawn((
                        Mesh3d(l_spine_mesh.clone()),
                        MeshMaterial3d(l_material.clone()),
                        Transform::from_xyz(0.3, 0.0, 0.05),
                    ));
                    // L foot: turns in -Z at the outer end, forming the base of the L
                    // (mirrored about the xy plane).
                    l.spawn((
                        Mesh3d(l_foot_mesh.clone()),
                        MeshMaterial3d(l_material.clone()),
                        Transform::from_xyz(0.05, 0.0, 0.25),
                    ));
                });
        }).remove::<StartThrow>().insert(ThrowingAnimation{
            elapsed: 0.0,
        });
    }
}

/// On a gamepad West-button (left action) press, or the keyboard's Z key, start a
/// forward swing on the pressing player's `LObject`. Reuses the same controller→player
/// roster mapping as `move_player`. A press while a swing is already in flight is a
/// no-op (the `Without<Swinging>` filter).
pub fn animate_throwing_action(
    time: Res<Time>,
    players: Query<(&mut ThrowingAnimation, &Children), With<ThrowingAnimation>>,
    mut indicators: Query<&mut Transform, With<ThrowIndicator>>,
) {

    for (mut throwing, children) in players {
        throwing.elapsed += time.delta_secs();
        let progress = throwing.elapsed.clamp(0.0, 2.0) / 2.0;
        let progress = -1.0 - progress;
        for child in children {
            if let Ok(mut transform) = indicators.get_mut(*child) {
                transform.translation = Vec3 { x: 0.0, y: 0.0, z: progress};
            }
        }
    }
}

/// Advance any in-flight `LObject` swings, writing the local yaw, and end them when done.
/// A `sin(π·t)` arch eases the spine out to the cube front (local -Z) and back to rest
/// over `SWING_DURATION`, so one timer drives both strokes.
pub fn animate_swing(
    mut commands: Commands,
    time: Res<Time>,
    mut query: Query<(Entity, &mut Transform, &mut Swinging, &ChildOf), With<Boomerang>>,
) {
    for (entity, mut transform, mut swing, child_of) in &mut query {
        swing.elapsed += time.delta_secs();
        let t = (swing.elapsed / SWING_DURATION).clamp(0.0, 1.0);
        let angle = SWING_PEAK_ANGLE * (std::f32::consts::PI * t).sin();
        transform.rotation = Quat::from_rotation_y(angle); // translation (0.5,0,0) untouched
        transform.translation = Vec3 { x: angle.cos() * 0.5, y: 0.0, z: angle.sin()*-0.5 };
        if swing.elapsed >= SWING_DURATION {
            transform.rotation = Quat::IDENTITY;
            commands.entity(entity).remove::<Swinging>();
        }
    }
}

/// Detects boomerang strikes. While a boomerang is mid-swing, a contact between one of
/// its blade segments and another player's body is a strike. `CollisionStart` already
/// reports each collider's rigid body, so the blade's body is the striker and the other
/// body is the struck player. Only the client that owns the striker sends the event, so
/// the server sees one `StrikePlayer` per strike rather than one per simulating client.
pub fn detect_swing_strikes(
    mut commands: Commands,
    mut collisions: MessageReader<CollisionStart>,
    in_replay: Res<InReplay>,
    client: Res<GameClientWrapper>,
    players: Query<(Entity, &PlayerId), (Without<Dying>, Without<Dead>)>,
    blades: Query<&ChildOf, With<BoomerangBlade>>,
    swinging: Query<(), With<Swinging>>,
    ticker: Res<Ticker>,
    mut sent_events: ResMut<LocalGameEvents>,
) {
    // Snapshot which player ids this client controls locally.
    let local_ids: Vec<u8> = {
        let client = client.client.read().unwrap();
        let players = client.players.read().unwrap();
        players.iter().map(|p| p.id).collect()
    };

    for event in collisions.read() {
        // Exactly one collider must be a blade. Neither -> a body-to-body bump;
        // both -> boomerang vs boomerang. Neither is a strike.
        let blade1 = blades.get(event.collider1).ok();
        let blade2 = blades.get(event.collider2).ok();
        let (blade_child_of, blade_body, other_body) = match (blade1, blade2) {
            (Some(c), None) => (c, event.body1, event.body2),
            (None, Some(c)) => (c, event.body2, event.body1),
            _ => continue,
        };

        // Swing gate: the blade's parent boomerang must be mid-swing.
        if swinging.get(blade_child_of.parent()).is_err() {
            continue;
        }

        let (Some(blade_body), Some(other_body)) = (blade_body, other_body) else { continue; };
        let (Ok(striker), Ok(struck)) = (players.get(blade_body), players.get(other_body)) else {
            continue;
        };
        if striker.1.player_id == struck.1.player_id {
            continue;
        }

        // Only the striker's owning client reports the strike.
        if !local_ids.contains(&striker.1.player_id) {
            continue;
        }

        let game_event = GameEffect::StrikePlayer {
            striker_id: striker.1.player_id,
            struck_id: struck.1.player_id,
        };
        commands.entity(struck.0).insert(Dying{elapsed: 0.0});
        record_game_effect(&in_replay, &client, &ticker, &mut sent_events, game_event);
    }
}

/// Detects boomerang strikes. While a boomerang is mid-swing, a contact between one of
/// its blade segments and another player's body is a strike. `CollisionStart` already
/// reports each collider's rigid body, so the blade's body is the striker and the other
/// body is the struck player. Only the client that owns the striker sends the event, so
/// the server sees one `StrikePlayer` per strike rather than one per simulating client.
pub fn detect_throw_strikes(
    mut commands: Commands,
    mut collisions: MessageReader<CollisionStart>,
    in_replay: Res<InReplay>,
    client: Res<GameClientWrapper>,
    players: Query<&PlayerId>,
    blades: Query<&ChildOf, With<BoomerangBlade>>,
    thrown: Query<&Thrown, With<Thrown>>,
    ticker: Res<Ticker>,
    mut sent_events: ResMut<LocalGameEvents>,
) {
    // Snapshot which player ids this client controls locally.
    let local_ids: Vec<u8> = {
        let client = client.client.read().unwrap();
        let players = client.players.read().unwrap();
        players.iter().map(|p| p.id).collect()
    };

    for event in collisions.read() {
        println!("Detected collision");
        // Exactly one collider must be a blade. Neither -> a body-to-body bump;
        // both -> boomerang vs boomerang. Neither is a strike.
        let blade1 = blades.get(event.collider1).ok();
        let blade2 = blades.get(event.collider2).ok();
        let (blade_child_of, other_body) = match (blade1, blade2) {
            (Some(c), None) => (c, event.collider2),
            (None, Some(c)) => (c, event.collider1),
            (None, None) => {
                println!("Not the right objects: 1");
                continue
            },
            (Some(_), Some(_)) => {
                println!("Not the right objects: 1.5");
                continue
            },
        };

        let Ok(blade) = thrown.get(blade_child_of.parent()) else {
            println!("Not the right objects: 2: {}", blade_child_of.parent().index());
            continue
        };
        println!("Collided blade entity id: {}", blade_child_of.parent().index());
        println!("Players: {}", players.iter().len());
        let Ok(player) = players.get(other_body) else {
            println!("Not the right objects: 2.5: {}", other_body);
            continue
        };

    }
}

/// Detects boomerang strikes. While a boomerang is mid-swing, a contact between one of
/// its blade segments and another player's body is a strike. `CollisionStart` already
/// reports each collider's rigid body, so the blade's body is the striker and the other
/// body is the struck player. Only the client that owns the striker sends the event, so
/// the server sees one `StrikePlayer` per strike rather than one per simulating client.
pub fn detect_parries(
    mut commands: Commands,
    mut collisions: MessageReader<CollisionStart>,
    in_replay: Res<InReplay>,
    client: Res<GameClientWrapper>,
    mut players: Query<(Entity, &mut LinearVelocity, &mut ConstantLinearAcceleration, &PlayerId, &Children, &Transform), Without<Boomerang>>,
    blades: Query<&ChildOf, With<BoomerangBlade>>,
    swinging: Query<(), With<Swinging>>,
    ticker: Res<Ticker>,
    mut sent_events: ResMut<LocalGameEvents>,
    mut lobjects: Query<(Entity, &mut Transform), (With<Boomerang>)>,
) {

    for event in collisions.read() {
        // Exactly one collider must be a blade. Neither -> a body-to-body bump;
        // both -> boomerang vs boomerang. Neither is a strike.
        let blade_1 = blades.get(event.collider1).ok();
        let blade_2 = blades.get(event.collider2).ok();
        let (Some(blade_1), Some(blade_2)) = (blade_1, blade_2) else { continue; };
        let (Some(player_1), Some(player_2)) = (event.body1, event.body2) else { continue; };

        // Swing gate: the blade's parent boomerang must be mid-swing.
        if swinging.get(blade_1.parent()).is_err() || swinging.get(blade_2.parent()).is_err(){
            continue;
        }

        let Ok([mut player_1, mut player_2]) = players.get_many_mut([player_1, player_2]) else {
            continue;
        };

        if player_1.3.player_id == player_2.3.player_id {
            continue;
        }

        let game_event = GameEffect::Parry {
            player_1_id: std::cmp::max(player_1.3.player_id, player_2.3.player_id),
            player_2_id: std::cmp::min(player_1.3.player_id, player_2.3.player_id),
        };

        for child in player_1.4.iter() {
            if let Ok((boomerang, mut transform)) = lobjects.get_mut(child) {
                commands.entity(boomerang).remove::<Swinging>();
                transform.translation = Vec3::ZERO;
                transform.rotation = Quat::IDENTITY;
            }
        }
        *player_1.1 = LinearVelocity((player_1.5.translation - player_2.5.translation).normalize() * 5.0);
        *player_1.2 = ConstantLinearAcceleration(Vec3::ZERO);

        for child in player_2.4.iter() {
            if let Ok((boomerang, mut transform)) = lobjects.get_mut(child) {
                commands.entity(boomerang).remove::<Swinging>();
                transform.translation = Vec3::ZERO;
                transform.rotation = Quat::IDENTITY;
            }
        }
        record_game_effect(&in_replay, &client, &ticker, &mut sent_events, game_event.clone());
        *player_2.1 = LinearVelocity((player_2.5.translation - player_1.5.translation).normalize() * 5.0);
        *player_2.2 = ConstantLinearAcceleration(Vec3::ZERO);
    }
}

/// When a player is newly marked `Dead`, relayer its body and blade colliders onto the
/// `Dead` layer (filtering to the `Platform` only) so the dead body still rests on the
/// floor but passes through every other player and boomerang. The hierarchy is fixed
/// depth-2 (player -> Boomerang -> blades), so we walk it directly.
pub fn apply_dead_collision_layers(
    newly_dead: Query<(Entity, &Children), Added<Dying>>,
    boomerangs: Query<&Children, With<Boomerang>>,
    mut commands: Commands,
) {
    let dead_layers = CollisionLayers::new(GameLayer::Dead, GameLayer::Platform);
    for (body, children) in &newly_dead {
        commands.entity(body).insert(dead_layers);
        for &child in children {
            if let Ok(blades) = boomerangs.get(child) {
                for &blade in blades {
                    commands.entity(blade).insert(dead_layers);
                }
            }
        }
    }
}

/// Shrinks a dead player's body from full size down to `DEAD_SCALE` over `DEATH_DURATION`
/// and holds it there. The `Dead` marker is never removed, so the body stays on the field.
pub fn animate_death(mut commands: Commands, time: Res<Time>, mut query: Query<(Entity, &PlayerId, &mut Transform, &mut Dying)>) {
    for (player, player_id, mut transform, mut dead) in &mut query {
        dead.elapsed += time.delta_secs();
        let t = (dead.elapsed / DEATH_DURATION).clamp(0.0, 1.0);
        transform.scale = Vec3::splat(1.0 - t * (1.0 - DEAD_SCALE));
        if dead.elapsed >= DEATH_DURATION {
            commands.entity(player).remove::<Dying>();
            commands.entity(player).insert(Dead{});
        }
    }
}

/// Records a `SentGameEvent` for the current tick if `move_player`/`start_swing`/
/// `detect_strikes` didn't already log one (i.e. no `GameEvent` was sent this tick).
/// Must run after those three systems so `sent_events`'s last tick reflects whether
/// they fired this frame.
pub fn record_tick_state(
    mut ticker: ResMut<Ticker>,
    mut sent_events: ResMut<LocalGameEvents>,
    player_query: Query<(&PlayerId, &Transform, &LinearVelocity, &ConstantLinearAcceleration, Option<&Children>, Option<&Dying>, Option<&Dead>, Option<&StartThrow>, Option<&ThrowingAnimation>)>,
    stationary_boomerangs: Query<&Boomerang, Without<Swinging>>,
    swinging_boomerangs: Query<(&Boomerang, &Swinging), With<Swinging>>,
    thrown_boomerangs: Query<(&Boomerang, &Thrown, Option<&LinearVelocity>, Option<&Transform>, Option<&ConstantLinearAcceleration>, Option<&AngularVelocity>), With<Thrown>>,
) {
    if ticker.1 {
        ticker.0 += 1;
    } else {
        ticker.1 = true;
    }
    if player_query.iter().len() == 0 {
        panic!("No players at tick: {}", ticker.0);
    }
    let game_state = GameState{
        players: player_query.iter()
        .map(|(player, transform, velocity, acceleration, children, dying, dead, start_throw, throwing)| {
            let mut player_boomerang_stationary: Option<PlayerBoomerangState> = None;
            if let Some(children) = children {
                for child in children {
                    if let Ok(_) = stationary_boomerangs.get(*child) {
                        player_boomerang_stationary = Some(PlayerBoomerangState::Stationary);
                        break;
                    }
                    if let Ok((_, swinging)) = swinging_boomerangs.get(*child) {
                        player_boomerang_stationary = Some(PlayerBoomerangState::Swinging { elapsed: swinging.elapsed });
                        break;
                    }
                }
            }
            PlayerState {
                status: match dying {
                    Some(dead) => PlayerStatus::Dying { elapsed: dead.elapsed },
                    None => match dead {
                        Some(_) => PlayerStatus::Dead {},
                        None => PlayerStatus::Alive,
                    },
                },
                player_id: player.player_id,
                color: player.color,
                position: transform.translation,
                velocity: velocity.0,
                rotation: transform.rotation,
                acceleration: acceleration.0,
                bommerang: player_boomerang_stationary,
                throwing_state: match start_throw {
                    Some(start_throw) => Some(ThrowingState::StartThrow {}),
                    None => match throwing {
                        Some(throwing) => Some(ThrowingState::Throwing { elapsed: throwing.elapsed }),
                        None => None,
                    }
                }
            }
        })
        .collect(),
        // thrown_boomerangs: vec![],
        thrown_boomerangs: thrown_boomerangs.iter().map(|(boomerang, thrown, linear_velocity, transform, acceleration, angular_velocity)| {
            ThrownBoomerangeState {
                player_id: thrown.player_id,
                position: transform.map_or_else(|| { Vec3::ZERO }, |transform| {transform.translation}),
                velocity: linear_velocity.map_or_else(|| { Vec3::ZERO }, |linear_velocity| {linear_velocity.0}),
                rotation: transform.map_or_else(|| { Quat::IDENTITY }, |transform| {transform.rotation}),
                acceleration: acceleration.map_or_else(|| { Vec3::ZERO }, |acceleration| {acceleration.0}),
                angular_veloctiy: angular_velocity.map_or_else(|| { Vec3::ZERO }, |angular_velocity| {angular_velocity.0}),
            }
        }).collect(),
    };
    sent_events.add_state(TickRecord {
        tick: ticker.0,
        game_state: game_state,
        player_actions: BTreeSet::new(),
        game_effects: BTreeSet::new(),
    });
}

pub fn record_player_action(
    client: &Res<GameClientWrapper>,
    ticker: &Ticker,
    sent_events: &mut ResMut<LocalGameEvents>,
    player_action: &PlayerAction,
    send_to_server: bool,
) {
    if let Some(sender) = &client.client.read().unwrap().sender {
        sent_events.insert_received_player_actions( vec![ (ticker.0, player_action.clone()) ]);
        if send_to_server {
            sender
                .send(ClientEvent::PlayerAction { tick: ticker.0, game_event: player_action.clone() })
                .ok();
        }
    }
}

pub fn record_game_effect(
    in_replay: &Res<InReplay>,
    client: &Res<GameClientWrapper>,
    ticker: &Res<Ticker>,
    sent_events: &mut ResMut<LocalGameEvents>,
    game_effect: GameEffect,
) {
    if let Some(sender) = &client.client.read().unwrap().sender {
        sent_events.insert_received_game_effects( vec![ (ticker.0, game_effect.clone()) ]);
        if !in_replay.0 {
            sender
                .send(ClientEvent::GameEffect { tick: ticker.0, game_event: game_effect })
                .ok();
        }
    }
}

pub fn spawn_world(commands: &mut Commands, ticker: &Ticker, materials: &mut ResMut<Assets<StandardMaterial>>, meshes: &mut ResMut<Assets<Mesh>>, players: Query<Entity, With<PlayerId>>, boomerangs: Query<Entity, (With<Boomerang>, With<Thrown>)>, game_state: GameState) {
    for player in players {
        commands.entity(player).despawn();
    }
    for boomerangs in boomerangs {
        commands.entity(boomerangs).despawn();
    }
    let l_spine_mesh = meshes.add(Cuboid::new(1.0, 0.1, 0.2));
    let l_foot_mesh = meshes.add(Cuboid::new(0.2, 0.1, 0.8));
    let l_material = materials.add(Color::srgb(0.7, 0.7, 0.7));
    if game_state.players.is_empty() {
        panic!("No players at {}", ticker.0);
    }
    for player in game_state.players.clone() {
        let dying_scale = match player.status {
            PlayerStatus::Alive => 0.0,
            PlayerStatus::Dead => 1.0,
            PlayerStatus::Dying { elapsed } => {
                1.0 - (elapsed / DEATH_DURATION).clamp(0.0, 1.0)
            },
        };
        let dying_scale = Vec3::splat(1.0 - dying_scale * (1.0 - DEAD_SCALE));
        let mut player_entity = commands
            .spawn((
                Mesh3d(meshes.add(Cylinder::new(0.5, 1.0))),
                MeshMaterial3d(materials.add(Color::srgba(player.color.red as f32 / 256.0, player.color.green as f32 / 256.0, player.color.blue as f32 / 256.0, 0.5))),
                // NB: `.rotate()` mutates and returns `()` (which is a valid empty Bundle,
                // so it compiles but silently inserts no Transform at all) — the builder
                // form `.with_rotation()` is required here.
                Transform::from_translation(player.position).with_rotation(player.rotation).with_scale(dying_scale),
                RigidBody::Dynamic,
                Collider::cylinder(0.5, 1.0),
                // Facing is driven manually (see `drain_server_events`); lock physics
                // rotation so collisions don't tumble the cube and fight that facing.
                LockedAxes::ROTATION_LOCKED,
                ConstantLinearAcceleration(player.acceleration),
                LinearVelocity(player.velocity),
                PlayerId { player_id: player.player_id, color: player.color },
            ));
        println!("Player entity id: {}", player_entity.id().index());
        if let Some(boomerang_state) = player.bommerang {
            player_entity.with_children(|parent| {
                // The L as a single entity, anchored at the point where it meets the
                // cube (the right face, local x = 0.5). Its segments are positioned
                // relative to this anchor.
                // let mut boomerang = parent.spawn((
                //     Boomerang,
                //     Transform::from_xyz(0.5, 0.0, 0.0),
                //     Visibility::default(),
                // ));
                let mut boomerang: EntityCommands<'_> = parent.spawn((
                    Boomerang,
                    Visibility::default(),
                ));
                println!("Boomerang id: {}", boomerang.id().index());
                // Restore an in-flight swing from the snapshot. `animate_swing` derives the
                // boomerang transform entirely from `elapsed`, so the component alone is
                // enough; the pose corrects itself on the next physics step.
                boomerang.insert(
            Transform::from_xyz(0.5, 0.0, 0.0),
                );
                spawn_boomerang(&mut boomerang, materials, meshes);
                if let Some(PlayerBoomerangState::Swinging { elapsed }) = player.bommerang {
                    boomerang.insert(Swinging { elapsed });
                }
                // boomerang
                //     .with_children(|l| {
                //         // L spine: runs along +X out from the anchor (cube right face).
                //         l.spawn((
                //             Mesh3d(l_spine_mesh.clone()),
                //             MeshMaterial3d(l_material.clone()),
                //             Transform::from_xyz(0.5, 0.0, 0.0),
                //             Collider::cuboid(1.0, 0.1, 0.2),
                //             BoomerangBlade,
                //             CollisionEventsEnabled,
                //         ));
                //         // L foot: turns in -Z at the outer end, forming the base of the L
                //         // (mirrored about the xy plane).
                //         l.spawn((
                //             Mesh3d(l_foot_mesh.clone()),
                //             MeshMaterial3d(l_material.clone()),
                //             Transform::from_xyz(0.9, 0.0, -0.3),
                //             Collider::cuboid(0.2, 0.1, 0.8),
                //             BoomerangBlade,
                //             CollisionEventsEnabled,
                //         ));
                //     });
            });
        }
        if let PlayerStatus::Dying { elapsed } = player.status {
            player_entity.insert(Dying{elapsed: elapsed});
        }
        if let PlayerStatus::Dead {} = player.status {
            player_entity.insert(Dead{});
        }
        if let Some(throwing_state) = player.throwing_state {
            let l_spine_mesh = meshes.add(Cuboid::new(0.4, 0.1, 0.1));
            let l_foot_mesh = meshes.add(Cuboid::new(0.1, 0.1, 0.5));
            let l_material = materials.add(Color::srgb(0.7, 0.7, 0.7));
            match throwing_state {
                ThrowingState::StartThrow {} => {
                    player_entity.insert(StartThrow {});
                },
                ThrowingState::Throwing { elapsed } => {
                    player_entity.insert(ThrowingAnimation { elapsed });
                },
            };
            player_entity.with_children(|parent| {
                let mut indicator = parent.spawn((
                    ThrowIndicator,
                    match throwing_state {
                        ThrowingState::StartThrow {} => {
                            Transform::from_xyz(0.0, 0.0, -1.0)
                        },
                        ThrowingState::Throwing { elapsed } => {
                            let progress = elapsed.clamp(0.0, 2.0)/ 2.0;
                            let progress = -1.0 - progress;
                            Transform::from_xyz(0.0, 0.0, progress)
                        }
                    }.with_rotation(Quat::from_rotation_y(f32::atan2(-1.0, 1.0))),
                ));
                indicator
                    .with_children(|l| {
                        // L spine: runs along +X out from the anchor (cube right face).
                        let arrow_edge = l.spawn((
                            Mesh3d(l_spine_mesh.clone()),
                            MeshMaterial3d(l_material.clone()),
                            Transform::from_xyz(0.3, 0.0, 0.05),
                        ));
                        println!("Arrow edge spawned: {}", arrow_edge.id().index());
                        // L foot: turns in -Z at the outer end, forming the base of the L
                        // (mirrored about the xy plane).
                        let arrow_edge = l.spawn((
                            Mesh3d(l_foot_mesh.clone()),
                            MeshMaterial3d(l_material.clone()),
                            Transform::from_xyz(0.05, 0.0, 0.25),
                        ));
                        println!("Arrow edge spawned: {}", arrow_edge.id().index());
                    });
                println!("Indicator spawned: {}", indicator.id().index());
            });
        }
    }
    for thrown_boomerang in game_state.thrown_boomerangs {
        let mut boomerang = commands.spawn((
            Boomerang,
            Visibility::default(),
        ));
        println!("Boomerang id: {}", boomerang.id().index());
        boomerang.insert((
            Thrown{player_id: thrown_boomerang.player_id},
            Transform::from_translation(thrown_boomerang.position).with_rotation(thrown_boomerang.rotation),
            LinearVelocity(thrown_boomerang.velocity),
            ConstantLinearAcceleration(thrown_boomerang.acceleration),
            AngularVelocity(thrown_boomerang.angular_veloctiy),
            RigidBody::Dynamic,
        ));
        spawn_boomerang(
            &mut boomerang,
            materials,
            meshes,
        );
    }
}

pub fn spawn_boomerang<'a>(
    boomerang: &'a mut EntityCommands,
    materials: &mut ResMut<Assets<StandardMaterial>>,
    meshes: &mut ResMut<Assets<Mesh>>,
) {
    let l_spine_mesh = meshes.add(Cuboid::new(0.5, 0.05, 0.1));
    let l_foot_mesh = meshes.add(Cuboid::new(0.1, 0.05, 0.4));
    let l_material = materials.add(Color::srgb(0.7, 0.7, 0.7));
    let l_material_2 = materials.add(Color::srgb(0.7, 1.0, 0.7));
    // The L as a single entity, anchored at the point where it meets the
    // cube (the right face, local x = 0.5). Its segments are positioned
    // relative to this anchor.
    // Restore an in-flight swing from the snapshot. `animate_swing` derives the
    // boomerang transform entirely from `elapsed`, so the component alone is
    // enough; the pose corrects itself on the next physics step.
    // if let Some(PlayerBoomerangState::Swinging { elapsed }) = player.bommerang {
    //     boomerang.insert(Swinging { elapsed });
    // }
    boomerang
        .with_children(|l| {
            // L spine: runs along +X out from the anchor (cube right face).
            let boomerang_blade = l.spawn((
                Mesh3d(l_spine_mesh.clone()),
                MeshMaterial3d(l_material.clone()),
                Transform::from_xyz(0.25, 0.0, 0.0),
                Collider::cuboid(0.5, 0.05, 0.1),
                BoomerangBlade,
                CollisionEventsEnabled,
            ));
            println!("BoomerangBlade spawned: {}", boomerang_blade.id().index());
            // L foot: turns in -Z at the outer end, forming the base of the L
            // (mirrored about the xy plane).
            let boomerang_blade = l.spawn((
                Mesh3d(l_foot_mesh.clone()),
                MeshMaterial3d(l_material_2.clone()),
                Transform::from_xyz(0.45, 0.0, -0.25),
                Collider::cuboid(0.1, 0.05, 0.4),
                BoomerangBlade,
                CollisionEventsEnabled,
            ));
            println!("BoomerangBlade spawned: {}", boomerang_blade.id().index());
        });
}

// pub fn get_new_actions(params: &mut Local<SystemState<(
//         Commands,
//         Res<GameClientWrapper>,
//         Query<(Entity, &Player, &mut LinearVelocity, &mut ConstantLinearAcceleration, &mut Rotation, &mut Transform), Without<Dead>>,
//         Query<(&Player, &Children), Without<Dead>>,
//         Query<Entity, (With<Boomerang>, Without<Swinging>)>,
//         ResMut<Ticker>,
//         ResMut<LocalGameEvents>,
//         Query<Entity, With<Player>>,
//         ResMut<Assets<Mesh>>,
//         ResMut<Assets<StandardMaterial>>,
//         ResMut<InReplay>,
//     )>>,
//     world: &mut World,
//     existing_actions: Vec<>
// ) -> Vec<(u64, PlayerAction)> {
//     let client = params.get_mut(world).1;
//     let client = client.client.read().unwrap();
//     let events = {
//         let mut server_events = client.received_events.lock().unwrap();
//         let events = server_events.clone();
//         *server_events = vec![];
//         events
//     };
//     let mut game_events: Vec<(u64, PlayerAction)> = events.iter().filter_map(|event| { if let ServerEvent::PlayerAction{tick: tick, game_event: game_event} = event { Some((*tick, game_event.clone())) } else { None } }).collect();
//     game_events.sort_by(|a, b| {a.0.cmp(&b.0)});
//     game_events
// }