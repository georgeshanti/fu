use bevy::prelude::*;

use crate::{app::{GameClientWrapper, screens::game_play::{actions::StartThrow, entities::boomerang::Boomerang, state::{Dead, PlayerId}}}, server::Controller};

/// Total duration of the death shrink, in seconds, and the scale a dead body settles at.
pub const DEATH_DURATION: f32 = 0.4;
pub const DEAD_SCALE: f32 = 0.5;

/// Present on a player once struck; drives the shrink animation. Never removed —
/// a dead player stays on the field at half size.
#[derive(Component)]
pub struct Dying {
    pub elapsed: f32,
}

/// Total duration of one swing (forward and back), in seconds.
const SWING_DURATION: f32 = 0.25;

/// Peak yaw of the swing. The spine rests along local +X and must reach the cube
/// front (local -Z). For `Quat::from_rotation_y(θ)`, local +X maps to
/// (cos θ, 0, -sin θ); reaching (0,0,-1) requires θ = +π/2. (A negative angle would
/// swing to the cube's back, +Z — wrong.)
const SWING_PEAK_ANGLE: f32 = std::f32::consts::FRAC_PI_2;

#[derive(Component)]
pub struct ThrowingAnimation {
    pub elapsed: f32,
}

/// Present on an `LObject` only while a swing is animating; tracks elapsed time.
/// Removed (and rotation snapped to rest) when `elapsed >= SWING_DURATION`.
#[derive(Component)]
pub struct Swinging {
    pub elapsed: f32,
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