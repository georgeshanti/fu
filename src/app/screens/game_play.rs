use avian3d::prelude::*;
use bevy::prelude::*;

use crate::{
    app::{GameClientWrapper, screens::{app_state::AppState, lobby::PendingSpawns}},
    server::{ClientEvent, Controller, ServerEvent},
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
pub struct LObject;

/// Horizontal movement speed of the player, in meters per second.
const PLAYER_SPEED: f32 = 5.0;

/// Minimum time between movement events when direction is unchanged (keep-alive heartbeat), in seconds.
const DIRECTION_EVENT_INTERVAL: u128 = 50;

/// Quantization factor for joystick axes: 2^7 / 2 = 64 levels per side,
/// giving 128 discrete steps across the clamped -1..1 range.
const DIRECTION_QUANTIZATION: f32 = 128.0;

/// Seconds to wait (showing the countdown overlay) before telling the server we're ready.
const COUNTDOWN_SECS: f32 = 3.0;

/// Remaining time on the pre-game countdown. Present only while counting down.
#[derive(Resource)]
pub struct Countdown {
    remaining: f32,
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
                    Mesh3d(meshes.add(Cuboid::new(1.0, 1.0, 1.0))),
                    MeshMaterial3d(materials.add(Color::srgb(0.8, 0.3, 0.3))),
                    Transform::from_translation(*position),
                    RigidBody::Dynamic,
                    Collider::cuboid(1.0, 1.0, 1.0),
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
                            LObject,
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
                            ));
                            // L foot: turns in -Z at the outer end, forming the base of the L
                            // (mirrored about the xy plane).
                            l.spawn((
                                Mesh3d(l_foot_mesh.clone()),
                                MeshMaterial3d(l_material.clone()),
                                Transform::from_xyz(0.9, 0.0, -0.3),
                                Collider::cuboid(0.2, 0.1, 0.8),
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

pub fn drain_server_events(
    mut commands: Commands,
    client: Res<GameClientWrapper>,
    mut query: Query<(&Player, &mut LinearVelocity, &mut ConstantLinearAcceleration, &mut Rotation)>,
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
        match event {
            ServerEvent::Movement { player_id, x, y } => {
                for (player, mut vel, mut accel, mut rotation) in &mut query {
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
            ServerEvent::StartRound => {
                // Countdown finished: tell the server we're ready and remove the overlay.
                for entity in &overlay {
                    commands.entity(entity).despawn();
                }
                commands.remove_resource::<Countdown>();
                next_state.set(AppState::Playing);
            }
            _ => {}
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
    mut query: Query<&mut Player>,
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

    // Keyboard (WASD): digital, normalized to full speed.
    if let Some((id, _)) = roster.iter().find(|(_, c)| *c == Controller::Keyboard) {
        let mut direction = Vec3::ZERO;
        if keyboard.pressed(KeyCode::KeyW) {
            direction.z -= 1.0; // forward, away from the camera
        }
        if keyboard.pressed(KeyCode::KeyS) {
            direction.z += 1.0; // back
        }
        if keyboard.pressed(KeyCode::KeyA) {
            direction.x -= 1.0; // left
        }
        if keyboard.pressed(KeyCode::KeyD) {
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
                let interval_elapsed = player.last_direction_event_timestamp.elapsed().unwrap().as_millis() >= DIRECTION_EVENT_INTERVAL;
                if direction_changed || interval_elapsed {
                    player.direction = velocity;
                    player.last_direction_event_timestamp = std::time::SystemTime::now();
                    if let Some(sender) = &client.client.read().unwrap().sender {
                        sender.send(ClientEvent::Movement { player_id, x: velocity.x, y: velocity.z }).ok();
                    }
                }
            }
        }
    }
}