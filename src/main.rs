use avian3d::prelude::*;
use bevy::prelude::*;

mod app;
mod client;
mod server;

/// Identifies an entity as a player-controlled body.
#[derive(Component)]
struct Player {
    player_id: u8,
}

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(PhysicsPlugins::default())
        .add_systems(Startup, setup)
        .add_systems(Update, move_player)
        .run();
}

/// Horizontal movement speed of the player, in meters per second.
const PLAYER_SPEED: f32 = 5.0;

/// Reads WASD input and drives the player's horizontal velocity, leaving the
/// vertical component to gravity / the physics solver.
fn move_player(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut query: Query<&mut LinearVelocity, With<Player>>,
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
    for mut linear_velocity in &mut query {
        linear_velocity.0.x = velocity.x;
        linear_velocity.0.z = velocity.z;
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
        Player { player_id: 0 },
    ));
}
