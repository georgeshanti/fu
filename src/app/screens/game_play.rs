use avian3d::prelude::*;
use bevy::prelude::*;

use crate::{
    app::{GameClientWrapper, screens::{app_state::AppState, lobby::PendingSpawns}},
    server::{ClientEvent, ServerEvent},
};

/// Identifies an entity as a player-controlled body.
#[derive(Component)]
pub struct Player {
    pub player_id: u8,
    pub direction: Vec3,
}

/// Horizontal movement speed of the player, in meters per second.
const PLAYER_SPEED: f32 = 5.0;

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
        for (player, position) in &spawns.0 {
            commands.spawn((
                Mesh3d(meshes.add(Cuboid::new(1.0, 1.0, 1.0))),
                MeshMaterial3d(materials.add(Color::srgb(0.8, 0.3, 0.3))),
                Transform::from_translation(*position),
                RigidBody::Dynamic,
                Collider::cuboid(1.0, 1.0, 1.0),
                ConstantLinearAcceleration(Vec3::ZERO),
                Player { player_id: player.id, direction: Vec3::ZERO },
            ));
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
    mut query: Query<(&Player, &mut LinearVelocity, &mut ConstantLinearAcceleration)>,
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
                for (player, mut vel, mut accel) in &mut query {
                    if player.player_id == player_id {
                        vel.x = x;
                        vel.z = y;
                        *accel = ConstantLinearAcceleration(Vec3::new(x, 0.0, y).normalize_or_zero() * PLAYER_ACCEL);
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
pub fn move_player(
    keyboard: Res<ButtonInput<KeyCode>>,
    client: Res<GameClientWrapper>,
    mut query: Query<&mut Player>,
) {
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

    let velocity = direction.normalize_or_zero() * PLAYER_SPEED;
    for mut player in &mut query {
        if player.direction != velocity {
            player.direction = velocity;
            if let Some(sender) = &client.client.read().unwrap().sender {
                sender.send(ClientEvent::Movement { player_id: 0, x: velocity.x, y: velocity.z }).ok();
            }
        }
    }
}