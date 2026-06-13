pub mod screens;
pub mod common;

use std::sync::{Arc, RwLock};

use avian3d::prelude::*;
use bevy::prelude::*;

use crate::app::screens::game_state::*;
use crate::app::common::text::*;
use crate::app::screens::game_menu::*;
use crate::app::screens::join_game::*;
use crate::{client::GameClient, server::{ClientEvent, ServerEvent}};

/// Identifies an entity as a player-controlled body.
#[derive(Component)]
struct Player {
    player_id: u8,
    direction: Vec3,
}

/// Horizontal movement speed of the player, in meters per second.
const PLAYER_SPEED: f32 = 5.0;

/// Constant linear acceleration applied to the player to overcome ground friction.
/// Derived from Coulomb friction: μ × g = 0.5 × 9.81 = 4.905 m/s²
const PLAYER_ACCEL: f32 = 4.905;

#[derive(Resource)]
pub struct GameClientWrapper {
    pub client: Arc<RwLock<GameClient>>,
}

pub fn run() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(PhysicsPlugins::default())
        .init_state::<AppState>()
        .add_systems(OnEnter(AppState::Menu), setup_menu)
        .add_systems(
            Update,
            (handle_join_game_button, handle_create_game_button, update_server_status_text)
                .run_if(in_state(AppState::Menu)),
        )
        .add_systems(OnExit(AppState::Menu), cleanup_menu)
        .add_systems(OnEnter(AppState::JoinGame), setup_join_screen)
        .add_systems(
            Update,
            (focus_input_field, update_input, handle_join_online_submit_button, handle_join_local_server_button)
                .run_if(in_state(AppState::JoinGame)),
        )
        .add_systems(OnExit(AppState::JoinGame), cleanup_join_screen)
        .add_systems(OnEnter(AppState::Playing), setup)
        .add_systems(
            Update,
            (move_player, drain_server_events).run_if(in_state(AppState::Playing)),
        ) 
        .run();
}

fn drain_server_events(
    client: Res<GameClientWrapper>,
    mut query: Query<(&Player, &mut LinearVelocity, &mut ConstantLinearAcceleration)>,
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
            }
        }
    }
}

/// Reads WASD input and drives the player's horizontal velocity, leaving the
/// vertical component to gravity / the physics solver.
fn move_player(
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

/// Spawns a minimal 3D scene: a camera, a light, a static ground plane, and a
/// single dynamic cube that falls under gravity and comes to rest on the ground.
fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
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

    // Static ground plane (20 x 20).
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(20.0, 20.0))),
        MeshMaterial3d(materials.add(Color::srgb(0.3, 0.5, 0.3))),
        RigidBody::Static,
        Collider::cuboid(20.0, 0.01, 20.0),
    ));

    // Dynamic cube spawned above the ground; gravity pulls it down to rest.
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(1.0, 1.0, 1.0))),
        MeshMaterial3d(materials.add(Color::srgb(0.8, 0.3, 0.3))),
        Transform::from_xyz(0.0, 6.0, 0.0),
        RigidBody::Dynamic,
        Collider::cuboid(1.0, 1.0, 1.0),
        ConstantLinearAcceleration(Vec3::ZERO),
        Player { player_id: 0, direction: Vec3::ZERO, },
    ));
}
