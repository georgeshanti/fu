use avian3d::prelude::*;
use bevy::prelude::*;

use crate::{
    app::{GameClientWrapper, screens::{app_state::AppState, lobby::PendingSpawns}}, server::{ClientEvent, GameEvent, Controller, ServerEvent},
};

/// Identifies an entity as a player-controlled body.
#[derive(Component)]
pub struct Player {
    pub player_id: u8,
    pub direction: Vec3,
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
pub struct Ticker(pub u64);

/// One outgoing `GameEvent` this client has sent to the server, paired with the
/// `Ticker` value it was stamped with.
pub struct SentGameEvent {
    pub tick: u64,
    pub event: GameEvent,
}

/// Every `GameEvent` this client has transmitted so far. Present only while
/// `AppState::Playing` (inserted alongside `Ticker`).
#[derive(Resource, Default)]
pub struct SentGameEvents(pub Vec<SentGameEvent>);

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
        Transform::from_xyz(0.0, 5.0, 12.0).looking_at(Vec3::ZERO, Vec3::Y),
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
                    Player { player_id: player.id, direction: Vec3::ZERO, last_direction_event_timestamp: std::time::SystemTime::now() },
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
            commands.insert_resource(Ticker(0));
            commands.insert_resource(SentGameEvents::default());
            next_state.set(AppState::Playing);
        }
    }
}

pub fn drain_server_events(
    mut commands: Commands,
    client: Res<GameClientWrapper>,
    mut query: Query<(Entity, &Player, &mut LinearVelocity, &mut ConstantLinearAcceleration, &mut Rotation), Without<Dead>>,
    swing_targets: Query<(&Player, &Children), Without<Dead>>,
    lobjects: Query<Entity, (With<Boomerang>, Without<Swinging>)>,
    overlay: Query<Entity, With<CountdownOverlay>>,
    mut next_state: ResMut<NextState<AppState>>,
    mut ticker: ResMut<Ticker>,
) {
    ticker.0 += 1;

    let client = client.client.read().unwrap();
    let events = {
        let mut server_events = client.received_events.lock().unwrap();
        let events = server_events.clone();
        *server_events = vec![];
        events
    };
    for event in events {
        if let ServerEvent::GameEvent { tick, game_event: game_event } = event {
            match game_event {
                GameEvent::Movement { player_id, x, y } => {
                    for (_entity, player, mut vel, mut accel, mut rotation) in &mut query {
                        if player.player_id == player_id {
                            vel.x = x;
                            vel.z = y;
                            *accel = ConstantLinearAcceleration(Vec3::new(x, 0.0, y).normalize_or_zero() * PLAYER_ACCEL);
                            // Point the player (and its anchored L) toward the movement
                            // direction. Forward is -Z, so yaw = atan2(-x, -z). Leave the
                            // facing unchanged when stationary.
                            if Vec3::new(x, 0.0, y).length_squared() > 1e-6 {
                                *rotation = Quat::from_rotation_y(f32::atan2(-x, -y)).into();
                            }
                        }
                    }
                },
                GameEvent::Swing { player_id } => {
                    for (player, children) in &swing_targets {
                        if player.player_id != player_id { continue; }
                        for child in children.iter() {
                            if let Ok(boomerang) = lobjects.get(child) {
                                commands.entity(boomerang).insert(Swinging { elapsed: 0.0 });
                            }
                        }
                    }
                }
                GameEvent::StrikePlayer { struck_id, .. } => {
                    // Mark the struck player dead; `animate_death` shrinks it and
                    // `apply_dead_collision_layers` relayers it to touch only the platform.
                    for (entity, player, ..) in &query {
                        if player.player_id == struck_id {
                            commands.entity(entity).insert(Dead { elapsed: 0.0 });
                        }
                    }
                }
                _ => {}
            }
        }
    }
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
    mut sent_events: ResMut<SentGameEvents>,
    mut query: Query<&mut Player, Without<Dead>>,
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
        for mut player in &mut query {
            if player.player_id == player_id {
                let direction_changed = player.direction != velocity;
                // let interval_elapsed = player.last_direction_event_timestamp.elapsed().unwrap().as_millis() >= DIRECTION_EVENT_INTERVAL;
                let interval_elapsed = false;
                if direction_changed || interval_elapsed {
                    player.direction = velocity;
                    player.last_direction_event_timestamp = std::time::SystemTime::now();
                    if let Some(sender) = &client.client.read().unwrap().sender {
                        let game_event = GameEvent::Movement { player_id, x: velocity.x, y: velocity.z };
                        sent_events.0.push(SentGameEvent { tick: ticker.0, event: game_event.clone() });
                        sender.send(ClientEvent::GameEvent { tick: ticker.0, game_event }).ok();
                    }
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
    mut sent_events: ResMut<SentGameEvents>,
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
                        if let Some(sender) = &client.client.read().unwrap().sender {
                            let game_event = GameEvent::Swing { player_id: *id };
                            sent_events.0.push(SentGameEvent { tick: ticker.0, event: game_event.clone() });
                            sender.send(ClientEvent::GameEvent {
                                tick: ticker.0,
                                game_event,
                            }).ok();
                        }
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
                    if let Some(sender) = &client.client.read().unwrap().sender {
                        let game_event = GameEvent::Swing { player_id: *id };
                        sent_events.0.push(SentGameEvent { tick: ticker.0, event: game_event.clone() });
                        sender.send(ClientEvent::GameEvent {
                            tick: ticker.0,
                            game_event,
                        }).ok();
                    }
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
    client: Res<GameClientWrapper>,
    players: Query<&Player>,
    blades: Query<&ChildOf, With<BoomerangBlade>>,
    swinging: Query<(), With<Swinging>>,
    ticker: Res<Ticker>,
    mut sent_events: ResMut<SentGameEvents>,
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

        if let Some(sender) = &client.client.read().unwrap().sender {
            let game_event = GameEvent::StrikePlayer {
                striker_id: striker.player_id,
                struck_id: struck.player_id,
            };
            sent_events.0.push(SentGameEvent { tick: ticker.0, event: game_event.clone() });
            sender
                .send(ClientEvent::GameEvent { tick: ticker.0, game_event })
                .ok();
        }
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