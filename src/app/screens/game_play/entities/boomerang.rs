use avian3d::collision::{collider::{Collider, CollisionLayers}, collision_events::CollisionEventsEnabled};
use bevy::prelude::*;

use crate::app::screens::game_play::world::GameLayer;

#[derive(Component)]
pub struct Boomerang;

#[derive(Component)]
pub struct BoomerangBlade;

#[derive(Component)]
pub struct Thrown {
    pub player_id: Option<u8>,
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
    boomerang
        .with_children(|l| {
            // L spine: runs along +X out from the anchor (cube right face).
            let boomerang_blade = l.spawn((
                Mesh3d(l_spine_mesh.clone()),
                MeshMaterial3d(l_material.clone()),
                Transform::from_xyz(0.25, 0.0, 0.0),
                Collider::cuboid(0.5, 0.05, 0.1),
                CollisionLayers::new(GameLayer::Active, [GameLayer::Environment, GameLayer::Active]),
                BoomerangBlade,
                CollisionEventsEnabled,
            ));
            // L foot: turns in -Z at the outer end, forming the base of the L
            // (mirrored about the xy plane).
            let boomerang_blade = l.spawn((
                Mesh3d(l_foot_mesh.clone()),
                MeshMaterial3d(l_material_2.clone()),
                Transform::from_xyz(0.45, 0.0, -0.25),
                Collider::cuboid(0.1, 0.05, 0.4),
                CollisionLayers::new(GameLayer::Active, [GameLayer::Environment, GameLayer::Active]),
                BoomerangBlade,
                CollisionEventsEnabled,
            ));
        });
}