use std::{backtrace::Backtrace, collections::{BTreeMap, BTreeSet}};

use avian3d::{dynamics::integrator::IntegrationSystems::Velocity, prelude::*};
use bevy::{ecs::{change_detection::Tick, system::SystemState}, prelude::*};

use crate::{
    app::{GameClientWrapper, screens::{app_state::AppState, lobby::PendingSpawns}}, server::{ClientEvent, Controller, GameEffect, OrderedF32, PlayerAction, ServerEvent},
};

/// Identifies an entity as a player-controlled body.
#[derive(Component)]
pub struct Player {
    pub player_id: u8,
    pub last_direction_event_timestamp: std::time::SystemTime,
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
pub struct Dead {
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

/// Minimum delay after a swing ends before the player may swing again.
const SWING_COOLDOWN: std::time::Duration = std::time::Duration::from_millis(500);

/// Present on a player from when their swing ends until `SWING_COOLDOWN` has elapsed.
/// While present it blocks `start_swing`; `tick_swing_cooldown` removes it once expired.
#[derive(Component)]
pub struct SwingCooldown {
    until: std::time::SystemTime,
}

/// Horizontal movement speed of the player, in meters per second.
const PLAYER_SPEED: f32 = 5.0;

/// Minimum time between movement events when direction is unchanged (keep-alive heartbeat), in seconds.
const DIRECTION_EVENT_INTERVAL: u128 = 50;

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
pub enum PlayerBoomerangState {
    Stationary,
    Swinging{elapsed: f32},
}

#[derive(Resource, Default, Clone)]
pub struct PlayerDirections(BTreeMap<u8, Vec3>);

/// A snapshot of one locally-controlled player's physics at a given tick.
#[derive(Clone)]
pub struct PlayerState {
    pub player_id: u8,
    pub position: Vec3,
    pub velocity: Vec3,
    pub rotation: Quat,
    pub acceleration: Vec3,
    pub bommerang: Option<PlayerBoomerangState>,
}

#[derive(Clone)]
pub struct GameState {
    players: Vec<PlayerState>,
}

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

/// Constant linear acceleration applied to the player to overcome ground friction.
/// Derived from Coulomb friction: μ × g = 0.5 × 9.81 = 4.905 m/s²
const PLAYER_ACCEL: f32 = 4.905;

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
) {
    // Camera, positioned back and up, looking at the origin.
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(0.0, 1.0, 12.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    // Directional light so the meshes are visible.
    commands.spawn((
        DirectionalLight {
            shadows_enabled: true,
            ..default()
        },
        Transform::from_xyz(4.0, 8.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    // Static platform (20 x 20).
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(20.0, 20.0))),
        MeshMaterial3d(materials.add(Color::srgb(0.3, 0.5, 0.3))),
        RigidBody::Static,
        Collider::cuboid(20.0, 0.01, 20.0),
        // Own layer so dead bodies can filter to collide with only the platform.
        CollisionLayers::new(GameLayer::Platform, LayerMask::ALL),
    ));

    // One dynamic cube per player, at the spawn point assigned by the server.
    if let Some(spawns) = spawns {
        // Shared assets for the L-shaped object held off each player's right side.
        // The L lies flat in the horizontal (X-Z) plane, thin in Y, at cube mid-height.
        let l_spine_mesh = meshes.add(Cuboid::new(1.0, 0.1, 0.2));
        let l_foot_mesh = meshes.add(Cuboid::new(0.2, 0.1, 0.8));
        let l_material = materials.add(Color::srgb(0.7, 0.7, 0.7));

        for (player, position) in &spawns.0 {
            commands
                .spawn((
                    Mesh3d(meshes.add(Cylinder::new(0.5, 1.0))),
                    MeshMaterial3d(materials.add(Color::srgb(0.8, 0.3, 0.3))),
                    Transform::from_translation(*position),
                    RigidBody::Dynamic,
                    Collider::cylinder(0.5, 1.0),
                    // Facing is driven manually (see `drain_server_events`); lock physics
                    // rotation so collisions don't tumble the cube and fight that facing.
                    LockedAxes::ROTATION_LOCKED,
                    ConstantLinearAcceleration(Vec3::ZERO),
                    Player { player_id: player.id, last_direction_event_timestamp: std::time::SystemTime::now() },
                ))
                .with_children(|parent| {
                    // The L as a single entity, anchored at the point where it meets the
                    // cube (the right face, local x = 0.5). Its segments are positioned
                    // relative to this anchor.
                    parent
                        .spawn((
                            Boomerang,
                            Transform::from_xyz(0.5, 0.0, 0.0),
                            Visibility::default(),
                        ))
                        .with_children(|l| {
                            // L spine: runs along +X out from the anchor (cube right face).
                            l.spawn((
                                Mesh3d(l_spine_mesh.clone()),
                                MeshMaterial3d(l_material.clone()),
                                Transform::from_xyz(0.5, 0.0, 0.0),
                                Collider::cuboid(1.0, 0.1, 0.2),
                                BoomerangBlade,
                                CollisionEventsEnabled,
                            ));
                            // L foot: turns in -Z at the outer end, forming the base of the L
                            // (mirrored about the xy plane).
                            l.spawn((
                                Mesh3d(l_foot_mesh.clone()),
                                MeshMaterial3d(l_material.clone()),
                                Transform::from_xyz(0.9, 0.0, -0.3),
                                Collider::cuboid(0.2, 0.1, 0.8),
                                BoomerangBlade,
                                CollisionEventsEnabled,
                            ));
                        });
                });
        }
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
            next_state.set(AppState::Playing);
        }
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
        Query<(Entity, &Player, &mut LinearVelocity, &mut ConstantLinearAcceleration, &mut Rotation, &mut Transform), Without<Dead>>,
        Query<(&Player, &Children), Without<Dead>>,
        Query<Entity, (With<Boomerang>, Without<Swinging>)>,
        ResMut<Ticker>,
        ResMut<LocalGameEvents>,
        Query<Entity, With<Player>>,
        ResMut<Assets<Mesh>>,
        ResMut<Assets<StandardMaterial>>,
        ResMut<InReplay>,
    )>>,
) {
    let mut new_player_actions = {
        let client = params.get_mut(world).1;
        let client = client.client.read().unwrap();
        let events = {
            let mut server_events = client.received_events.lock().unwrap();
            let events = server_events.clone();
            *server_events = vec![];
            events
        };
        let mut game_events: Vec<(u64, PlayerAction)> = events.iter().filter_map(|event| { if let ServerEvent::PlayerAction{tick: tick, game_event: game_event} = event { Some((*tick, game_event.clone())) } else { None } }).collect();
        game_events.sort_by(|a, b| {a.0.cmp(&b.0)});
        game_events
    };
    if !new_player_actions.is_empty() {
        let mut existing_records = {
            println!("Stuff: {}", new_player_actions.first().unwrap().0);
            let first_tick = new_player_actions.first().unwrap().0;
            let (mut commands, _, _, _, _, mut ticker, mut local_game_events, players, mut meshes, mut materials, mut in_replay) = params.get_mut(world);
            println!("first tick value: {}", local_game_events.game_events.first().unwrap().tick);
            let game_state = local_game_events.game_events.get(first_tick as usize).unwrap().game_state.clone();
            spawn_world(&mut commands, &mut materials, &mut meshes, players, game_state);
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
        let final_tick = {
            let ticker = params.get_mut(world).5;
            std::cmp::max(new_player_actions.last().unwrap().0, ticker.0)
        };
        let mut current_tick = {
            let (_, _, _, _, _, ticker, _, players, mut meshes, mut materials, mut in_replay) = params.get_mut(world);
            ticker.0
        };
        println!("current_tick: {}, final_tick: {}", current_tick, final_tick);
        while current_tick <= final_tick {
            {
                if !new_player_actions.is_empty() {
                    println!("New player action: {:?} {:?}", new_player_actions.first().unwrap().0, new_player_actions.first().unwrap().1.clone());
                }
                while !new_player_actions.is_empty() && new_player_actions.first().unwrap().0 == current_tick {
                    let (mut commands, client, mut query, swing_targets, lobjects, mut ticker, mut sent_events, _, _, _, _) =
                        params.get_mut(world);
                    let first = new_player_actions.first().unwrap();
                    {
                        record_player_action(&client, &ticker, &mut sent_events, &first.1, false);
                    }
                    println!("Replaying player action: {:?}", first.1);
                    match first.1 {
                        PlayerAction::Movement { player_id, x, y } => {
                            for (_entity, player, mut vel, mut accel, mut rotation, mut transform) in &mut query {
                                if player.player_id == player_id {
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
                                        *rotation = yaw.into();
                                        transform.rotation = yaw;
                                    }
                                }
                            }
                        },
                        PlayerAction::Swing { player_id } => {
                            for (player, children) in &swing_targets {
                                if player.player_id != player_id { continue; }
                                for child in children.iter() {
                                    if let Ok(boomerang) = lobjects.get(child) {
                                        println!("Adding swing to player's boomerang");
                                        commands.entity(boomerang).insert(Swinging { elapsed: 0.0 });
                                    }
                                }
                            }
                        }
                    }
                    new_player_actions.remove(0);
                    params.apply(world);
                }
                {
                    if !existing_records.is_empty() {
                        for player_action in existing_records.first().unwrap().player_actions.iter() {
                            let (mut commands, client, mut query, swing_targets, lobjects, mut ticker, mut sent_events, _, _, _, _) =
                                params.get_mut(world);
                            match player_action {
                                PlayerAction::Movement { player_id, x, y } => {
                                    for (_entity, player, mut vel, mut accel, mut rotation, mut transform) in &mut query {
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
                                                *rotation = yaw.into();
                                                transform.rotation = yaw;
                                            }
                                        }
                                    }
                                },
                                PlayerAction::Swing { player_id } => {
                                    for (player, children) in &swing_targets {
                                        if player.player_id != *player_id { continue; }
                                        for child in children.iter() {
                                            if let Ok(boomerang) = lobjects.get(child) {
                                                commands.entity(boomerang).insert(Swinging { elapsed: 0.0 });
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        params.apply(world);
                    }
                }
            }
            println!("Running scehdule");
            world.run_schedule(PhysicsSchedule);
            params.apply(world);
            println!("Ran scehdule");
            if !existing_records.is_empty() {
                let (_, client, _, _, _, _, mut local_game_events, _, _, _, _) = params.get_mut(world);
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
                let (_, _, _, _, _, ticker, _, _, _, _, _) = params.get_mut(world);
                ticker.0
            };
        }
        {
            let (_, _, _, _, _, _, _, _, _, _, mut in_replay) = params.get_mut(world);
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
    mut query: Query<(&mut Player, &Transform, &LinearVelocity, &Rotation, &ConstantLinearAcceleration), Without<Dead>>,
    mut player_directions: ResMut<PlayerDirections>,
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
    for (player_id, velocity) in moves {
        for (mut player, ..) in &mut query {
            if player.player_id == player_id {
                if !player_directions.0.contains_key(&player_id) {
                    player_directions.0.insert(player_id, Vec3::ZERO);
                }
                let direction = player_directions.0.get_mut(&player_id).unwrap();
                let direction_changed = direction.clone() != velocity;
                // let interval_elapsed = player.last_direction_event_timestamp.elapsed().unwrap().as_millis() >= DIRECTION_EVENT_INTERVAL;
                let interval_elapsed = false;
                if direction_changed || interval_elapsed {
                    *direction = velocity;
                    player.last_direction_event_timestamp = std::time::SystemTime::now();
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
    players: Query<(&Player, &Children), Without<SwingCooldown>>,
    lobjects: Query<(), (With<Boomerang>, Without<Swinging>)>,
) {
    let roster: Vec<(u8, Controller)> = {
        let client = client.client.read().unwrap();
        let players = client.players.read().unwrap();
        players.iter().map(|p| (p.id, p.controller)).collect()
    };
    if keyboard.just_pressed(KeyCode::KeyZ) {
        if let Some((id, _)) = roster.iter().find(|(_, c)| *c == Controller::Keyboard) {
            for (player, children) in &players {
                if player.player_id != *id { continue; }
                for child in children.iter() {
                    if lobjects.get(child).is_ok() {
                        let game_event = PlayerAction::Swing { player_id: *id };
                        record_player_action(&client, &ticker, &mut sent_events, &game_event, true);
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
                    let game_event = PlayerAction::Swing { player_id: *id };
                    record_player_action(&client, &ticker, &mut sent_events, &game_event, true);
                }
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
        println!("Animating swing");
        swing.elapsed += time.delta_secs();
        let t = (swing.elapsed / SWING_DURATION).clamp(0.0, 1.0);
        let angle = SWING_PEAK_ANGLE * (std::f32::consts::PI * t).sin();
        transform.rotation = Quat::from_rotation_y(angle); // translation (0.5,0,0) untouched
        transform.translation = Vec3 { x: angle.cos() * 0.5, y: 0.0, z: angle.sin()*-0.5 };
        if swing.elapsed >= SWING_DURATION {
            transform.rotation = Quat::IDENTITY; // snap exactly to rest
            commands.entity(entity).remove::<Swinging>();
            // Start the player's swing cooldown from the moment the swing ends.
            commands.entity(child_of.parent()).insert(SwingCooldown {
                until: std::time::SystemTime::now() + SWING_COOLDOWN,
            });
        }
    }
}

/// Removes a player's `SwingCooldown` once `SWING_COOLDOWN` has elapsed since their last
/// swing ended, letting them swing again.
pub fn tick_swing_cooldown(
    mut commands: Commands,
    cooldowns: Query<(Entity, &SwingCooldown)>,
) {
    let now = std::time::SystemTime::now();
    for (entity, cooldown) in &cooldowns {
        if now >= cooldown.until {
            commands.entity(entity).remove::<SwingCooldown>();
        }
    }
}

/// Detects boomerang strikes. While a boomerang is mid-swing, a contact between one of
/// its blade segments and another player's body is a strike. `CollisionStart` already
/// reports each collider's rigid body, so the blade's body is the striker and the other
/// body is the struck player. Only the client that owns the striker sends the event, so
/// the server sees one `StrikePlayer` per strike rather than one per simulating client.
pub fn detect_strikes(
    mut collisions: MessageReader<CollisionStart>,
    in_replay: Res<InReplay>,
    client: Res<GameClientWrapper>,
    players: Query<&Player>,
    blades: Query<&ChildOf, With<BoomerangBlade>>,
    swinging: Query<(), With<Swinging>>,
    ticker: Res<Ticker>,
    mut sent_events: ResMut<LocalGameEvents>,
    state_query: Query<(&Player, &Transform, &LinearVelocity, &Rotation)>,
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
        if striker.player_id == struck.player_id {
            continue;
        }

        // Only the striker's owning client reports the strike.
        if !local_ids.contains(&striker.player_id) {
            continue;
        }

        let game_event = GameEffect::StrikePlayer {
            striker_id: striker.player_id,
            struck_id: struck.player_id,
        };
        record_game_effect(&in_replay, &client, &ticker, &mut sent_events, game_event);
    }
}

/// When a player is newly marked `Dead`, relayer its body and blade colliders onto the
/// `Dead` layer (filtering to the `Platform` only) so the dead body still rests on the
/// floor but passes through every other player and boomerang. The hierarchy is fixed
/// depth-2 (player -> Boomerang -> blades), so we walk it directly.
pub fn apply_dead_collision_layers(
    newly_dead: Query<(Entity, &Children), Added<Dead>>,
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
pub fn animate_death(time: Res<Time>, mut query: Query<(&mut Transform, &mut Dead)>) {
    for (mut transform, mut dead) in &mut query {
        dead.elapsed += time.delta_secs();
        let t = (dead.elapsed / DEATH_DURATION).clamp(0.0, 1.0);
        transform.scale = Vec3::splat(1.0 - t * (1.0 - DEAD_SCALE));
    }
}

/// Records a `SentGameEvent` for the current tick if `move_player`/`start_swing`/
/// `detect_strikes` didn't already log one (i.e. no `GameEvent` was sent this tick).
/// Must run after those three systems so `sent_events`'s last tick reflects whether
/// they fired this frame.
pub fn record_tick_state(
    mut ticker: ResMut<Ticker>,
    mut sent_events: ResMut<LocalGameEvents>,
    player_query: Query<(&Player, &Transform, &LinearVelocity, &Rotation, &ConstantLinearAcceleration, &Children)>,
    stationary_boomerangs: Query<&Boomerang, Without<Swinging>>,
    swinging_boomerangs: Query<(&Boomerang, &Swinging), With<Swinging>>,
) {
    if ticker.1 {
        ticker.0 += 1;
    } else {
        ticker.1 = true;
    }
    let game_state = GameState{
        players: player_query.iter()
        .map(|(player, transform, velocity, rotation, acceleration, children)| {
            let mut player_boomerang_stationary: Option<PlayerBoomerangState> = None;
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
            PlayerState {
                player_id: player.player_id,
                position: transform.translation,
                velocity: velocity.0,
                rotation: rotation.0,
                acceleration: acceleration.0,
                bommerang: player_boomerang_stationary,
            }
        })
        .collect()
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
    // println!("Here?: {:?}", Backtrace::capture());
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

pub fn spawn_world(commands: &mut Commands, materials: &mut ResMut<Assets<StandardMaterial>>, meshes: &mut ResMut<Assets<Mesh>>, players: Query<Entity, With<Player>>, game_state: GameState) {
    for player in players {
        commands.entity(player).despawn();
    }
    let l_spine_mesh = meshes.add(Cuboid::new(1.0, 0.1, 0.2));
    let l_foot_mesh = meshes.add(Cuboid::new(0.2, 0.1, 0.8));
    let l_material = materials.add(Color::srgb(0.7, 0.7, 0.7));
    for player in game_state.players.clone() {
        commands
            .spawn((
                Mesh3d(meshes.add(Cylinder::new(0.5, 1.0))),
                MeshMaterial3d(materials.add(Color::srgb(0.8, 0.3, 0.3))),
                // NB: `.rotate()` mutates and returns `()` (which is a valid empty Bundle,
                // so it compiles but silently inserts no Transform at all) — the builder
                // form `.with_rotation()` is required here.
                Transform::from_translation(player.position).with_rotation(player.rotation),
                RigidBody::Dynamic,
                Collider::cylinder(0.5, 1.0),
                // Facing is driven manually (see `drain_server_events`); lock physics
                // rotation so collisions don't tumble the cube and fight that facing.
                LockedAxes::ROTATION_LOCKED,
                ConstantLinearAcceleration(player.acceleration),
                LinearVelocity(player.velocity),
                Player { player_id: player.player_id, last_direction_event_timestamp: std::time::SystemTime::now() },
            ))
            .with_children(|parent| {
                // The L as a single entity, anchored at the point where it meets the
                // cube (the right face, local x = 0.5). Its segments are positioned
                // relative to this anchor.
                let mut boomerang = parent.spawn((
                    Boomerang,
                    Transform::from_xyz(0.5, 0.0, 0.0),
                    Visibility::default(),
                ));
                // Restore an in-flight swing from the snapshot. `animate_swing` derives the
                // boomerang transform entirely from `elapsed`, so the component alone is
                // enough; the pose corrects itself on the next physics step.
                if let Some(PlayerBoomerangState::Swinging { elapsed }) = player.bommerang {
                    boomerang.insert(Swinging { elapsed });
                }
                boomerang
                    .with_children(|l| {
                        // L spine: runs along +X out from the anchor (cube right face).
                        l.spawn((
                            Mesh3d(l_spine_mesh.clone()),
                            MeshMaterial3d(l_material.clone()),
                            Transform::from_xyz(0.5, 0.0, 0.0),
                            Collider::cuboid(1.0, 0.1, 0.2),
                            BoomerangBlade,
                            CollisionEventsEnabled,
                        ));
                        // L foot: turns in -Z at the outer end, forming the base of the L
                        // (mirrored about the xy plane).
                        l.spawn((
                            Mesh3d(l_foot_mesh.clone()),
                            MeshMaterial3d(l_material.clone()),
                            Transform::from_xyz(0.9, 0.0, -0.3),
                            Collider::cuboid(0.2, 0.1, 0.8),
                            BoomerangBlade,
                            CollisionEventsEnabled,
                        ));
                    });
            });
    }
}