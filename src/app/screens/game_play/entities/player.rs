use avian3d::dynamics::rigid_body::RigidBody;
use bevy::prelude::*;
use avian3d::prelude::*;
use crate::{app::screens::game_play::{animations::{DEAD_SCALE, DEATH_DURATION}, state::PlayerId, world::GameLayer}, server::{Player, PlayerState, PlayerStatus}};

pub fn create_player_entity<'a>(
    commands: &'a mut Commands,
    player: PlayerState,
    materials: &mut ResMut<Assets<StandardMaterial>>,
    meshes: &mut ResMut<Assets<Mesh>>
) -> EntityCommands<'a> {
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
           PlayerId { player_id: player.player_id, color: player.color },
           Visibility::default(),
           RigidBody::Dynamic,
           Collider::cylinder(0.5, 1.0),
           CollisionLayers::new(GameLayer::Active, [GameLayer::Environment, GameLayer::Active]),
           // Facing is driven manually (see `drain_server_events`); lock physics
           // rotation so collisions don't tumble the cube and fight that facing.
           LockedAxes::ROTATION_LOCKED,
           ConstantLinearAcceleration(player.acceleration),
           LinearVelocity(player.velocity),
           Transform::from_translation(player.position).with_rotation(player.rotation).with_scale(dying_scale),
       ));
    player_entity.with_children(|parent| {
       parent.spawn((
           Mesh3d(meshes.add(Cylinder::new(0.5, 1.0))),
           MeshMaterial3d(materials.add(Color::srgba(player.color.red as f32 / 256.0, player.color.green as f32 / 256.0, player.color.blue as f32 / 256.0, 0.5))),
           // NB: `.rotate()` mutates and returns `()` (which is a valid empty Bundle,
           // so it compiles but silently inserts no Transform at all) — the builder
           // form `.with_rotation()` is required here.
       ));
    });
    player_entity
}
