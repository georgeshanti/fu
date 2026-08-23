use std::collections::BTreeMap;

use avian3d::prelude::*;
use bevy::prelude::*;

use crate::{app::{GameClientWrapper, screens::{app_state::AppState, game_play::{actions::{PlayerDirections, PlayerJumpCooldowns, PlayerSwingCooldowns, StartThrow}, animations::{DEAD_SCALE, DEATH_DURATION, Dying, Swinging, ThrowIndicator, ThrowingAnimation}, entities::boomerang::{Boomerang, Thrown, spawn_boomerang}, phases::end::{CountdownOverlay, CountdownText}, state::{Countdown, Dead, InReplay, LocalGameEvents, PendingSpawns, PlayerId, PlayerInfo, PlayerInfos, Ticker}}}}, server::{ClientEvent, GameState, PlayerBoomerangState, PlayerState, PlayerStatus, ServerEvent, ThrowingState}};

/// Seconds to wait (showing the countdown overlay) before telling the server we're ready.
const COUNTDOWN_SECS: f32 = 3.0;

/// Physics collision layers. Living players and their blades stay on the implicit
/// `Default` layer; the platform gets its own layer so a `Dead` body can filter to
/// touch only the platform (and thus pass through every other player and boomerang).
#[derive(PhysicsLayer, Default, Clone, Copy)]
pub enum GameLayer {
    #[default]
    Default,
    Environment,
    Active,
    Dead,
}

/// The static parts of the playfield — light and platform. Tagged so they can be
/// despawned between rounds; without that, re-entering `AppState::SpawningPlayers`
/// would stack a second copy of each on top of the first.
#[derive(Component)]
pub struct GamePlayRoot;

/// The gameplay `Camera3d`. Separate from `GamePlayRoot` only to follow the
/// `XxxRoot` + `XxxCamera` convention every other screen uses.
#[derive(Component)]
pub struct GamePlayCamera;

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
        CollisionLayers::new(GameLayer::Environment, LayerMask::ALL),
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
                CollisionLayers::new(GameLayer::Active, [GameLayer::Environment, GameLayer::Active]),
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
            player_entity.insert(CollisionLayers::new(GameLayer::Dead, GameLayer::Environment));
        }
        if let PlayerStatus::Dead {} = player.status {
            player_entity.insert(Dead{});
            player_entity.insert(CollisionLayers::new(GameLayer::Dead, GameLayer::Environment));
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